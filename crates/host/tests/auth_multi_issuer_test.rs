//! F2 — resource-server trust: multiple issuers / audiences, one JWKS per
//! issuer, OAuth 2.0 scope → permissions, and the `client_id` identity field.
//!
//! Everything here is offline: `Jwt.JwksSources` is only *built* (a
//! `JwksResolver` per source, no fetch), and key resolution is driven through
//! `StaticKeyResolver` / `MultiIssuerKeyResolver`, so no test depends on an
//! identity provider being reachable.

mod test_utils;

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use jsonwebtoken::{encode, Algorithm, DecodingKey, EncodingKey, Header};
use serde_json::{json, Value};
use webx_core::auth::{IAuthenticationHandler, IdentityUser};
use webx_core::config::{
    InvalidTokenBehavior, JwtAlgorithm, JwtJwksSection, JwtJwksSourceSection, JwtSection,
};
use webx_core::error::{Error, Result};
use webx_core::mode::AppMode;
use webx_host::auth_jwks::{
    IJwtKeyResolver, LocalFirstKeyResolver, MultiIssuerKeyResolver, StaticKeyResolver,
};
use webx_host::auth_jwt::{JwtAuth, JwtValidation};
use webx_host::server::Host;

const SECRET: &str = "f2-multi-issuer-test-hs256-secret-at-least-32-bytes";
const ISSUER_A: &str = "https://id.alpha.example.com";
const ISSUER_B: &str = "https://id.beta.example.com";
const ISSUER_C: &str = "https://id.gamma.example.com";
const KID: &str = "kid-shared";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// `Result::expect_err` needs `T: Debug`; `DecodingKey`, `Host` and the
/// resolver do not implement it, so tests unwrap the error explicitly.
fn expect_error<T>(result: Result<T>, message: &str) -> Error {
    match result {
        Ok(_) => panic!("{message}"),
        Err(error) => error,
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after unix epoch")
        .as_secs()
}

fn sign_hs256(claims: Value) -> String {
    encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(SECRET.as_bytes()),
    )
    .expect("sign test token")
}

fn hs256_auth(validation: &JwtValidation) -> JwtAuth {
    JwtAuth::from_validation(DecodingKey::from_secret(SECRET.as_bytes()), validation)
}

/// A fresh P-256 key pair (PEM private, PEM public).
fn ec_key_pair() -> (String, String) {
    let pair = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).expect("EC key");
    (pair.serialize_pem(), pair.public_key_pem())
}

fn sign_es256(private_pem: &str, kid: &str) -> String {
    let mut header = Header::new(Algorithm::ES256);
    header.kid = Some(kid.to_string());
    encode(
        &header,
        &json!({
            "sub": "user-1",
            "iat": now_secs(),
            "exp": now_secs() + 3600,
        }),
        &EncodingKey::from_ec_pem(private_pem.as_bytes()).expect("encoding key"),
    )
    .expect("sign ES256 token")
}

fn jwks_endpoint(url: &str) -> JwtJwksSection {
    let mut section = JwtJwksSection::default();
    section.url = url.to_string();
    section
}

fn jwks_source(issuer: &str, url: &str) -> JwtJwksSourceSection {
    let mut source = JwtJwksSourceSection::default();
    source.issuer = issuer.to_string();
    source.url = url.to_string();
    source
}

/// A section with two issuers and one JWKS per issuer, no network involved.
fn multi_issuer_section() -> JwtSection {
    let mut section = JwtSection::default();
    section.algorithms = vec![JwtAlgorithm::Es256];
    section.issuers = vec![ISSUER_A.to_string(), ISSUER_B.to_string()];
    section.jwks_sources = vec![
        jwks_source(
            ISSUER_A,
            "https://id.alpha.example.com/.well-known/jwks.json",
        ),
        jwks_source(
            ISSUER_B,
            "https://id.beta.example.com/.well-known/jwks.json",
        ),
    ];
    section
}

async fn authenticate(
    auth: &JwtAuth,
    token: &str,
) -> Result<Option<Box<dyn webx_core::auth::IClaims>>> {
    let mut ctx = test_utils::TestHttpContext::new("GET", "/protected")
        .with_header("authorization", &format!("Bearer {token}"));
    auth.authenticate(&mut ctx).await
}

async fn accepts(auth: &JwtAuth, token: &str) -> bool {
    authenticate(auth, token)
        .await
        .expect("the default invalid-token behaviour is anonymous, never an error")
        .is_some()
}

async fn permissions(auth: &JwtAuth, token: &str) -> Vec<String> {
    authenticate(auth, token)
        .await
        .expect("authentication must not fail")
        .expect("the token must authenticate")
        .permissions()
        .to_vec()
}

async fn roles(auth: &JwtAuth, token: &str) -> Vec<String> {
    authenticate(auth, token)
        .await
        .expect("authentication must not fail")
        .expect("the token must authenticate")
        .roles()
        .to_vec()
}

async fn auth_error(auth: &JwtAuth, token: &str) -> Error {
    expect_error(
        authenticate(auth, token).await,
        "the token must be rejected with an error",
    )
}

async fn identity(auth: &JwtAuth, token: &str) -> IdentityUser {
    let claims = authenticate(auth, token)
        .await
        .expect("authentication must not fail")
        .expect("the token must authenticate");
    IdentityUser::from_claims(claims.as_ref())
}

// ---------------------------------------------------------------------------
// A. Multiple issuers / audiences
// ---------------------------------------------------------------------------

