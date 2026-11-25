//! In-memory [`IOutbox`]: the same claim/lease/backoff/dead-letter semantics as
//! a durable store, kept in a `Mutex`.
//!
//! Use it for local development, tests and single-process prototypes. Rows live
//! in memory only: a restart loses pending events, so production producers must
//! enqueue through a durable storage adapter instead.

use std::collections::BTreeMap;
use std::sync::Mutex;

use async_trait::async_trait;
use webx_core::error::Result;

use crate::port::{IOutbox, OutboxEntry};
use crate::time::unix_now_secs;

/// Longest `last_error` text kept on a row.
const MAX_ERROR_CHARS: usize = 500;

/// Maximum backoff exponent (delay is capped at `base * 2^10`).
const MAX_BACKOFF_EXPONENT: u32 = 10;

/// A read-only view of one in-memory row, for assertions and debugging.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutboxRecord {
    /// Row primary key.
    pub id: String,
    /// Stable event name.
    pub event_name: String,
    /// JSON payload recorded by the producer.
    pub payload: String,
    /// Failed delivery attempts so far.
    pub attempts: i32,
    /// Earliest Unix-seconds time of the next attempt.
    pub next_attempt_at: i64,
    /// Lease deadline (Unix seconds); `0` means the row is not leased.
    pub locked_until: i64,
    /// Unix-seconds time the handlers succeeded.
    pub delivered_at: Option<i64>,
    /// Unix-seconds time the row was dead-lettered.
    pub dead_lettered_at: Option<i64>,
    /// Last delivery error (truncated).
    pub last_error: Option<String>,
    /// Row creation time (Unix seconds).
    pub created_at: i64,
}

impl OutboxRecord {
    /// Whether the row has reached a terminal state.
    pub fn is_finished(&self) -> bool {
        self.delivered_at.is_some() || self.dead_lettered_at.is_some()
    }
}

struct Row {
    event_name: String,
    payload: String,
    attempts: i32,
    next_attempt_at: i64,
    locked_until: i64,
    delivered_at: Option<i64>,
    dead_lettered_at: Option<i64>,
    last_error: Option<String>,
    created_at: i64,
}

/// A `Mutex`-backed [`IOutbox`] for development and tests.
pub struct InMemoryOutbox {
    rows: Mutex<BTreeMap<String, Row>>,
    max_attempts: i32,
}

impl InMemoryOutbox {
    /// Build an empty store; `max_attempts` is the dead-letter threshold used by
    /// [`IOutbox::fail`] (at least 1).
    pub fn new(max_attempts: i32) -> Self {
        Self {
            rows: Mutex::new(BTreeMap::new()),
            max_attempts: max_attempts.max(1),
        }
    }

    /// Append a pending row that is due immediately.
    ///
    /// This is a development/test helper, **not** a producer path: in a real
    /// application the producer writes the row in the same transaction as the
    /// business data through the storage adapter. Returns `false` when `id`
    /// already exists (the existing row is left untouched, mirroring the
    /// producer-side unique key).
    pub fn enqueue(&self, id: &str, event_name: &str, payload: &str) -> bool {
        let mut rows = self.lock();
        if rows.contains_key(id) {
            return false;
        }
        let now = unix_now_secs();
        rows.insert(
            id.to_string(),
            Row {
                event_name: event_name.to_string(),
                payload: payload.to_string(),
                attempts: 0,
                next_attempt_at: now,
                locked_until: 0,
                delivered_at: None,
                dead_lettered_at: None,
                last_error: None,
                created_at: now,
            },
        );
        true
    }

    /// Snapshot all rows ordered by id, for assertions and debugging.
    pub fn records(&self) -> Vec<OutboxRecord> {
        self.lock()
            .iter()
            .map(|(id, row)| OutboxRecord {
                id: id.clone(),
                event_name: row.event_name.clone(),
                payload: row.payload.clone(),
                attempts: row.attempts,
                next_attempt_at: row.next_attempt_at,
                locked_until: row.locked_until,
                delivered_at: row.delivered_at,
                dead_lettered_at: row.dead_lettered_at,
                last_error: row.last_error.clone(),
                created_at: row.created_at,
            })
            .collect()
    }

