//! Clock port (2.4-5).
//!
//! Application code that needs "now" (audit timestamps, expiries, scheduling
//! decisions, quota windows) injects [`IClock`] instead of calling
//! `SystemTime::now()` directly. Production resolves [`SystemClock`]; tests
//! resolve [`TestClock`] and move time deterministically — no sleeps, no
//! flakiness.
//!
//! ```ignore
//! #[derive(Inject)]
//! struct CreateOrderHandler {
//!     #[inject]
//!     clock: Arc<dyn IClock>,
//! }
//! ```
//!
//! Monotonic durations (timeouts, retries, backoff) are **not** clock-based:
//! they stay on tokio timers, which `tokio::time::pause()` /
//! `#[tokio::test(start_paused = true)]` controls deterministically.

use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Time source injection point.
pub trait IClock: Send + Sync {
    /// Current wall-clock time.
    fn now(&self) -> SystemTime;

    /// Current unix time in seconds (clamped at the epoch).
    fn now_unix(&self) -> u64 {
        self.now()
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| elapsed.as_secs())
            .unwrap_or(0)
    }

    /// Current unix time in milliseconds (clamped at the epoch).
    fn now_unix_millis(&self) -> u64 {
        self.now()
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| elapsed.as_millis() as u64)
            .unwrap_or(0)
    }
}

/// Production clock: reads the system wall clock.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl IClock for SystemClock {
    fn now(&self) -> SystemTime {
        SystemTime::now()
    }
}

/// Controllable clock for tests and simulations.
///
/// Starts at a fixed instant and only moves when [`TestClock::advance`] or
/// [`TestClock::set`] is called.
#[derive(Debug)]
pub struct TestClock {
    now: Mutex<SystemTime>,
}

impl TestClock {
    /// Clock frozen at `at`.
    pub fn new(at: SystemTime) -> Self {
        Self {
            now: Mutex::new(at),
        }
    }

    /// Clock frozen at `unix_seconds` after the epoch.
    pub fn from_unix(unix_seconds: u64) -> Self {
        Self::new(UNIX_EPOCH + Duration::from_secs(unix_seconds))
    }

    /// Move the clock forward by `by`.
    pub fn advance(&self, by: Duration) {
        let mut now = self
            .now
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *now = now
            .checked_add(by)
            .unwrap_or_else(|| UNIX_EPOCH + Duration::from_secs(u64::MAX));
    }

    /// Set the clock to an absolute instant.
    pub fn set(&self, at: SystemTime) {
        let mut now = self
            .now
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *now = at;
    }
}

impl Default for TestClock {
    fn default() -> Self {
        Self::new(SystemTime::now())
    }
}

impl IClock for TestClock {
    fn now(&self) -> SystemTime {
        *self
            .now
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_clock_only_moves_when_told_to() {
        let clock = TestClock::from_unix(1_700_000_000);
        assert_eq!(clock.now_unix(), 1_700_000_000);
        assert_eq!(clock.now_unix_millis(), 1_700_000_000_000);
        clock.advance(Duration::from_secs(90));
        assert_eq!(clock.now_unix(), 1_700_000_090, "no wall-clock drift");
        clock.set(UNIX_EPOCH + Duration::from_secs(5));
        assert_eq!(clock.now_unix(), 5);
    }

    #[test]
    fn system_clock_tracks_wall_time() {
        let clock = SystemClock;
        let first = clock.now_unix();
        let second = SystemClock.now_unix();
        assert!(second >= first);
    }
}