#[tokio::test]
async fn issuers_list_accepts_each_configured_issuer() {
    let (private_a, public_a) = ec_key_pair();
    let (private_b, public_b) = ec_key_pair();

    // Multi-issuer trust requires one key source per issuer (a shared HMAC
    // secret or a shared JWKS cannot express per-issuer trust).
    let validation = JwtValidation::builder()
        .algorithms([JwtAlgorithm::Es256])
        .require_issuers([ISSUER_A, ISSUER_B])
        .jwks_sources(vec![
            jwks_source(ISSUER_A, "https://id.alpha.example.com/jwks.json"),
            jwks_source(ISSUER_B, "https://id.beta.example.com/jwks.json"),
        ])
        .build()
        .expect("two issuers are a valid trust list");
    assert_eq!(
        validation.issuers(),
        &[ISSUER_A.to_string(), ISSUER_B.to_string()]
    );
    assert_eq!(
        validation.issuer(),
        None,
        "the single-value getter must not pretend a multi-issuer list has one entry"
    );

    let resolver = || {
        Arc::new(
            MultiIssuerKeyResolver::from_resolvers(vec![
                static_source(&public_a, "kid-a"),
                static_source(&public_b, "kid-b"),
            ])
            .expect("two sources")
            .bind_issuers(vec![ISSUER_A.to_string(), ISSUER_B.to_string()])
            .expect("bound sources"),
        )
    };
    let auth = JwtAuth::from_validation_with_resolver(None, &validation, resolver());
    for (private, kid, issuer) in [
        (&private_a, "kid-a", ISSUER_A),
        (&private_b, "kid-b", ISSUER_B),
    ] {
        let token = es256_token_with_iss(private, kid, issuer);
        assert!(
            accepts(&auth, &token).await,
            "a token from the configured issuer {issuer} must authenticate"
        );
    }

    let untrusted = es256_token_with_iss(&private_a, "kid-a", ISSUER_C);
    assert!(
        !accepts(&auth, &untrusted).await,
        "a third, unlisted issuer must never authenticate"
    );

    // An unlisted issuer is not merely ignored when the host is fail-closed.
    let rejecting = JwtValidation::builder()
        .algorithms([JwtAlgorithm::Es256])
        .require_issuers([ISSUER_A, ISSUER_B])
        .invalid_token_behavior(InvalidTokenBehavior::Reject)
        .jwks_sources(vec![
            jwks_source(ISSUER_A, "https://id.alpha.example.com/jwks.json"),
            jwks_source(ISSUER_B, "https://id.beta.example.com/jwks.json"),
        ])
        .build()
        .expect("validation");
    let rejecting_auth = JwtAuth::from_validation_with_resolver(None, &rejecting, resolver());
    let error = auth_error(&rejecting_auth, &untrusted).await;
    assert_eq!(error.status_code(), 401, "got {error:?}");
}

#[tokio::test]
async fn audiences_list_accepts_any_listed_audience() {
    let validation = JwtValidation::builder()
        .require_audiences(["travel-api", "billing-api"])
        .build()
        .expect("two audiences are a valid trust list");
    assert_eq!(
        validation.audiences(),
        &["travel-api".to_string(), "billing-api".to_string()]
    );
    assert_eq!(validation.audience(), None);

    let auth = hs256_auth(&validation);
    for audience in ["travel-api", "billing-api"] {
        let token = sign_hs256(json!({
            "sub": "user-1",
            "aud": audience,
            "exp": now_secs() + 3600,
        }));
        assert!(
            accepts(&auth, &token).await,
            "audience {audience} is listed and must be accepted"
        );
    }

    // RFC 7519 allows `aud` to be an array: any member may match.
    let array_audience = sign_hs256(json!({
        "sub": "user-1",
        "aud": ["someone-else", "billing-api"],
        "exp": now_secs() + 3600,
    }));
    assert!(
        accepts(&auth, &array_audience).await,
        "an `aud` array containing a listed audience must authenticate"
    );

    let unlisted = sign_hs256(json!({
        "sub": "user-1",
        "aud": "unknown-api",
        "exp": now_secs() + 3600,
    }));
    assert!(!accepts(&auth, &unlisted).await);
}

#[tokio::test]
async fn single_issuer_config_behaviour_unchanged() {
    // Configuration path: `Jwt.Issuer` / `Jwt.Audience` still mean exactly one
    // entry each, and the single-value getters keep working.
    let mut section = JwtSection::default();
    section.issuer = Some(ISSUER_A.to_string());
    section.audience = Some("travel-api".to_string());
    let validation = JwtValidation::from_section(&section).expect("single issuer section");
    assert_eq!(validation.issuer(), Some(ISSUER_A));
    assert_eq!(validation.audience(), Some("travel-api"));
    assert_eq!(validation.issuers(), &[ISSUER_A.to_string()]);
    assert_eq!(validation.audiences(), &["travel-api".to_string()]);

    let auth = hs256_auth(&validation);
    let good = sign_hs256(json!({
        "sub": "user-1",
        "iss": ISSUER_A,
        "aud": "travel-api",
        "exp": now_secs() + 3600,
    }));
    assert!(accepts(&auth, &good).await);

    let wrong_issuer = sign_hs256(json!({
        "sub": "user-1",
        "iss": ISSUER_B,
        "aud": "travel-api",
        "exp": now_secs() + 3600,
    }));
    assert!(!accepts(&auth, &wrong_issuer).await);

    let wrong_audience = sign_hs256(json!({
        "sub": "user-1",
        "iss": ISSUER_A,
        "aud": "billing-api",
        "exp": now_secs() + 3600,
    }));
    assert!(!accepts(&auth, &wrong_audience).await);

    // Builder path: `require_issuer` is the one-element spelling.
    let single = JwtValidation::builder()
        .require_issuer(ISSUER_A)
        .require_audience("travel-api")
        .build()
        .expect("builder validation");
    assert_eq!(single.issuer(), Some(ISSUER_A));
    assert_eq!(single.issuers(), &[ISSUER_A.to_string()]);
    assert_eq!(single.audience(), Some("travel-api"));
    assert!(accepts(&hs256_auth(&single), &good).await);
}

