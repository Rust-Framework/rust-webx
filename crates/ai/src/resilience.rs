//! P1 resilience primitives: retry/backoff, circuit breaker, bulkhead.
//!
//! These are deliberately small and dependency-free (besides tokio): the
//! executor composes them; providers do not know about them.

use crate::provider::{ModelKey, ProviderError};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use thiserror::Error;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

/// Retry/backoff policy for one request.
#[derive(Debug, Clone, Copy)]
pub struct RetryPolicy {
    max_attempts: usize,
    base_delay: Duration,
    max_delay: Duration,
    jitter: bool,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            base_delay: Duration::from_millis(100),
            max_delay: Duration::from_secs(2),
            jitter: true,
        }
    }
}

impl RetryPolicy {
    /// A policy that never waits between attempts (tests, local providers).
    pub fn immediate() -> Self {
        Self {
            max_attempts: 3,
            base_delay: Duration::ZERO,
            max_delay: Duration::ZERO,
            jitter: false,
        }
    }

    /// Override the attempt cap (must be at least 1).
    pub fn with_max_attempts(mut self, max_attempts: usize) -> Self {
        self.max_attempts = max_attempts.max(1);
        self
    }

    /// Override the exponential backoff base.
    pub fn with_base_delay(mut self, base_delay: Duration) -> Self {
        self.base_delay = base_delay;
        self
    }

    /// Override the backoff cap.
    pub fn with_max_delay(mut self, max_delay: Duration) -> Self {
        self.max_delay = max_delay;
        self
    }

    /// Enable or disable full jitter.
    pub fn with_jitter(mut self, jitter: bool) -> Self {
        self.jitter = jitter;
        self
    }

    /// Maximum provider attempts for one request.
    pub fn max_attempts(&self) -> usize {
        self.max_attempts
    }

    /// Delay before attempt `attempt` (1-based). `retry_after` wins when set.
    pub fn delay(&self, attempt: usize, retry_after: Option<Duration>) -> Duration {
        if let Some(retry_after) = retry_after {
            return retry_after;
        }
        let exponent = attempt.saturating_sub(1).min(31) as u32;
        let backoff = self
            .base_delay
            .saturating_mul(2u32.saturating_pow(exponent))
            .min(self.max_delay);
        if !self.jitter || backoff.is_zero() {
            return backoff;
        }
        // 50%..100% of the capped backoff; deterministic enough for a retry hint.
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| elapsed.subsec_nanos())
            .unwrap_or_default();
        let factor = 0.5 + f64::from(nanos % 1000) / 2000.0;
        backoff.mul_f64(factor)
    }

    /// Whether a retryable error should be retried after `attempt` (1-based).
    pub fn should_retry(&self, attempt: usize, error: &ProviderError) -> bool {
        attempt < self.max_attempts && error.is_retryable()
    }
}

/// Observable circuit state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CircuitState {
    /// Normal traffic.
    Closed,
    /// Rejecting traffic until the cooldown ends.
    Open,
    /// One probe is allowed.
    HalfOpen,
}

/// One model's circuit state, for readiness/metrics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CircuitSnapshot {
    /// Upstream key.
    pub model: ModelKey,
    /// Effective state at the snapshot instant (an expired cooldown reports
    /// `HalfOpen`, because the next request is allowed as the probe).
    pub state: CircuitState,
}

#[derive(Debug, Clone, Copy)]
enum State {
    Closed { failures: u32 },
    Open { until: Instant },
    /// A single probe window. `probe_until` bounds the probe so a caller that
    /// obtains permission but never records a result (provider hot-reloaded
    /// away, bulkhead full, task cancelled) cannot wedge the circuit forever.
    /// Once the window elapses, the next request is allowed to probe again.
    HalfOpen { probe_until: Instant },
}

/// Per-model circuit breaker.
pub struct CircuitBreaker {
    failure_threshold: u32,
    cooldown: Duration,
    states: Mutex<HashMap<ModelKey, State>>,
}

impl CircuitBreaker {
    /// A breaker that opens after `failure_threshold` consecutive failures and
    /// probes again after `cooldown`.
    pub fn new(failure_threshold: u32, cooldown: Duration) -> Self {
        Self {
            failure_threshold: failure_threshold.max(1),
            cooldown,
            states: Mutex::new(HashMap::new()),
        }
    }

