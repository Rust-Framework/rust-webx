//! 2.5 M1 — cookie session scheme.
//!
//! The scheme only authenticates; login/logout endpoints own `Set-Cookie`.
//! These tests exercise the repository round-trip, expiry/idle handling,
//! rotation helpers and the "Bearer wins over a stale cookie" rule.

mod test_utils;

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use webx_core::auth::IAuthenticationScheme;
use webx_core::security::{ISessionRepository, InMemorySessionRepository, SessionRecord};
use webx_host::auth_cookie::{CookieOptions, SameSite, SessionCookieScheme};

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after epoch")
        .as_secs()
}

fn scheme(repo: Arc<InMemorySessionRepository>) -> SessionCookieScheme {
    SessionCookieScheme::new(
        repo as Arc<dyn ISessionRepository>,
        CookieOptions::default(),
    )
    .expect("valid cookie options")
}

fn cookie(name: &str, value: &str) -> String {
    format!("{name}={value}")
}

#[tokio::test]
async fn issued_session_authenticates_with_tenant_and_roles() {
    let repo = Arc::new(InMemorySessionRepository::new());
    let scheme = scheme(Arc::clone(&repo));

    let session = scheme
        .issue_session(
            SessionRecord::new("", "user-1", 0, 0)
                .with_name("Ada")
                .with_tenant("tenant-1")
                .with_roles(["admin"])
                .with_permissions(["orders:read"]),
        )
        .await
        .expect("issue session");

    let mut ctx = test_utils::TestHttpContext::new("GET", "/api/me").with_header(
        "cookie",
        &cookie(&scheme.options().name, &session.session_id),
    );
    let claims = scheme
        .authenticate(&mut ctx)
        .await
        .expect("authenticate")
        .expect("session should authenticate");

    assert_eq!(claims.subject(), "user-1");
    assert_eq!(claims.get_username(), Some("Ada"));
    assert_eq!(claims.get_tenantid(), Some("tenant-1"));
    assert!(claims.has_role("admin"));
    assert!(claims.permissions().contains(&"orders:read".to_string()));
}

#[tokio::test]
async fn unknown_session_is_anonymous() {
    let repo = Arc::new(InMemorySessionRepository::new());
    let scheme = scheme(repo);
    let mut ctx = test_utils::TestHttpContext::new("GET", "/")
        .with_header("cookie", &cookie(&scheme.options().name, "not-a-session"));
    assert!(scheme
        .authenticate(&mut ctx)
        .await
        .expect("authenticate")
        .is_none());
}

#[tokio::test]
async fn expired_session_is_deleted_and_anonymous() {
    let repo = Arc::new(InMemorySessionRepository::new());
    let scheme = scheme(Arc::clone(&repo));
    let now = now_secs();
    repo.create(SessionRecord::new("expired", "user-1", now - 100, now - 1))
        .await
        .expect("seed expired session");

    let mut ctx = test_utils::TestHttpContext::new("GET", "/")
        .with_header("cookie", &cookie(&scheme.options().name, "expired"));
    assert!(scheme
        .authenticate(&mut ctx)
        .await
        .expect("authenticate")
        .is_none());
    assert!(
        repo.get("expired").await.expect("repo").is_none(),
        "an expired session must not be resurrected"
    );
}

#[tokio::test]
async fn idle_expired_session_is_anonymous() {
    let repo = Arc::new(InMemorySessionRepository::new());
    let options = CookieOptions {
        idle_timeout: std::time::Duration::from_secs(30),
        ..CookieOptions::default()
    };
    let scheme = SessionCookieScheme::new(repo.clone() as Arc<dyn ISessionRepository>, options)
        .expect("options");

    let now = now_secs();
    repo.create(SessionRecord::new("idle", "user-1", now - 100, now + 1000))
        .await
        .expect("seed idle session");

    let mut ctx = test_utils::TestHttpContext::new("GET", "/")
        .with_header("cookie", &cookie(&scheme.options().name, "idle"));
    assert!(scheme
        .authenticate(&mut ctx)
        .await
        .expect("authenticate")
        .is_none());
}

#[tokio::test]
async fn revoked_session_is_anonymous() {
    let repo = Arc::new(InMemorySessionRepository::new());
    let scheme = scheme(Arc::clone(&repo));
    let session = scheme.issue_for("user-1").await.expect("issue");
    scheme
        .revoke_session(&session.session_id)
        .await
        .expect("revoke");

    let mut ctx = test_utils::TestHttpContext::new("GET", "/").with_header(
        "cookie",
        &cookie(&scheme.options().name, &session.session_id),
    );
    assert!(scheme
        .authenticate(&mut ctx)
        .await
        .expect("authenticate")
        .is_none());
}

#[tokio::test]
async fn authorization_header_wins_over_a_stale_cookie() {
    let repo = Arc::new(InMemorySessionRepository::new());
    let scheme = scheme(Arc::clone(&repo));
    let session = scheme.issue_for("user-1").await.expect("issue");

    let ctx = test_utils::TestHttpContext::new("GET", "/")
        .with_header("authorization", "Bearer a.b.c")
        .with_header(
            "cookie",
            &cookie(&scheme.options().name, &session.session_id),
        );
    assert!(
        !scheme.matches(&ctx),
        "an explicit Bearer request must be handled by the JWT scheme, not the cookie"
    );
}

#[test]
fn set_cookie_is_hardened_and_clear_cookie_expires() {
    let options = CookieOptions::default();
    let set = options.set_cookie("sid");
    assert!(set.contains("HttpOnly"));
    assert!(set.contains("Secure"));
    assert!(set.contains("SameSite=Lax"));
    assert!(set.contains("Max-Age="));

    let clear = options.clear_cookie();
    assert!(clear.contains("Max-Age=0"));
}

#[test]
fn same_site_none_without_secure_is_rejected() {
    let options = CookieOptions {
        same_site: SameSite::None,
        secure: false,
        ..CookieOptions::default()
    };
    assert!(SessionCookieScheme::new(Arc::new(InMemorySessionRepository::new()), options).is_err());
}
