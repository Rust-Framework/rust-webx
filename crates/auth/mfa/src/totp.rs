//! TOTP (RFC 6238) with replay protection.

use std::fmt;

use data_encoding::BASE32_NOPAD;
use hmac::{Hmac, Mac};
use sha1::Sha1;
use sha2::{Sha256, Sha512};
use webx_core::error::{Error, Result};
use webx_core::security::constant_time_eq;

/// Hash algorithm used by a TOTP credential.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TotpAlgorithm {
    /// SHA-1 (the interoperable default; RFC 6238 test vectors use it).
    Sha1,
    /// SHA-256 (authenticator apps support it; enrollment URI carries it).
    Sha256,
    /// SHA-512.
    Sha512,
}

impl TotpAlgorithm {
    /// otpauth URI spelling.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Sha1 => "SHA1",
            Self::Sha256 => "SHA256",
            Self::Sha512 => "SHA512",
        }
    }
}

/// TOTP parameters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TotpConfig {
    /// 6–8 digits.
    pub digits: u32,
    /// Time step in seconds (usually 30).
    pub step_seconds: u64,
    /// Accepted clock skew, in steps (0–2).
    pub skew_steps: u32,
    /// Hash algorithm.
    pub algorithm: TotpAlgorithm,
}

impl Default for TotpConfig {
    fn default() -> Self {
        Self {
            digits: 6,
            step_seconds: 30,
            skew_steps: 1,
            algorithm: TotpAlgorithm::Sha1,
        }
    }
}

impl TotpConfig {
    /// Validate the parameters (fail before generating/verifying codes).
    pub fn validate(&self) -> Result<()> {
        if !(6..=8).contains(&self.digits) {
            return Err(Error::Validation(
                "TOTP digits must be between 6 and 8".to_string(),
            ));
        }
        if self.step_seconds == 0 || self.step_seconds > 300 {
            return Err(Error::Validation(
                "TOTP step must be between 1 and 300 seconds".to_string(),
            ));
        }
        if self.skew_steps > 2 {
            return Err(Error::Validation(
                "TOTP skew must be at most 2 steps".to_string(),
            ));
        }
        Ok(())
    }
}

/// A TOTP shared secret.
#[derive(Clone, PartialEq, Eq)]
pub struct TotpSecret(Vec<u8>);

impl fmt::Debug for TotpSecret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("TotpSecret")
            .field(&"<redacted>")
            .finish()
    }
}

impl TotpSecret {
    /// Generate a 160-bit secret (RFC 4226 recommendation).
    ///
    /// The bytes come from the operating-system CSPRNG (`getrandom`). A system
    /// RNG failure is unrecoverable for a security primitive, so this panics
    /// instead of falling back to weaker entropy.
    ///
    /// # Panics
    ///
    /// Panics when the operating-system CSPRNG is unavailable. There is no
    /// weaker fallback; callers should treat this as a process-level fault.
    pub fn generate() -> Self {
        let mut bytes = [0u8; 20];
        getrandom::fill(&mut bytes).expect("operating-system CSPRNG is unavailable");
        Self(bytes.to_vec())
    }

    /// Parse a base32 secret (spaces/hyphens ignored, case-insensitive).
    pub fn from_base32(secret: &str) -> Result<Self> {
        let normalized: String = secret
            .chars()
            .filter(|c| !c.is_whitespace() && *c != '-')
            .map(|c| c.to_ascii_uppercase())
            .collect();
        if normalized.is_empty() {
            return Err(Error::Validation(
                "TOTP secret must not be empty".to_string(),
            ));
        }
        let bytes = BASE32_NOPAD
            .decode(normalized.as_bytes())
            .map_err(|_| Error::Validation("TOTP secret is not valid base32".to_string()))?;
        if bytes.len() < 10 || bytes.len() > 64 {
            return Err(Error::Validation(
                "TOTP secret must be between 80 and 512 bits".to_string(),
            ));
        }
        Ok(Self(bytes))
    }

    /// Encode for enrollment.
    pub fn to_base32(&self) -> String {
        BASE32_NOPAD.encode(&self.0)
    }