    /// Whether a request may be attempted now.
    pub fn allow(&self, key: &ModelKey) -> bool {
        let mut states = self.lock();
        match states.get(key).copied() {
            None | Some(State::Closed { .. }) => true,
            Some(State::Open { until }) => {
                if Instant::now() >= until {
                    states.insert(
                        key.clone(),
                        State::HalfOpen {
                            probe_until: Instant::now() + self.cooldown,
                        },
                    );
                    true
                } else {
                    false
                }
            }
            Some(State::HalfOpen { probe_until }) => {
                if Instant::now() < probe_until {
                    false
                } else {
                    // The previous probe never reported back; start a fresh
                    // window instead of wedging the circuit permanently.
                    states.insert(
                        key.clone(),
                        State::HalfOpen {
                            probe_until: Instant::now() + self.cooldown,
                        },
                    );
                    true
                }
            }
        }
    }

    /// Record a successful call.
    pub fn record_success(&self, key: &ModelKey) {
        self.lock()
            .insert(key.clone(), State::Closed { failures: 0 });
    }

    /// Abandon an in-flight probe without counting it as success or failure.
    ///
    /// Call this when a previously permitted attempt is given up before it
    /// reaches the provider (provider hot-reloaded away, bulkhead full). It
    /// releases the half-open window so the model is not locked out waiting for
    /// a result that will never arrive.
    pub fn release_probe(&self, key: &ModelKey) {
        let mut states = self.lock();
        if matches!(states.get(key), Some(State::HalfOpen { .. })) {
            // Expire the probe immediately; the next request may try again.
            states.insert(
                key.clone(),
                State::HalfOpen {
                    probe_until: Instant::now(),
                },
            );
        }
    }

    /// Record a failed call; opens the circuit at the threshold.
    pub fn record_failure(&self, key: &ModelKey) {
        let mut states = self.lock();
        let next = match states.get(key).copied() {
            Some(State::HalfOpen { .. }) => State::Open {
                until: Instant::now() + self.cooldown,
            },
            Some(State::Closed { failures }) => {
                let failures = failures.saturating_add(1);
                if failures >= self.failure_threshold {
                    State::Open {
                        until: Instant::now() + self.cooldown,
                    }
                } else {
                    State::Closed { failures }
                }
            }
            Some(State::Open { until }) => State::Open { until },
            None => {
                if self.failure_threshold <= 1 {
                    State::Open {
                        until: Instant::now() + self.cooldown,
                    }
                } else {
                    State::Closed { failures: 1 }
                }
            }
        };
        states.insert(key.clone(), next);
    }

    /// Current state (for metrics/admin).
    pub fn state(&self, key: &ModelKey) -> CircuitState {
        match self.lock().get(key).copied() {
            None | Some(State::Closed { .. }) => CircuitState::Closed,
            Some(State::Open { .. }) => CircuitState::Open,
            Some(State::HalfOpen { .. }) => CircuitState::HalfOpen,
        }
    }

    /// Snapshot every tracked model. An `Open` circuit whose cooldown elapsed
    /// reports `HalfOpen`: the next request becomes the probe.
    pub fn snapshot(&self) -> Vec<CircuitSnapshot> {
        let now = Instant::now();
        self.lock()
            .iter()
            .map(|(model, state)| CircuitSnapshot {
                model: model.clone(),
                state: match state {
                    State::Closed { .. } => CircuitState::Closed,
                    State::HalfOpen { .. } => CircuitState::HalfOpen,
                    State::Open { until } if now >= *until => CircuitState::HalfOpen,
                    State::Open { .. } => CircuitState::Open,
                },
            })
            .collect()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<ModelKey, State>> {
        self.states
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// Bulkhead failure.
#[derive(Debug, Error)]
pub enum BulkheadError {
    /// The provider already has `max_concurrent` in-flight calls.
    #[error("provider concurrency limit reached: {provider}")]
    Full {
        /// Provider id.
        provider: String,
    },
    /// The semaphore was closed.
    #[error("provider bulkhead closed: {provider}")]
    Closed {
        /// Provider id.
        provider: String,
    },
}

/// A held concurrency slot; dropping it releases the slot.
pub struct BulkheadPermit {
    _permit: OwnedSemaphorePermit,
}

impl std::fmt::Debug for BulkheadPermit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BulkheadPermit").finish_non_exhaustive()
    }
}

/// Per-provider concurrency limit.
pub struct Bulkhead {
    max_concurrent: usize,
    max_wait: Duration,
    providers: Mutex<HashMap<String, Arc<Semaphore>>>,
}

impl Bulkhead {
    /// Cap each provider at `max_concurrent` in-flight calls; wait up to
    /// `max_wait` for a slot before failing fast.
    pub fn new(max_concurrent: usize, max_wait: Duration) -> Self {
        Self {
            max_concurrent: max_concurrent.max(1),
            max_wait,
            providers: Mutex::new(HashMap::new()),
        }
    }

