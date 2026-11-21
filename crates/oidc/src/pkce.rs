//! PKCE (RFC 7636) verifier/challenge generation.
//!
//! S256 only: the challenge is `base64url(sha256(verifier))` without padding.

use base64::Engine as _;
use sha2::{Digest, Sha256};
use std::fmt;
use uuid::Uuid;

/// One PKCE pair.
#[derive(Clone, PartialEq, Eq)]
pub struct Pkce {
    verifier: String,
    challenge: String,
}

impl fmt::Debug for Pkce {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Pkce")
            .field("verifier", &"<redacted>")
            .field("challenge", &self.challenge)
            .finish()
    }
}

impl Pkce {
    /// Generate a fresh verifier/challenge pair.
    ///
    /// The verifier is 64 hex characters (256 bits), inside RFC 7636's
    /// 43–128-character range.
    pub fn generate() -> Self {
        let verifier = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
        let challenge = Self::challenge_for(&verifier);
        Self {
            verifier,
            challenge,
        }
    }

    /// `code_challenge` for `verifier` (S256).
    pub fn challenge_for(verifier: &str) -> String {
        let digest = Sha256::digest(verifier.as_bytes());
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest)
    }

    /// The verifier (kept server-side, sent only to the token endpoint).
    pub fn verifier(&self) -> &str {
        &self.verifier
    }

    /// The challenge (sent in the authorization URL).
    pub fn challenge(&self) -> &str {
        &self.challenge
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn challenge_is_s256_base64url_without_padding() {
        let pkce = Pkce::generate();
        assert_eq!(pkce.verifier().len(), 64);
        assert!(pkce.verifier().chars().all(|c| c.is_ascii_hexdigit()));
        assert!(!pkce.challenge().contains('='));
        assert_eq!(pkce.challenge(), Pkce::challenge_for(pkce.verifier()));
        assert_ne!(Pkce::generate().verifier(), pkce.verifier());
    }
}
