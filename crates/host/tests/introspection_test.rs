//! 2.5 M4 — RFC 7662 introspection and RFC 7009 revocation.

use std::sync::Arc;

use base64::Engine as _;
use jsonwebtoken::{DecodingKey, Validation};
use webx_core::security::{
    ISessionStore, ITokenIssuer, InMemoryRefreshTokenStore, InMemorySessionStore, TokenClaims,
};
use webx_host::auth_jwt::{JwtAuth, JwtTokenIssuer};
use webx_host::introspection::{
    IntrospectionBasicAuth, IntrospectionResponse, TokenIntrospector, TokenRevoker,
};
use webx_host::refresh::RefreshTokenService;

const SECRET: &str = "introspection-test-secret-at-least-32-bytes";

fn validator() -> Arc<JwtAuth> {
    let mut validation = Validation::default();
    validation.set_audience(&["api"]);
    Arc::new(JwtAuth::new(
        DecodingKey::from_secret(SECRET.as_bytes()),
        validation,
    ))
}

fn issuer() -> JwtTokenIssuer {
    JwtTokenIssuer::from_secret(SECRET.as_bytes(), std::time::Duration::from_secs(3600))
        .with_audience("api")
}

async fn access_token() -> String {
    issuer()
        .issue(
            TokenClaims::new("user-1")
                .with_claim("scope", "read write")
                .with_claim("client_id", "client-1")
                .with_claim("name", "Ada"),
        )
        .await
        .expect("issue")
        .token
}

#[tokio::test]
async fn active_token_reports_metadata() {
    let token = access_token().await;
    let response = TokenIntrospector::new(validator())
        .introspect(&token)
        .await
        .expect("introspect");
    assert!(response.active);
    assert_eq!(response.sub.as_deref(), Some("user-1"));
    assert_eq!(response.scope.as_deref(), Some("read write"));
    assert_eq!(response.client_id.as_deref(), Some("client-1"));
    assert_eq!(response.username.as_deref(), Some("Ada"));
    assert_eq!(response.token_type.as_deref(), Some("Bearer"));
    assert!(response.exp.is_some());
    assert!(response.iat.is_some());
    assert!(response.jti.is_some());
    assert_eq!(response.aud, Some(serde_json::json!("api")));
}

#[tokio::test]
async fn invalid_and_revoked_tokens_are_inactive() {
    let token = access_token().await;
    let sessions: Arc<dyn ISessionStore> = Arc::new(InMemorySessionStore::new());
    let introspector =
        TokenIntrospector::new(validator()).with_session_store(Arc::clone(&sessions));

    let invalid = introspector
        .introspect("not-a-jwt")
        .await
        .expect("invalid is not an error");
    assert!(!invalid.active);
    assert_eq!(
        serde_json::to_string(&invalid).unwrap(),
        r#"{"active":false}"#
    );

    let active = introspector.introspect(&token).await.expect("active");
    let jti = active.jti.expect("jti");
    sessions
        .revoke_until(&jti, active.exp.expect("exp"))
        .await
        .expect("revoke");
    let revoked = introspector.introspect(&token).await.expect("revoked");
    assert!(!revoked.active);
}

#[tokio::test]
async fn revocation_endpoint_semantics() {
    let sessions: Arc<dyn ISessionStore> = Arc::new(InMemorySessionStore::new());
    let revoker = TokenRevoker::new(validator(), Arc::clone(&sessions));
    let token = access_token().await;

    revoker.revoke(&token).await.expect("revoke");
    let response = TokenIntrospector::new(validator())
        .with_session_store(Arc::clone(&sessions))
        .introspect(&token)
        .await
        .expect("introspect");
    assert!(!response.active);

    // Unknown/garbage tokens succeed (RFC 7009) without touching the store.
    revoker.revoke("unknown").await.expect("unknown is a no-op");
    revoker.revoke("a.b").await.expect("malformed is a no-op");
}

#[tokio::test]
async fn refresh_tokens_are_revoked_through_the_same_endpoint() {
    let sessions: Arc<dyn ISessionStore> = Arc::new(InMemorySessionStore::new());
    let refresh = Arc::new(RefreshTokenService::new(
        Arc::new(InMemoryRefreshTokenStore::new()),
        std::time::Duration::from_secs(3600),
        std::time::Duration::from_secs(24 * 3600),
    ));
    let issued = refresh.issue("user-1", None, None).await.expect("issue");
    let revoker = TokenRevoker::new(validator(), sessions).with_refresh(Arc::clone(&refresh));

    revoker.revoke(&issued.token).await.expect("revoke refresh");
    assert!(refresh.rotate(&issued.token).await.is_err());
    assert!(!refresh
        .revoke_token(&issued.token)
        .await
        .expect("second revoke"));
}

#[test]
fn introspection_client_auth_is_basic_and_constant_time_checked() {
    let auth = IntrospectionBasicAuth::new("client-1", "secret-1").expect("config");
    let valid = format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode("client-1:secret-1")
    );
    assert!(auth.authorize(Some(&valid)));
    assert!(!auth.authorize(Some("Bearer token")));
    assert!(!auth.authorize(None));
    let wrong = format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode("client-1:wrong")
    );
    assert!(!auth.authorize(Some(&wrong)));
    assert!(format!("{auth:?}").contains("<redacted>"));
    assert!(IntrospectionBasicAuth::new("", "secret").is_err());
}

#[test]
fn inactive_response_has_only_active_field() {
    let response = IntrospectionResponse {
        active: false,
        scope: None,
        client_id: None,
        username: None,
        token_type: None,
        exp: None,
        iat: None,
        nbf: None,
        sub: None,
        aud: None,
        iss: None,
        jti: None,
    };
    assert_eq!(
        serde_json::to_string(&response).unwrap(),
        r#"{"active":false}"#
    );
}
