//! 2.5 M1 —JWT validation hardening.
//!
//! Covers the fail-closed configuration surface (issuer/audience/leeway/size/
//! algorithm/behavior), empty `sub`, future `iat`, and the two invalid-token
//! semantics. The tests drive `JwtAuth` directly through the mock context, so
//! they exercise the same code the host's `JwtScheme` uses.

mod test_utils;

use jsonwebtoken::{encode, DecodingKey, EncodingKey, Header};
use serde_json::json;
use std::time::{SystemTime, UNIX_EPOCH};
use webx_core::auth::IAuthenticationHandler;
use webx_core::config::{InvalidTokenBehavior, JwtAlgorithm, JwtSection};
use webx_core::error::Error;
use webx_core::mode::AppMode;
use webx_host::auth_jwt::{JwtAuth, JwtValidation};
use webx_host::server::Host;

const SECRET: &str = "m1-jwt-validation-test-secret-at-least-32-bytes";

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after unix epoch")
        .as_secs()
}

fn sign(claims: serde_json::Value) -> String {
    encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(SECRET.as_bytes()),
    )
    .expect("sign test token")
}

/// Minimal base64url (no padding) encoder for hand-built `alg:none` tokens.
fn b64url(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let b1 = *chunk.get(1).unwrap_or(&0);
        let b2 = *chunk.get(2).unwrap_or(&0);
        let n = ((chunk[0] as u32) << 16) | ((b1 as u32) << 8) | b2 as u32;
        out.push(TABLE[((n >> 18) & 63) as usize] as char);
        out.push(TABLE[((n >> 12) & 63) as usize] as char);
        if chunk.len() > 1 {
            out.push(TABLE[((n >> 6) & 63) as usize] as char);
        }
        if chunk.len() > 2 {
            out.push(TABLE[(n & 63) as usize] as char);
        }
    }
    out
}

fn valid_claims() -> serde_json::Value {
    json!({ "sub": "user-1", "exp": now_secs() + 3600 })
}

fn auth(validation: &JwtValidation) -> JwtAuth {
    JwtAuth::from_validation(DecodingKey::from_secret(SECRET.as_bytes()), validation)
}

async fn authenticate(auth: &JwtAuth, token: &str) -> Result<bool, Error> {
    let mut ctx = test_utils::TestHttpContext::new("GET", "/protected")
        .with_header("authorization", &format!("Bearer {token}"));
    auth.authenticate(&mut ctx)
        .await
        .map(|claims| claims.is_some())
}

fn anonymous_validation() -> JwtValidation {
    JwtValidation::builder()
        .build()
        .expect("default validation")
}

fn reject_validation() -> JwtValidation {
    JwtValidation::builder()
        .invalid_token_behavior(InvalidTokenBehavior::Reject)
        .build()
        .expect("reject validation")
}

// ---------------------------------------------------------------------------
// Configuration is fail-closed
// ---------------------------------------------------------------------------

#[test]
fn defaults_are_safe_and_stable() {
    let validation = anonymous_validation();
    assert_eq!(validation.algorithms(), &[JwtAlgorithm::Hs256]);
    assert_eq!(validation.issuer(), None);
    assert_eq!(validation.audience(), None);
    assert_eq!(validation.leeway().as_secs(), 30);
    assert_eq!(validation.max_token_bytes(), 8192);
    assert_eq!(
        validation.invalid_token_behavior(),
        InvalidTokenBehavior::Anonymous
    );
}

#[test]
fn asymmetric_algorithm_requires_jwks() {
    let error = JwtValidation::builder()
        .algorithms([JwtAlgorithm::Rs256])
        .build()
        .expect_err("RS256 is enabled by JWKS in M2, not M1");
    assert!(error.code() == "dependency_injection", "got {error:?}");
    assert!(
        error.to_string().contains("Jwks"),
        "the error must name the M2 capability: {error}"
    );
}

#[test]
fn empty_algorithm_list_is_rejected() {
    let error = JwtValidation::builder()
        .algorithms([])
        .build()
        .expect_err("an empty algorithm list must never verify anything");
    assert!(error.code() == "dependency_injection", "got {error:?}");
}

#[test]
fn empty_issuer_is_rejected() {
    let error = JwtValidation::builder()
        .require_issuer("   ")
        .build()
        .expect_err("an empty issuer would disable the check silently");
    assert!(error.code() == "dependency_injection", "got {error:?}");
}