// ---------------------------------------------------------------------------
// Configuration is fail-closed
// ---------------------------------------------------------------------------

#[test]
fn issuer_and_issuers_are_mutually_exclusive() {
    let mut section = JwtSection::default();
    section.issuer = Some(ISSUER_A.to_string());
    section.issuers = vec![ISSUER_B.to_string()];

    let error = JwtValidation::from_section(&section).expect_err("both spellings must fail");
    assert_eq!(error.code(), "dependency_injection", "got {error:?}");
    let message = error.to_string();
    assert!(
        message.contains("Jwt.Issuer") && message.contains("Jwt.Issuers"),
        "the error must name both keys: {message}"
    );
    assert!(message.contains("mutually exclusive"), "got {message}");
}

#[test]
fn audience_and_audiences_are_mutually_exclusive() {
    let mut section = JwtSection::default();
    section.audience = Some("travel-api".to_string());
    section.audiences = vec!["billing-api".to_string()];

    let error = JwtValidation::from_section(&section).expect_err("both spellings must fail");
    assert_eq!(error.code(), "dependency_injection", "got {error:?}");
    let message = error.to_string();
    assert!(
        message.contains("Jwt.Audience") && message.contains("Jwt.Audiences"),
        "the error must name both keys: {message}"
    );
    assert!(message.contains("mutually exclusive"), "got {message}");
}

#[test]
fn blank_duplicate_and_oversized_trust_entries_are_rejected() {
    // A blank entry would silently accept `iss: ""`.
    let error = JwtValidation::builder()
        .require_issuers(["   "])
        .build()
        .expect_err("a blank issuer must fail startup");
    assert_eq!(error.code(), "dependency_injection", "got {error:?}");
    assert!(error.to_string().contains("Jwt.Issuers"), "got {error}");

    // A duplicate is an operator mistake, not something to collapse silently.
    let error = JwtValidation::builder()
        .require_issuers([ISSUER_A, ISSUER_A])
        .build()
        .expect_err("a duplicated issuer must fail startup");
    assert_eq!(error.code(), "dependency_injection", "got {error:?}");
    assert!(error.to_string().contains("duplicated"), "got {error}");

    // Same rules for audiences.
    let error = JwtValidation::builder()
        .require_audiences(["travel-api", " travel-api "])
        .build()
        .expect_err("trimmed duplicates are still duplicates");
    assert_eq!(error.code(), "dependency_injection", "got {error:?}");
    assert!(error.to_string().contains("Jwt.Audiences"), "got {error}");

    // At most 32 entries: a longer trust list is a configuration mistake.
    let too_many: Vec<String> = (0..33)
        .map(|index| format!("https://id{index}.example.com"))
        .collect();
    let error = JwtValidation::builder()
        .require_issuers(too_many)
        .build()
        .expect_err("33 issuers exceed the cap");
    assert_eq!(error.code(), "dependency_injection", "got {error:?}");
    assert!(error.to_string().contains("at most 32"), "got {error}");

    // 32 is still fine (asymmetric trust list with one source per issuer is the
    // multi-issuer shape; HS256 or a shared JWKS with 32 issuers is rejected).
    let exactly_32: Vec<String> = (0..32)
        .map(|index| format!("https://id{index}.example.com"))
        .collect();
    let first_issuer = exactly_32[0].clone();
    assert!(JwtValidation::builder()
        .algorithms([JwtAlgorithm::Es256])
        .require_issuers(exactly_32)
        .jwks_sources(vec![jwks_source(
            &first_issuer,
            "https://id.alpha.example.com/.well-known/jwks.json"
        )])
        .build()
        .is_ok());
}

#[test]
fn asymmetric_algorithms_accept_jwks_sources_without_jwks() {
    // Gate: a configured `Jwt.JwksSources` must have a consumer, so it removes
    // the "asymmetric algorithm without Jwt.Jwks" startup error.
    let validation =
        JwtValidation::from_section(&multi_issuer_section()).expect("sources replace Jwt.Jwks");
    assert!(validation.jwks().is_none());
    assert_eq!(validation.jwks_sources().len(), 2);
    assert_eq!(validation.jwks_sources()[0].issuer, ISSUER_A);
    assert_eq!(validation.jwks_sources()[1].issuer, ISSUER_B);

    // Building the resolver must not touch the network.
    let resolver = MultiIssuerKeyResolver::from_sections(validation.jwks_sources())
        .expect("one resolver per source");
    assert_eq!(resolver.len(), 2);
    assert!(!resolver.is_empty());

    // Without any key source the rule is unchanged.
    let mut section = multi_issuer_section();
    section.jwks_sources.clear();
    let error = JwtValidation::from_section(&section)
        .expect_err("ES256 with no key source must stay a startup error");
    assert!(error.to_string().contains("Jwt.JwksSources"), "got {error}");

    // Endpoint rules are the single-source rules, applied per source.
    type Mutate = fn(&mut JwtJwksSourceSection);
    let cases: [(&str, Mutate); 4] = [
        ("Url", |source| {
            source.url = "http://insecure.example.com/jwks.json".to_string();
        }),
        ("CacheSeconds", |source| source.cache_seconds = 0),
        ("TimeoutMs", |source| source.timeout_ms = 10),
        ("MaxKeys", |source| source.max_keys = 0),
    ];
    for (field, mutate) in cases {
        let mut section = multi_issuer_section();
        mutate(&mut section.jwks_sources[0]);
        let error = match JwtValidation::from_section(&section) {
            Ok(_) => panic!("Jwt.JwksSources[].{field} must be validated"),
            Err(error) => error,
        };
        assert_eq!(error.code(), "dependency_injection", "{field}: {error:?}");
    }
}

