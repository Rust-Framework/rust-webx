//! 2.5 M3 — mTLS identity mapping from the verified client-certificate header.

mod test_utils;

use std::sync::Arc;

use async_trait::async_trait;
use webx_core::auth::{IAuthenticationScheme, IClaims};
use webx_core::config::TlsClientAuth;
use webx_core::error::Result;
use webx_core::http::CLIENT_CERT_FINGERPRINT_HEADER;
use webx_core::mode::AppMode;
use webx_host::auth_jwt::JwtClaims;
use webx_host::auth_mtls::{IMtlsIdentityResolver, MtlsScheme};
use webx_host::server::Host;

struct FakeResolver;

#[async_trait]
impl IMtlsIdentityResolver for FakeResolver {
    async fn resolve(&self, fingerprint: &str) -> Result<Option<Box<dyn IClaims>>> {
        if fingerprint == "known-fingerprint" {
            Ok(Some(Box::new(JwtClaims::new("user-1"))))
        } else {
            Ok(None)
        }
    }
}

fn scheme() -> MtlsScheme {
    MtlsScheme::new(Arc::new(FakeResolver))
}

#[tokio::test]
async fn known_client_certificate_authenticates() {
    let mut ctx = test_utils::TestHttpContext::new("GET", "/resource")
        .with_header(CLIENT_CERT_FINGERPRINT_HEADER, "known-fingerprint");
    let scheme = scheme();
    assert!(scheme.matches(&ctx));
    let claims = scheme
        .authenticate(&mut ctx)
        .await
        .expect("authenticate")
        .expect("identity");
    assert_eq!(claims.subject(), "user-1");
}

#[tokio::test]
async fn unknown_certificate_is_401_not_anonymous() {
    let mut ctx = test_utils::TestHttpContext::new("GET", "/resource")
        .with_header(CLIENT_CERT_FINGERPRINT_HEADER, "unknown");
    let error = scheme()
        .authenticate(&mut ctx)
        .await
        .err()
        .expect("unknown certificate");
    assert!(error.code() == "unauthorized", "{error:?}");
}

#[tokio::test]
async fn missing_header_does_not_match() {
    let mut ctx = test_utils::TestHttpContext::new("GET", "/resource");
    assert!(!scheme().matches(&ctx));
    assert!(scheme()
        .authenticate(&mut ctx)
        .await
        .expect("no header is anonymous")
        .is_none());
}

#[test]
fn half_configured_mtls_fails_at_build() {
    let missing_ca = Host::builder()
        .mode(AppMode::Development)
        .no_spa()
        .configure(|app| {
            app.useOptions(|options| {
                options.tls.client_auth = TlsClientAuth::Required;
            });
        })
        .try_build()
        .err()
        .expect("ClientAuth without a CA must fail");
    assert!(missing_ca.to_string().contains("ClientCaPath"));

    let dir = tempfile::tempdir().expect("temp dir");
    let ca = dir.path().join("ca.pem");
    std::fs::write(&ca, "dummy").expect("ca file");

    let ca_path = ca.to_string_lossy().into_owned();
    let http_only = Host::builder()
        .mode(AppMode::Development)
        .no_spa()
        .configure(move |app| {
            let ca_path = ca_path.clone();
            app.useOptions(move |options| {
                options.tls.client_auth = TlsClientAuth::Required;
                options.tls.client_ca_path = ca_path;
                options.app.urls = vec!["http://127.0.0.1:0".to_string()];
            });
        })
        .try_build()
        .err()
        .expect("mTLS without an https listener must fail");
    assert!(http_only.to_string().contains("https://"));

    let ca_path = ca.to_string_lossy().into_owned();
    let ca_without_auth = Host::builder()
        .mode(AppMode::Development)
        .no_spa()
        .configure(move |app| {
            let ca_path = ca_path.clone();
            app.useOptions(move |options| {
                options.tls.client_ca_path = ca_path;
            });
        })
        .try_build()
        .err()
        .expect("ClientCaPath with ClientAuth=Off must fail");
    assert!(ca_without_auth.to_string().contains("ClientAuth"));
}
