//! Quota reservation and settlement.
//!
//! A gateway charges several dimensions at once — requests per minute, tokens
//! per minute/day, concurrent streams, and money. Checking a limit and then
//! charging after the request is racy: two concurrent requests can both pass the
//! check and oversell. This module therefore uses **reserve → settle/release**:
//!
//! 1. [`IQuotaStore::reserve`] charges the estimate and (for streams) takes a
//!    concurrency slot, atomically.
//! 2. [`IQuotaStore::settle`] replaces the estimate with the real usage/cost and
//!    releases the slot.
//! 3. [`IQuotaStore::release`] refunds an unused reservation when the upstream
//!    call never started.
//!
//! `settle` and `release` are idempotent on the lease id, so a retried
//! settlement cannot double-charge.
//!
//! The in-memory implementation is for a single process. A distributed store
//! (Redis Lua, database transaction) implements the same port; the hot path
//! should cache limits instead of reading them per request.

use crate::usage::Usage;
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use thiserror::Error;

/// Default lease lifetime when the caller does not set one.
///
/// A lease bounds how long a crashed request can hold a concurrency slot. Token
/// and cost estimates stay charged on expiry (conservative, never undercharge);
/// only the slot is released.
pub const DEFAULT_LEASE_TTL: Duration = Duration::from_secs(60);

const MINUTE: Duration = Duration::from_secs(60);
const DAY_SECS: u64 = 86_400;

/// Identity a quota applies to: tenant + API key + model.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct QuotaKey {
    /// Tenant that owns the key.
    pub tenant_id: String,
    /// API key id (never the key itself).
    pub key_id: String,
    /// Public model/alias.
    pub model: String,
}

impl QuotaKey {
    /// Build a key.
    pub fn new(
        tenant_id: impl Into<String>,
        key_id: impl Into<String>,
        model: impl Into<String>,
    ) -> Self {
        Self {
            tenant_id: tenant_id.into(),
            key_id: key_id.into(),
            model: model.into(),
        }
    }
}

/// A dimension a quota can exhaust.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuotaDimension {
    /// Requests in the current minute.
    RequestsPerMinute,
    /// Tokens in the current minute.
    TokensPerMinute,
    /// Tokens in the current UTC day.
    TokensPerDay,
    /// Concurrent streams.
    ConcurrentStreams,
    /// Money spent, in micro-USD.
    Budget,
}

/// Limits for one [`QuotaKey`]. `None` means unlimited.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct QuotaLimits {
    /// Request count per fixed minute.
    pub requests_per_minute: Option<u64>,
    /// Token count per fixed minute.
    pub tokens_per_minute: Option<u64>,
    /// Token count per UTC day.
    pub tokens_per_day: Option<u64>,
    /// Maximum concurrent streams.
    pub concurrent_streams: Option<u32>,
    /// Maximum spend in integer micro-USD.
    pub budget_micros: Option<u64>,
}

impl QuotaLimits {
    /// No limits at all.
    pub fn unlimited() -> Self {
        Self::default()
    }

    /// Set the per-minute request limit.
    pub fn requests_per_minute(mut self, limit: u64) -> Self {
        self.requests_per_minute = Some(limit);
        self
    }

    /// Set the per-minute token limit.
    pub fn tokens_per_minute(mut self, limit: u64) -> Self {
        self.tokens_per_minute = Some(limit);
        self
    }

    /// Set the per-day token limit.
    pub fn tokens_per_day(mut self, limit: u64) -> Self {
        self.tokens_per_day = Some(limit);
        self
    }

    /// Set the concurrent-stream limit.
    pub fn concurrent_streams(mut self, limit: u32) -> Self {
        self.concurrent_streams = Some(limit);
        self
    }

    /// Set the spend limit in micro-USD.
    pub fn budget_micros(mut self, limit: u64) -> Self {
        self.budget_micros = Some(limit);
        self
    }
}

/// Resolve limits for a key (database/cache lookup in a real deployment).
pub trait IQuotaPolicy: Send + Sync {
    /// Limits that apply to `key`; return [`QuotaLimits::unlimited`] for none.
    fn limits_for(&self, key: &QuotaKey) -> QuotaLimits;
}

