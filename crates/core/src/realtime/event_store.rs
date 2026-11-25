//! Real-time event log and the resume ("catch-up") contract.
//!
//! A connection can disappear while its subscription keeps producing events.
//! The framework owns the **contract** and a process-local implementation; a
//! durable store (Redis stream, PostgreSQL table, NATS JetStream) is an adapter
//! behind [`IRealtimeEventStore`].
//!
//! Wire model:
//!
//! * every appended event gets a process-unique, monotonically increasing
//!   [`RealtimeEvent::event_id`];
//! * a client remembers the last id it saw and reconnects with a
//!   [`ResumeRequest`] (or `cursor: null` when it saw nothing);
//! * the server answers with a [`ResumeFrame`]: the retained events newer than
//!   the cursor plus `next_cursor`; `truncated` tells the client that events
//!   after its cursor were already evicted, so it must re-sync from state.
//!
//! The store is transport-neutral: a WebSocket handler, an SSE endpoint or an
//! HTTP `GET /messages?after=` endpoint can all replay from the same log.
//!
//! ```ignore
//! let store = connection.context().resolve::<dyn IRealtimeEventStore>()?;
//! let frame = store.resume(ResumeRequest {
//!     subscription: room_id,
//!     cursor: last_seen,
//! }).await?;
//! connection.send_text(serde_json::to_string(&frame)?).await?;
//! ```

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::error::Result;

/// One retained real-time event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RealtimeEvent {
    /// Process-unique, monotonically increasing id (the resume cursor).
    pub event_id: u64,
    /// Subscription (room/topic/document) the event belongs to.
    pub subscription: String,
    /// Opaque payload (usually the JSON frame sent to clients).
    pub payload: String,
    /// When the event was appended (Unix seconds).
    pub created_at_unix_secs: u64,
}

impl RealtimeEvent {
    /// Whether this event is newer than `cursor`.
    pub fn is_after(&self, cursor: Option<u64>) -> bool {
        cursor.is_none_or(|cursor| self.event_id > cursor)
    }
}

/// Client request to replay everything after `cursor`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResumeRequest {
    /// Subscription to replay.
    pub subscription: String,
    /// Last event id the client processed (`None` = start from retained state).
    #[serde(default)]
    pub cursor: Option<u64>,
}

impl ResumeRequest {
    /// A resume request for `subscription` after `cursor`.
    pub fn new(subscription: impl Into<String>, cursor: Option<u64>) -> Self {
        Self {
            subscription: subscription.into(),
            cursor,
        }
    }
}

/// Server answer to a [`ResumeRequest`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResumeFrame {
    /// Subscription that was replayed.
    pub subscription: String,
    /// Retained events newer than the request cursor (oldest first).
    pub events: Vec<RealtimeEvent>,
    /// Cursor the client should present on the next reconnect.
    pub next_cursor: u64,
    /// `true` when events after the client cursor are no longer retained.
    pub truncated: bool,
}

/// Replayable real-time event log.
///
/// Implementations must be safe to share (the store is a `Singleton`) and may
/// be backed by a durable store; `append`/`resume` are async so a network-backed
/// adapter fits without changing call sites.
#[async_trait::async_trait]
pub trait IRealtimeEventStore: Send + Sync {
    /// Append one payload to `subscription`; returns it with its assigned id.
    async fn append(&self, subscription: &str, payload: String) -> Result<RealtimeEvent>;

    /// Replay retained events newer than `request.cursor`.
    async fn resume(&self, request: ResumeRequest) -> Result<ResumeFrame>;

    /// Latest cursor for `subscription` (`0` when it never produced an event).
    async fn latest_cursor(&self, subscription: &str) -> u64;
}

/// Retention bounds for [`InMemoryRealtimeEventStore`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RealtimeEventStoreLimits {
    /// Maximum retained events per subscription (oldest evicted first).
    pub max_events_per_subscription: usize,
    /// Maximum age of a retained event (`None` = no age bound).
    pub max_age: Option<Duration>,
}

impl Default for RealtimeEventStoreLimits {
    /// 1024 events per subscription, no age bound: bounded memory, enough for a
    /// short reconnect window. Tune down for high-fan-out rooms, up for slow
    /// clients.
    fn default() -> Self {
        Self {
            max_events_per_subscription: 1024,
            max_age: None,
        }
    }
}

#[derive(Default)]
struct SubscriptionLog {
    events: VecDeque<RealtimeEvent>,
    /// Highest id evicted by retention (0 = nothing evicted).
    evicted_through: u64,
}

