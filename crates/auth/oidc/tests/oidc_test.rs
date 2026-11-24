//! 2.5 M2 — OIDC RP: discovery, PKCE, code exchange and id_token validation.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use base64::Engine as _;
use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
use serde_json::{json, Value};
use sha2::{Digest as _, Sha256};
use url::Url;
use webx_core::error::{Error, Result};
use webx_oidc::{
    AuthorizationRequest, ClientAuth, HttpOidcHttp, IOidcHttp, OidcClient, OidcConfig, Pkce,
};

const ISSUER: &str = "https://id.example.com";
const CLIENT_ID: &str = "client-1";
const KID: &str = "kid-1";
const REDIRECT: &str = "https://app.example.com/cb";

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

fn jwks_document(public_pem: &str) -> Value {
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
        "keys": [{
            "kty": "EC",
            "kid": KID,
            "use": "sig",
            "crv": "P-256",
            "x": b64.encode(&xy[..32]),
            "y": b64.encode(&xy[32..])
        }]
    })
}

fn discovery_document() -> Value {
    json!({
        "issuer": ISSUER,
        "authorization_endpoint": "https://id.example.com/authorize",
        "token_endpoint": "https://id.example.com/token",
        "jwks_uri": "https://id.example.com/jwks",
        "code_challenge_methods_supported": ["S256"],
        "id_token_signing_alg_values_supported": ["ES256"],
        "response_types_supported": ["code"]
    })
}

fn id_token(
    private_pem: &str,
    kid: &str,
    issuer: &str,
    audience: Value,
    nonce: &str,
    exp: u64,
) -> String {
    id_token_with_at_hash(
        private_pem,
        kid,
        issuer,
        audience,
        nonce,
        exp,
        &at_hash("access"),
    )
}

fn at_hash(access_token: &str) -> String {
    let digest = Sha256::digest(access_token.as_bytes());
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&digest[..digest.len() / 2])
}

fn id_token_with_at_hash(
    private_pem: &str,
    kid: &str,
    issuer: &str,
    audience: Value,
    nonce: &str,
    exp: u64,
    at_hash: &str,
) -> String {
    let mut header = Header::new(Algorithm::ES256);
    header.kid = Some(kid.to_string());
    encode(
        &header,
        &json!({
            "sub": "user-1",
            "iss": issuer,
            "aud": audience,
            "exp": exp,
            "iat": now_secs(),
            "nonce": nonce,
            "email": "user@example.com",
            "name": "Ada",
            "at_hash": at_hash
        }),
        &EncodingKey::from_ec_pem(private_pem.as_bytes()).expect("encoding key"),
    )
    .expect("sign id_token")
}

struct FakeHttp {
    discovery: Value,
    jwks: Value,
    token: Mutex<Value>,
    posts: AtomicUsize,
    last_form: Mutex<Vec<(String, String)>>,
    last_headers: Mutex<Vec<(String, String)>>,
}

impl FakeHttp {
    fn new(discovery: Value, jwks: Value, token: Value) -> Self {
        Self {
            discovery,
            jwks,
            token: Mutex::new(token),
            posts: AtomicUsize::new(0),
            last_form: Mutex::new(Vec::new()),
            last_headers: Mutex::new(Vec::new()),
        }
    }

    fn form(&self) -> Vec<(String, String)> {
        self.last_form.lock().unwrap().clone()
    }

    fn headers(&self) -> Vec<(String, String)> {
        self.last_headers.lock().unwrap().clone()
    }
}

#[async_trait]
impl IOidcHttp for FakeHttp {
    async fn get_json(&self, url: &str) -> Result<Value> {
        if url.contains("openid-configuration") {
            Ok(self.discovery.clone())
        } else if url.ends_with("/jwks") {
            Ok(self.jwks.clone())
        } else {
            Err(Error::status(503, format!("unexpected GET {url}")))
        }
    }

