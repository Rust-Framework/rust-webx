//! Timing-safe comparison and single-use secrets (XR-3).
//!
//! Business endpoints keep re-implementing two small security primitives: a
//! comparison that does not leak *where* two secrets differ, and a "hand this
//! token out once, then burn it" store (installation wizards, invite links,
//! password-reset links, one-shot webhook signatures). Both live here so
//! applications do not hand-roll them:
//!
//! * [`constant_time_eq`] — byte-slice comparison without an early exit.
//! * [`OneTimeSecret`] — a TTL-bound secret consumed at most once, with the
//!   check-and-consume done in one atomic step, so concurrent requests cannot
//!   both win.
//!
//! # The framework does not mint the secret
//!
//! [`OneTimeSecret::issue`] takes the value from the caller: this crate has no
//! random-number dependency, and a security primitive must not quietly invent
//! entropy. Generate the value with the RNG your application already uses
//! (e.g. `uuid::Uuid::new_v4()` twice in `webx-host`, or `getrandom`), then
//! hand it to [`OneTimeSecret::issue`].
//!
//! # Storing only a hash
//!
//! [`OneTimeSecret::consume`] compares the caller's value against the stored
//! one with [`constant_time_eq`]. To keep only a digest in memory, issue the
//! digest of the minted value and consume the digest of the candidate — the
//! comparison stays constant-time and the plaintext never enters the store:
//!
//! ```ignore
//! let store = OneTimeSecret::new(Duration::from_secs(30 * 60));
//! store.issue(sha256_hex(&minted))?;                 // your own hasher
//! let valid = store.consume(&sha256_hex(&candidate)); // same hasher
//! ```

use crate::error::{Error, Result};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

/// Compare two byte slices without returning early on a difference.
///
/// The loop always runs `max(a.len(), b.len())` iterations and ORs every
/// difference into one accumulator, so the time spent does not depend on *where*
/// the inputs differ — a plain `==` on `String`/`&[u8]` stops at the first
/// mismatching byte and leaks how long a shared prefix is.
///
/// **Length is not hidden.** The runtime is proportional to the longer input and
/// the result distinguishes lengths, which is safe because the *length* of a
/// secret is public by construction (framework secrets are at least 32 bytes and
/// digests have a fixed size); only the *content* must stay secret. Do not use
/// this to compare values whose length is itself confidential — pad both sides
/// first.
///
/// ```
/// # use webx_core::security::constant_time_eq;
/// assert!(constant_time_eq(b"correct horse", b"correct horse"));
/// assert!(!constant_time_eq(b"correct horse", b"correct horsX"));
/// assert!(!constant_time_eq(b"short", b"short and longer"));
/// ```
pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    let mut difference = 0u8;
    // `max`, not `min`: comparing the tail of the longer input against a
    // virtual zero byte keeps the work independent of where a mismatch starts.
    for index in 0..a.len().max(b.len()) {
        let left = a.get(index).copied().unwrap_or_default();
        let right = b.get(index).copied().unwrap_or_default();
        difference |= left ^ right;
    }
    // Both halves are already computed, so the extra length check cannot turn
    // into a timing signal beyond the one described above.
    (difference == 0) & (a.len() == b.len())
}

/// Lifecycle of a [`OneTimeSecret`] slot.
///
/// Returned by [`OneTimeSecret::state`] so callers can distinguish "never
/// issued" from "already used" and "timed out" — the difference between a 401
/// and a 410 for an installation or password-reset endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OneTimeSecretState {
    /// Nothing has been issued (new store, or after [`OneTimeSecret::revoke`]).
    Empty,
    /// A secret is issued and can still be consumed.
    Pending,
    /// The secret is beyond its TTL and can no longer be consumed.
    Expired,
    /// The secret was consumed; it cannot be consumed again.
    Consumed,
}

struct Issued {
    secret: String,
    issued_at: Instant,
    consumed: bool,
}

