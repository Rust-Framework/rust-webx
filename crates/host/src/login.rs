//! Login hardening: password policy, enumeration-resistant verification, login
//! throttling and a bounded password-hasher wrapper.
//!
//! These are the pieces every login endpoint needs but should not hand-roll:
//!
//! * [`PasswordPolicy`] — NIST SP 800-63B style length limits (no composition
//!   rules, no periodic rotation), optional common-password blocklist.
//! * [`verify_password_or_dummy`] — verify against the stored hash, or perform
//!   equivalent hashing work for an unknown account so response timing does not
//!   reveal whether the user exists.
//! * [`ILoginThrottle`] / [`MemoryLoginThrottle`] — per-account and per-client
//!   exponential backoff; never a permanent lock (which an attacker could use to
//!   lock a victim out).
//! * [`BoundedPasswordHasher`] — caps concurrent Argon2 work and fails fast with
//!   `503` instead of queueing unbounded CPU-bound tasks.
//!
//! The host registers its default Argon2 hasher wrapped in
//! [`BoundedPasswordHasher`]; applications can register their own via
//! `HostBuilder::use_password_hasher`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use tokio::sync::{Semaphore, SemaphorePermit};
use webx_core::error::{Error, Result};
use webx_core::security::IPasswordHasher;

/// Largest password, in bytes, handed to the hasher.
///
/// The character limit is the policy; this byte cap keeps a hostile
/// multi-megabyte input from becoming an Argon2 workload.
pub const MAX_PASSWORD_BYTES: usize = 4096;

/// Password policy (NIST SP 800-63B style).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PasswordPolicy {
    min_len: usize,
    max_len: usize,
    blocklist: Vec<String>,
}

impl Default for PasswordPolicy {
    fn default() -> Self {
        Self::nist()
    }
}

impl PasswordPolicy {
    /// The framework default: at least 12 characters, at most 1024, no
    /// composition rules, no blocklist.
    pub fn nist() -> Self {
        Self {
            min_len: 12,
            max_len: 1024,
            blocklist: Vec::new(),
        }
    }

    /// Minimum length in characters.
    pub fn min_len(mut self, min_len: usize) -> Self {
        self.min_len = min_len;
        self
    }

    /// Maximum length in characters (bounded by [`MAX_PASSWORD_BYTES`]).
    pub fn max_len(mut self, max_len: usize) -> Self {
        self.max_len = max_len;
        self
    }

    /// Add common/breached passwords to reject (case-insensitive).
    pub fn with_blocklist<I, S>(mut self, blocklist: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.blocklist = blocklist
            .into_iter()
            .map(|entry| entry.into().to_ascii_lowercase())
            .collect();
        self
    }

    /// Validate one candidate password.
    ///
    /// Errors are `Error::Validation` with a user-facing reason; a login
    /// endpoint should map them to its generic "invalid credentials" response,
    /// while a registration endpoint can return the reason.
    pub fn validate(&self, password: &str) -> Result<()> {
        let chars = password.chars().count();
        if chars < self.min_len {
            return Err(Error::Validation(format!(
                "password must be at least {} characters",
                self.min_len
            )));
        }
        if chars > self.max_len {
            return Err(Error::Validation(format!(
                "password must be at most {} characters",
                self.max_len
            )));
        }
        if password.len() > MAX_PASSWORD_BYTES {
            return Err(Error::Validation(format!(
                "password must be at most {MAX_PASSWORD_BYTES} bytes"
            )));
        }
        if !self.blocklist.is_empty() {
            let candidate = password.to_ascii_lowercase();
            if self.blocklist.iter().any(|entry| entry == &candidate) {
                return Err(Error::Validation(
                    "password is too common; choose another one".to_string(),
                ));
            }
        }
        Ok(())
    }
}

/// Verify a password against the stored hash, or spend equivalent work when the
/// account does not exist.
///
/// The `None` branch calls [`IPasswordHasher::hash`] (one Argon2 computation,
/// fresh salt) so an unknown account is not answered measurably faster than a
/// wrong password. The return value is always `false` for `None`.
pub async fn verify_password_or_dummy(
    hasher: &dyn IPasswordHasher,
    password: &str,
    stored_hash: Option<&str>,
) -> Result<bool> {
    match stored_hash {
        Some(stored_hash) => hasher.verify(password, stored_hash).await,
        None => {
            // Hash-and-discard: same CPU/memory cost class as verify.
            let _ = hasher.hash(password).await?;
            Ok(false)
        }
    }
}