    async fn post_form(
        &self,
        url: &str,
        form: &[(&str, String)],
        headers: &[(&str, String)],
    ) -> Result<Value> {
        assert_eq!(url, "https://id.example.com/token");
        self.posts.fetch_add(1, Ordering::SeqCst);
        *self.last_form.lock().unwrap() = form
            .iter()
            .map(|(key, value)| (key.to_string(), value.clone()))
            .collect();
        *self.last_headers.lock().unwrap() = headers
            .iter()
            .map(|(key, value)| (key.to_string(), value.clone()))
            .collect();
        Ok(self.token.lock().unwrap().clone())
    }
}

fn http_with_token(token: Value, public_pem: &str) -> Arc<FakeHttp> {
    Arc::new(FakeHttp::new(
        discovery_document(),
        jwks_document(public_pem),
        token,
    ))
}

async fn build_client(http: Arc<FakeHttp>) -> OidcClient {
    OidcClient::discover(http, ISSUER, CLIENT_ID, ClientAuth::None)
        .await
        .expect("discover")
}

fn token_json(id_token: String) -> Value {
    json!({
        "access_token": "access",
        "id_token": id_token,
        "refresh_token": "refresh",
        "expires_in": 3600,
        "token_type": "Bearer"
    })
}

#[tokio::test]
async fn authorization_url_carries_pkce_state_and_nonce() {
    let (_, public) = key_pair();
    let http = http_with_token(token_json(String::new()), &public);
    let client = build_client(http).await;
    let request = client
        .authorization_url(
            AuthorizationRequest::new(REDIRECT)
                .pkce_s256()
                .state("state-1")
                .nonce("nonce-1"),
        )
        .expect("authorization url");

    let url = Url::parse(&request.url).expect("url");
    let params: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
    assert_eq!(
        params.get("response_type").map(String::as_str),
        Some("code")
    );
    assert_eq!(params.get("client_id").map(String::as_str), Some(CLIENT_ID));
    assert_eq!(params.get("state").map(String::as_str), Some("state-1"));
    assert_eq!(params.get("nonce").map(String::as_str), Some("nonce-1"));
    assert_eq!(
        params.get("code_challenge_method").map(String::as_str),
        Some("S256")
    );
    let verifier = request.pkce_verifier.expect("verifier");
    let expected_challenge = Pkce::challenge_for(&verifier);
    assert_eq!(
        params.get("code_challenge").map(String::as_str),
        Some(expected_challenge.as_str())
    );
    drop(public);
}

#[tokio::test]
async fn exchange_validates_and_maps_the_session() {
    let (private, public) = key_pair();
    let http = http_with_token(
        token_json(id_token(
            &private,
            KID,
            ISSUER,
            json!(CLIENT_ID),
            "nonce-1",
            now_secs() + 3600,
        )),
        &public,
    );
    let client = build_client(http).await;

    let session = client
        .exchange_code("code-1", REDIRECT, Some("verifier"), Some("nonce-1"))
        .await
        .expect("exchange");
    assert_eq!(session.subject, "user-1");
    assert_eq!(session.email.as_deref(), Some("user@example.com"));
    assert_eq!(session.name.as_deref(), Some("Ada"));
    assert_eq!(session.access_token.as_deref(), Some("access"));
    assert_eq!(session.refresh_token.as_deref(), Some("refresh"));
    let claims = session.to_token_claims();
    assert_eq!(claims.subject, "user-1");
    assert_eq!(
        claims.extra.get("email").map(String::as_str),
        Some("user@example.com")
    );
    drop(public);
}

#[tokio::test]
async fn nonce_issuer_audience_and_expiry_are_enforced() {
    let (private, public) = key_pair();

    for (label, token) in [
        (
            "nonce",
            id_token(
                &private,
                KID,
                ISSUER,
                json!(CLIENT_ID),
                "wrong",
                now_secs() + 3600,
            ),
        ),
        (
            "issuer",
            id_token(
                &private,
                KID,
                "https://evil.example.com",
                json!(CLIENT_ID),
                "nonce-1",
                now_secs() + 3600,
            ),
        ),
        (
            "audience",
            id_token(
                &private,
                KID,
                ISSUER,
                json!("other-client"),
                "nonce-1",
                now_secs() + 3600,
            ),
        ),
        (
            "expired",
            id_token(
                &private,
                KID,
                ISSUER,
                json!(CLIENT_ID),
                "nonce-1",
                now_secs() - 3600,
            ),
        ),
    ] {
        let http = http_with_token(token_json(token), &public);
        let client = build_client(http).await;
        let error = client
            .exchange_code("code-1", REDIRECT, Some("verifier"), Some("nonce-1"))
            .await
            .err()
            .unwrap_or_else(|| panic!("{label} must be rejected"));
        assert!(
            error.code() == "unauthorized",
            "{label}: expected 401, got {error:?}"
        );
    }
    drop(public);
}