#[test]
fn oversized_clock_skew_is_rejected() {
    let error = JwtValidation::builder()
        .leeway(std::time::Duration::from_secs(301))
        .build()
        .expect_err("a 5-minute grace period must not silently become 10 minutes");
    assert!(error.code() == "dependency_injection", "got {error:?}");
}

#[test]
fn zero_or_oversized_token_budget_is_rejected() {
    assert!(matches!(
        JwtValidation::builder().max_token_bytes(0).build(),
        Err(error) if error.code() == "dependency_injection"
    ));
    assert!(matches!(
        JwtValidation::builder()
            .max_token_bytes(1024 * 1024 + 1)
            .build(),
        Err(error) if error.code() == "dependency_injection"
    ));
}

#[test]
fn section_defaults_bind_into_validation() {
    let validation = JwtValidation::from_section(&JwtSection::default())
        .expect("the default section is a valid configuration");
    assert_eq!(validation.algorithms(), &[JwtAlgorithm::Hs256]);
    assert_eq!(validation.leeway().as_secs(), 30);
    assert_eq!(validation.max_token_bytes(), 8192);
}

// ---------------------------------------------------------------------------
// Claim checks
// ---------------------------------------------------------------------------

#[tokio::test]
async fn empty_sub_is_never_an_identity() {
    let token = sign(json!({ "sub": "", "exp": now_secs() + 3600 }));
    assert!(
        !authenticate(&auth(&anonymous_validation()), &token)
            .await
            .expect("anonymous mode stays Ok(None)"),
        "an empty sub must not produce claims"
    );

    let error = authenticate(&auth(&reject_validation()), &token)
        .await
        .expect_err("reject mode must answer 401");
    assert!(error.code() == "unauthorized", "got {error:?}");
}

#[tokio::test]
async fn missing_sub_is_never_an_identity() {
    let token = sign(json!({ "exp": now_secs() + 3600 }));
    assert!(!authenticate(&auth(&anonymous_validation()), &token)
        .await
        .expect("anonymous mode stays Ok(None)"));
}

#[tokio::test]
async fn issuer_mismatch_is_rejected() {
    let validation = JwtValidation::builder()
        .require_issuer("https://id.example.com")
        .build()
        .expect("validation");
    let token = sign(json!({
        "sub": "user-1",
        "iss": "https://staging.example.com",
        "exp": now_secs() + 3600
    }));
    assert!(!authenticate(&auth(&validation), &token)
        .await
        .expect("anonymous mode"));
}

#[tokio::test]
async fn matching_issuer_and_audience_authenticate() {
    let validation = JwtValidation::builder()
        .require_issuer("https://id.example.com")
        .require_audience("travel-api")
        .build()
        .expect("validation");
    let token = sign(json!({
        "sub": "user-1",
        "iss": "https://id.example.com",
        "aud": "travel-api",
        "exp": now_secs() + 3600
    }));
    assert!(
        authenticate(&auth(&validation), &token)
            .await
            .expect("token should authenticate"),
        "matching iss/aud must produce claims"
    );
}

#[tokio::test]
async fn audience_mismatch_is_rejected() {
    let validation = JwtValidation::builder()
        .require_audience("travel-api")
        .build()
        .expect("validation");
    let token = sign(json!({
        "sub": "user-1",
        "aud": "another-api",
        "exp": now_secs() + 3600
    }));
    assert!(!authenticate(&auth(&validation), &token)
        .await
        .expect("anonymous mode"));
}

#[tokio::test]
async fn expired_token_respects_leeway() {
    let token = sign(json!({ "sub": "user-1", "exp": now_secs().saturating_sub(10) }));

    let tolerant = JwtValidation::builder()
        .leeway(std::time::Duration::from_secs(30))
        .build()
        .expect("validation");
    assert!(
        authenticate(&auth(&tolerant), &token)
            .await
            .expect("within leeway"),
        "a token 10s past `exp` is valid with a 30s leeway"
    );

    let strict = JwtValidation::builder()
        .leeway(std::time::Duration::from_secs(0))
        .build()
        .expect("validation");
    assert!(!authenticate(&auth(&strict), &token)
        .await
        .expect("outside leeway"));
}

#[tokio::test]
async fn future_nbf_is_rejected() {
    let validation = anonymous_validation();
    let token = sign(json!({
        "sub": "user-1",
        "nbf": now_secs() + 600,
        "exp": now_secs() + 3600
    }));
    assert!(!authenticate(&auth(&validation), &token)
        .await
        .expect("anonymous mode"));
}

