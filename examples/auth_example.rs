//! auth_example 鈥?JWT Bearer authentication + resource-based authorization.
//!
//! Demonstrates:
//!   - JWT token generation (via `jsonwebtoken` crate)
//!   - Protecting endpoints with `jwt_middleware` + `resource_auth_middleware`
//!   - Role-based access control on specific route patterns
//!
//! Run with: `cargo run --example auth_example`
//!
//! # Test flow
//!
//! ```bash
//! # Public 鈥?no auth required
//! curl http://localhost:5000/api/public
//!
//! # Protected 鈥?needs a valid token
//! curl -H "Authorization: Bearer <admin-token>" http://localhost:5000/api/users
//!
//! # Forbidden 鈥?user token on admin-only route
//! curl -H "Authorization: Bearer <user-token>" http://localhost:5000/api/admin
//! ```
//!
//! The server prints test tokens to stdout at startup for convenience.

use jsonwebtoken::{encode, EncodingKey, Header};
use lrwf::*;
use serde::Serialize;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

// 鈹€鈹€ Request / Response contracts 鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€

struct PublicRequest;

#[get("/api/public")]
impl IRequest<String> for PublicRequest {}

struct UsersRequest;

#[get("/api/users")]
impl IRequest<String> for UsersRequest {}

struct AdminRequest;

#[get("/api/admin")]
impl IRequest<String> for AdminRequest {}

// 鈹€鈹€ Handlers 鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€

#[derive(Default)]
struct PublicHandler;
#[derive(Default)]
struct UsersHandler;
#[derive(Default)]
struct AdminHandler;

#[handler]
#[async_trait]
impl IRequestHandler<PublicRequest, String> for PublicHandler {
    async fn handle(&self, _req: PublicRequest) -> Result<String> {
        Ok("This is a public endpoint 鈥?no authentication required.".into())
    }
}

#[handler]
#[async_trait]
impl IRequestHandler<UsersRequest, String> for UsersHandler {
    async fn handle(&self, _req: UsersRequest) -> Result<String> {
        Ok("Welcome, authenticated user! You have 'user' role.".into())
    }
}

#[handler]
#[async_trait]
impl IRequestHandler<AdminRequest, String> for AdminHandler {
    async fn handle(&self, _req: AdminRequest) -> Result<String> {
        Ok("Welcome, administrator! You have 'admin' role.".into())
    }
}

// 鈹€鈹€ JWT helper 鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€

#[derive(Serialize)]
struct TokenClaims {
    sub: String,
    roles: Vec<String>,
    permissions: Vec<String>,
    exp: usize,
}

fn generate_token(
    sub: &str,
    roles: Vec<String>,
    permissions: Vec<String>,
    secret: &[u8],
) -> String {
    let exp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as usize
        + 3600; // 1 hour

    let claims = TokenClaims {
        sub: sub.to_string(),
        roles,
        permissions,
        exp,
    };

    encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(secret),
    )
    .expect("Failed to encode JWT")
}

// 鈹€鈹€ Main 鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€

#[tokio::main]
async fn main() {
    let secret = b"demo-secret-key-change-in-production";

    // Generate test tokens for manual curl testing
    let admin_token = generate_token("admin", vec!["admin".into()], vec![], secret);
    let user_token = generate_token("alice", vec!["user".into()], vec![], secret);

    println!("鈺愨晲鈺愨晲鈺愨晲鈺愨晲鈺愨晲鈺愨晲鈺愨晲鈺愨晲鈺愨晲鈺愨晲鈺愨晲鈺愨晲鈺愨晲鈺愨晲鈺愨晲鈺愨晲鈺愨晲鈺愨晲鈺愨晲");
    println!("  Auth example 鈥?test tokens");
    println!("鈺愨晲鈺愨晲鈺愨晲鈺愨晲鈺愨晲鈺愨晲鈺愨晲鈺愨晲鈺愨晲鈺愨晲鈺愨晲鈺愨晲鈺愨晲鈺愨晲鈺愨晲鈺愨晲鈺愨晲鈺愨晲鈺愨晲");
    println!("  Admin token:  Bearer {}", admin_token);
    println!("  User  token:  Bearer {}", user_token);
    println!("鈺愨晲鈺愨晲鈺愨晲鈺愨晲鈺愨晲鈺愨晲鈺愨晲鈺愨晲鈺愨晲鈺愨晲鈺愨晲鈺愨晲鈺愨晲鈺愨晲鈺愨晲鈺愨晲鈺愨晲鈺愨晲鈺愨晲");

    // Build JWT auth handler
    use jsonwebtoken::{DecodingKey, Validation};
    let auth_handler: Arc<dyn IAuthenticationHandler> = Arc::new(JwtAuth::new(
        DecodingKey::from_secret(secret),
        Validation::default(),
    ));

    // Build authorization policy
    let authz_policy: Arc<dyn IAuthorizationPolicy> = Arc::new(
        ResourceAuthorization::new()
            .allow_role("/api/users", "user")
            .allow_role("/api/users", "admin")
            .allow_role("/api/admin", "admin"),
    );

    let ah = Arc::clone(&auth_handler);
    let ap = Arc::clone(&authz_policy);

    Host::builder()
        .mode(AppMode::Development)
        .register(move |svc| {
            svc.singleton::<dyn IMiddleware>(move |_| Arc::new(jwt_middleware(Arc::clone(&ah))))
                .singleton::<dyn IMiddleware>(move |_| {
                    Arc::new(resource_auth_middleware(Arc::clone(&ap)))
                })
        })
        .configure(|app| {
            app.useOptions(|o| {
                o.app.name = "Auth Example API".into();
            });
        })
        .build()
        .run()
        .await
        .expect("Server failed to start");
}