/// What the throttle says about the next login attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoginThrottleDecision {
    /// Whether the attempt may proceed.
    pub allowed: bool,
    /// When `allowed` is false, how long the caller should wait.
    pub retry_after: Duration,
}

impl LoginThrottleDecision {
    /// An allowed attempt.
    pub const fn allowed() -> Self {
        Self {
            allowed: true,
            retry_after: Duration::ZERO,
        }
    }
}

/// Per-account and per-client login throttling.
///
/// Implementations must be safe to share across requests (the in-memory one
/// uses a mutex) and must never answer an error for a normal "too many
/// attempts" outcome — that is [`LoginThrottleDecision::allowed`] `false`.
#[async_trait::async_trait]
pub trait ILoginThrottle: Send + Sync {
    /// Check both the account and the client before verifying a password.
    async fn check(&self, account: &str, client_key: &str) -> Result<LoginThrottleDecision>;

    /// Record one failed attempt (wrong password, unknown account, bad MFA).
    async fn record_failure(&self, account: &str, client_key: &str) -> Result<()>;

    /// Clear the counters after a successful login.
    async fn record_success(&self, account: &str, client_key: &str) -> Result<()>;
}

/// Throttle parameters.
#[derive(Debug, Clone)]
pub struct ThrottlePolicy {
    /// Failures allowed before backoff starts.
    pub soft_limit: u32,
    /// First backoff delay.
    pub base_delay: Duration,
    /// Backoff ceiling.
    pub max_delay: Duration,
    /// Failures older than this window no longer count.
    pub window: Duration,
}

impl Default for ThrottlePolicy {
    fn default() -> Self {
        Self {
            soft_limit: 5,
            base_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(5 * 60),
            window: Duration::from_secs(15 * 60),
        }
    }
}

struct ThrottleEntry {
    failures: u32,
    last_failure: Instant,
    blocked_until: Instant,
}

/// Process-local [`ILoginThrottle`].
///
/// Keys are namespaced (`a:` account, `i:` client) so the same string cannot
/// collide. The map is bounded; at capacity the oldest entry is evicted (login
/// throttling must not itself become a memory-exhaustion vector).
pub struct MemoryLoginThrottle {
    policy: ThrottlePolicy,
    max_keys: usize,
    state: Mutex<HashMap<String, ThrottleEntry>>,
}

impl MemoryLoginThrottle {
    /// Create a throttle with [`ThrottlePolicy::default`].
    pub fn new(policy: ThrottlePolicy) -> Self {
        Self {
            policy,
            max_keys: 100_000,
            state: Mutex::new(HashMap::new()),
        }
    }

    /// Cap the number of tracked keys (default 100,000).
    pub fn with_max_keys(mut self, max_keys: usize) -> Self {
        self.max_keys = max_keys.max(1);
        self
    }

    /// Number of tracked keys (diagnostics/tests).
    pub fn tracked_keys(&self) -> usize {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len()
    }

    fn account_key(account: &str) -> String {
        format!("a:{}", account.trim().to_ascii_lowercase())
    }

    fn client_key(client_key: &str) -> String {
        format!("i:{}", client_key.trim())
    }

    fn remaining(entry: &ThrottleEntry, now: Instant) -> Duration {
        entry.blocked_until.saturating_duration_since(now)
    }

    fn record(&self, key: String, now: Instant) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);

        if state.len() >= self.max_keys && !state.contains_key(&key) {
            state.retain(|_, entry| now.duration_since(entry.last_failure) < self.policy.window);
            if state.len() >= self.max_keys {
                if let Some(oldest) = state
                    .iter()
                    .min_by_key(|(_, entry)| entry.last_failure)
                    .map(|(key, _)| key.clone())
                {
                    state.remove(&oldest);
                }
            }
        }

        let policy = &self.policy;
        let entry = state.entry(key).or_insert_with(|| ThrottleEntry {
            failures: 0,
            last_failure: now,
            blocked_until: now,
        });

        if now.duration_since(entry.last_failure) > policy.window {
            entry.failures = 0;
        }
        entry.failures = entry.failures.saturating_add(1);
        entry.last_failure = now;

        if entry.failures >= policy.soft_limit {
            // 1s, 2s, 4s, ... capped at max_delay. The exponent is bounded so
            // an attacker cannot overflow the shift or wait forever.
            let exponent = (entry.failures - policy.soft_limit).min(20);
            let factor = 1u32.checked_shl(exponent).unwrap_or(u32::MAX);
            let delay = policy
                .base_delay
                .saturating_mul(factor)
                .min(policy.max_delay);
            entry.blocked_until = now + delay;
        }
    }
}

