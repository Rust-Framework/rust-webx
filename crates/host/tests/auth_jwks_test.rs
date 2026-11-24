//! 2.5 M2 — JWKS-backed ES256 verification and `kid` rotation.

mod test_utils;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
use serde_json::json;
use webx_core::auth::IAuthenticationHandler;
use webx_core::config::{JwtAlgorithm, JwtJwksSection};
use webx_core::error::{Error, Result};
use webx_core::mode::AppMode;
use webx_host::auth_jwks::{
    IJwksFetcher, IJwtKeyResolver, JwkSet, JwksHealthCheck, JwksResolver, StaticKeyResolver,
};
use webx_host::auth_jwt::{JwtAuth, JwtValidation};
use webx_host::health::IHealthCheck;
use webx_host::server::Host;

const KID: &str = "kid-1";

fn key_pair() -> (String, String) {
    let pair = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).expect("EC key");
    (pair.serialize_pem(), pair.public_key_pem())
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_secs()
}

fn es256_token(private_pem: &str, kid: &str) -> String {
    let mut header = Header::new(Algorithm::ES256);
    header.kid = Some(kid.to_string());
    encode(
        &header,
        &json!({ "sub": "user-1", "exp": now_secs() + 3600 }),
        &EncodingKey::from_ec_pem(private_pem.as_bytes()).expect("encoding key"),
    )
    .expect("sign")
}

fn validation() -> JwtValidation {
    JwtValidation::builder()
        .algorithms([JwtAlgorithm::Es256])
        .jwks(Some(JwtJwksSection {
            url: "https://id.example.com/.well-known/jwks.json".to_string(),
            ..JwtJwksSection::default()
        }))
        .build()
        .expect("validation")
}

fn auth_with_static_key(public_pem: &str) -> JwtAuth {
    let resolver = StaticKeyResolver::new()
        .with_pem(KID, Algorithm::ES256, public_pem.as_bytes())
        .expect("static key");
    JwtAuth::from_validation_with_resolver(None, &validation(), Arc::new(resolver))
}

async fn authenticate(auth: &JwtAuth, token: &str) -> Result<bool> {
    let mut ctx = test_utils::TestHttpContext::new("GET", "/protected")
        .with_header("authorization", &format!("Bearer {token}"));
    auth.authenticate(&mut ctx).await.map(|c| c.is_some())
}

#[tokio::test]
async fn es256_token_verifies_through_a_resolved_kid() {
    let (private, public) = key_pair();
    let token = es256_token(&private, KID);
    assert!(
        authenticate(&auth_with_static_key(&public), &token)
            .await
            .expect("verify"),
        "a token signed by the JWKS key must authenticate"
    );
}

#[tokio::test]
async fn unknown_kid_is_not_authenticated() {
    let (private, public) = key_pair();
    let token = es256_token(&private, "other-kid");
    assert!(
        !authenticate(&auth_with_static_key(&public), &token)
            .await
            .expect("verify"),
        "an unknown kid must not fall back to another key"
    );
}

#[tokio::test]
async fn hs256_token_is_rejected_when_only_es256_is_allowed() {
    let (_, public) = key_pair();
    let token = encode(
        &Header::default(),
        &json!({ "sub": "user-1", "exp": now_secs() + 3600 }),
        &EncodingKey::from_secret(b"some-hs256-secret-at-least-32-bytes"),
    )
    .expect("HS256 token");
    assert!(
        !authenticate(&auth_with_static_key(&public), &token)
            .await
            .expect("verify"),
        "algorithm must be checked against Jwt.Algorithms before key lookup"
    );
}

/// Extract the P-256 x/y coordinates from an SPKI PEM (test helper only).
fn es256_jwk_document(public_pem: &str) -> String {
    let der = base64::engine::general_purpose::STANDARD
        .decode(
            public_pem
                .lines()
                .filter(|line| !line.starts_with("-----"))
                .collect::<String>(),
        )
        .expect("SPKI base64");
    let xy = &der[der.len() - 64..];
    let encode = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    json!({
        "keys": [{
            "kty": "EC",
            "kid": KID,
            "use": "sig",
            "crv": "P-256",
            "x": encode.encode(&xy[..32]),
            "y": encode.encode(&xy[32..])
        }]
    })
    .to_string()
}

struct FakeFetcher {
    calls: AtomicUsize,
    document: String,
    fail: bool,
}

#[async_trait::async_trait]
impl IJwksFetcher for FakeFetcher {
    async fn fetch(&self) -> Result<JwkSet> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.fail {
            return Err(Error::Status(503, "jwks down".to_string()));
        }
        serde_json::from_str(&self.document)
            .map_err(|error| Error::Status(503, format!("bad jwks: {error}")))
    }
}

#[tokio::test]
async fn jwks_cache_hits_and_unknown_kid_refreshes_once() {
    let (private, public) = key_pair();
    let fetcher = Arc::new(FakeFetcher {
        calls: AtomicUsize::new(0),
        document: es256_jwk_document(&public),
        fail: false,
    });
    let resolver = JwksResolver::new(fetcher.clone(), Duration::from_secs(60), 8);

    let first = resolver.resolve(KID, Algorithm::ES256).await.expect("key");
    let second = resolver.resolve(KID, Algorithm::ES256).await.expect("key");
    assert_eq!(fetcher.calls.load(Ordering::SeqCst), 1, "cache hit");
    assert!(Arc::ptr_eq(&first, &second), "the same key is cached");

    let error = resolver
        .resolve("missing", Algorithm::ES256)
        .await
        .err()
        .expect("unknown kid");
    assert!(matches!(error, Error::Unauthorized(_)), "got {error:?}");
    assert_eq!(
        fetcher.calls.load(Ordering::SeqCst),
        2,
        "an unknown kid triggers exactly one refresh"
    );

    // The cached document still answers the original kid.
    resolver
        .resolve(KID, Algorithm::ES256)
        .await
        .expect("cached");
    assert_eq!(fetcher.calls.load(Ordering::SeqCst), 2);
    drop(private);
}