#[test]
fn jwks_and_jwks_sources_are_mutually_exclusive() {
    let mut section = multi_issuer_section();
    section.jwks = Some(jwks_endpoint("https://single.example.com/jwks.json"));

    let error = JwtValidation::from_section(&section)
        .expect_err("two key-source spellings must fail startup");
    assert_eq!(error.code(), "dependency_injection", "got {error:?}");
    let message = error.to_string();
    assert!(
        message.contains("Jwt.Jwks") && message.contains("Jwt.JwksSources"),
        "the error must name both keys: {message}"
    );
    assert!(message.contains("mutually exclusive"), "got {message}");
}

#[test]
fn jwks_source_must_be_bound_to_a_listed_issuer() {
    // (a) The source's issuer is not in `Jwt.Issuers`.
    let mut section = multi_issuer_section();
    section.jwks_sources[1].issuer = ISSUER_C.to_string();
    let error = JwtValidation::from_section(&section).expect_err("unbound source");
    assert_eq!(error.code(), "dependency_injection", "got {error:?}");
    let message = error.to_string();
    assert!(message.contains(ISSUER_C), "got {message}");
    assert!(message.contains("Jwt.Issuers"), "got {message}");

    // (b) A blank source issuer cannot be bound to anything.
    let mut section = multi_issuer_section();
    section.jwks_sources[0].issuer = "  ".to_string();
    let error = JwtValidation::from_section(&section).expect_err("blank source issuer");
    assert_eq!(error.code(), "dependency_injection", "got {error:?}");
    assert!(error.to_string().contains("Jwt.JwksSources"), "got {error}");

    // (c) Two sources for the same issuer are ambiguous at configuration time.
    let mut section = multi_issuer_section();
    section.jwks_sources[1].issuer = ISSUER_A.to_string();
    let error = JwtValidation::from_section(&section).expect_err("duplicate source issuer");
    assert_eq!(error.code(), "dependency_injection", "got {error:?}");
    assert!(error.to_string().contains("not unique"), "got {error}");

    // (d) Sources without any `Jwt.Issuers` cannot be bound at all.
    let mut section = multi_issuer_section();
    section.issuers.clear();
    let error = JwtValidation::from_section(&section).expect_err("sources need a trust list");
    assert_eq!(error.code(), "dependency_injection", "got {error:?}");
    assert!(error.to_string().contains("Jwt.Issuers"), "got {error}");
}

#[test]
fn blank_scope_claim_or_separator_is_rejected() {
    let mut section = JwtSection::default();
    section.scope_claim = "  ".to_string();
    let error = JwtValidation::from_section(&section).expect_err("blank scope claim");
    assert_eq!(error.code(), "dependency_injection", "got {error:?}");
    assert!(error.to_string().contains("Jwt.ScopeClaim"), "got {error}");

    // Empty separator would split every scope name into characters.
    let mut section = JwtSection::default();
    section.scope_separator = String::new();
    let error = JwtValidation::from_section(&section).expect_err("blank scope separator");
    assert_eq!(error.code(), "dependency_injection", "got {error:?}");
    assert!(
        error.to_string().contains("Jwt.ScopeSeparator"),
        "got {error}"
    );

    // Defaults: `scope` / a single space, no `Jwt.RequireScope` requirement.
    let defaults = JwtValidation::from_section(&JwtSection::default()).expect("defaults");
    assert_eq!(defaults.scope_claim(), "scope");
    assert_eq!(defaults.scope_separator(), " ");
    assert!(!defaults.require_scope());
    assert!(
        !defaults.scope_as_permissions(),
        "scope adoption must be opt-in: a foreign scope claim must not widen permissions"
    );
}

/// Scope extraction alone is not authorization: unless the operator opts in,
/// a token's `scope` claim never lands in `permissions()`.
#[tokio::test]
async fn scope_is_not_a_permission_unless_opted_in() {
    let auth = hs256_auth(
        &JwtValidation::builder()
            .build()
            .expect("default validation"),
    );
    let token = sign_hs256(json!({
        "sub": "user-1",
        "exp": now_secs() + 3600,
        "permissions": ["users.read"],
        "scope": "orders.write",
    }));
    assert_eq!(
        permissions(&auth, &token).await,
        vec!["users.read".to_string()],
        "the default must not turn a scope claim into a permission"
    );
    assert!(
        accepts(&auth, &token).await,
        "an unmerged scope is still a valid token"
    );
}

// ---------------------------------------------------------------------------
// C. scope → permissions, `Jwt.RequireScope`
// ---------------------------------------------------------------------------

#[tokio::test]
async fn scope_claims_become_permissions() {
    let validation = JwtValidation::builder()
        .scope_as_permissions(true)
        .build()
        .expect("default validation");
    let auth = hs256_auth(&validation);

    let token = sign_hs256(json!({
        "sub": "user-1",
        "exp": now_secs() + 3600,
        "permissions": ["orders.read", "users.read"],
        // `orders.read` repeats on purpose: the merge must deduplicate.
        "scope": "orders.read orders.write",
    }));

    assert_eq!(
        permissions(&auth, &token).await,
        vec![
            "orders.read".to_string(),
            "users.read".to_string(),
            "orders.write".to_string()
        ],
        "permissions claims come first, scopes are appended once, order is preserved"
    );
    assert!(
        roles(&auth, &token).await.is_empty(),
        "an OAuth scope is not a role"
    );

    // A scope-less token still authenticates (RequireScope defaults to false).
    let scope_less = sign_hs256(json!({
        "sub": "user-1",
        "exp": now_secs() + 3600,
        "permissions": ["users.read"],
    }));
    assert_eq!(
        permissions(&auth, &scope_less).await,
        vec!["users.read".to_string()],
        "scope extraction is additive and never removes existing permissions"
    );
}

