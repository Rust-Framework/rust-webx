//! Configuration for the durable payment webhook inbox.

use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Payment webhook inbox options.
///
/// Bind it from configuration (`HostBuilder::add_options::<PaymentInboxOptions>("PaymentInbox")`
/// plus `APP__PaymentInbox__*` overrides) or build it in code. All durations are
/// whole seconds; `0` is normalised to `1` at use time instead of failing
/// startup.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct PaymentInboxOptions {
    /// Run the background drain worker. The [`IPaymentInbox`] port is always
    /// registered, so webhook handlers can still persist events when draining is
    /// disabled (for example during a controlled replay).
    ///
    /// [`IPaymentInbox`]: crate::IPaymentInbox
    pub enabled: bool,
    /// Worker tick period in seconds.
    pub interval_seconds: u64,
    /// Rows claimed per tick (bounded so a tick stays short).
    pub batch_size: usize,
    /// Claim lease in seconds. A worker that dies mid-delivery leaves the row
    /// leased for at most this long before another worker may reclaim it.
    pub lease_seconds: i64,
    /// Failed deliveries before a row is dead-lettered.
    pub max_attempts: i32,
    /// Keep processed rows for this many days (`0` disables retention cleanup).
    pub retention_days: u64,
}

impl Default for PaymentInboxOptions {
    fn default() -> Self {
        Self {
            enabled: true,
            interval_seconds: 5,
            batch_size: 32,
            lease_seconds: 60,
            max_attempts: 6,
            retention_days: 30,
        }
    }
}

impl PaymentInboxOptions {
    /// Default options (worker enabled, 5s tick, 32 rows, 60s lease, 6 attempts,
    /// 30-day retention).
    pub fn new() -> Self {
        Self::default()
    }

    /// Keep the inbox registrations but do not start the background worker.
    pub fn disabled(mut self) -> Self {
        self.enabled = false;
        self
    }

    /// Worker tick period.
    pub fn interval(&self) -> Duration {
        Duration::from_secs(self.interval_seconds.max(1))
    }

    /// Rows claimed per tick.
    pub fn batch_size(&self) -> usize {
        self.batch_size.max(1)
    }

    /// Claim lease length in seconds.
    pub fn lease_seconds(&self) -> i64 {
        self.lease_seconds.max(1)
    }

    /// Maximum failed deliveries before dead-lettering.
    pub fn max_attempts(&self) -> i32 {
        self.max_attempts.max(1)
    }

    /// Base retry delay in seconds (doubled per attempt by the inbox).
    pub fn retry_base_seconds(&self) -> i64 {
        i64::try_from(self.interval_seconds.max(1)).unwrap_or(i64::MAX)
    }

    /// Processed-row retention in seconds (`0` disables cleanup).
    pub fn retention_seconds(&self) -> i64 {
        i64::try_from(
            self.retention_days
                .min(u64::MAX / 86_400)
                .saturating_mul(86_400),
        )
        .unwrap_or(i64::MAX)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_sane() {
        let options = PaymentInboxOptions::default();
        assert!(options.enabled);
        assert_eq!(options.interval(), Duration::from_secs(5));
        assert_eq!(options.batch_size(), 32);
        assert_eq!(options.lease_seconds(), 60);
        assert_eq!(options.max_attempts(), 6);
        assert_eq!(options.retention_seconds(), 30 * 86_400);
    }

    #[test]
    fn zero_values_are_normalised() {
        let options = PaymentInboxOptions {
            enabled: false,
            interval_seconds: 0,
            batch_size: 0,
            lease_seconds: 0,
            max_attempts: 0,
            retention_days: 0,
        };
        assert_eq!(options.interval(), Duration::from_secs(1));
        assert_eq!(options.batch_size(), 1);
        assert_eq!(options.lease_seconds(), 1);
        assert_eq!(options.max_attempts(), 1);
        assert_eq!(options.retry_base_seconds(), 1);
        assert_eq!(options.retention_seconds(), 0);
    }

    #[test]
    fn retention_days_cannot_overflow() {
        let options = PaymentInboxOptions {
            retention_days: u64::MAX,
            ..PaymentInboxOptions::default()
        };
        assert_eq!(options.retention_seconds(), i64::MAX);
    }
}