    /// Snapshot one row.
    pub fn record(&self, id: &str) -> Option<OutboxRecord> {
        self.lock().get(id).map(|row| OutboxRecord {
            id: id.to_string(),
            event_name: row.event_name.clone(),
            payload: row.payload.clone(),
            attempts: row.attempts,
            next_attempt_at: row.next_attempt_at,
            locked_until: row.locked_until,
            delivered_at: row.delivered_at,
            dead_lettered_at: row.dead_lettered_at,
            last_error: row.last_error.clone(),
            created_at: row.created_at,
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, BTreeMap<String, Row>> {
        self.rows
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

#[async_trait]
impl IOutbox for InMemoryOutbox {
    async fn claim(&self, now: i64, limit: usize, lease_secs: i64) -> Result<Vec<OutboxEntry>> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let lease_until = now.saturating_add(lease_secs.max(1));
        let mut rows = self.lock();

        let mut due: Vec<(i64, String)> = rows
            .iter()
            .filter(|(_, row)| {
                row.delivered_at.is_none()
                    && row.dead_lettered_at.is_none()
                    && row.next_attempt_at <= now
                    && row.locked_until <= now
            })
            .map(|(id, row)| (row.next_attempt_at, id.clone()))
            .collect();
        due.sort();

        let mut claimed = Vec::with_capacity(due.len().min(limit));
        for (_, id) in due.into_iter().take(limit) {
            if let Some(row) = rows.get_mut(&id) {
                row.locked_until = lease_until;
                claimed.push(OutboxEntry {
                    id,
                    event_name: row.event_name.clone(),
                    payload: row.payload.clone(),
                    attempts: row.attempts,
                });
            }
        }
        Ok(claimed)
    }

    async fn complete(&self, id: &str) -> Result<()> {
        let now = unix_now_secs();
        let mut rows = self.lock();
        if let Some(row) = rows.get_mut(id) {
            if row.delivered_at.is_none() && row.dead_lettered_at.is_none() {
                row.delivered_at = Some(now);
                row.locked_until = 0;
            }
        }
        Ok(())
    }

    async fn fail(&self, id: &str, error: &str, retry_base_secs: i64) -> Result<()> {
        let now = unix_now_secs();
        let mut rows = self.lock();
        let Some(row) = rows.get_mut(id) else {
            return Ok(());
        };
        if row.is_finished() {
            // A late failure report for an already-finished row is a no-op.
            return Ok(());
        }

        let message = truncate(error, MAX_ERROR_CHARS);
        let attempts = row.attempts.saturating_add(1);
        let dead_lettered = attempts >= self.max_attempts;
        let next_attempt_at = if dead_lettered {
            row.next_attempt_at
        } else {
            // 2^(attempts - 1) base delay: base, 2x, 4x, ... capped.
            let exponent =
                u32::try_from(attempts.saturating_sub(1).min(MAX_BACKOFF_EXPONENT as i32))
                    .unwrap_or(0);
            let delay = retry_base_secs
                .max(1)
                .saturating_mul(2i64.saturating_pow(exponent));
            now.saturating_add(delay)
        };

        row.attempts = attempts;
        row.locked_until = 0;
        row.last_error = Some(message.clone());
        row.next_attempt_at = next_attempt_at;
        if dead_lettered {
            row.dead_lettered_at = Some(now);
        }
        Ok(())
    }

    async fn dead_letter(&self, id: &str, error: &str) -> Result<()> {
        let now = unix_now_secs();
        let mut rows = self.lock();
        if let Some(row) = rows.get_mut(id) {
            if row.delivered_at.is_none() && row.dead_lettered_at.is_none() {
                row.dead_lettered_at = Some(now);
                row.locked_until = 0;
                row.last_error = Some(truncate(error, MAX_ERROR_CHARS));
            }
        }
        Ok(())
    }

    async fn purge(&self, before: i64) -> Result<u64> {
        let mut rows = self.lock();
        let previous = rows.len();
        rows.retain(|_, row| {
            !(row.delivered_at.is_some_and(|at| at < before)
                || row.dead_lettered_at.is_some_and(|at| at < before))
        });
        Ok(u64::try_from(previous.saturating_sub(rows.len())).unwrap_or(u64::MAX))
    }
}

impl Row {
    fn is_finished(&self) -> bool {
        self.delivered_at.is_some() || self.dead_lettered_at.is_some()
    }
}

/// Cap `text` at `max` characters on a character boundary.
fn truncate(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}