#[tokio::test]
async fn scope_array_and_custom_claim_are_supported() {
    let auth = hs256_auth(
        &JwtValidation::builder()
            .scope_as_permissions(true)
            .build()
            .expect("default validation"),
    );

    // JSON array form, with a duplicate inside it.
    let array_scope = sign_hs256(json!({
        "sub": "user-1",
        "exp": now_secs() + 3600,
        "scope": ["orders.read", "orders.write", "orders.read"],
    }));
    assert_eq!(
        permissions(&auth, &array_scope).await,
        vec!["orders.read".to_string(), "orders.write".to_string()]
    );

    // Configurable claim name and separator.
    let custom = hs256_auth(
        &JwtValidation::builder()
            .scope_as_permissions(true)
            .scope_claim("scp")
            .scope_separator(",")
            .build()
            .expect("custom scope extraction"),
    );
    let token = sign_hs256(json!({
        "sub": "user-1",
        "exp": now_secs() + 3600,
        "scp": "orders.read, orders.write",
        "scope": "ignored.read",
    }));
    assert_eq!(
        permissions(&custom, &token).await,
        vec!["orders.read".to_string(), "orders.write".to_string()],
        "only the configured claim is consumed, and it is split on the separator"
    );

    // Blank values passed through the runtime builder fall back to the
    // documented defaults instead of swallowing every scope.
    let fallback = JwtAuth::from_validation(
        DecodingKey::from_secret(SECRET.as_bytes()),
        &JwtValidation::builder().build().expect("validation"),
    )
    .with_scope_extraction("   ", "")
    .with_scope_as_permissions(true);
    let token = sign_hs256(json!({
        "sub": "user-1",
        "exp": now_secs() + 3600,
        "scope": "orders.read orders.write",
    }));
    assert_eq!(
        permissions(&fallback, &token).await,
        vec!["orders.read".to_string(), "orders.write".to_string()]
    );
}

#[tokio::test]
async fn require_scope_rejects_tokens_without_scope() {
    let validation = JwtValidation::builder()
        .require_scope(true)
        .build()
        .expect("RequireScope is valid on its own");
    assert!(validation.require_scope());
    let auth = hs256_auth(&validation);

    // With a scope the token authenticates normally.
    let scoped = sign_hs256(json!({
        "sub": "user-1",
        "exp": now_secs() + 3600,
        "scope": "orders.read",
    }));
    assert!(accepts(&auth, &scoped).await);

    let scope_less = sign_hs256(json!({
        "sub": "user-1",
        "exp": now_secs() + 3600,
        "permissions": ["orders.read"],
    }));
    let error = auth_error(&auth, &scope_less).await;
    assert_eq!(
        error.status_code(),
        403,
        "missing scope is 403, not 401: {error:?}"
    );
    assert_eq!(error.code(), "forbidden", "got {error:?}");
    assert_eq!(error.public_detail(), Some("token has no scope"));
    assert!(
        !error.to_string().contains(&scope_less),
        "the rejection must never echo the token"
    );

    // An empty/blank scope claim counts as "no scope".
    let blank = sign_hs256(json!({
        "sub": "user-1",
        "exp": now_secs() + 3600,
        "scope": "   ",
    }));
    assert_eq!(auth_error(&auth, &blank).await.status_code(), 403);
}

#[tokio::test]
async fn require_scope_off_is_additive() {
    // Explicit `false` is the default: scopes are accepted, never required.
    let auth = hs256_auth(
        &JwtValidation::builder()
            .require_scope(false)
            .scope_as_permissions(true)
            .build()
            .expect("validation"),
    );

    let scope_less = sign_hs256(json!({
        "sub": "user-1",
        "exp": now_secs() + 3600,
    }));
    let claims = authenticate(&auth, &scope_less)
        .await
        .expect("authentication must not fail");
    assert!(claims.is_some(), "RequireScope=false must not reject");
    assert!(permissions(&auth, &scope_less).await.is_empty());

    let scoped = sign_hs256(json!({
        "sub": "user-1",
        "exp": now_secs() + 3600,
        "scope": "orders.read",
    }));
    assert_eq!(
        permissions(&auth, &scoped).await,
        vec!["orders.read".to_string()],
        "scope still lands in permissions when RequireScope is off"
    );
}

// ---------------------------------------------------------------------------
// B. Multiple key sources, fail-closed
// ---------------------------------------------------------------------------

/// A key source that answers `503`, standing in for an unreachable IdP.
struct UnavailableSource;

#[async_trait::async_trait]
impl IJwtKeyResolver for UnavailableSource {
    async fn resolve(&self, _kid: &str, _alg: Algorithm) -> Result<Arc<DecodingKey>> {
        Err(Error::status(503, "key backend down"))
    }
}

fn static_source(public_pem: &str, kid: &str) -> Arc<dyn IJwtKeyResolver> {
    Arc::new(
        StaticKeyResolver::new()
            .with_pem(kid, Algorithm::ES256, public_pem.as_bytes())
            .expect("static key"),
    )
}

fn es256_validation() -> JwtValidation {
    JwtValidation::builder()
        .algorithms([JwtAlgorithm::Es256])
        .jwks(Some(jwks_endpoint(
            "https://id.alpha.example.com/.well-known/jwks.json",
        )))
        .build()
        .expect("ES256 validation")
}

