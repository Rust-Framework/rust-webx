use rust_webapp_core::auth::{IAuthorizationPolicy, IClaims};
use std::collections::HashMap;

/// Minimal mock claims for testing authorization.
struct MockClaims {
    sub: String,
    roles: Vec<String>,
    permissions: Vec<String>,
}

impl MockClaims {
    fn new(sub: &str, roles: &[&str], permissions: &[&str]) -> Self {
        Self {
            sub: sub.to_string(),
            roles: roles.iter().map(|s| s.to_string()).collect(),
            permissions: permissions.iter().map(|s| s.to_string()).collect(),
        }
    }
}

impl IClaims for MockClaims {
    fn subject(&self) -> &str {
        &self.sub
    }

    fn roles(&self) -> &[String] {
        &self.roles
    }

    fn permissions(&self) -> &[String] {
        &self.permissions
    }

    fn claims(&self) -> &HashMap<String, String> {
        unreachable!("claims() not used in authz tests")
    }

    fn clone_box(&self) -> Box<dyn IClaims> {
        Box::new(MockClaims {
            sub: self.sub.clone(),
            roles: self.roles.clone(),
            permissions: self.permissions.clone(),
        })
    }
}

#[tokio::test]
async fn authz_user_with_allowed_role_is_authorized() {
    let policy = rust_webapp_host::authz::ResourceAuthorization::new().allow_role("/api/admin", "admin");

    let claims = MockClaims::new("user-1", &["admin"], &[]);
    let result = policy.authorize(&claims, "/api/admin", "GET").await;
    assert!(
        result.is_ok(),
        "Admin user should be authorized for /api/admin"
    );
}

#[tokio::test]
async fn authz_user_without_role_is_denied() {
    let policy = rust_webapp_host::authz::ResourceAuthorization::new().allow_role("/api/admin", "admin");

    let claims = MockClaims::new("user-2", &["user"], &[]);
    let result = policy.authorize(&claims, "/api/admin", "GET").await;
    assert!(
        result.is_err(),
        "Non-admin user should be denied for /api/admin"
    );
}

#[tokio::test]
async fn authz_user_with_permission_is_authorized() {
    let policy = rust_webapp_host::authz::ResourceAuthorization::new()
        .allow_permission("/api/settings", "settings:write");

    let claims = MockClaims::new("user-3", &[], &["settings:write"]);
    let result = policy.authorize(&claims, "/api/settings", "POST").await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn authz_user_without_permission_is_denied() {
    let policy = rust_webapp_host::authz::ResourceAuthorization::new()
        .allow_permission("/api/settings", "settings:write");

    let claims = MockClaims::new("user-4", &[], &["settings:read"]);
    let result = policy.authorize(&claims, "/api/settings", "POST").await;
    assert!(result.is_err());
}

#[tokio::test]
async fn authz_multiple_roles_first_allowed() {
    let policy = rust_webapp_host::authz::ResourceAuthorization::new()
        .allow_role("/api/dashboard", "admin")
        .allow_role("/api/dashboard", "moderator");

    let claims = MockClaims::new("mod", &["moderator"], &[]);
    let result = policy.authorize(&claims, "/api/dashboard", "GET").await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn authz_role_checked_before_permission() {
    let policy = rust_webapp_host::authz::ResourceAuthorization::new()
        .allow_role("/api/data", "admin")
        .allow_permission("/api/data", "data:read");

    // User has neither role nor permission
    let claims = MockClaims::new("guest", &["guest"], &["nothing"]);
    let result = policy.authorize(&claims, "/api/data", "GET").await;
    assert!(result.is_err());
}

#[tokio::test]
async fn authz_empty_policy_denies_all() {
    let policy = rust_webapp_host::authz::ResourceAuthorization::new();
    let claims = MockClaims::new("anyone", &["admin"], &["all"]);
    let result = policy.authorize(&claims, "/api/anything", "GET").await;
    assert!(result.is_err(), "Empty policy should deny all access");
}

#[tokio::test]
async fn authz_different_paths_independent() {
    let policy = rust_webapp_host::authz::ResourceAuthorization::new()
        .allow_role("/api/public", "user")
        .allow_role("/api/private", "admin");

    let user_claims = MockClaims::new("user", &["user"], &[]);
    let admin_claims = MockClaims::new("admin", &["admin", "user"], &[]);

    // user can access /api/public but not /api/private
    assert!(policy
        .authorize(&user_claims, "/api/public", "GET")
        .await
        .is_ok());
    assert!(policy
        .authorize(&user_claims, "/api/private", "GET")
        .await
        .is_err());
    // admin (with both admin and user roles) can access both
    assert!(policy
        .authorize(&admin_claims, "/api/public", "GET")
        .await
        .is_ok());
    assert!(policy
        .authorize(&admin_claims, "/api/private", "GET")
        .await
        .is_ok());
}
