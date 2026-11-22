//! Two-step login recipe: password first, second factor upgrades the session.
//!
//! The framework ships the primitives (TOTP with replay protection, hashed
//! recovery codes, `amr`/`acr` assurance) but not the account model. These tests
//! pin the documented recipe: a password-only token is **not** MFA, a verified
//! second factor upgrades the claims to `acr = urn:webx:acr:mfa`, and recovery
//! codes are single use.

use webx_core::security::TokenClaims;
use webx_mfa::recovery::DEFAULT_RECOVERY_CODE_COUNT;
use webx_mfa::{MfaAssurance, RecoveryCodeSet, TotpConfig, TotpSecret, ACR_MFA, AMR_PASSWORD};

fn acr(claims: &TokenClaims) -> Option<&str> {
    claims.extra.get("acr").map(String::as_str)
}

fn amr(claims: &TokenClaims) -> Option<&str> {
    claims.extra.get("amr").map(String::as_str)
}

#[test]
fn password_only_session_cannot_satisfy_a_step_up() {
    let limited = TokenClaims::new("user-1").with_claim("amr", AMR_PASSWORD);

    assert!(!MfaAssurance::satisfies_step_up(acr(&limited)));
    assert_eq!(amr(&limited), Some(AMR_PASSWORD));
}

#[test]
fn totp_verification_is_replay_protected_and_upgrades_the_session() {
    let config = TotpConfig::default();
    let secret = TotpSecret::generate();
    let now = 1_700_000_000u64;

    let code = secret.code_at(now, config).expect("code");
    let verified = secret.verify(&code, now, config, None).expect("first use");
    // Persisting `matched_step` is what makes the next use a replay.
    let replay = secret
        .verify(&code, now, config, Some(verified.matched_step))
        .expect_err("a consumed step must not verify twice");
    assert_eq!(replay.status_code(), 401);

    let upgraded = MfaAssurance::totp()
        .apply_to_claims(TokenClaims::new("user-1").with_claim("amr", AMR_PASSWORD));
    assert_eq!(acr(&upgraded), Some(ACR_MFA));
    assert_eq!(amr(&upgraded), Some("pwd,totp"));
    assert!(MfaAssurance::satisfies_step_up(acr(&upgraded)));
}

#[test]
fn recovery_codes_are_single_use_and_upgrade_the_session() {
    let (codes, mut set) = RecoveryCodeSet::generate(DEFAULT_RECOVERY_CODE_COUNT);
    assert_eq!(set.remaining(), DEFAULT_RECOVERY_CODE_COUNT);

    assert!(set.verify_and_consume(&codes[0]), "first use succeeds");
    assert!(!set.verify_and_consume(&codes[0]), "replay is rejected");
    assert!(!set.verify_and_consume("not-a-code"));
    assert_eq!(set.remaining(), DEFAULT_RECOVERY_CODE_COUNT - 1);

    let upgraded = MfaAssurance::recovery_code().apply_to_claims(TokenClaims::new("user-1"));
    assert_eq!(acr(&upgraded), Some(ACR_MFA));
    assert_eq!(amr(&upgraded), Some("pwd,recovery_code"));
    assert!(MfaAssurance::satisfies_step_up(acr(&upgraded)));
}