#[async_trait::async_trait]
impl ILoginThrottle for MemoryLoginThrottle {
    async fn check(&self, account: &str, client_key: &str) -> Result<LoginThrottleDecision> {
        let now = Instant::now();
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let mut worst = Duration::ZERO;
        for key in [Self::account_key(account), Self::client_key(client_key)] {
            if let Some(entry) = state.get(&key) {
                worst = worst.max(Self::remaining(entry, now));
            }
        }
        Ok(LoginThrottleDecision {
            allowed: worst.is_zero(),
            retry_after: worst,
        })
    }

    async fn record_failure(&self, account: &str, client_key: &str) -> Result<()> {
        let now = Instant::now();
        self.record(Self::account_key(account), now);
        self.record(Self::client_key(client_key), now);
        Ok(())
    }

    async fn record_success(&self, account: &str, client_key: &str) -> Result<()> {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.remove(&Self::account_key(account));
        state.remove(&Self::client_key(client_key));
        Ok(())
    }
}

/// Caps concurrent password-hashing work.
///
/// Argon2 is memory/CPU-bound; without a cap, a burst of login attempts spawns
/// unbounded blocking work and can starve the process. The wrapper fails fast
/// with `503 Service Unavailable` when no permit arrives within
/// `queue_timeout`, which the caller can surface with `Retry-After`.
pub struct BoundedPasswordHasher {
    inner: Arc<dyn IPasswordHasher>,
    permits: Arc<Semaphore>,
    queue_timeout: Duration,
}

impl BoundedPasswordHasher {
    /// Wrap `inner` with at most `max_concurrent` in-flight calls.
    pub fn new(
        inner: Arc<dyn IPasswordHasher>,
        max_concurrent: usize,
        queue_timeout: Duration,
    ) -> Self {
        Self {
            inner,
            permits: Arc::new(Semaphore::new(max_concurrent.max(1))),
            queue_timeout,
        }
    }

    /// The wrapped hasher.
    pub fn inner(&self) -> &Arc<dyn IPasswordHasher> {
        &self.inner
    }

    async fn permit(&self) -> Result<SemaphorePermit<'_>> {
        tokio::time::timeout(self.queue_timeout, self.permits.acquire())
            .await
            .map_err(|_| {
                Error::Status(
                    503,
                    "password hashing is saturated; retry later".to_string(),
                )
            })?
            .map_err(|_| Error::Internal("password hasher semaphore closed".to_string()))
    }
}

#[async_trait::async_trait]
impl IPasswordHasher for BoundedPasswordHasher {
    async fn hash(&self, password: &str) -> Result<String> {
        let _permit = self.permit().await?;
        self.inner.hash(password).await
    }

    async fn verify(&self, password: &str, stored_hash: &str) -> Result<bool> {
        let _permit = self.permit().await?;
        self.inner.verify(password, stored_hash).await
    }

    fn needs_rehash(&self, stored_hash: &str) -> bool {
        self.inner.needs_rehash(stored_hash)
    }
}

/// Default in-flight password-hashing limit: `min(available_parallelism, 4)`.
pub fn default_password_concurrency() -> usize {
    std::thread::available_parallelism()
        .map(|parallelism| parallelism.get())
        .unwrap_or(4)
        .clamp(1, 4)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nist_policy_rejects_short_and_accepts_unicode_by_characters() {
        let policy = PasswordPolicy::nist();
        assert!(policy.validate("short").is_err());
        assert!(policy.validate("12345678901").is_err(), "11 chars");
        assert!(policy.validate("123456789012").is_ok(), "12 chars");
        assert!(
            policy.validate("密码密码密码密码密码密码").is_ok(),
            "12 CJK characters = 36 bytes but 12 characters"
        );
    }

    #[test]
    fn policy_rejects_blocklisted_passwords_case_insensitively() {
        let policy = PasswordPolicy::nist().with_blocklist(["Password123!"]);
        assert!(policy.validate("Password123!").is_err());
        assert!(policy.validate("password123!").is_err());
        assert!(policy.validate("Another-Passw0rd").is_ok());
    }

    #[test]
    fn policy_bounds_the_byte_length() {
        let policy = PasswordPolicy::nist().max_len(10);
        let huge = "a".repeat(MAX_PASSWORD_BYTES + 1);
        assert!(policy.validate(&huge).is_err());
    }

    #[test]
    fn default_password_concurrency_is_bounded() {
        let permits = default_password_concurrency();
        assert!((1..=4).contains(&permits));
    }
}