/// A fixed default plus per-key overrides.
#[derive(Debug, Clone, Default)]
pub struct StaticQuotaPolicy {
    default: QuotaLimits,
    by_key: HashMap<QuotaKey, QuotaLimits>,
}

impl StaticQuotaPolicy {
    /// A policy where every key gets `default`.
    pub fn new(default: QuotaLimits) -> Self {
        Self {
            default,
            by_key: HashMap::new(),
        }
    }

    /// Add an override for `key`.
    pub fn with_limits(mut self, key: QuotaKey, limits: QuotaLimits) -> Self {
        self.by_key.insert(key, limits);
        self
    }

    /// Add or replace an override.
    pub fn set_limits(&mut self, key: QuotaKey, limits: QuotaLimits) {
        self.by_key.insert(key, limits);
    }
}

impl IQuotaPolicy for StaticQuotaPolicy {
    fn limits_for(&self, key: &QuotaKey) -> QuotaLimits {
        self.by_key
            .get(key)
            .cloned()
            .unwrap_or_else(|| self.default.clone())
    }
}

/// A reservation request.
#[derive(Debug, Clone)]
pub struct ReserveRequest {
    /// Tenant/key/model the quota belongs to.
    pub key: QuotaKey,
    /// Tokens to charge up front (prompt + expected completion).
    pub estimated_tokens: u64,
    /// Cost to charge up front, in micro-USD.
    pub estimated_cost_micros: u64,
    /// Concurrency slots to take (0 for non-stream calls).
    pub streams: u32,
    /// Lease lifetime; defaults to [`DEFAULT_LEASE_TTL`].
    pub lease_ttl: Option<Duration>,
}

impl ReserveRequest {
    /// A request for `key` with a token estimate and no cost/slot.
    pub fn new(key: QuotaKey, estimated_tokens: u64) -> Self {
        Self {
            key,
            estimated_tokens,
            estimated_cost_micros: 0,
            streams: 0,
            lease_ttl: None,
        }
    }

    /// Take concurrency slots.
    pub fn streams(mut self, streams: u32) -> Self {
        self.streams = streams;
        self
    }

    /// Charge an estimated cost up front.
    pub fn estimated_cost_micros(mut self, cost: u64) -> Self {
        self.estimated_cost_micros = cost;
        self
    }

    /// Override the lease lifetime.
    pub fn lease_ttl(mut self, ttl: Duration) -> Self {
        self.lease_ttl = Some(ttl);
        self
    }
}

/// The handle returned by [`IQuotaStore::reserve`].
#[derive(Debug, Clone)]
pub struct Reservation {
    /// Idempotency key for settle/release.
    pub lease_id: String,
    /// Quota identity.
    pub key: QuotaKey,
    /// Tokens charged at reservation time.
    pub reserved_tokens: u64,
    /// Cost charged at reservation time.
    pub reserved_cost_micros: u64,
    /// Concurrency slots held.
    pub concurrency_streams: u32,
    /// Lease lifetime used for this reservation.
    pub lease_ttl: Duration,
}

/// Why a reservation was rejected.
#[derive(Debug, Clone, Error)]
pub enum QuotaError {
    /// A limit would be exceeded.
    #[error("quota exceeded: {dimension:?} (limit {limit})")]
    Exceeded {
        /// The exhausted dimension.
        dimension: QuotaDimension,
        /// The configured limit.
        limit: u64,
        /// Hint for when the dimension resets; `None` means "wait for a slot or
        /// an operator change".
        retry_after: Option<Duration>,
    },
    /// The store itself failed (network/database).
    #[error("quota store unavailable: {0}")]
    Unavailable(String),
    /// The request is invalid (for example a zero token estimate where one is
    /// required).
    #[error("invalid quota request: {0}")]
    Invalid(String),
}

impl QuotaError {
    /// Whether this is a limit rejection (map to `429`/OpenAI
    /// `insufficient_quota`) rather than an infrastructure failure.
    pub fn is_exceeded(&self) -> bool {
        matches!(self, Self::Exceeded { .. })
    }

