//! API-key parsing and HMAC storage helpers.
//!
//! This module is deliberately storage-free: it gives the authentication
//! validator the two primitives every key store needs, without prescribing a
//! database schema.
//!
//! * [`parse_presented`] splits `sk-{key_id}-{secret}` into the indexed id and
//!   the secret.
//! * [`hash_secret`] / [`verify_secret`] store and check
//!   `HMAC-SHA256(server_pepper, secret)` — a fast keyed hash, **not** a
//!   password hash (Argon2 on every request would cost hundreds of
//!   milliseconds).
//!
//! API keys are high-entropy random values, so a keyed hash plus the indexed
//! `key_id` is the right trade-off: a database leak does not reveal the keys,
//! and the hot path stays a single lookup plus one HMAC.

use hmac::{Hmac, Mac};
use sha2::Sha256;
use std::fmt;
use webx_core::security::constant_time_eq;

/// Default key prefix used by the SDK-facing API.
pub const DEFAULT_PREFIX: &str = "sk-";

/// Minimum accepted secret length, in bytes.
///
/// Short secrets are guessable; 16 bytes is the floor for development keys.
/// Production keys should use at least 32 random bytes.
pub const MIN_SECRET_LEN: usize = 16;

/// Length of [`hash_secret`]'s output.
pub const SECRET_HASH_LEN: usize = 32;

type HmacSha256 = Hmac<Sha256>;

/// The two parts of a presented API key.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct PresentedApiKey<'a> {
    /// Indexed identifier (the only part stored in cleartext / logs).
    pub key_id: &'a str,
    /// The secret handed to the client; never log or store it.
    pub secret: &'a str,
}

impl fmt::Debug for PresentedApiKey<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Never emit the secret via Debug: `{:?}` on this value must not leak
        // credentials into logs. Only the key id is safe to display.
        f.debug_struct("PresentedApiKey")
            .field("key_id", &self.key_id)
            .field("secret", &"<redacted>")
            .finish()
    }
}

/// Split `{prefix}{key_id}-{secret}` into its parts.
///
/// The secret is the segment after the **last** `-`, so `key_id` may contain
/// hyphens (UUIDs are a common choice). The secret itself is base62/hex and
/// must not contain `-`. Returns `None` for a wrong prefix, an empty id, an
/// invalid id character, or a too-short secret.
pub fn parse_presented<'a>(presented: &'a str, prefix: &str) -> Option<PresentedApiKey<'a>> {
    let rest = presented.strip_prefix(prefix)?;
    let (key_id, secret) = rest.rsplit_once('-')?;
    if key_id.is_empty() || secret.len() < MIN_SECRET_LEN {
        return None;
    }
    if !key_id
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    {
        return None;
    }
    Some(PresentedApiKey { key_id, secret })
}

/// `HMAC-SHA256(pepper, secret)`.
///
/// `pepper` is a server-held secret (for example the installer-generated key
/// file); rotating it invalidates every stored hash, so rotate with a key
/// version rather than in place.
pub fn hash_secret(pepper: &[u8], secret: &str) -> [u8; SECRET_HASH_LEN] {
    let mut mac = HmacSha256::new_from_slice(pepper).expect("HMAC accepts any key length");
    mac.update(secret.as_bytes());
    let bytes = mac.finalize().into_bytes();
    let mut out = [0u8; SECRET_HASH_LEN];
    out.copy_from_slice(&bytes);
    out
}

/// Constant-time verification of `secret` against a stored hash.
///
/// A hash of the wrong length simply fails; the comparison does not leak where
/// the values differ.
pub fn verify_secret(pepper: &[u8], secret: &str, expected_hash: &[u8]) -> bool {
    let actual = hash_secret(pepper, secret);
    constant_time_eq(&actual, expected_hash)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_redacts_the_secret() {
        let parsed = parse_presented("sk-key-1-0123456789abcdef", DEFAULT_PREFIX).expect("parses");
        let rendered = format!("{parsed:?}");
        assert!(!rendered.contains("0123456789abcdef"), "Debug leaked the secret: {rendered}");
        assert!(rendered.contains("key-1"));
        assert!(rendered.contains("<redacted>"));
    }

    #[test]
    fn parses_prefix_id_and_secret() {
        let parsed = parse_presented("sk-key-1-0123456789abcdef", DEFAULT_PREFIX).expect("parses");
        assert_eq!(parsed.key_id, "key-1");
        assert_eq!(parsed.secret, "0123456789abcdef");
    }

    #[test]
    fn rejects_wrong_prefix_short_secret_or_empty_id() {
        assert!(parse_presented("ak-key-1-0123456789abcdef", DEFAULT_PREFIX).is_none());
        assert!(parse_presented("sk--0123456789abcdef", DEFAULT_PREFIX).is_none());
        assert!(parse_presented("sk-key-1-short", DEFAULT_PREFIX).is_none());
        assert!(parse_presented("sk-key-1", DEFAULT_PREFIX).is_none());
    }

    #[test]
    fn key_id_may_be_a_uuid_while_secret_stays_the_last_segment() {
        let parsed = parse_presented(
            "sk-550e8400-e29b-41d4-a716-446655440000-0123456789abcdef",
            DEFAULT_PREFIX,
        )
        .expect("parses");
        assert_eq!(parsed.key_id, "550e8400-e29b-41d4-a716-446655440000");
        assert_eq!(parsed.secret, "0123456789abcdef");
    }

    #[test]
    fn hashes_are_deterministic_and_pepper_dependent() {
        let first = hash_secret(b"pepper-one", "secret-value-0123456789");
        let second = hash_secret(b"pepper-one", "secret-value-0123456789");
        let other_pepper = hash_secret(b"pepper-two", "secret-value-0123456789");
        let other_secret = hash_secret(b"pepper-one", "secret-value-9876543210");

        assert_eq!(first, second);
        assert_ne!(first, other_pepper);
        assert_ne!(first, other_secret);
        assert_eq!(first.len(), SECRET_HASH_LEN);
    }

    #[test]
    fn verify_accepts_only_the_matching_secret_and_hash() {
        let hash = hash_secret(b"pepper-one", "secret-value-0123456789");
        assert!(verify_secret(
            b"pepper-one",
            "secret-value-0123456789",
            &hash
        ));
        assert!(!verify_secret(
            b"pepper-one",
            "secret-value-0000000000",
            &hash
        ));
        assert!(!verify_secret(
            b"pepper-two",
            "secret-value-0123456789",
            &hash
        ));
        assert!(!verify_secret(
            b"pepper-one",
            "secret-value-0123456789",
            &[]
        ));
    }
}