#[tokio::test]
async fn duplicate_kid_across_sources_is_rejected() {
    let (private_a, public_a) = ec_key_pair();
    let (_, public_b) = ec_key_pair();

    // The same `kid` published by two sources: `iss` is not trustworthy before
    // verification, so the host must not guess which key signed the token.
    let multi = MultiIssuerKeyResolver::from_resolvers(vec![
        static_source(&public_a, KID),
        static_source(&public_b, KID),
    ])
    .expect("two sources");

    let error = expect_error(
        multi.resolve(KID, Algorithm::ES256).await,
        "an ambiguous key id must not resolve",
    );
    assert_eq!(
        error.status_code(),
        503,
        "ambiguity is a service-level refusal, not 401: {error:?}"
    );
    assert!(
        error.to_string().contains("ambiguous key id"),
        "the operator must see why: {error}"
    );

    // Through `JwtAuth` the same ambiguity is fail-closed: not anonymous.
    let auth = JwtAuth::from_validation_with_resolver(None, &es256_validation(), Arc::new(multi));
    let token = sign_es256(&private_a, KID);
    let error = expect_error(
        auth.validate_token(&token).await,
        "the ambiguous key must not produce claims",
    );
    assert_eq!(error.status_code(), 503, "got {error:?}");

    // A single, unambiguous key in either source still works, so the rejection
    // is about the duplicate — not about multi-source resolution in general.
    let unique = MultiIssuerKeyResolver::from_resolvers(vec![
        static_source(&public_a, KID),
        static_source(&public_b, "kid-beta"),
    ])
    .expect("two sources");
    assert!(
        unique.resolve(KID, Algorithm::ES256).await.is_ok(),
        "a kid published by exactly one source resolves"
    );
}

#[tokio::test]
async fn multi_source_resolution_is_exact_and_fails_closed() {
    let (_, public_a) = ec_key_pair();
    let (_, public_b) = ec_key_pair();

    let multi = MultiIssuerKeyResolver::from_resolvers(vec![
        static_source(&public_a, "kid-alpha"),
        static_source(&public_b, "kid-beta"),
    ])
    .expect("two sources");

    // Exactly one hit, in the second source.
    assert!(multi.resolve("kid-beta", Algorithm::ES256).await.is_ok());

    // No hit at all is an unknown credential.
    let error = expect_error(
        multi.resolve("kid-unknown", Algorithm::ES256).await,
        "an unknown kid must not resolve",
    );
    assert_eq!(error.status_code(), 401, "got {error:?}");

    // A source that is down fails the whole request closed, even when another
    // source could answer: an outage never degrades into anonymous.
    let failing = MultiIssuerKeyResolver::from_resolvers(vec![
        static_source(&public_a, "kid-alpha"),
        Arc::new(UnavailableSource),
    ])
    .expect("two sources");
    let error = expect_error(
        failing.resolve("kid-alpha", Algorithm::ES256).await,
        "a source outage must not be ignored",
    );
    assert_eq!(error.status_code(), 503, "got {error:?}");

    // A list with no source is a startup error, not a verifier that answers 401
    // forever.
    let error = expect_error(
        MultiIssuerKeyResolver::from_resolvers(Vec::new()),
        "no source must fail",
    );
    assert_eq!(error.code(), "dependency_injection", "got {error:?}");
}

// ---------------------------------------------------------------------------
// D. IdentityUser.client_id
// ---------------------------------------------------------------------------

#[tokio::test]
async fn client_id_is_read_from_client_id_and_azp() {
    let auth = hs256_auth(
        &JwtValidation::builder()
            .build()
            .expect("default validation"),
    );

    let explicit = sign_hs256(json!({
        "sub": "user-1",
        "exp": now_secs() + 3600,
        "client_id": "svc-reports",
        "azp": "should-not-win",
    }));
    assert_eq!(identity(&auth, &explicit).await.client_id, "svc-reports");

    // RFC 7519 `azp` is the fallback when `client_id` is absent.
    let azp_only = sign_hs256(json!({
        "sub": "user-1",
        "exp": now_secs() + 3600,
        "azp": "spa-web",
    }));
    assert_eq!(identity(&auth, &azp_only).await.client_id, "spa-web");

    // Neither claim: empty string, and the rest of the identity is unaffected.
    let neither = sign_hs256(json!({
        "sub": "user-1",
        "exp": now_secs() + 3600,
        "name": "Ada",
    }));
    let identity = identity(&auth, &neither).await;
    assert_eq!(identity.client_id, "");
    assert_eq!(identity.name, "Ada");
}

// ---------------------------------------------------------------------------
// Host wiring: the configuration keys have a consumer
// ---------------------------------------------------------------------------

#[test]
fn host_with_jwks_sources_builds_without_fetching() {
    let _host = Host::builder()
        .mode(AppMode::Development)
        .no_spa()
        .configure(|app| {
            app.options(|options| {
                options.jwt.secret.clear();
                options.jwt.algorithms = vec![JwtAlgorithm::Es256];
                options.jwt.issuers = vec![ISSUER_A.to_string(), ISSUER_B.to_string()];
                options.jwt.audiences = vec!["travel-api".to_string()];
                options.jwt.require_scope = true;
                options.jwt.jwks_sources = vec![
                    jwks_source(
                        ISSUER_A,
                        "https://id.alpha.example.com/.well-known/jwks.json",
                    ),
                    jwks_source(
                        ISSUER_B,
                        "https://id.beta.example.com/.well-known/jwks.json",
                    ),
                ];
            })
        })
        .use_authentication()
        .try_build()
        .expect("Jwt.JwksSources must replace Jwt.Jwks and build offline");
}