    /// Hint for when the exhausted dimension resets.
    pub fn retry_after(&self) -> Option<Duration> {
        match self {
            Self::Exceeded { retry_after, .. } => *retry_after,
            _ => None,
        }
    }

    /// Suggested HTTP status: `429` for a limit, `503` for an unavailable
    /// store, `400` for an invalid request.
    pub fn http_status(&self) -> u16 {
        match self {
            Self::Exceeded { .. } => 429,
            Self::Unavailable(_) => 503,
            Self::Invalid(_) => 400,
        }
    }
}

/// Reservation store port.
#[async_trait]
pub trait IQuotaStore: Send + Sync {
    /// Reserve `request` atomically or fail with [`QuotaError`].
    async fn reserve(&self, request: ReserveRequest) -> Result<Reservation, QuotaError>;

    /// Replace the estimate with real usage/cost and release the concurrency
    /// slot. Idempotent: a repeated call for the same lease is a no-op.
    async fn settle(
        &self,
        reservation: Reservation,
        usage: Usage,
        cost_micros: u64,
    ) -> Result<(), QuotaError>;

    /// Refund the estimate and release the concurrency slot (the request never
    /// produced billable output). Idempotent.
    async fn release(&self, reservation: Reservation) -> Result<(), QuotaError>;
}

/// Single-process quota store.
///
/// Fixed windows: the minute starts at the first reservation of that minute and
/// resets 60s later; the day is UTC. This is intentionally simple and is not a
/// distributed limiter — implement [`IQuotaStore`] over Redis/a database when
/// several instances must agree.
pub struct MemoryQuotaStore {
    policy: Arc<dyn IQuotaPolicy>,
    inner: Mutex<Inner>,
}

struct Inner {
    states: HashMap<QuotaKey, KeyState>,
    next_lease: u64,
}

struct KeyState {
    minute_start: Instant,
    minute_requests: u64,
    minute_tokens: u64,
    day_index: u64,
    day_tokens: u64,
    concurrent: u32,
    spent_micros: u64,
    active: HashMap<String, ActiveLease>,
}

struct ActiveLease {
    expires_at: Instant,
    reserved_tokens: u64,
    reserved_cost_micros: u64,
    streams: u32,
}

impl KeyState {
    fn new(now: Instant, day_index: u64) -> Self {
        Self {
            minute_start: now,
            minute_requests: 0,
            minute_tokens: 0,
            day_index,
            day_tokens: 0,
            concurrent: 0,
            spent_micros: 0,
            active: HashMap::new(),
        }
    }

    fn roll(&mut self, now: Instant, day_index: u64) {
        if now.saturating_duration_since(self.minute_start) >= MINUTE {
            self.minute_start = now;
            self.minute_requests = 0;
            self.minute_tokens = 0;
        }
        if self.day_index != day_index {
            self.day_index = day_index;
            self.day_tokens = 0;
        }
    }

    fn minute_retry_after(&self, now: Instant) -> Duration {
        let elapsed = now.saturating_duration_since(self.minute_start).as_secs();
        Duration::from_secs(MINUTE.as_secs().saturating_sub(elapsed).max(1))
    }
}

impl Inner {
    /// Drop expired leases. Token/cost estimates stay charged; only the
    /// concurrency slot is released.
    fn purge_expired(&mut self, now: Instant) {
        for state in self.states.values_mut() {
            let expired: Vec<String> = state
                .active
                .iter()
                .filter(|(_, lease)| lease.expires_at <= now)
                .map(|(id, _)| id.clone())
                .collect();
            for id in expired {
                if let Some(lease) = state.active.remove(&id) {
                    state.concurrent = state.concurrent.saturating_sub(lease.streams);
                }
            }
        }
    }
}

impl MemoryQuotaStore {
    /// Build a store from a policy.
    pub fn new(policy: Arc<dyn IQuotaPolicy>) -> Self {
        Self {
            policy,
            inner: Mutex::new(Inner {
                states: HashMap::new(),
                next_lease: 0,
            }),
        }
    }
}

