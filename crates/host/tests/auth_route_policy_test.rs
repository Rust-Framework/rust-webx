//! 2.5 M3-04 — opt-in per-path authentication scheme allow-lists.

mod test_utils;

use std::sync::Arc;

use async_trait::async_trait;
use webx_core::auth::{IAuthenticationScheme, IClaims};
use webx_core::error::Result;
use webx_core::http::{IClaimsExt, IHttpContext};
use webx_core::middleware::IMiddleware;
use webx_host::auth::AuthenticationMiddleware;
use webx_host::auth_jwt::JwtClaims;

struct HeaderScheme {
    name: &'static str,
    header: &'static str,
}

#[async_trait]
impl IAuthenticationScheme for HeaderScheme {
    fn name(&self) -> &'static str {
        self.name
    }

    fn matches(&self, ctx: &dyn IHttpContext) -> bool {
        ctx.request().header(self.header).is_some()
    }

    async fn authenticate(&self, _ctx: &mut dyn IHttpContext) -> Result<Option<Box<dyn IClaims>>> {
        Ok(Some(Box::new(JwtClaims::new(format!(
            "{}-user",
            self.name
        )))))
    }
}

fn middleware() -> AuthenticationMiddleware {
    AuthenticationMiddleware::new(vec![
        Arc::new(HeaderScheme {
            name: "jwt",
            header: "authorization",
        }),
        Arc::new(HeaderScheme {
            name: "mtls",
            header: "x-webx-client-cert-sha256",
        }),
    ])
    .with_route_policies(vec![("/mtls".to_string(), vec!["mtls"])])
}

fn context(path: &str, headers: &[(&str, &str)]) -> test_utils::TestHttpContext {
    let mut ctx = test_utils::TestHttpContext::new("GET", path);
    for (name, value) in headers {
        ctx = ctx.with_header(name, value);
    }
    ctx
}

#[tokio::test]
async fn mtls_path_ignores_a_jwt_credential() {
    let middleware = middleware();
    let mut ctx = context(
        "/mtls/resource",
        &[
            ("authorization", "Bearer a.b.c"),
            ("x-webx-client-cert-sha256", "fingerprint"),
        ],
    );
    let _ = middleware.invoke(&mut ctx).await.expect("invoke");
    assert_eq!(ctx.claims().expect("claims").subject(), "mtls-user");
}

#[tokio::test]
async fn policy_without_an_allowed_credential_stays_anonymous() {
    let middleware = middleware();
    let mut ctx = context("/mtls/resource", &[("authorization", "Bearer a.b.c")]);
    let _ = middleware.invoke(&mut ctx).await.expect("invoke");
    assert!(ctx.claims().is_none());
}

#[tokio::test]
async fn unmatched_paths_keep_the_global_chain() {
    let middleware = middleware();
    let mut ctx = context(
        "/api/resource",
        &[
            ("authorization", "Bearer a.b.c"),
            ("x-webx-client-cert-sha256", "fingerprint"),
        ],
    );
    let _ = middleware.invoke(&mut ctx).await.expect("invoke");
    assert_eq!(ctx.claims().expect("claims").subject(), "jwt-user");
}

#[tokio::test]
async fn a_similar_prefix_is_not_captured() {
    let middleware = middleware();
    // `/mtlsx` must not inherit the `/mtls` policy; it uses the global chain.
    let mut ctx = context(
        "/mtlsx",
        &[
            ("authorization", "Bearer a.b.c"),
            ("x-webx-client-cert-sha256", "fingerprint"),
        ],
    );
    let _ = middleware.invoke(&mut ctx).await.expect("invoke");
    assert_eq!(ctx.claims().expect("claims").subject(), "jwt-user");
}

#[test]
fn host_builder_accepts_route_policies() {
    use webx_core::mode::AppMode;
    use webx_host::server::Host;

    let _host = Host::builder()
        .mode(AppMode::Development)
        .no_spa()
        .add_authentication_scheme(Arc::new(HeaderScheme {
            name: "jwt",
            header: "authorization",
        }))
        .use_auth_scheme_for("/mtls", ["jwt"])
        .try_build()
        .expect("route policy builds");
}