    /// Raw bytes (tests/interop).
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    /// Code for one time step.
    pub fn code_at_step(&self, step: u64, config: TotpConfig) -> Result<String> {
        config.validate()?;
        let counter = step.to_be_bytes();
        let digest = hmac_digest(&self.0, &counter, config.algorithm)?;
        let offset = (digest[digest.len() - 1] & 0x0f) as usize;
        let binary = (((digest[offset] & 0x7f) as u32) << 24)
            | ((digest[offset + 1] as u32) << 16)
            | ((digest[offset + 2] as u32) << 8)
            | digest[offset + 3] as u32;
        let modulus = 10u32.pow(config.digits);
        let code = binary % modulus;
        Ok(format!("{:0width$}", code, width = config.digits as usize))
    }

    /// Code for `unix_secs`.
    pub fn code_at(&self, unix_secs: u64, config: TotpConfig) -> Result<String> {
        if config.step_seconds == 0 {
            return Err(Error::Validation("TOTP step must be positive".to_string()));
        }
        self.code_at_step(unix_secs / config.step_seconds, config)
    }

    /// Verify a code and enforce single use.
    ///
    /// `last_accepted_step` is the step of the last successful verification
    /// stored for this credential; a code at or before it is a replay.
    pub fn verify(
        &self,
        code: &str,
        unix_secs: u64,
        config: TotpConfig,
        last_accepted_step: Option<u64>,
    ) -> Result<TotpVerification> {
        config.validate()?;
        let code = code.trim();
        if code.len() != config.digits as usize || !code.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(Error::Unauthorized("invalid TOTP code".to_string()));
        }

        let current = unix_secs / config.step_seconds;
        let mut matched = None;
        for delta in -(i64::from(config.skew_steps))..=i64::from(config.skew_steps) {
            let Some(step) = current.checked_add_signed(delta) else {
                continue;
            };
            let expected = self.code_at_step(step, config)?;
            if constant_time_eq(expected.as_bytes(), code.as_bytes()) {
                matched = Some((step, delta as i32));
                break;
            }
        }

        let (matched_step, delta_steps) =
            matched.ok_or_else(|| Error::Unauthorized("invalid TOTP code".to_string()))?;
        if let Some(last) = last_accepted_step {
            if matched_step <= last {
                return Err(Error::Unauthorized(
                    "TOTP code has already been used".to_string(),
                ));
            }
        }
        Ok(TotpVerification {
            matched_step,
            delta_steps,
        })
    }

    /// `otpauth://` enrollment URI for authenticator apps.
    pub fn enrollment_uri(&self, issuer: &str, account: &str, config: TotpConfig) -> String {
        format!(
            "otpauth://totp/{}:{}?secret={}&issuer={}&algorithm={}&digits={}&period={}",
            percent_encode(issuer),
            percent_encode(account),
            self.to_base32(),
            percent_encode(issuer),
            config.algorithm.name(),
            config.digits,
            config.step_seconds
        )
    }
}

/// Result of a successful TOTP verification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TotpVerification {
    /// Time step of the accepted code (persist it for replay protection).
    pub matched_step: u64,
    /// `-skew..=skew` offset from the current step.
    pub delta_steps: i32,
}

fn hmac_digest(secret: &[u8], message: &[u8], algorithm: TotpAlgorithm) -> Result<Vec<u8>> {
    fn finish<M: Mac>(mut mac: M, message: &[u8]) -> Vec<u8> {
        mac.update(message);
        mac.finalize().into_bytes().to_vec()
    }
    match algorithm {
        TotpAlgorithm::Sha1 => Hmac::<Sha1>::new_from_slice(secret)
            .map(|mac| finish(mac, message))
            .map_err(|_| Error::Internal("invalid TOTP key".to_string())),
        TotpAlgorithm::Sha256 => Hmac::<Sha256>::new_from_slice(secret)
            .map(|mac| finish(mac, message))
            .map_err(|_| Error::Internal("invalid TOTP key".to_string())),
        TotpAlgorithm::Sha512 => Hmac::<Sha512>::new_from_slice(secret)
            .map(|mac| finish(mac, message))
            .map_err(|_| Error::Internal("invalid TOTP key".to_string())),
    }
}

