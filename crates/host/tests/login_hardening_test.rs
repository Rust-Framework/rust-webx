//! 2.5 M1 — login hardening: policy, enumeration resistance, throttling and
//! the bounded password-hasher wrapper.

mod test_utils;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use webx_core::error::{Error, Result};
use webx_core::security::IPasswordHasher;
use webx_host::login::{
    default_password_concurrency, verify_password_or_dummy, BoundedPasswordHasher, ILoginThrottle,
    MemoryLoginThrottle, PasswordPolicy, ThrottlePolicy, MAX_PASSWORD_BYTES,
};

// ---------------------------------------------------------------------------
// Fake hasher
// ---------------------------------------------------------------------------

struct FakeHasher {
    hash_calls: AtomicUsize,
    verify_calls: AtomicUsize,
    active: AtomicUsize,
    max_active: AtomicUsize,
    delay: Duration,
}

impl FakeHasher {
    fn new(delay: Duration) -> Self {
        Self {
            hash_calls: AtomicUsize::new(0),
            verify_calls: AtomicUsize::new(0),
            active: AtomicUsize::new(0),
            max_active: AtomicUsize::new(0),
            delay,
        }
    }

    async fn work(&self) {
        let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.max_active.fetch_max(active, Ordering::SeqCst);
        tokio::time::sleep(self.delay).await;
        self.active.fetch_sub(1, Ordering::SeqCst);
    }
}

#[async_trait::async_trait]
impl IPasswordHasher for FakeHasher {
    async fn hash(&self, _password: &str) -> Result<String> {
        self.hash_calls.fetch_add(1, Ordering::SeqCst);
        self.work().await;
        Ok("$fake$hash".to_string())
    }

    async fn verify(&self, _password: &str, _stored_hash: &str) -> Result<bool> {
        self.verify_calls.fetch_add(1, Ordering::SeqCst);
        self.work().await;
        Ok(false)
    }

    fn needs_rehash(&self, _stored_hash: &str) -> bool {
        false
    }
}

// ---------------------------------------------------------------------------
// Password policy
// ---------------------------------------------------------------------------

#[test]
fn nist_policy_enforces_length_but_not_composition() {
    let policy = PasswordPolicy::nist();
    assert!(policy.validate("short").is_err());
    assert!(policy.validate("12345678901").is_err(), "11 chars");
    assert!(policy.validate("123456789012").is_ok(), "12 chars");
    assert!(
        policy.validate("correct horse battery staple").is_ok(),
        "spaces are allowed"
    );
    assert!(
        policy.validate("密码密码密码密码密码密码").is_ok(),
        "12 CJK characters count as 12 characters"
    );
}

#[test]
fn policy_rejects_blocklisted_and_oversized_passwords() {
    let policy = PasswordPolicy::nist().with_blocklist(["Password123!"]);
    assert!(policy.validate("Password123!").is_err());
    assert!(policy.validate("password123!").is_err());

    let small = PasswordPolicy::nist().max_len(10);
    assert!(small.validate(&"a".repeat(MAX_PASSWORD_BYTES + 1)).is_err());
}

// ---------------------------------------------------------------------------
// Enumeration resistance
// ---------------------------------------------------------------------------