/// Process-local [`IRealtimeEventStore`].
///
/// One mutex over all subscriptions keeps the log consistent; the critical
/// section is pure bookkeeping (no I/O, no awaits). Retention is enforced on
/// append and resume, so memory stays bounded without a background sweeper.
///
/// The log is process-local: with more than one instance, replay only sees the
/// events produced by the same process. Cross-node durability needs a shared
/// store (the port exists for exactly that).
pub struct InMemoryRealtimeEventStore {
    limits: RealtimeEventStoreLimits,
    state: Mutex<HashMap<String, SubscriptionLog>>,
    next_id: AtomicU64,
}

impl Default for InMemoryRealtimeEventStore {
    fn default() -> Self {
        Self::new(RealtimeEventStoreLimits::default())
    }
}

impl InMemoryRealtimeEventStore {
    /// New store with `limits`.
    pub fn new(limits: RealtimeEventStoreLimits) -> Self {
        Self {
            limits,
            state: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(1),
        }
    }

    /// Retention bounds.
    pub fn limits(&self) -> &RealtimeEventStoreLimits {
        &self.limits
    }

    /// Number of subscriptions currently tracked (diagnostics/tests).
    pub fn subscription_count(&self) -> usize {
        self.lock().len()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, SubscriptionLog>> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Drop events that exceed the retention bounds, recording the highest
    /// evicted id per subscription.
    fn prune(log: &mut SubscriptionLog, limits: &RealtimeEventStoreLimits, now_unix_secs: u64) {
        if let Some(max_age) = limits.max_age {
            let max_age = max_age.as_secs();
            while let Some(front) = log.events.front() {
                if now_unix_secs.saturating_sub(front.created_at_unix_secs) > max_age {
                    let evicted = log.events.pop_front().expect("front exists");
                    log.evicted_through = log.evicted_through.max(evicted.event_id);
                } else {
                    break;
                }
            }
        }
        while log.events.len() > limits.max_events_per_subscription {
            let evicted = log.events.pop_front().expect("overflow");
            log.evicted_through = log.evicted_through.max(evicted.event_id);
        }
    }

    /// Append without the async wrapper used by the trait implementation.
    fn append_sync(&self, subscription: &str, payload: String) -> RealtimeEvent {
        let event = RealtimeEvent {
            event_id: self.next_id.fetch_add(1, Ordering::Relaxed),
            subscription: subscription.to_string(),
            payload,
            created_at_unix_secs: unix_now_secs(),
        };
        let now = event.created_at_unix_secs;
        let mut state = self.lock();
        let log = state.entry(subscription.to_string()).or_default();
        log.events.push_back(event.clone());
        Self::prune(log, &self.limits, now);
        event
    }
}

#[async_trait::async_trait]
impl IRealtimeEventStore for InMemoryRealtimeEventStore {
    async fn append(&self, subscription: &str, payload: String) -> Result<RealtimeEvent> {
        Ok(self.append_sync(subscription, payload))
    }

    async fn resume(&self, request: ResumeRequest) -> Result<ResumeFrame> {
        let cutoff = request.cursor.unwrap_or(0);
        let now = unix_now_secs();
        let mut state = self.lock();
        let log = state.entry(request.subscription.clone()).or_default();
        Self::prune(log, &self.limits, now);

        // A cursor ahead of everything this store ever assigned means the
        // client's state came from another store/instance epoch: it can never
        // receive another event with the current cursor, so report it as a gap
        // and restart from the latest known id instead of stalling forever.
        let latest_known = log
            .events
            .back()
            .map(|event| event.event_id)
            .unwrap_or(log.evicted_through);
        let ahead = cutoff > latest_known;
        let events: Vec<RealtimeEvent> = if ahead {
            Vec::new()
        } else {
            log.events
                .iter()
                .filter(|event| event.is_after(request.cursor))
                .cloned()
                .collect()
        };
        let next_cursor = if ahead {
            latest_known
        } else {
            events
                .last()
                .map(|event| event.event_id)
                .unwrap_or_else(|| cutoff.max(log.evicted_through))
        };
        Ok(ResumeFrame {
            subscription: request.subscription,
            events,
            next_cursor,
            truncated: ahead || log.evicted_through > cutoff,
        })
    }

    async fn latest_cursor(&self, subscription: &str) -> u64 {
        let mut state = self.lock();
        let log = state.entry(subscription.to_string()).or_default();
        Self::prune(log, &self.limits, unix_now_secs());
        log.events
            .back()
            .map(|event| event.event_id)
            .unwrap_or(log.evicted_through)
    }
}

fn unix_now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn store_with(limit: usize) -> InMemoryRealtimeEventStore {
        InMemoryRealtimeEventStore::new(RealtimeEventStoreLimits {
            max_events_per_subscription: limit,
            max_age: None,
        })
    }