#[tokio::test]
async fn future_iat_is_rejected() {
    let validation = anonymous_validation();
    let token = sign(json!({
        "sub": "user-1",
        "iat": now_secs() + 600,
        "exp": now_secs() + 3600
    }));
    assert!(!authenticate(&auth(&validation), &token)
        .await
        .expect("anonymous mode"));
}

#[tokio::test]
async fn oversized_token_is_rejected_before_verification() {
    let validation = JwtValidation::builder()
        .max_token_bytes(64)
        .build()
        .expect("validation");
    let token = sign(valid_claims());
    assert!(token.len() > 64, "test token must exceed the budget");

    assert!(!authenticate(&auth(&validation), &token)
        .await
        .expect("anonymous mode stays Ok(None)"));

    let reject = JwtValidation::builder()
        .max_token_bytes(64)
        .invalid_token_behavior(InvalidTokenBehavior::Reject)
        .build()
        .expect("validation");
    let error = authenticate(&auth(&reject), &token)
        .await
        .expect_err("reject mode must answer 401");
    assert!(error.code() == "unauthorized", "got {error:?}");
}

#[tokio::test]
async fn valid_token_still_authenticates() {
    let token = sign(valid_claims());
    assert!(authenticate(&auth(&anonymous_validation()), &token)
        .await
        .expect("valid token"));
}

#[tokio::test]
async fn algorithm_confusion_is_rejected() {
    // A token signed with a different HMAC algorithm must not be accepted by
    // the HS256 verifier, even though the secret is the same.
    let token = encode(
        &Header::new(jsonwebtoken::Algorithm::HS384),
        &valid_claims(),
        &EncodingKey::from_secret(SECRET.as_bytes()),
    )
    .expect("sign HS384 token");
    assert!(!authenticate(&auth(&anonymous_validation()), &token)
        .await
        .expect("anonymous mode"));
}

#[tokio::test]
async fn alg_none_is_rejected() {
    let header = b64url(br#"{"alg":"none","typ":"JWT"}"#);
    let payload = b64url(format!(r#"{{"sub":"user-1","exp":{}}}"#, now_secs() + 3600).as_bytes());
    let token = format!("{header}.{payload}.");
    assert!(!authenticate(&auth(&anonymous_validation()), &token)
        .await
        .expect("anonymous mode"));
}

// ---------------------------------------------------------------------------
// Host wiring
// ---------------------------------------------------------------------------

#[test]
fn host_build_rejects_unsupported_jwt_algorithm() {
    let error = Host::builder()
        .mode(AppMode::Development)
        .no_spa()
        .configure(|app| {
            app.options(|options| {
                options.jwt.secret = SECRET.to_string();
                options.jwt.algorithms = vec![JwtAlgorithm::Rs256];
            });
        })
        .use_authentication()
        .try_build()
        .err()
        .expect("an algorithm the host cannot verify must abort startup");
    assert!(error.code() == "dependency_injection", "got {error:?}");
    assert!(error.to_string().contains("Jwks"), "got {error}");
}

#[test]
fn host_build_accepts_explicit_validation_override() {
    let validation = JwtValidation::builder()
        .require_issuer("https://id.example.com")
        .require_audience("travel-api")
        .invalid_token_behavior(InvalidTokenBehavior::Reject)
        .build()
        .expect("validation");

    Host::builder()
        .mode(AppMode::Development)
        .no_spa()
        .configure(|app| {
            app.options(|options| {
                options.jwt.secret = SECRET.to_string();
            });
        })
        .use_authentication()
        .use_jwt_validation(validation)
        .try_build()
        .expect("explicit validation must build");
}

#[tokio::test]
async fn default_issuer_mints_the_configured_iss_and_aud() {
    let host = Host::builder()
        .mode(AppMode::Development)
        .no_spa()
        .configure(|app| {
            app.options(|options| {
                options.jwt.secret = SECRET.to_string();
                options.jwt.issuer = Some("https://id.example.com".to_string());
                options.jwt.audience = Some("travel-api".to_string());
            });
        })
        .use_authentication()
        .try_build()
        .expect("host with iss/aud configuration must build");

    let issuer = host
        .provider()
        .get::<dyn webx_core::security::ITokenIssuer>()
        .expect("the default issuer is registered");
    let issued = issuer
        .issue(webx_core::security::TokenClaims::new("user-1"))
        .await
        .expect("issue token");

    let validation = JwtValidation::builder()
        .require_issuer("https://id.example.com")
        .require_audience("travel-api")
        .build()
        .expect("validation");
    assert!(
        authenticate(&auth(&validation), &issued.token)
            .await
            .expect("self-issued token must verify"),
        "the default issuer must mint the iss/aud the verifier requires"
    );
}