#[tokio::test]
async fn at_hash_mismatch_is_rejected() {
    let (private, public) = key_pair();
    let token = token_json(id_token_with_at_hash(
        &private,
        KID,
        ISSUER,
        json!(CLIENT_ID),
        "nonce-1",
        now_secs() + 3600,
        "definitely-not-the-access-token-hash",
    ));
    let client = build_client(http_with_token(token, &public)).await;
    let error = client
        .exchange_code("code-1", REDIRECT, Some("verifier"), Some("nonce-1"))
        .await
        .expect_err("at_hash mismatch");
    assert!(error.code() == "unauthorized", "{error:?}");
}

#[tokio::test]
async fn at_hash_without_access_token_is_rejected() {
    let (private, public) = key_pair();
    let token = json!({
        "id_token": id_token(
            &private,
            KID,
            ISSUER,
            json!(CLIENT_ID),
            "nonce-1",
            now_secs() + 3600,
        ),
        "expires_in": 3600
    });
    let client = build_client(http_with_token(token, &public)).await;
    let error = client
        .exchange_code("code-1", REDIRECT, Some("verifier"), Some("nonce-1"))
        .await
        .expect_err("at_hash without access token");
    assert!(error.code() == "unauthorized", "{error:?}");
}

#[test]
fn oidc_config_rejects_unbounded_values() {
    let config = OidcConfig {
        leeway: std::time::Duration::from_secs(301),
        ..OidcConfig::default()
    };
    assert!(config.validate().is_err());
    let config = OidcConfig {
        jwks_ttl: std::time::Duration::ZERO,
        ..OidcConfig::default()
    };
    assert!(config.validate().is_err());
    let config = OidcConfig {
        max_keys: 0,
        ..OidcConfig::default()
    };
    assert!(config.validate().is_err());
}

#[test]
fn unadvertised_pkce_requires_an_explicit_opt_in() {
    let discovery = webx_oidc::OidcDiscovery {
        issuer: ISSUER.to_string(),
        authorization_endpoint: "https://id.example.com/authorize".to_string(),
        token_endpoint: "https://id.example.com/token".to_string(),
        jwks_uri: "https://id.example.com/jwks".to_string(),
        userinfo_endpoint: None,
        end_session_endpoint: None,
        code_challenge_methods_supported: Vec::new(),
        id_token_signing_alg_values_supported: vec!["ES256".to_string()],
        response_types_supported: vec!["code".to_string()],
        scopes_supported: Vec::new(),
    };
    assert!(discovery.validate(ISSUER).is_err());
    assert!(discovery.validate_with(ISSUER, false).is_ok());
}

#[tokio::test]
async fn public_client_authorization_url_requires_pkce() {
    let (_, public) = key_pair();
    let client = build_client(http_with_token(token_json(String::new()), &public)).await;
    let error = client
        .authorization_url(AuthorizationRequest::new(REDIRECT))
        .expect_err("public client without PKCE");
    assert!(error.code() == "validation_failed", "{error:?}");
}

#[tokio::test]
async fn public_client_requires_pkce_verifier() {
    let (_, public) = key_pair();
    let http = http_with_token(token_json(String::new()), &public);
    let client = build_client(http).await;
    let error = client
        .exchange_code("code-1", REDIRECT, None, Some("nonce-1"))
        .await
        .expect_err("public client without PKCE");
    assert!(error.code() == "validation_failed", "{error:?}");
}