fn percent_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len() * 3);
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(byte as char)
            }
            other => {
                out.push('%');
                out.push_str(&format!("{other:02X}"));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rfc_secret() -> TotpSecret {
        TotpSecret::from_base32("GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ").expect("secret")
    }

    fn config8() -> TotpConfig {
        TotpConfig {
            digits: 8,
            ..TotpConfig::default()
        }
    }

    #[test]
    fn rfc6238_sha1_vectors() {
        let secret = rfc_secret();
        for (time, expected) in [
            (59, "94287082"),
            (1_111_111_109, "07081804"),
            (1_111_111_111, "14050471"),
            (1_234_567_890, "89005924"),
            (2_000_000_000, "69279037"),
            (20_000_000_000, "65353130"),
        ] {
            let code = secret.code_at(time, config8()).expect("code");
            assert_eq!(code, expected, "time {time}");
        }
    }

    #[test]
    fn rfc6238_sha256_and_sha512_vectors() {
        let sha256 = TotpSecret::from_base32(
            &data_encoding::BASE32_NOPAD.encode(b"12345678901234567890123456789012"),
        )
        .expect("sha256 secret");
        assert_eq!(
            sha256
                .code_at(
                    59,
                    TotpConfig {
                        digits: 8,
                        algorithm: TotpAlgorithm::Sha256,
                        ..TotpConfig::default()
                    }
                )
                .expect("code"),
            "46119246"
        );

        let sha512 = TotpSecret::from_base32(
            &data_encoding::BASE32_NOPAD
                .encode(b"1234567890123456789012345678901234567890123456789012345678901234"),
        )
        .expect("sha512 secret");
        assert_eq!(
            sha512
                .code_at(
                    59,
                    TotpConfig {
                        digits: 8,
                        algorithm: TotpAlgorithm::Sha512,
                        ..TotpConfig::default()
                    }
                )
                .expect("code"),
            "90693936"
        );
    }

    #[test]
    fn six_digit_code_and_skew() {
        let secret = rfc_secret();
        let config = TotpConfig::default();
        let code = secret.code_at(59, config).expect("code");
        assert_eq!(code, "287082");
        assert!(secret.verify(&code, 59, config, None).is_ok());
        assert!(
            secret.verify(&code, 89, config, None).is_ok(),
            "one step skew"
        );
        assert!(
            secret.verify(&code, 149, config, None).is_err(),
            "two steps away is outside default skew"
        );
    }

    #[test]
    fn replay_is_rejected_after_the_accepted_step() {
        let secret = rfc_secret();
        let config = TotpConfig::default();
        let code = secret.code_at(59, config).expect("code");
        let verified = secret.verify(&code, 59, config, None).expect("first use");
        assert_eq!(verified.matched_step, 1);
        assert!(secret
            .verify(&code, 59, config, Some(verified.matched_step))
            .is_err());
    }

    #[test]
    fn invalid_codes_are_rejected() {
        let secret = rfc_secret();
        let config = TotpConfig::default();
        for code in ["", "12345", "abcdef", "000000"] {
            assert!(secret.verify(code, 59, config, None).is_err(), "{code}");
        }
    }

    #[test]
    fn base32_round_trip_and_uri() {
        let secret = TotpSecret::generate();
        let encoded = secret.to_base32();
        let decoded = TotpSecret::from_base32(&encoded).expect("decode");
        assert_eq!(decoded, secret);
        assert!(TotpSecret::from_base32("not base32!").is_err());
        let uri = secret.enrollment_uri("Travel", "alice@example.com", TotpConfig::default());
        assert!(uri.starts_with("otpauth://totp/Travel:alice%40example.com?secret="));
        assert!(uri.contains("algorithm=SHA1"));
        assert!(uri.contains("digits=6"));
        assert!(uri.contains("period=30"));
    }

    #[test]
    fn generate_returns_a_fresh_160_bit_secret() {
        let first = TotpSecret::generate();
        let second = TotpSecret::generate();
        assert_eq!(first.as_bytes().len(), 20);
        assert_eq!(second.as_bytes().len(), 20);
        assert_ne!(first.as_bytes(), second.as_bytes());
        // 160 bits of unpadded base32 is 32 characters; the enrollment output
        // format did not change with the switch to the OS CSPRNG.
        assert_eq!(first.to_base32().len(), 32);
    }

    #[test]
    fn config_validation_rejects_bad_parameters() {
        assert!(TotpConfig {
            digits: 5,
            ..TotpConfig::default()
        }
        .validate()
        .is_err());
        assert!(TotpConfig {
            step_seconds: 0,
            ..TotpConfig::default()
        }
        .validate()
        .is_err());
        assert!(TotpConfig {
            skew_steps: 3,
            ..TotpConfig::default()
        }
        .validate()
        .is_err());
    }
}
