//! Outbox options.

use serde::{Deserialize, Serialize};

/// Transactional outbox worker options.
///
/// Bind from configuration (`HostBuilder::add_options::<OutboxOptions>("Outbox")`
/// plus `APP__Outbox__*`) or build in code. All durations are whole seconds.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct OutboxOptions {
    /// Run the background dispatcher. The `IOutbox` port is always registered,
    /// so producers still enqueue when draining is disabled (controlled replay).
    pub enabled: bool,
    /// Dispatcher tick period in seconds.
    pub interval_seconds: u64,
    /// Rows claimed per tick (bounded so a tick stays short).
    pub batch_size: usize,
    /// Claim lease in seconds: a dispatcher that dies mid-delivery leaves the row
    /// leased for at most this long before another worker may reclaim it.
    pub lease_seconds: i64,
    /// Failed deliveries before a row is dead-lettered.
    pub max_attempts: i32,
    /// Base retry delay in seconds (doubles per attempt, capped).
    pub retry_base_seconds: i64,
    /// Keep delivered/dead rows for this many days (`0` disables cleanup).
    pub retention_days: u64,
}

impl Default for OutboxOptions {
    fn default() -> Self {
        Self {
            enabled: true,
            interval_seconds: 5,
            batch_size: 32,
            lease_seconds: 60,
            max_attempts: 6,
            retry_base_seconds: 2,
            retention_days: 7,
        }
    }
}

impl OutboxOptions {
    /// Default options (worker enabled, 5s tick, 32 rows, 60s lease, 6 attempts,
    /// 2s base backoff, 7-day retention).
    pub fn new() -> Self {
        Self::default()
    }

    /// Keep the port registration but do not start the dispatcher.
    pub fn disabled(mut self) -> Self {
        self.enabled = false;
        self
    }

    /// Worker tick interval (at least 1s).
    pub fn interval(&self) -> std::time::Duration {
        std::time::Duration::from_secs(self.interval_seconds.max(1))
    }

    /// Rows claimed per tick (at least 1).
    pub fn batch_size(&self) -> usize {
        self.batch_size.max(1)
    }

    /// Claim lease in seconds (at least 1).
    pub fn lease_seconds(&self) -> i64 {
        self.lease_seconds.max(1)
    }

    /// Failed deliveries before dead-letter (at least 1).
    pub fn max_attempts(&self) -> i32 {
        self.max_attempts.max(1)
    }

    /// Base retry delay in seconds (at least 1).
    pub fn retry_base_seconds(&self) -> i64 {
        self.retry_base_seconds.max(1)
    }

    /// Retention window in seconds (`0` disables cleanup).
    pub fn retention_seconds(&self) -> i64 {
        i64::try_from(self.retention_days)
            .unwrap_or(i64::MAX / 86_400)
            .saturating_mul(86_400)
    }
}