/// A TTL-bound secret that can be consumed at most once.
///
/// Typical use is an out-of-band one-time token: the server mints a value,
/// hands it to the operator/user once (console line, invite link, e-mail), and
/// the endpoint that accepts it calls [`consume`](Self::consume). The
/// check-and-execute window is a single `Mutex` critical section, so N
/// concurrent requests carrying the same value produce exactly one `true`
/// (compare-and-swap, not "check now, mark later").
///
/// # Semantics
///
/// * [`issue`](Self::issue) replaces the slot and re-arms it; only **one**
///   secret is active at a time. Empty values are rejected with
///   [`Error::Validation`] and leave the current state untouched.
/// * [`consume`](Self::consume) returns `true` exactly once per issued secret,
///   only while it is neither expired nor consumed. A wrong candidate consumes
///   nothing, so the legitimate holder can still use the secret.
/// * The clock is [`Instant`]: a TTL is wall-clock independent and survives
///   system time adjustments; the store itself is process-local, so a consumed
///   or expired secret dies with the process.
///
/// # Example
///
/// ```
/// # use std::time::Duration;
/// # use webx_core::security::{OneTimeSecret, OneTimeSecretState};
/// let setup = OneTimeSecret::new(Duration::from_secs(30 * 60));
/// setup.issue("install-token").unwrap();
///
/// assert_eq!(setup.state(), OneTimeSecretState::Pending);
/// assert!(!setup.consume("wrong-token"), "a wrong guess burns nothing");
/// assert!(setup.consume("install-token"));
/// assert!(!setup.consume("install-token"), "the second use fails");
/// assert_eq!(setup.state(), OneTimeSecretState::Consumed);
///
/// // A consumed token can still be recognized for replay diagnostics (410).
/// assert!(setup.matches("install-token"));
/// ```
///
/// # Expiry
///
/// A secret is valid while less than the TTL has elapsed:
///
/// ```
/// # use std::time::Duration;
/// # use webx_core::security::{OneTimeSecret, OneTimeSecretState};
/// let expired = OneTimeSecret::new(Duration::ZERO);
/// expired.issue("too-late").unwrap();
/// assert_eq!(expired.state(), OneTimeSecretState::Expired);
/// assert!(!expired.consume("too-late"));
/// assert!(!expired.matches("too-late"));
/// ```
///
/// (Use a zero TTL only in tests — a real secret needs a positive lifetime.)
pub struct OneTimeSecret {
    ttl: Duration,
    slot: Mutex<Option<Issued>>,
}

impl OneTimeSecret {
    /// Create an empty slot whose secrets live for `ttl`.
    ///
    /// A zero TTL expires the next issued secret immediately; that is useful in
    /// tests and useless in production, so pick a real lifetime.
    pub fn new(ttl: Duration) -> Self {
        Self {
            ttl,
            slot: Mutex::new(None),
        }
    }

    /// Configured TTL.
    pub fn ttl(&self) -> Duration {
        self.ttl
    }

    /// Arm the slot with `secret`, replacing (and thereby revoking) any
    /// previous value.
    ///
    /// The caller supplies the value — see the module-level docs for the RNG
    /// and hash-at-rest recipes.
    ///
    /// An empty secret is rejected with [`Error::Validation`] because it would
    /// make `consume("")` succeed; the previous secret stays in place so a
    /// failed re-issue cannot silently extend or shorten the current one.
    pub fn issue(&self, secret: impl Into<String>) -> Result<()> {
        let secret = secret.into();
        if secret.is_empty() {
            return Err(Error::Validation(
                "a one-time secret must not be empty".to_string(),
            ));
        }
        *self.slot.lock().unwrap_or_else(PoisonError::into_inner) = Some(Issued {
            secret,
            issued_at: Instant::now(),
            consumed: false,
        });
        Ok(())
    }

    /// Consume the slot if `candidate` matches the issued secret.
    ///
    /// Atomic check-and-consume: among concurrent callers with the same
    /// candidate exactly one observes `true`. Returns `false` for an empty
    /// slot, a consumed secret, an expired secret, or a mismatch.
    pub fn consume(&self, candidate: &str) -> bool {
        let mut slot = self.slot.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(issued) = slot.as_mut() else {
            return false;
        };
        if issued.consumed || self.is_expired(issued) {
            return false;
        }
        if !constant_time_eq(issued.secret.as_bytes(), candidate.as_bytes()) {
            // A wrong guess must not burn the secret: an attacker spraying
            // candidates would otherwise lock the legitimate holder out.
            return false;
        }
        issued.consumed = true;
        true
    }

