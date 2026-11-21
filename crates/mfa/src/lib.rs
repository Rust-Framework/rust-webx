//! Multi-factor authentication primitives for rust-webx.
//!
//! - [`totp`] — RFC 6238 TOTP with replay protection and `otpauth://` URIs.
//! - [`recovery`] — one-time recovery codes stored only as SHA-256 hashes.
//! - [`assurance`] — `amr`/`acr` claims for step-up authorization.
//!
//! ```ignore
//! let secret = TotpSecret::generate();
//! let uri = secret.enrollment_uri("Travel", "alice@example.com", TotpConfig::default());
//! let verification = secret.verify(code, now_unix_secs, TotpConfig::default(), last_step)?;
//! // persist verification.matched_step as last_step; later codes at or before it are replays
//! let claims = MfaAssurance::totp().apply_to_claims(TokenClaims::new(&user.id));
//! ```

pub mod assurance;
pub mod recovery;
pub mod totp;

pub use assurance::{MfaAssurance, ACR_MFA, AMR_PASSWORD, AMR_RECOVERY_CODE, AMR_TOTP};
pub use recovery::{
    hash_code, RecoveryCodeRecord, RecoveryCodeSet, DEFAULT_RECOVERY_CODE_COUNT, RECOVERY_CODE_LEN,
};
pub use totp::{TotpAlgorithm, TotpConfig, TotpSecret, TotpVerification};
