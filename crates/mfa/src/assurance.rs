//! `amr`/`acr` assurance helpers for step-up authorization.

use webx_core::security::TokenClaims;

/// Password authentication method (`amr`).
pub const AMR_PASSWORD: &str = "pwd";
/// TOTP authentication method (`amr`).
pub const AMR_TOTP: &str = "totp";
/// Recovery-code authentication method (`amr`).
pub const AMR_RECOVERY_CODE: &str = "recovery_code";
/// Framework assurance level for a completed second factor.
pub const ACR_MFA: &str = "urn:webx:acr:mfa";

/// Methods that satisfied a login, plus the assurance level to embed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MfaAssurance {
    /// Authentication method references, e.g. `["pwd", "totp"]`.
    pub amr: Vec<String>,
    /// Authentication context class reference.
    pub acr: String,
}

impl MfaAssurance {
    /// Password + TOTP.
    pub fn totp() -> Self {
        Self {
            amr: vec![AMR_PASSWORD.to_string(), AMR_TOTP.to_string()],
            acr: ACR_MFA.to_string(),
        }
    }

    /// Password + recovery code.
    pub fn recovery_code() -> Self {
        Self {
            amr: vec![AMR_PASSWORD.to_string(), AMR_RECOVERY_CODE.to_string()],
            acr: ACR_MFA.to_string(),
        }
    }

    /// Explicit methods/level for custom flows.
    pub fn new<I, S>(amr: I, acr: impl Into<String>) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            amr: amr.into_iter().map(Into::into).collect(),
            acr: acr.into(),
        }
    }

    /// Add `amr`/`acr` claims to a token before issuance.
    pub fn apply_to_claims(&self, claims: TokenClaims) -> TokenClaims {
        claims
            .with_claim("amr", self.amr.join(","))
            .with_claim("acr", self.acr.clone())
    }

    /// Whether an `acr` claim satisfies the MFA step-up requirement.
    pub fn satisfies_step_up(acr: Option<&str>) -> bool {
        acr == Some(ACR_MFA)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn totp_assurance_adds_amr_and_acr() {
        let claims = MfaAssurance::totp().apply_to_claims(TokenClaims::new("user-1"));
        assert_eq!(
            claims.extra.get("amr").map(String::as_str),
            Some("pwd,totp")
        );
        assert_eq!(claims.extra.get("acr").map(String::as_str), Some(ACR_MFA));
        assert!(MfaAssurance::satisfies_step_up(Some(ACR_MFA)));
        assert!(!MfaAssurance::satisfies_step_up(Some("urn:other")));
        assert!(!MfaAssurance::satisfies_step_up(None));
    }

    #[test]
    fn recovery_assurance_names_the_method() {
        let assurance = MfaAssurance::recovery_code();
        assert!(assurance.amr.contains(&AMR_RECOVERY_CODE.to_string()));
    }
}