    /// Whether `candidate` is the issued secret, without consuming it.
    ///
    /// Meant for classifying a failed [`consume`](Self::consume) — e.g. an
    /// installation endpoint that answers `410 Gone` when the consumed token is
    /// replayed instead of `401`. Do **not** use it as the authorization gate:
    /// it does not consume, so two concurrent requests could both pass it.
    pub fn matches(&self, candidate: &str) -> bool {
        let slot = self.slot.lock().unwrap_or_else(PoisonError::into_inner);
        match slot.as_ref() {
            Some(issued) => {
                !self.is_expired(issued)
                    && constant_time_eq(issued.secret.as_bytes(), candidate.as_bytes())
            }
            None => false,
        }
    }

    /// Current lifecycle state (diagnostics, tests, 401-vs-410 decisions).
    pub fn state(&self) -> OneTimeSecretState {
        let slot = self.slot.lock().unwrap_or_else(PoisonError::into_inner);
        match slot.as_ref() {
            None => OneTimeSecretState::Empty,
            Some(issued) if issued.consumed => OneTimeSecretState::Consumed,
            Some(issued) if self.is_expired(issued) => OneTimeSecretState::Expired,
            Some(_) => OneTimeSecretState::Pending,
        }
    }

    /// Drop the slot without consuming it (cancel an outstanding secret).
    pub fn revoke(&self) {
        *self.slot.lock().unwrap_or_else(PoisonError::into_inner) = None;
    }

    fn is_expired(&self, issued: &Issued) -> bool {
        issued.issued_at.elapsed() >= self.ttl
    }
}

