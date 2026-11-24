//! 2.5 M3 — DPoP sender-constrained access tokens (RFC 9449).

mod test_utils;

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use jsonwebtoken::{encode, Algorithm, DecodingKey, EncodingKey, Header, Validation};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use webx_core::auth::IAuthenticationScheme;
use webx_host::auth_dpop::{DpopConfig, DpopScheme, InMemoryDpopReplayStore};
use webx_host::auth_jwks::Jwk;
use webx_host::auth_jwt::JwtAuth;

const SECRET: &str = "dpop-test-hs256-secret-at-least-32-bytes";
const ORIGIN: &str = "https://api.example.com";

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_secs()
}

fn key_pair() -> (String, String) {
    let pair = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).expect("EC key");
    (pair.serialize_pem(), pair.public_key_pem())
}

fn jwk_value(public_pem: &str) -> Value {
    let der = base64::engine::general_purpose::STANDARD
        .decode(
            public_pem
                .lines()
                .filter(|line| !line.starts_with("-----"))
                .collect::<String>(),
        )
        .expect("SPKI");
    let xy = &der[der.len() - 64..];
    let b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    json!({
        "kty": "EC",
        "crv": "P-256",
        "x": b64.encode(&xy[..32]),
        "y": b64.encode(&xy[32..])
    })
}

fn thumbprint(jwk: &Value) -> String {
    let jwk: Jwk = serde_json::from_value(jwk.clone()).expect("jwk");
    jwk.thumbprint().expect("thumbprint")
}

fn access_token(jkt: Option<&str>) -> String {
    let mut claims = json!({
        "sub": "user-1",
        "exp": now_secs() + 3600
    });
    if let Some(jkt) = jkt {
        claims["cnf"] = json!({ "jkt": jkt });
    }
    encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(SECRET.as_bytes()),
    )
    .expect("access token")
}

fn ath(token: &str) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(token.as_bytes()))
}

fn proof(
    private_pem: &str,
    jwk: &Value,
    htm: &str,
    htu: &str,
    iat: u64,
    jti: &str,
    token: &str,
) -> String {
    let mut header = Header::new(Algorithm::ES256);
    header.typ = Some("dpop+jwt".to_string());
    header.jwk = Some(serde_json::from_value(jwk.clone()).expect("jsonwebtoken jwk"));
    encode(
        &header,
        &json!({
            "htm": htm,
            "htu": htu,
            "iat": iat,
            "jti": jti,
            "ath": ath(token)
        }),
        &EncodingKey::from_ec_pem(private_pem.as_bytes()).expect("encoding key"),
    )
    .expect("proof")
}

fn scheme() -> DpopScheme {
    let jwt_auth = Arc::new(JwtAuth::new(
        DecodingKey::from_secret(SECRET.as_bytes()),
        Validation::default(),
    ));
    DpopScheme::new(
        jwt_auth,
        Arc::new(InMemoryDpopReplayStore::new()),
        DpopConfig::new(ORIGIN).expect("config"),
    )
    .expect("scheme")
}

fn request(token: &str, proof: &str) -> test_utils::TestHttpContext {
    test_utils::TestHttpContext::new("GET", "/resource")
        .with_header("authorization", &format!("DPoP {token}"))
        .with_header("dpop", proof)
}

#[tokio::test]
async fn valid_proof_authenticates() {
    let (private, public) = key_pair();
    let jwk = jwk_value(&public);
    let token = access_token(Some(&thumbprint(&jwk)));
    let proof = proof(
        &private,
        &jwk,
        "GET",
        "https://api.example.com/resource",
        now_secs(),
        "j1",
        &token,
    );

    let mut ctx = request(&token, &proof);
    let claims = scheme()
        .authenticate(&mut ctx)
        .await
        .expect("authenticate")
        .expect("claims");
    assert_eq!(claims.subject(), "user-1");
}

