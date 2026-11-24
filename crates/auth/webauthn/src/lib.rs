//! WebAuthn/Passkey relying-party verification for rust-webx.
//!
//! Scope: **verification primitives** for registration and assertion, with
//! ES256 credentials (the passkey default). The application owns the endpoints,
//! the challenge/session storage and the credential database; this crate
//! validates:
//!
//! * `clientDataJSON` type/challenge/origin/crossOrigin;
//! * `authData` RP ID hash, UP/UV/AT flags and signature counter;
//! * the COSE public key (EC2/P-256, ES256) and the assertion signature;
//! * the registration `none` attestation format (other formats are rejected
//!   until their verifier is explicitly implemented).
//!
//! Challenges are generated as 256-bit random values and stored server-side
//! (session/Cookie or `OneTimeSecret`); [`WebAuthnChallenge::verify`] performs
//! the constant-time comparison.

pub mod verify;

use base64::Engine as _;
use sha2::Digest as _;
use webx_core::error::{Error, Result};
use webx_core::security::constant_time_eq;

/// Relying-party configuration.
#[derive(Debug, Clone)]
pub struct WebAuthnConfig {
    /// Effective domain, e.g. `example.com` (no scheme/path).
    pub rp_id: String,
    /// Accepted origins, e.g. `https://app.example.com`.
    pub origins: Vec<String>,
    /// Require the UV flag on registration/assertion.
    pub require_user_verification: bool,
}

impl WebAuthnConfig {
    /// Build for one RP ID/origin (HTTPS enforced).
    pub fn new(rp_id: impl Into<String>, origin: impl Into<String>) -> Result<Self> {
        let rp_id = rp_id.into();
        let origin = origin.into();
        if rp_id.trim().is_empty()
            || rp_id.contains("://")
            || rp_id.contains('/')
            || rp_id.contains(':')
        {
            return Err(Error::di(
                "WebAuthn rp_id must be a bare domain (no scheme/port/path)".to_string(),
            ));
        }
        if !origin.starts_with("https://") {
            return Err(Error::di("WebAuthn origin must be https://".to_string()));
        }
        Ok(Self {
            rp_id,
            origins: vec![origin],
            require_user_verification: false,
        })
    }

    /// Require user verification (biometric/PIN) as well as user presence.
    pub fn require_user_verification(mut self, required: bool) -> Self {
        self.require_user_verification = required;
        self
    }

    /// Add another accepted origin (e.g. a native app custom scheme).
    pub fn with_origin(mut self, origin: impl Into<String>) -> Self {
        self.origins.push(origin.into());
        self
    }

    fn rp_id_hash(&self) -> [u8; 32] {
        sha2::Sha256::digest(self.rp_id.as_bytes()).into()
    }
}

/// A server-generated challenge. Store it in the login session; never derive it
/// from user input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebAuthnChallenge {
    bytes: Vec<u8>,
}

impl WebAuthnChallenge {
    /// Generate a 256-bit challenge.
    pub fn generate() -> Self {
        let mut bytes = Vec::with_capacity(32);
        bytes.extend_from_slice(uuid::Uuid::new_v4().as_bytes());
        bytes.extend_from_slice(uuid::Uuid::new_v4().as_bytes());
        Self { bytes }
    }

    /// Raw challenge bytes.
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Reconstruct a challenge from server-stored bytes.
    ///
    /// Intended for tests and for loading a challenge persisted in a session;
    /// never derive this value from client input.
    pub fn from_bytes(bytes: impl Into<Vec<u8>>) -> Self {
        Self {
            bytes: bytes.into(),
        }
    }

    /// Base64url (no padding), as sent to the browser.
    pub fn to_base64url(&self) -> String {
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&self.bytes)
    }

    /// Constant-time comparison with the value echoed in `clientDataJSON`.
    pub fn verify(&self, candidate: &str) -> bool {
        base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(candidate)
            .ok()
            .is_some_and(|bytes| constant_time_eq(self.bytes.as_slice(), bytes.as_slice()))
    }
}

/// Stored credential material (one row per passkey).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CredentialRecord {
    /// Raw credential id (base64url-decoded from the client response).
    pub credential_id: Vec<u8>,
    /// CBOR-encoded COSE public key.
    pub public_key: Vec<u8>,
    /// Last accepted signature counter.
    pub sign_count: u32,
}

/// Result of a successful registration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegisteredCredential {
    /// Verifier data to persist.
    pub credential: CredentialRecord,
    /// AAGUID (all zeroes for many passkeys).
    pub aaguid: Vec<u8>,
    /// Attestation format (`none` in this MVP).
    pub fmt: String,
}

/// Result of a successful assertion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AssertionResult {
    /// New signature counter to persist.
    pub sign_count: u32,
}