#[tokio::test]
async fn single_element_trust_lists_are_stamped_by_the_default_issuer() {
    // `Issuers: [X]` is the multi-value spelling of `Issuer: X`, so the host's
    // own minted token must carry the same `iss`/`aud` — otherwise enabling
    // either check would reject every token the host itself issues.
    let host = Host::builder()
        .mode(AppMode::Development)
        .no_spa()
        .configure(|app| {
            app.options(|options| {
                options.jwt.secret = SECRET.to_string();
                options.jwt.issuers = vec![ISSUER_A.to_string()];
                options.jwt.audiences = vec!["travel-api".to_string()];
            })
        })
        .use_authentication()
        .try_build()
        .expect("a one-element trust list must build");

    let issuer = host
        .provider()
        .get::<dyn webx_core::security::ITokenIssuer>()
        .expect("the default issuer is registered");
    let issued = issuer
        .issue(webx_core::security::TokenClaims::new("user-1"))
        .await
        .expect("issue through the host");

    let mut permissive = jsonwebtoken::Validation::new(Algorithm::HS256);
    permissive.required_spec_claims.clear();
    permissive.validate_exp = false;
    permissive.validate_aud = false;
    let payload = jsonwebtoken::decode::<Value>(
        &issued.token,
        &DecodingKey::from_secret(SECRET.as_bytes()),
        &permissive,
    )
    .expect("the issued token is a well-formed HS256 token")
    .claims;
    assert_eq!(payload["iss"], json!(ISSUER_A));
    assert_eq!(payload["aud"], json!("travel-api"));

    // Closed loop: the same configuration verifies the token it minted.
    let mut section = JwtSection::default();
    section.secret = SECRET.to_string();
    section.issuers = vec![ISSUER_A.to_string()];
    section.audiences = vec!["travel-api".to_string()];
    let validation = JwtValidation::from_section(&section).expect("validation");
    assert!(
        accepts(&hs256_auth(&validation), &issued.token).await,
        "the host's own token must pass its own multi-value verifier"
    );
}

#[test]
fn host_rejects_conflicting_single_and_multi_issuer_config() {
    let error = expect_error(
        Host::builder()
            .mode(AppMode::Development)
            .no_spa()
            .configure(|app| {
                app.options(|options| {
                    options.jwt.issuer = Some(ISSUER_A.to_string());
                    options.jwt.issuers = vec![ISSUER_B.to_string()];
                })
            })
            .use_authentication()
            .try_build(),
        "the host must not guess which issuer list wins",
    );
    assert_eq!(error.code(), "dependency_injection", "got {error:?}");
    assert!(
        error.to_string().contains("mutually exclusive"),
        "got {error}"
    );
}

// ---------------------------------------------------------------------------
// B2. Issuer ↔ key-source binding (fail-closed)
// ---------------------------------------------------------------------------

/// The verifying key is bound to the issuer that publishes it: a token signed by
/// source B must not be able to claim source A's `iss` and inherit A's
/// authorization semantics, even though both issuers are trusted.
#[tokio::test]
async fn issuer_is_bound_to_the_key_source() {
    let (private_a, public_a) = ec_key_pair();
    let (private_b, public_b) = ec_key_pair();

    let validation = JwtValidation::builder()
        .algorithms([JwtAlgorithm::Es256])
        .require_issuers([ISSUER_A, ISSUER_B])
        .jwks_sources(vec![
            jwks_source(
                ISSUER_A,
                "https://id.alpha.example.com/.well-known/jwks.json",
            ),
            jwks_source(
                ISSUER_B,
                "https://id.beta.example.com/.well-known/jwks.json",
            ),
        ])
        .build()
        .expect("multi-issuer validation");

    let multi = MultiIssuerKeyResolver::from_resolvers(vec![
        static_source(&public_a, "kid-a"),
        static_source(&public_b, "kid-b"),
    ])
    .expect("two sources")
    .bind_issuers(vec![ISSUER_A.to_string(), ISSUER_B.to_string()])
    .expect("bound sources");
    let auth = JwtAuth::from_validation_with_resolver(None, &validation, Arc::new(multi));

    let signed = |private: &str, kid: &str, issuer: &str| -> String {
        let mut header = Header::new(Algorithm::ES256);
        header.kid = Some(kid.to_string());
        encode(
            &header,
            &json!({
                "sub": "user-1",
                "iss": issuer,
                "iat": now_secs(),
                "exp": now_secs() + 3600,
            }),
            &EncodingKey::from_ec_pem(private.as_bytes()).expect("encoding key"),
        )
        .expect("sign ES256 token")
    };

    assert!(accepts(&auth, &signed(&private_a, "kid-a", ISSUER_A)).await);
    assert!(accepts(&auth, &signed(&private_b, "kid-b", ISSUER_B)).await);

    let confused = signed(&private_b, "kid-b", ISSUER_A);
    assert!(
        !accepts(&auth, &confused).await,
        "a token must not borrow another trusted issuer's identity"
    );
}

#[test]
fn bind_issuers_requires_one_non_blank_issuer_per_source() {
    let (_, public) = ec_key_pair();
    let resolvers = || vec![static_source(&public, "kid-a")];

    let unbound = MultiIssuerKeyResolver::from_resolvers(resolvers()).expect("one source");
    let error = expect_error(unbound.bind_issuers(Vec::new()), "missing binding");
    assert_eq!(error.code(), "dependency_injection", "got {error:?}");
    assert!(error.to_string().contains("issuer binding"), "got {error}");

    let unbound = MultiIssuerKeyResolver::from_resolvers(resolvers()).expect("one source");
    let error = expect_error(
        unbound.bind_issuers(vec!["   ".to_string()]),
        "blank binding",
    );
    assert!(
        error.to_string().contains("must not be empty"),
        "got {error}"
    );
}

// ---------------------------------------------------------------------------
// B3. Hardening regressions (security review: atomic binding, M-1/M-8)
// ---------------------------------------------------------------------------

fn es256_token_with_iss(private_pem: &str, kid: &str, issuer: &str) -> String {
    let mut header = Header::new(Algorithm::ES256);
    header.kid = Some(kid.to_string());
    encode(
        &header,
        &json!({
            "sub": "user-1",
            "iss": issuer,
            "iat": now_secs(),
            "exp": now_secs() + 3600,
        }),
        &EncodingKey::from_ec_pem(private_pem.as_bytes()).expect("encoding key"),
    )
    .expect("sign ES256 token")
}