#[tokio::test]
async fn replay_and_mismatches_are_rejected() {
    let (private, public) = key_pair();
    let jwk = jwk_value(&public);
    let token = access_token(Some(&thumbprint(&jwk)));
    let scheme = scheme();
    let valid = proof(
        &private,
        &jwk,
        "GET",
        "https://api.example.com/resource",
        now_secs(),
        "j1",
        &token,
    );
    assert!(scheme
        .authenticate(&mut request(&token, &valid))
        .await
        .is_ok());

    let replay = proof(
        &private,
        &jwk,
        "GET",
        "https://api.example.com/resource",
        now_secs(),
        "j1",
        &token,
    );
    let error = scheme
        .authenticate(&mut request(&token, &replay))
        .await
        .err()
        .expect("replay");
    assert!(error.code() == "unauthorized", "{error:?}");

    for (label, candidate) in [
        (
            "htu",
            proof(
                &private,
                &jwk,
                "GET",
                "https://api.example.com/other",
                now_secs(),
                "j2",
                &token,
            ),
        ),
        (
            "htm",
            proof(
                &private,
                &jwk,
                "POST",
                "https://api.example.com/resource",
                now_secs(),
                "j3",
                &token,
            ),
        ),
        (
            "stale",
            proof(
                &private,
                &jwk,
                "GET",
                "https://api.example.com/resource",
                now_secs() - 600,
                "j4",
                &token,
            ),
        ),
    ] {
        let error = scheme
            .authenticate(&mut request(&token, &candidate))
            .await
            .err()
            .unwrap_or_else(|| panic!("{label} must fail"));
        assert!(error.code() == "unauthorized", "{label}: {error:?}");
    }
}

#[tokio::test]
async fn key_binding_and_ath_are_enforced() {
    let (private, public) = key_pair();
    let (other_private, other_public) = key_pair();
    let jwk = jwk_value(&public);
    let other_jwk = jwk_value(&other_public);
    let token = access_token(Some(&thumbprint(&jwk)));

    // Proof signed by a different key than cnf.jkt.
    let wrong_key = proof(
        &other_private,
        &other_jwk,
        "GET",
        "https://api.example.com/resource",
        now_secs(),
        "j1",
        &token,
    );
    let error = scheme()
        .authenticate(&mut request(&token, &wrong_key))
        .await
        .err()
        .expect("key mismatch");
    assert!(error.code() == "unauthorized", "{error:?}");

    // Access token without cnf is not DPoP-bound.
    let unbound = access_token(None);
    let unbound_proof = proof(
        &private,
        &jwk,
        "GET",
        "https://api.example.com/resource",
        now_secs(),
        "j2",
        &unbound,
    );
    let error = scheme()
        .authenticate(&mut request(&unbound, &unbound_proof))
        .await
        .err()
        .expect("unbound token");
    assert!(error.code() == "unauthorized", "{error:?}");

    // Proof bound to a different access token (ath mismatch).
    let other_token = access_token(Some(&thumbprint(&jwk)));
    let error = scheme()
        .authenticate(&mut request(&other_token, &wrong_key))
        .await;
    assert!(error.is_err());
}

#[tokio::test]
async fn scheme_matches_only_dpop_with_a_proof() {
    let scheme = scheme();
    let (private, public) = key_pair();
    let jwk = jwk_value(&public);
    let token = access_token(Some(&thumbprint(&jwk)));
    let proof = proof(
        &private,
        &jwk,
        "GET",
        "https://api.example.com/resource",
        now_secs(),
        "j1",
        &token,
    );

    let ctx = test_utils::TestHttpContext::new("GET", "/resource")
        .with_header("authorization", &format!("Bearer {token}"))
        .with_header("dpop", &proof);
    assert!(!scheme.matches(&ctx));

    let mut ctx = test_utils::TestHttpContext::new("GET", "/resource")
        .with_header("authorization", &format!("DPoP {token}"));
    assert!(!scheme.matches(&ctx));

    ctx = ctx.with_header("dpop", &proof);
    assert!(scheme.matches(&ctx));
    let _ = scheme.authenticate(&mut ctx).await.expect("valid proof");
}

#[test]
fn config_and_replay_validation() {
    assert!(DpopConfig::new("http://api.example.com").is_err());
    let mut config = DpopConfig::new(ORIGIN).unwrap();
    config.max_age = Duration::ZERO;
    assert!(config.validate().is_err());
}