    /// Acquire a slot for `provider`.
    pub async fn acquire(&self, provider: &str) -> Result<BulkheadPermit, BulkheadError> {
        let semaphore = self.semaphore(provider);
        match tokio::time::timeout(self.max_wait, semaphore.acquire_owned()).await {
            Ok(Ok(permit)) => Ok(BulkheadPermit { _permit: permit }),
            Ok(Err(_)) => Err(BulkheadError::Closed {
                provider: provider.to_string(),
            }),
            Err(_) => Err(BulkheadError::Full {
                provider: provider.to_string(),
            }),
        }
    }

    /// Try to acquire a slot without waiting.
    pub fn try_acquire(&self, provider: &str) -> Option<BulkheadPermit> {
        self.semaphore(provider)
            .try_acquire_owned()
            .ok()
            .map(|permit| BulkheadPermit { _permit: permit })
    }

    fn semaphore(&self, provider: &str) -> Arc<Semaphore> {
        let mut providers = self
            .providers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        providers
            .entry(provider.to_string())
            .or_insert_with(|| Arc::new(Semaphore::new(self.max_concurrent)))
            .clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> ModelKey {
        ModelKey::new("provider", "model")
    }

    #[test]
    fn backoff_is_exponential_and_capped() {
        let policy = RetryPolicy::default()
            .with_base_delay(Duration::from_millis(100))
            .with_max_delay(Duration::from_millis(250))
            .with_jitter(false);
        assert_eq!(policy.delay(1, None), Duration::from_millis(100));
        assert_eq!(policy.delay(2, None), Duration::from_millis(200));
        assert_eq!(policy.delay(3, None), Duration::from_millis(250));
        assert_eq!(
            policy.delay(1, Some(Duration::from_secs(2))),
            Duration::from_secs(2)
        );
    }

    #[test]
    fn retry_requires_a_retryable_error_and_an_attempt_left() {
        let policy = RetryPolicy::default().with_max_attempts(2);
        assert!(policy.should_retry(1, &ProviderError::Unavailable("down".into())));
        assert!(!policy.should_retry(2, &ProviderError::Unavailable("down".into())));
        assert!(!policy.should_retry(1, &ProviderError::InvalidRequest("bad".into())));
    }

    #[test]
    fn circuit_opens_at_threshold_and_half_opens_after_cooldown() {
        let cooldown = Duration::from_millis(5);
        let breaker = CircuitBreaker::new(2, cooldown);
        assert_eq!(breaker.state(&key()), CircuitState::Closed);
        assert!(breaker.allow(&key()));
        breaker.record_failure(&key());
        assert!(breaker.allow(&key()));
        breaker.record_failure(&key());
        assert_eq!(breaker.state(&key()), CircuitState::Open);
        assert!(!breaker.allow(&key()));

        std::thread::sleep(cooldown + Duration::from_millis(1));
        // After cooldown, the next allow becomes the single half-open probe.
        assert!(breaker.allow(&key()));
        assert_eq!(breaker.state(&key()), CircuitState::HalfOpen);
        assert!(!breaker.allow(&key()), "only one probe");
        breaker.record_success(&key());
        assert_eq!(breaker.state(&key()), CircuitState::Closed);
    }

    #[test]
    fn half_open_probe_self_heals_when_result_is_never_recorded() {
        let cooldown = Duration::from_millis(10);
        let breaker = CircuitBreaker::new(1, cooldown);
        assert!(breaker.allow(&key()));
        breaker.record_failure(&key());
        assert_eq!(breaker.state(&key()), CircuitState::Open);

        std::thread::sleep(cooldown + Duration::from_millis(1));
        // First probe is granted, then the caller disappears (never records).
        assert!(breaker.allow(&key()));
        assert_eq!(breaker.state(&key()), CircuitState::HalfOpen);
        assert!(!breaker.allow(&key()), "one probe at a time");

        // The probe window expires on its own: the circuit must not wedge.
        std::thread::sleep(cooldown + Duration::from_millis(1));
        assert!(breaker.allow(&key()), "a stuck probe must not lock the model out forever");

        // release_probe hands back the window immediately.
        breaker.release_probe(&key());
        assert!(breaker.allow(&key()), "release_probe frees the window at once");
    }

    #[tokio::test]
    async fn bulkhead_limits_concurrency_and_releases_on_drop() {
        let bulkhead = Bulkhead::new(1, Duration::from_millis(1));
        let first = bulkhead.acquire("p").await.expect("first slot");
        assert!(bulkhead.try_acquire("p").is_none());
        assert!(matches!(
            bulkhead.acquire("p").await,
            Err(BulkheadError::Full { .. })
        ));
        drop(first);
        assert!(bulkhead.try_acquire("p").is_some());
    }
}