#[tokio::test]
async fn jwks_fetch_failure_is_503_not_anonymous() {
    let fetcher = Arc::new(FakeFetcher {
        calls: AtomicUsize::new(0),
        document: "{}".to_string(),
        fail: true,
    });
    let resolver = JwksResolver::new(fetcher, Duration::from_secs(60), 8);
    let error = resolver
        .resolve(KID, Algorithm::ES256)
        .await
        .err()
        .expect("fetch failure");
    assert!(matches!(error, Error::Status(503, _)), "got {error:?}");
}

#[test]
fn host_builds_with_jwks_only_and_no_shared_secret() {
    Host::builder()
        .mode(AppMode::Development)
        .no_spa()
        .configure(|app| {
            app.useOptions(|options| {
                options.jwt.secret.clear();
                options.jwt.algorithms = vec![JwtAlgorithm::Rs256];
                options.jwt.jwks = Some(JwtJwksSection {
                    url: "https://id.example.com/.well-known/jwks.json".to_string(),
                    ..JwtJwksSection::default()
                });
            });
        })
        .add_authentication()
        .try_build()
        .expect("JWKS-only authentication must build without an HMAC secret");
}

#[tokio::test]
async fn a_single_key_jwks_can_verify_a_token_without_kid() {
    let (_, public) = key_pair();
    let fetcher = Arc::new(FakeFetcher {
        calls: AtomicUsize::new(0),
        document: es256_jwk_document(&public),
        fail: false,
    });
    let resolver = JwksResolver::new(fetcher, Duration::from_secs(60), 8);
    resolver
        .resolve("", Algorithm::ES256)
        .await
        .expect("single key is unambiguous without kid");
}

#[tokio::test]
async fn multiple_keys_without_kid_fail_closed() {
    let (_, public) = key_pair();
    let mut document: serde_json::Value =
        serde_json::from_str(&es256_jwk_document(&public)).expect("jwks");
    let second = document["keys"][0].clone();
    document["keys"].as_array_mut().expect("keys").push(second);
    let fetcher = Arc::new(FakeFetcher {
        calls: AtomicUsize::new(0),
        document: document.to_string(),
        fail: false,
    });
    let resolver = JwksResolver::new(fetcher, Duration::from_secs(60), 8);
    assert!(
        resolver.resolve("", Algorithm::ES256).await.is_err(),
        "ambiguous kid must not be guessed"
    );
}

#[tokio::test]
async fn jwks_health_check_passes_and_fails() {
    let (_, public) = key_pair();
    let healthy = Arc::new(JwksResolver::new(
        Arc::new(FakeFetcher {
            calls: AtomicUsize::new(0),
            document: es256_jwk_document(&public),
            fail: false,
        }),
        Duration::from_secs(60),
        8,
    ));
    assert_eq!(JwksHealthCheck::new(healthy).check().await.status, "pass");

    let broken = Arc::new(JwksResolver::new(
        Arc::new(FakeFetcher {
            calls: AtomicUsize::new(0),
            document: "{}".to_string(),
            fail: true,
        }),
        Duration::from_secs(60),
        8,
    ));
    assert_eq!(JwksHealthCheck::new(broken).check().await.status, "fail");
}

/// An unknown `kid` is attacker-controlled: it may trigger at most one upstream
/// fetch per `min_refresh_interval`, otherwise unauthenticated token spam would
/// serialize every authentication behind a JWKS round trip.
#[tokio::test]
async fn unknown_kid_refresh_is_rate_limited() {
    let (_private, public) = key_pair();
    let fetcher = Arc::new(FakeFetcher {
        calls: AtomicUsize::new(0),
        document: es256_jwk_document(&public),
        fail: false,
    });
    let resolver = JwksResolver::new(fetcher.clone(), Duration::from_secs(60), 8)
        .with_min_refresh_interval(Duration::from_secs(60));

    // Prime the cache: a cold resolver must be allowed to fetch.
    assert!(resolver.resolve(KID, Algorithm::ES256).await.is_ok());
    assert_eq!(fetcher.calls.load(Ordering::SeqCst), 1);

    // The first unknown kid may refresh once (the key may have rotated).
    let error = resolver
        .resolve("rotated-kid", Algorithm::ES256)
        .await
        .err()
        .expect("unknown kid");
    assert!(matches!(error, Error::Unauthorized(_)), "got {error:?}");
    assert_eq!(fetcher.calls.load(Ordering::SeqCst), 2);

    // Every further unknown kid inside the interval is rejected without a fetch.
    for _ in 0..25 {
        let error = resolver
            .resolve("attacker-kid", Algorithm::ES256)
            .await
            .err()
            .expect("unknown kid");
        assert!(matches!(error, Error::Unauthorized(_)), "got {error:?}");
    }
    assert_eq!(
        fetcher.calls.load(Ordering::SeqCst),
        2,
        "unknown kids must not reach the identity provider every time"
    );

    // A known kid still resolves from the refreshed cache without fetching.
    assert!(resolver.resolve(KID, Algorithm::ES256).await.is_ok());
    assert_eq!(fetcher.calls.load(Ordering::SeqCst), 2);
}
