//! 2.5 M1 — double-submit CSRF middleware and its host guard.

mod test_utils;

use std::ops::ControlFlow;
use std::sync::Arc;

use webx_core::middleware::IMiddleware;
use webx_core::mode::AppMode;
use webx_core::security::InMemorySessionRepository;
use webx_host::auth_cookie::{CookieOptions, SessionCookieScheme};
use webx_host::csrf::{CsrfConfig, CsrfMiddleware};
use webx_host::server::Host;

const SESSION: &str = "__Host-webx_session=sid";
const CSRF: &str = "__Host-webx_csrf=tok";

fn middleware() -> CsrfMiddleware {
    CsrfMiddleware::new(CsrfConfig::default()).expect("valid csrf config")
}

fn ctx(method: &str, cookie: Option<&str>, header: Option<&str>) -> test_utils::TestHttpContext {
    let mut ctx = test_utils::TestHttpContext::new(method, "/api/orders");
    if let Some(cookie) = cookie {
        ctx = ctx.with_header("cookie", cookie);
    }
    if let Some(header) = header {
        ctx = ctx.with_header("x-csrf-token", header);
    }
    ctx
}

#[tokio::test]
async fn matching_cookie_and_header_pass_on_unsafe_methods() {
    let mut ctx = ctx("POST", Some(&format!("{SESSION}; {CSRF}")), Some("tok"));
    let flow = middleware().invoke(&mut ctx).await.expect("csrf ok");
    assert!(matches!(flow, ControlFlow::Continue(())));
}

#[tokio::test]
async fn missing_header_is_forbidden() {
    let mut ctx = ctx("POST", Some(&format!("{SESSION}; {CSRF}")), None);
    let error = middleware()
        .invoke(&mut ctx)
        .await
        .expect_err("must reject");
    assert!(error.code() == "forbidden", "got {error:?}");
}

#[tokio::test]
async fn missing_cookie_is_forbidden() {
    let mut ctx = ctx("POST", Some(SESSION), Some("tok"));
    let error = middleware()
        .invoke(&mut ctx)
        .await
        .expect_err("must reject");
    assert!(error.code() == "forbidden", "got {error:?}");
}

#[tokio::test]
async fn mismatched_token_is_forbidden() {
    let mut ctx = ctx("POST", Some(&format!("{SESSION}; {CSRF}")), Some("other"));
    let error = middleware()
        .invoke(&mut ctx)
        .await
        .expect_err("must reject");
    assert!(error.code() == "forbidden", "got {error:?}");
}

#[tokio::test]
async fn safe_methods_are_exempt() {
    let mut ctx = ctx("GET", Some(SESSION), None);
    let flow = middleware().invoke(&mut ctx).await.expect("GET is exempt");
    assert!(matches!(flow, ControlFlow::Continue(())));
}

#[tokio::test]
async fn bearer_requests_are_unaffected() {
    // No session cookie → the middleware must not demand a CSRF token.
    let mut ctx = test_utils::TestHttpContext::new("POST", "/api/orders")
        .with_header("authorization", "Bearer a.b.c");
    let flow = middleware()
        .invoke(&mut ctx)
        .await
        .expect("Bearer traffic is not CSRF-protected");
    assert!(matches!(flow, ControlFlow::Continue(())));
}

#[tokio::test]
async fn cross_site_fetch_metadata_is_rejected() {
    let mut ctx = ctx("POST", Some(&format!("{SESSION}; {CSRF}")), Some("tok"))
        .with_header("sec-fetch-site", "cross-site");
    let error = middleware()
        .invoke(&mut ctx)
        .await
        .expect_err("must reject");
    assert!(error.code() == "forbidden", "got {error:?}");
}

#[test]
fn host_refuses_cookie_auth_without_csrf() {
    let repo = Arc::new(InMemorySessionRepository::new());
    let scheme =
        Arc::new(SessionCookieScheme::new(repo, CookieOptions::default()).expect("valid options"));

    let error = Host::builder()
        .mode(AppMode::Development)
        .no_spa()
        .add_authentication_scheme(scheme)
        .try_build()
        .err()
        .expect("cookie auth without CSRF must abort startup");
    assert!(error.code() == "dependency_injection", "got {error:?}");
    assert!(
        error.to_string().contains("CSRF"),
        "the error must name the fix: {error}"
    );
}

#[test]
fn host_accepts_cookie_auth_with_csrf() {
    let repo = Arc::new(InMemorySessionRepository::new());
    let scheme =
        Arc::new(SessionCookieScheme::new(repo, CookieOptions::default()).expect("valid options"));

    Host::builder()
        .mode(AppMode::Development)
        .no_spa()
        .add_authentication_scheme(scheme)
        .use_csrf(CsrfConfig::double_submit())
        .try_build()
        .expect("cookie auth with CSRF must build");
}