#[tokio::test]
async fn unknown_kid_and_wrong_algorithm_are_rejected() {
    let (private, public) = key_pair();
    let http = http_with_token(
        token_json(id_token(
            &private,
            "other-kid",
            ISSUER,
            json!(CLIENT_ID),
            "nonce-1",
            now_secs() + 3600,
        )),
        &public,
    );
    let client = build_client(http).await;
    assert!(client
        .exchange_code("code-1", REDIRECT, Some("verifier"), Some("nonce-1"))
        .await
        .is_err());

    // HS256 is never accepted for an id_token verified against JWKS.
    let hs = encode(
        &Header::default(),
        &json!({
            "sub": "user-1",
            "iss": ISSUER,
            "aud": CLIENT_ID,
            "exp": now_secs() + 3600,
            "nonce": "nonce-1"
        }),
        &EncodingKey::from_secret(b"shared-secret-at-least-32-bytes-long"),
    )
    .expect("hs token");
    let http = http_with_token(token_json(hs), &public);
    let client = build_client(http).await;
    assert!(client
        .exchange_code("code-1", REDIRECT, Some("verifier"), Some("nonce-1"))
        .await
        .is_err());
    drop(public);
}

#[tokio::test]
async fn client_secret_post_and_basic_are_applied() {
    let (private, public) = key_pair();
    let token = token_json(id_token(
        &private,
        KID,
        ISSUER,
        json!(CLIENT_ID),
        "nonce-1",
        now_secs() + 3600,
    ));

    let http = http_with_token(token.clone(), &public);
    let client = OidcClient::discover(
        http.clone(),
        ISSUER,
        CLIENT_ID,
        ClientAuth::ClientSecretPost("secret".to_string()),
    )
    .await
    .expect("discover");
    client
        .exchange_code("code-1", REDIRECT, Some("verifier"), Some("nonce-1"))
        .await
        .expect("exchange");
    assert!(http
        .form()
        .iter()
        .any(|(key, value)| key == "client_secret" && value == "secret"));

    let http = http_with_token(token, &public);
    let client = OidcClient::discover(
        http.clone(),
        ISSUER,
        CLIENT_ID,
        ClientAuth::ClientSecretBasic("secret".to_string()),
    )
    .await
    .expect("discover");
    client
        .exchange_code("code-1", REDIRECT, Some("verifier"), Some("nonce-1"))
        .await
        .expect("exchange");
    let authorization = http
        .headers()
        .into_iter()
        .find(|(key, _)| key == "authorization")
        .map(|(_, value)| value)
        .expect("basic header");
    assert!(authorization.starts_with("Basic "));
    drop(public);
}

#[tokio::test]
async fn discovery_rejects_issuer_mismatch_and_missing_s256() {
    let (_, public) = key_pair();
    let jwks = jwks_document(&public);
    let mut discovery = discovery_document();
    discovery["issuer"] = json!("https://other.example.com");
    let http = Arc::new(FakeHttp::new(discovery, jwks.clone(), json!({})));
    assert!(
        OidcClient::discover(http, ISSUER, CLIENT_ID, ClientAuth::None)
            .await
            .is_err()
    );

    let mut discovery = discovery_document();
    discovery["code_challenge_methods_supported"] = json!(["plain"]);
    let http = Arc::new(FakeHttp::new(discovery, jwks, json!({})));
    assert!(
        OidcClient::discover(http, ISSUER, CLIENT_ID, ClientAuth::None)
            .await
            .is_err()
    );
    drop(public);
}

#[test]
fn http_oidc_transport_requires_https_and_accepts_an_explicit_config() {
    assert!(HttpOidcHttp::new("http://id.example.com", std::time::Duration::from_secs(3)).is_err());
    assert!(HttpOidcHttp::new(ISSUER, std::time::Duration::from_secs(3)).is_ok());
    let config = OidcConfig::default();
    assert!(config
        .algorithms
        .iter()
        .all(|algorithm| *algorithm != webx_core::config::JwtAlgorithm::Hs256));
}