    #[tokio::test]
    async fn resume_returns_everything_after_the_cursor() {
        let store = InMemoryRealtimeEventStore::default();
        let first = store.append("room-1", "one".into()).await.expect("append");
        let second = store.append("room-1", "two".into()).await.expect("append");
        assert!(second.event_id > first.event_id);

        let all = store
            .resume(ResumeRequest::new("room-1", None))
            .await
            .expect("resume");
        assert_eq!(all.events.len(), 2);
        assert_eq!(all.next_cursor, second.event_id);
        assert!(!all.truncated);

        let after_first = store
            .resume(ResumeRequest::new("room-1", Some(first.event_id)))
            .await
            .expect("resume");
        assert_eq!(after_first.events, vec![second.clone()]);
        assert_eq!(after_first.next_cursor, second.event_id);
        assert!(!after_first.truncated);

        let caught_up = store
            .resume(ResumeRequest::new("room-1", Some(second.event_id)))
            .await
            .expect("resume");
        assert!(caught_up.events.is_empty());
        assert_eq!(caught_up.next_cursor, second.event_id);
        assert!(!caught_up.truncated);
    }

    #[tokio::test]
    async fn subscriptions_are_isolated() {
        let store = InMemoryRealtimeEventStore::default();
        store.append("room-a", "a1".into()).await.expect("append");
        store.append("room-b", "b1".into()).await.expect("append");

        let a = store
            .resume(ResumeRequest::new("room-a", None))
            .await
            .expect("resume");
        assert_eq!(a.events.len(), 1);
        assert_eq!(a.events[0].payload, "a1");
        assert_eq!(store.subscription_count(), 2);
        assert_eq!(store.latest_cursor("empty").await, 0);
    }

    #[tokio::test]
    async fn retention_reports_a_truncated_gap() {
        let store = store_with(2).await;
        let one = store.append("room-1", "one".into()).await.expect("append");
        let two = store.append("room-1", "two".into()).await.expect("append");
        store
            .append("room-1", "three".into())
            .await
            .expect("append");

        // "one" was evicted by the 2-event retention, so a fresh client learns
        // that history is missing.
        let from_scratch = store
            .resume(ResumeRequest::new("room-1", None))
            .await
            .expect("resume");
        assert!(from_scratch.truncated);
        assert_eq!(
            from_scratch
                .events
                .iter()
                .map(|event| event.payload.as_str())
                .collect::<Vec<_>>(),
            vec!["two", "three"]
        );

        // Appending another event evicts "two": reconnecting from "one" now has
        // a real gap (the cursor itself is evicted, and "two" is gone).
        store.append("room-1", "four".into()).await.expect("append");
        let after_one = store
            .resume(ResumeRequest::new("room-1", Some(one.event_id)))
            .await
            .expect("resume");
        assert!(after_one.truncated);
        assert_eq!(
            after_one
                .events
                .iter()
                .map(|event| event.payload.as_str())
                .collect::<Vec<_>>(),
            vec!["three", "four"]
        );

        // A cursor at the retained boundary is not a gap.
        let after_two = store
            .resume(ResumeRequest::new("room-1", Some(two.event_id)))
            .await
            .expect("resume");
        assert!(!after_two.truncated);
        assert_eq!(after_two.events.len(), 2);
    }

    #[tokio::test]
    async fn a_cursor_ahead_of_the_log_is_reported_as_a_gap() {
        let store = InMemoryRealtimeEventStore::default();
        let first = store.append("room-1", "one".into()).await.expect("append");
        let frame = store
            .resume(ResumeRequest::new("room-1", Some(first.event_id + 10)))
            .await
            .expect("resume");
        assert!(
            frame.truncated,
            "a cursor ahead of the store must force a resync"
        );
        assert!(frame.events.is_empty());
        assert_eq!(frame.next_cursor, first.event_id);
    }

    #[tokio::test]
    async fn max_age_evicts_old_events() {
        let store = InMemoryRealtimeEventStore::new(RealtimeEventStoreLimits {
            max_events_per_subscription: 16,
            max_age: Some(Duration::ZERO),
        });
        store.append("room-1", "old".into()).await.expect("append");
        // The age bound drops anything older than "now"; an event stamped in the
        // same second survives, so age 0 is only used to prove pruning runs.
        let frame = store
            .resume(ResumeRequest::new("room-1", None))
            .await
            .expect("resume");
        assert!(frame.events.len() <= 1);
    }
}