impl std::fmt::Debug for OneTimeSecret {
    /// Never renders the secret itself, only the TTL and the lifecycle state.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("OneTimeSecret")
            .field("ttl", &self.ttl)
            .field("state", &self.state())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Barrier};

    #[test]
    fn constant_time_eq_accepts_equal_input() {
        assert!(constant_time_eq(b"", b""));
        assert!(constant_time_eq(
            b"correct horse battery staple",
            b"correct horse battery staple"
        ));
    }

    #[test]
    fn constant_time_eq_rejects_any_difference() {
        // First byte, middle byte, last byte — a prefix-only comparison would
        // pass the last case.
        assert!(!constant_time_eq(b"correct horse", b"Xorrect horse"));
        assert!(!constant_time_eq(b"correct horse", b"correct Xorse"));
        assert!(!constant_time_eq(b"correct horse", b"correct horsX"));
    }

    #[test]
    fn constant_time_eq_rejects_length_mismatch_even_with_matching_prefix() {
        assert!(!constant_time_eq(b"abc", b"abc "));
        assert!(!constant_time_eq(b"abc ", b"abc"));
        assert!(!constant_time_eq(b"", b"a"));
        assert!(!constant_time_eq(b"a", b""));
    }

    #[test]
    fn constant_time_eq_handles_non_utf8_bytes() {
        assert!(constant_time_eq(&[0x00, 0xff, 0x80], &[0x00, 0xff, 0x80]));
        assert!(!constant_time_eq(&[0x00, 0xff, 0x80], &[0x00, 0xff, 0x81]));
    }

    #[test]
    fn issued_secret_is_consumable_exactly_once() {
        let store = OneTimeSecret::new(Duration::from_secs(300));
        assert_eq!(store.state(), OneTimeSecretState::Empty);
        store.issue("token-value").unwrap();
        assert_eq!(store.state(), OneTimeSecretState::Pending);

        assert!(store.consume("token-value"));
        assert_eq!(store.state(), OneTimeSecretState::Consumed);
        assert!(!store.consume("token-value"), "single use");
        assert!(!store.consume("token-value"), "still single use");
    }

    #[test]
    fn wrong_candidate_does_not_burn_the_secret() {
        let store = OneTimeSecret::new(Duration::from_secs(300));
        store.issue("token-value").unwrap();

        assert!(!store.consume("wrong-value"));
        assert!(!store.consume(""));
        assert_eq!(
            store.state(),
            OneTimeSecretState::Pending,
            "a spray of wrong guesses must not lock the holder out"
        );
        assert!(store.consume("token-value"), "the real value still works");
    }

    #[test]
    fn expired_secret_cannot_be_consumed_or_matched() {
        let store = OneTimeSecret::new(Duration::ZERO);
        store.issue("token-value").unwrap();
        assert_eq!(store.state(), OneTimeSecretState::Expired);
        assert!(!store.consume("token-value"));
        assert!(!store.matches("token-value"));
    }

    #[test]
    fn secret_survives_short_delay_but_not_beyond_ttl() {
        let store = OneTimeSecret::new(Duration::from_millis(20));
        store.issue("token-value").unwrap();
        assert!(store.matches("token-value"));
        std::thread::sleep(Duration::from_millis(120));
        assert_eq!(store.state(), OneTimeSecretState::Expired);
        assert!(!store.consume("token-value"));
    }

    #[test]
    fn matches_recognizes_consumed_secret_without_reviving_it() {
        let store = OneTimeSecret::new(Duration::from_secs(300));
        store.issue("token-value").unwrap();
        assert!(store.consume("token-value"));

        // Replay diagnostics: the value is still recognizable (410 instead of
        // 401), but it stays consumed.
        assert!(store.matches("token-value"));
        assert!(!store.matches("other-value"));
        assert!(!store.consume("token-value"));
        assert_eq!(store.state(), OneTimeSecretState::Consumed);
    }

    #[test]
    fn matches_does_not_consume() {
        let store = OneTimeSecret::new(Duration::from_secs(300));
        store.issue("token-value").unwrap();
        for _ in 0..3 {
            assert!(store.matches("token-value"));
        }
        assert_eq!(store.state(), OneTimeSecretState::Pending);
        assert!(store.consume("token-value"), "matches left it consumable");
    }

    #[test]
    fn reissuing_replaces_and_rearms_the_slot() {
        let store = OneTimeSecret::new(Duration::from_secs(300));
        store.issue("first").unwrap();
        assert!(store.consume("first"));

        store.issue("second").unwrap();
        assert_eq!(store.state(), OneTimeSecretState::Pending);
        assert!(!store.consume("first"), "the replaced secret is gone");
        assert!(store.consume("second"));
    }

    #[test]
    fn empty_secret_is_rejected_and_keeps_the_current_one() {
        let store = OneTimeSecret::new(Duration::from_secs(300));
        store.issue("token-value").unwrap();

        let error = store.issue("").expect_err("empty secrets are rejected");
        assert!(matches!(error, Error::Validation(_)), "got {error:?}");
        assert_eq!(store.state(), OneTimeSecretState::Pending);
        assert!(store.consume("token-value"));
    }

    #[test]
    fn revoke_clears_the_slot_without_consuming() {
        let store = OneTimeSecret::new(Duration::from_secs(300));
        store.issue("token-value").unwrap();

        store.revoke();
        assert_eq!(store.state(), OneTimeSecretState::Empty);
        assert!(!store.consume("token-value"));
        assert!(!store.matches("token-value"));

        store.revoke(); // idempotent
        assert_eq!(store.state(), OneTimeSecretState::Empty);
    }

    #[test]
    fn consume_and_matches_on_empty_slot_fail() {
        let store = OneTimeSecret::new(Duration::from_secs(300));
        assert!(!store.consume("anything"));
        assert!(!store.matches("anything"));
    }

    #[test]
    fn debug_output_does_not_leak_the_secret() {
        let store = OneTimeSecret::new(Duration::from_secs(300));
        store.issue("super-secret-value").unwrap();
        let rendered = format!("{store:?}");
        assert!(!rendered.contains("super-secret-value"), "got {rendered}");
        assert!(rendered.contains("Pending"), "got {rendered}");
    }

    #[test]
    fn concurrent_consumers_have_exactly_one_winner() {
        const THREADS: usize = 8;
        const ROUNDS: usize = 25;

        let store = Arc::new(OneTimeSecret::new(Duration::from_secs(300)));
        for round in 0..ROUNDS {
            store.issue(format!("round-{round}-secret")).unwrap();
            let barrier = Arc::new(Barrier::new(THREADS));
            let winners = Arc::new(AtomicUsize::new(0));

            let handles: Vec<_> = (0..THREADS)
                .map(|_| {
                    let store = Arc::clone(&store);
                    let barrier = Arc::clone(&barrier);
                    let winners = Arc::clone(&winners);
                    std::thread::spawn(move || {
                        // Line every thread up so they race on the same slot.
                        barrier.wait();
                        if store.consume(&format!("round-{round}-secret")) {
                            winners.fetch_add(1, Ordering::SeqCst);
                        }
                    })
                })
                .collect();
            for handle in handles {
                handle.join().expect("consumer thread must not panic");
            }

            assert_eq!(
                winners.load(Ordering::SeqCst),
                1,
                "round {round}: check-and-consume is a compare-and-swap"
            );
            assert_eq!(store.state(), OneTimeSecretState::Consumed);
        }
    }

    #[test]
    fn one_time_secret_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<OneTimeSecret>();
    }
}