#[async_trait]
impl IQuotaStore for MemoryQuotaStore {
    async fn reserve(&self, request: ReserveRequest) -> Result<Reservation, QuotaError> {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let now = Instant::now();
        let day_index = current_day_index();
        inner.purge_expired(now);

        let limits = self.policy.limits_for(&request.key);
        let lease_seq = inner.next_lease;
        inner.next_lease = inner.next_lease.wrapping_add(1);

        let state = inner
            .states
            .entry(request.key.clone())
            .or_insert_with(|| KeyState::new(now, day_index));
        state.roll(now, day_index);

        if let Some(limit) = limits.requests_per_minute {
            if state.minute_requests.saturating_add(1) > limit {
                return Err(QuotaError::Exceeded {
                    dimension: QuotaDimension::RequestsPerMinute,
                    limit,
                    retry_after: Some(state.minute_retry_after(now)),
                });
            }
        }
        if let Some(limit) = limits.tokens_per_minute {
            if state.minute_tokens.saturating_add(request.estimated_tokens) > limit {
                return Err(QuotaError::Exceeded {
                    dimension: QuotaDimension::TokensPerMinute,
                    limit,
                    retry_after: Some(state.minute_retry_after(now)),
                });
            }
        }
        if let Some(limit) = limits.tokens_per_day {
            if state.day_tokens.saturating_add(request.estimated_tokens) > limit {
                return Err(QuotaError::Exceeded {
                    dimension: QuotaDimension::TokensPerDay,
                    limit,
                    retry_after: Some(day_retry_after()),
                });
            }
        }
        if let Some(limit) = limits.concurrent_streams {
            if u64::from(state.concurrent).saturating_add(u64::from(request.streams))
                > u64::from(limit)
            {
                return Err(QuotaError::Exceeded {
                    dimension: QuotaDimension::ConcurrentStreams,
                    limit: u64::from(limit),
                    retry_after: None,
                });
            }
        }
        if let Some(limit) = limits.budget_micros {
            if state
                .spent_micros
                .saturating_add(request.estimated_cost_micros)
                > limit
            {
                return Err(QuotaError::Exceeded {
                    dimension: QuotaDimension::Budget,
                    limit,
                    retry_after: None,
                });
            }
        }

        // Commit the reservation.
        state.minute_requests = state.minute_requests.saturating_add(1);
        state.minute_tokens = state.minute_tokens.saturating_add(request.estimated_tokens);
        state.day_tokens = state.day_tokens.saturating_add(request.estimated_tokens);
        state.concurrent = state.concurrent.saturating_add(request.streams);
        state.spent_micros = state
            .spent_micros
            .saturating_add(request.estimated_cost_micros);

        let lease_ttl = request.lease_ttl.unwrap_or(DEFAULT_LEASE_TTL);
        let lease_id = format!("{}-{lease_seq}", request.key.key_id);
        state.active.insert(
            lease_id.clone(),
            ActiveLease {
                expires_at: now + lease_ttl,
                reserved_tokens: request.estimated_tokens,
                reserved_cost_micros: request.estimated_cost_micros,
                streams: request.streams,
            },
        );

        Ok(Reservation {
            lease_id,
            key: request.key,
            reserved_tokens: request.estimated_tokens,
            reserved_cost_micros: request.estimated_cost_micros,
            concurrency_streams: request.streams,
            lease_ttl,
        })
    }

    async fn settle(
        &self,
        reservation: Reservation,
        usage: Usage,
        cost_micros: u64,
    ) -> Result<(), QuotaError> {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let now = Instant::now();
        let day_index = current_day_index();
        inner.purge_expired(now);

        let Some(state) = inner.states.get_mut(&reservation.key) else {
            return Ok(());
        };
        state.roll(now, day_index);
        let Some(lease) = state.active.remove(&reservation.lease_id) else {
            // Already settled/released or expired: idempotent no-op.
            return Ok(());
        };

        state.concurrent = state.concurrent.saturating_sub(lease.streams);
        let actual_tokens = usage.total_tokens();
        adjust(
            &mut state.minute_tokens,
            lease.reserved_tokens,
            actual_tokens,
        );
        adjust(&mut state.day_tokens, lease.reserved_tokens, actual_tokens);
        adjust(
            &mut state.spent_micros,
            lease.reserved_cost_micros,
            cost_micros,
        );
        Ok(())
    }

