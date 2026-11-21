//! Live OIDC discovery/JWKS smoke test (network; run manually).
//!
//! ```powershell
//! cargo test -p rust-webx-oidc --test oidc_live_test -- --ignored --nocapture
//! ```
//!
//! CI does not run this test: it depends on public providers and outbound
//! network. It verifies the real wire format, exact issuer match, HTTPS JWKS
//! fetch and JWK parsing (not just the local fake IdP).

use std::sync::Arc;
use std::time::Duration;

use jsonwebtoken::Algorithm;
use webx_host::auth_jwks::IJwtKeyResolver;
use webx_host::auth_jwks::{HttpJwksFetcher, IJwksFetcher, JwksResolver};
use webx_oidc::{
    AuthorizationRequest, ClientAuth, HttpOidcHttp, OidcClient, OidcConfig, OidcDiscovery,
};

const LIVE_ISSUERS: &[&str] = &[
    "https://accounts.google.com",
    "https://demo.duendesoftware.com",
];

#[tokio::test]
#[ignore = "requires outbound network and a fresh authorization code; see oidc_live_test docs"]
async fn live_authorization_code_exchange() {
    let Ok(code) = std::env::var("OIDC_DUENDE_CODE") else {
        eprintln!("OIDC_DUENDE_CODE not set; skipping");
        return;
    };
    let verifier = std::env::var("OIDC_DUENDE_VERIFIER")
        .unwrap_or_else(|_| "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk".to_string());
    let nonce = std::env::var("OIDC_DUENDE_NONCE").unwrap_or_else(|_| "n".to_string());
    let issuer = "https://demo.duendesoftware.com";
    let http = HttpOidcHttp::new(issuer, Duration::from_secs(20)).expect("http");
    let client = OidcClient::discover(
        Arc::new(http),
        issuer,
        "interactive.public",
        ClientAuth::None,
    )
    .await
    .expect("Duende discovery");
    let session = client
        .exchange_code(
            &code,
            "com.duendesoftware.demo:/oauthredirect",
            Some(&verifier),
            Some(&nonce),
        )
        .await
        .expect("code exchange");
    assert!(!session.subject.trim().is_empty());
    eprintln!(
        "live authorization code exchange OK: sub={} email={:?}",
        session.subject, session.email
    );
}

#[tokio::test]
#[ignore = "requires outbound network; run manually with --ignored"]
async fn live_token_endpoint_rejects_a_bogus_code() {
    let issuer = "https://demo.duendesoftware.com";
    let http = HttpOidcHttp::new(issuer, Duration::from_secs(20)).expect("http");
    let client = OidcClient::discover(Arc::new(http), issuer, "live-check", ClientAuth::None)
        .await
        .expect("Duende discovery");
    let auth = client
        .authorization_url(
            AuthorizationRequest::new("https://app.example.com/cb")
                .pkce_s256()
                .state("live-state")
                .nonce("live-nonce"),
        )
        .expect("authorization URL");
    let error = client
        .exchange_code(
            "definitely-not-a-valid-code",
            "https://app.example.com/cb",
            auth.pkce_verifier.as_deref(),
            Some(&auth.nonce),
        )
        .await
        .expect_err("bogus code must fail");
    assert_eq!(
        error.status_code(),
        401,
        "a rejected code must be 401, not a gateway error: {error}"
    );
    eprintln!("live token endpoint rejected a bogus code: {error}");
}

#[tokio::test]
#[ignore = "requires outbound network; run manually with --ignored"]
async fn live_microsoft_multitenant_discovery() {
    // Microsoft's tenant-specific discovery is exact-issuer; its PKCE support
    // is real but not always advertised, so this crosses the explicit opt-in.
    let issuer = "https://login.microsoftonline.com/72f988bf-86f1-41af-91ab-2d7cd011db47/v2.0";
    let http = HttpOidcHttp::new(issuer, Duration::from_secs(20)).expect("http");
    let config = OidcConfig {
        require_pkce_advertised: false,
        ..OidcConfig::default()
    };
    let client = OidcClient::discover_with(
        Arc::new(http),
        issuer,
        "live-check",
        ClientAuth::None,
        config,
    )
    .await
    .expect("Microsoft discovery");
    assert_eq!(client.discovery().issuer, issuer);
    assert!(client.discovery().jwks_uri.starts_with("https://"));
    eprintln!("live Microsoft OIDC OK: issuer={issuer}");
}

#[tokio::test]
#[ignore = "requires outbound network; run manually with --ignored"]
async fn live_discovery_and_jwks() {
    let mut verified = 0usize;
    for issuer in LIVE_ISSUERS {
        let http = HttpOidcHttp::new(issuer, Duration::from_secs(20)).expect("http");
        let discovery = match OidcDiscovery::fetch(&http, issuer).await {
            Ok(discovery) => discovery,
            Err(error) => {
                // A public provider can be unreachable from a given network
                // (e.g. Google from mainland China). Skip it, but require at
                // least one reachable provider so the test is not vacuous.
                eprintln!("skip unreachable live issuer {issuer}: {error}");
                continue;
            }
        };
        assert_eq!(discovery.issuer.trim_end_matches('/'), *issuer);
        assert!(
            discovery
                .id_token_signing_alg_values_supported
                .iter()
                .any(|alg| alg == "RS256" || alg == "ES256"),
            "{issuer} must advertise RS256/ES256"
        );

        let fetcher = HttpJwksFetcher::new(&discovery.jwks_uri, Duration::from_secs(20), false)
            .expect("jwks fetcher");
        let set = fetcher
            .fetch()
            .await
            .unwrap_or_else(|error| panic!("jwks {issuer}: {error}"));
        assert!(!set.keys.is_empty(), "{issuer} returned no keys");

        let key = set
            .keys
            .iter()
            .find(|key| key.kid.is_some() && (key.kty == "RSA" || key.kty == "EC"))
            .unwrap_or_else(|| panic!("{issuer} has no usable signing key"));
        let kid = key.kid.clone().expect("kid");
        let algorithm = if key.kty == "RSA" {
            Algorithm::RS256
        } else {
            Algorithm::ES256
        };
        let resolver = JwksResolver::new(
            Arc::new(
                HttpJwksFetcher::new(&discovery.jwks_uri, Duration::from_secs(20), false)
                    .expect("resolver fetcher"),
            ),
            Duration::from_secs(60),
            32,
        );
        resolver
            .resolve(&kid, algorithm)
            .await
            .unwrap_or_else(|error| panic!("resolve {issuer} kid={kid}: {error}"));

        eprintln!(
            "live OIDC OK: issuer={issuer} keys={} kid={kid} alg={algorithm:?}",
            set.keys.len()
        );
        verified += 1;
    }
    assert!(verified >= 1, "no live issuer was reachable");
}