#[tokio::test]
async fn unknown_account_still_hashes() {
    let hasher = FakeHasher::new(Duration::ZERO);
    let result = verify_password_or_dummy(&hasher, "candidate", None)
        .await
        .expect("dummy verify");
    assert!(!result);
    assert_eq!(hasher.hash_calls.load(Ordering::SeqCst), 1);
    assert_eq!(hasher.verify_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn known_account_verifies_the_stored_hash() {
    let hasher = FakeHasher::new(Duration::ZERO);
    let result = verify_password_or_dummy(&hasher, "candidate", Some("$stored$"))
        .await
        .expect("verify");
    assert!(!result);
    assert_eq!(hasher.hash_calls.load(Ordering::SeqCst), 0);
    assert_eq!(hasher.verify_calls.load(Ordering::SeqCst), 1);
}

// ---------------------------------------------------------------------------
// Login throttle
// ---------------------------------------------------------------------------

fn throttle() -> MemoryLoginThrottle {
    MemoryLoginThrottle::new(ThrottlePolicy {
        soft_limit: 2,
        base_delay: Duration::from_millis(20),
        max_delay: Duration::from_millis(200),
        window: Duration::from_secs(5),
    })
}

#[tokio::test]
async fn failures_start_backoff_and_success_resets() {
    let throttle = throttle();
    assert!(
        throttle
            .check("alice", "10.0.0.1")
            .await
            .expect("check")
            .allowed
    );

    throttle.record_failure("alice", "10.0.0.1").await.unwrap();
    assert!(
        throttle
            .check("alice", "10.0.0.1")
            .await
            .expect("check")
            .allowed,
        "one failure is below the soft limit"
    );

    throttle.record_failure("alice", "10.0.0.1").await.unwrap();
    let decision = throttle.check("alice", "10.0.0.1").await.expect("check");
    assert!(!decision.allowed);
    assert!(
        decision.retry_after > Duration::ZERO,
        "a blocked attempt must carry a positive retry delay"
    );

    throttle.record_success("alice", "10.0.0.1").await.unwrap();
    assert!(
        throttle
            .check("alice", "10.0.0.1")
            .await
            .expect("check")
            .allowed,
        "success clears the counters"
    );
    assert_eq!(throttle.tracked_keys(), 0);
}

#[tokio::test]
async fn backoff_expires_and_other_accounts_are_independent() {
    let throttle = throttle();
    throttle.record_failure("alice", "10.0.0.1").await.unwrap();
    throttle.record_failure("alice", "10.0.0.1").await.unwrap();
    assert!(!throttle.check("alice", "10.0.0.1").await.unwrap().allowed);
    assert!(
        throttle.check("bob", "10.0.0.2").await.unwrap().allowed,
        "another account+client pair is not throttled by alice's failures"
    );

    tokio::time::sleep(Duration::from_millis(40)).await;
    assert!(
        throttle.check("alice", "10.0.0.1").await.unwrap().allowed,
        "backoff is temporary, not a permanent lock"
    );
}

#[tokio::test]
async fn client_key_is_throttled_independently_of_account() {
    let throttle = throttle();
    throttle.record_failure("alice", "10.0.0.1").await.unwrap();
    throttle.record_failure("bob", "10.0.0.1").await.unwrap();
    let decision = throttle.check("carol", "10.0.0.1").await.unwrap();
    assert!(
        !decision.allowed,
        "the client key carries the second failure"
    );
}

// ---------------------------------------------------------------------------
// Bounded hasher
// ---------------------------------------------------------------------------

#[tokio::test]
async fn bounded_hasher_caps_concurrency() {
    let fake = Arc::new(FakeHasher::new(Duration::from_millis(20)));
    let bounded = BoundedPasswordHasher::new(
        fake.clone() as Arc<dyn IPasswordHasher>,
        1,
        Duration::from_secs(1),
    );

    let first = bounded.hash("a");
    let second = bounded.hash("b");
    let (a, b) = tokio::join!(first, second);
    a.expect("first hash");
    b.expect("second hash");
    assert_eq!(
        fake.max_active.load(Ordering::SeqCst),
        1,
        "only one hashing call may run at a time"
    );
}

#[tokio::test]
async fn bounded_hasher_fails_fast_when_saturated() {
    let fake = Arc::new(FakeHasher::new(Duration::from_millis(200)));
    let bounded = Arc::new(BoundedPasswordHasher::new(
        fake.clone() as Arc<dyn IPasswordHasher>,
        1,
        Duration::from_millis(10),
    ));

    let first = tokio::spawn({
        let bounded = Arc::clone(&bounded);
        async move { bounded.hash("a").await }
    });
    tokio::time::sleep(Duration::from_millis(20)).await;

    // The first call holds the only permit; the second must time out with 503.
    let second = bounded.hash("b").await;
    assert!(
        matches!(second, Err(Error::Status(503, _))),
        "saturated hashing must fail fast, got {second:?}"
    );

    first
        .await
        .expect("first task")
        .expect("first hash still completes");
}

#[test]
fn default_password_concurrency_is_one_to_four() {
    assert!((1..=4).contains(&default_password_concurrency()));
}