    async fn release(&self, reservation: Reservation) -> Result<(), QuotaError> {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        inner.purge_expired(Instant::now());

        let Some(state) = inner.states.get_mut(&reservation.key) else {
            return Ok(());
        };
        let Some(lease) = state.active.remove(&reservation.lease_id) else {
            return Ok(());
        };

        state.concurrent = state.concurrent.saturating_sub(lease.streams);
        state.minute_tokens = state.minute_tokens.saturating_sub(lease.reserved_tokens);
        state.day_tokens = state.day_tokens.saturating_sub(lease.reserved_tokens);
        state.spent_micros = state
            .spent_micros
            .saturating_sub(lease.reserved_cost_micros);
        Ok(())
    }
}

/// Apply `actual` in place of `reserved`: refund when lower, charge the delta
/// when higher.
fn adjust(counter: &mut u64, reserved: u64, actual: u64) {
    if actual >= reserved {
        *counter = counter.saturating_add(actual - reserved);
    } else {
        *counter = counter.saturating_sub(reserved - actual);
    }
}

fn current_day_index() -> u64 {
    unix_secs() / DAY_SECS
}

fn unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or_default()
}

fn day_retry_after() -> Duration {
    let seconds = unix_secs() % DAY_SECS;
    Duration::from_secs(DAY_SECS.saturating_sub(seconds).max(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> QuotaKey {
        QuotaKey::new("tenant-1", "key-1", "model-1")
    }

    fn store(limits: QuotaLimits) -> MemoryQuotaStore {
        MemoryQuotaStore::new(Arc::new(StaticQuotaPolicy::new(limits)))
    }

    #[tokio::test]
    async fn requests_per_minute_exhausts() {
        let store = store(QuotaLimits::default().requests_per_minute(2));

        store
            .reserve(ReserveRequest::new(key(), 0))
            .await
            .expect("first request");
        store
            .reserve(ReserveRequest::new(key(), 0))
            .await
            .expect("second request");
        let error = store
            .reserve(ReserveRequest::new(key(), 0))
            .await
            .expect_err("third request is rejected");

        assert!(matches!(
            error,
            QuotaError::Exceeded {
                dimension: QuotaDimension::RequestsPerMinute,
                limit: 2,
                retry_after: Some(_)
            }
        ));
        assert!(error.is_exceeded());
    }

    #[tokio::test]
    async fn settle_replaces_the_token_estimate() {
        let store = store(QuotaLimits::default().tokens_per_minute(100));

        let reservation = store
            .reserve(ReserveRequest::new(key(), 80))
            .await
            .expect("reserves the estimate");
        let error = store
            .reserve(ReserveRequest::new(key(), 30))
            .await
            .expect_err("estimate exceeds the minute budget");
        assert!(matches!(
            error,
            QuotaError::Exceeded {
                dimension: QuotaDimension::TokensPerMinute,
                ..
            }
        ));

        store
            .settle(reservation, Usage::new(10, 10), 0)
            .await
            .expect("settles to actual usage");

        store
            .reserve(ReserveRequest::new(key(), 30))
            .await
            .expect("40 tokens used, 30 more fits");
    }

    #[tokio::test]
    async fn concurrent_streams_are_slots() {
        let store = store(QuotaLimits::default().concurrent_streams(1));

        let first = store
            .reserve(ReserveRequest::new(key(), 0).streams(1))
            .await
            .expect("first stream takes the slot");
        let error = store
            .reserve(ReserveRequest::new(key(), 0).streams(1))
            .await
            .expect_err("second stream has no slot");
        assert!(matches!(
            error,
            QuotaError::Exceeded {
                dimension: QuotaDimension::ConcurrentStreams,
                limit: 1,
                retry_after: None
            }
        ));

        store
            .settle(first, Usage::default(), 0)
            .await
            .expect("settle releases the slot");
        store
            .reserve(ReserveRequest::new(key(), 0).streams(1))
            .await
            .expect("slot is reusable");
    }

    #[tokio::test]
    async fn release_refunds_the_estimate() {
        let store = store(QuotaLimits::default().tokens_per_minute(100));

        let reservation = store
            .reserve(ReserveRequest::new(key(), 80))
            .await
            .expect("reserves");
        store.release(reservation).await.expect("releases");

        store
            .reserve(ReserveRequest::new(key(), 80))
            .await
            .expect("refunded estimate fits again");
    }

    #[tokio::test]
    async fn budget_is_charged_and_adjusted() {
        let store = store(QuotaLimits::default().budget_micros(100));

        let reservation = store
            .reserve(ReserveRequest::new(key(), 0).estimated_cost_micros(80))
            .await
            .expect("reserves the estimate");
        let error = store
            .reserve(ReserveRequest::new(key(), 0).estimated_cost_micros(30))
            .await
            .expect_err("budget would be exceeded");
        assert!(matches!(
            error,
            QuotaError::Exceeded {
                dimension: QuotaDimension::Budget,
                ..
            }
        ));

        store
            .settle(reservation, Usage::default(), 40)
            .await
            .expect("settles to the actual cost");
        store
            .reserve(ReserveRequest::new(key(), 0).estimated_cost_micros(30))
            .await
            .expect("40 spent, 30 more fits");
    }

    #[tokio::test]
    async fn settle_and_release_are_idempotent() {
        let store = store(QuotaLimits::default().tokens_per_minute(100));
        let reservation = store
            .reserve(ReserveRequest::new(key(), 10))
            .await
            .expect("reserves");

        store
            .settle(reservation.clone(), Usage::new(5, 5), 0)
            .await
            .expect("first settle");
        store
            .settle(reservation.clone(), Usage::new(5, 5), 0)
            .await
            .expect("second settle is a no-op");
        store
            .release(reservation)
            .await
            .expect("release after settle");
    }

    #[tokio::test]
    async fn per_key_overrides_win_over_the_default() {
        let policy = StaticQuotaPolicy::new(QuotaLimits::unlimited())
            .with_limits(key(), QuotaLimits::default().requests_per_minute(1));
        let store = MemoryQuotaStore::new(Arc::new(policy));

        store
            .reserve(ReserveRequest::new(key(), 0))
            .await
            .expect("first request");
        assert!(matches!(
            store
                .reserve(ReserveRequest::new(key(), 0))
                .await
                .expect_err("the key has its own limit"),
            QuotaError::Exceeded {
                dimension: QuotaDimension::RequestsPerMinute,
                ..
            }
        ));

        // A different key falls back to unlimited.
        let other = QuotaKey::new("tenant-2", "key-2", "model-1");
        store
            .reserve(ReserveRequest::new(other, 0))
            .await
            .expect("unlimited default");
    }

    #[tokio::test]
    async fn expired_leases_release_concurrency_slots() {
        let store = store(QuotaLimits::default().concurrent_streams(1));
        store
            .reserve(
                ReserveRequest::new(key(), 0)
                    .streams(1)
                    .lease_ttl(Duration::from_millis(1)),
            )
            .await
            .expect("first stream");

        tokio::time::sleep(Duration::from_millis(30)).await;

        store
            .reserve(ReserveRequest::new(key(), 0).streams(1))
            .await
            .expect("expired lease freed the slot");
    }

    #[test]
    fn error_maps_to_http_status_and_retry_after() {
        let exceeded = QuotaError::Exceeded {
            dimension: QuotaDimension::TokensPerMinute,
            limit: 10,
            retry_after: Some(Duration::from_secs(5)),
        };
        assert!(exceeded.is_exceeded());
        assert_eq!(exceeded.http_status(), 429);
        assert_eq!(exceeded.retry_after(), Some(Duration::from_secs(5)));

        assert_eq!(QuotaError::Unavailable("down".into()).http_status(), 503);
        assert_eq!(QuotaError::Invalid("bad".into()).http_status(), 400);
        assert_eq!(QuotaError::Unavailable("down".into()).retry_after(), None);
    }
}