#[tokio::test]
async fn issuer_binding_rejects_with_401_and_unbound_resolver_accepts() {
    let (private_a, public_a) = ec_key_pair();
    let (private_b, public_b) = ec_key_pair();

    let validation = JwtValidation::builder()
        .algorithms([JwtAlgorithm::Es256])
        .require_issuers([ISSUER_A, ISSUER_B])
        .invalid_token_behavior(InvalidTokenBehavior::Reject)
        .jwks_sources(vec![
            jwks_source(
                ISSUER_A,
                "https://id.alpha.example.com/.well-known/jwks.json",
            ),
            jwks_source(
                ISSUER_B,
                "https://id.beta.example.com/.well-known/jwks.json",
            ),
        ])
        .build()
        .expect("multi-issuer validation");

    let bound = MultiIssuerKeyResolver::from_resolvers(vec![
        static_source(&public_a, "kid-a"),
        static_source(&public_b, "kid-b"),
    ])
    .expect("two sources")
    .bind_issuers(vec![ISSUER_A.to_string(), ISSUER_B.to_string()])
    .expect("bound sources");
    let auth = JwtAuth::from_validation_with_resolver(None, &validation, Arc::new(bound));

    // B's key, A's issuer: key resolution succeeds, so the rejection is an
    // authentication failure (401 under `Reject`), not a 503.
    let confused = es256_token_with_iss(&private_b, "kid-b", ISSUER_A);
    let error = auth_error(&auth, &confused).await;
    assert_eq!(error.status_code(), 401, "got {error:?}");

    // The matching token still authenticates.
    let honest = es256_token_with_iss(&private_b, "kid-b", ISSUER_B);
    assert!(accepts(&auth, &honest).await);

    // Control: an *unbound* resolver accepts the same confused token, so the
    // rejection above is caused by the binding and nothing else.
    let unbound = MultiIssuerKeyResolver::from_resolvers(vec![
        static_source(&public_a, "kid-a"),
        static_source(&public_b, "kid-b"),
    ])
    .expect("two sources");
    let unbound_auth = JwtAuth::from_validation_with_resolver(None, &validation, Arc::new(unbound));
    assert!(
        accepts(&unbound_auth, &confused).await,
        "without a binding the coarse issuer list accepts the token"
    );

    // An empty kid (single-key source, token without a header kid) is bound too.
    let no_kid = Header::new(Algorithm::ES256);
    let empty_kid_token = encode(
        &no_kid,
        &json!({
            "sub": "user-1",
            "iss": ISSUER_A,
            "iat": now_secs(),
            "exp": now_secs() + 3600,
        }),
        &EncodingKey::from_ec_pem(private_a.as_bytes()).expect("encoding key"),
    )
    .expect("sign ES256 token");
    let empty_kid_bound =
        MultiIssuerKeyResolver::from_resolvers(vec![static_source(&public_a, "")])
            .expect("one source")
            .bind_issuers(vec![ISSUER_B.to_string()])
            .expect("bound");
    let empty_kid_auth =
        JwtAuth::from_validation_with_resolver(None, &validation, Arc::new(empty_kid_bound));
    let error = auth_error(&empty_kid_auth, &empty_kid_token).await;
    assert_eq!(
        error.status_code(),
        401,
        "a kid-less token must be bound to its source issuer too"
    );
}

#[tokio::test]
async fn local_first_binds_remote_tokens_to_their_issuer() {
    let (_, local_public) = ec_key_pair();
    let (private_b, public_b) = ec_key_pair();

    let validation = JwtValidation::builder()
        .algorithms([JwtAlgorithm::Es256])
        .require_issuers([ISSUER_A, ISSUER_B])
        .invalid_token_behavior(InvalidTokenBehavior::Reject)
        .jwks_sources(vec![
            jwks_source(
                ISSUER_A,
                "https://id.alpha.example.com/.well-known/jwks.json",
            ),
            jwks_source(
                ISSUER_B,
                "https://id.beta.example.com/.well-known/jwks.json",
            ),
        ])
        .build()
        .expect("multi-issuer validation");

    let remote = MultiIssuerKeyResolver::from_resolvers(vec![static_source(&public_b, "kid-b")])
        .expect("remote source")
        .bind_issuers(vec![ISSUER_B.to_string()])
        .expect("bound remote");
    let local = static_source(&local_public, "kid-local");
    let resolver = LocalFirstKeyResolver::new(local, Arc::new(remote));
    let auth = JwtAuth::from_validation_with_resolver(None, &validation, Arc::new(resolver));

    let confused = es256_token_with_iss(&private_b, "kid-b", ISSUER_A);
    assert_eq!(auth_error(&auth, &confused).await.status_code(), 401);

    let honest = es256_token_with_iss(&private_b, "kid-b", ISSUER_B);
    assert!(accepts(&auth, &honest).await);
}

#[test]
fn single_shared_jwks_cannot_trust_multiple_issuers() {
    let error = JwtValidation::builder()
        .algorithms([JwtAlgorithm::Es256])
        .require_issuers([ISSUER_A, ISSUER_B])
        .jwks(Some(jwks_endpoint(
            "https://id.alpha.example.com/.well-known/jwks.json",
        )))
        .build()
        .expect_err("a shared JWKS cannot bind keys to two issuers");
    assert_eq!(error.code(), "dependency_injection", "got {error:?}");
    assert!(error.to_string().contains("JwksSources"), "got {error}");
}

#[test]
fn hs256_cannot_trust_multiple_issuers() {
    let error = JwtValidation::builder()
        .require_issuers([ISSUER_A, ISSUER_B])
        .build()
        .expect_err("an HMAC secret cannot express per-issuer trust");
    assert_eq!(error.code(), "dependency_injection", "got {error:?}");
    assert!(error.to_string().contains("HS256"), "got {error}");
}
