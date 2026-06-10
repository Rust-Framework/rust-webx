use bcrypt::{hash, verify, DEFAULT_COST};
use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use lrwf::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use crate::contracts::auth::*;
use crate::domain::user::UserModel;

// =========================================================================
// JWT helpers
// =========================================================================

static JWT_SECRET: OnceLock<String> = OnceLock::new();

/// Initialise the JWT secret from app configuration.
/// Must be called once at startup.
pub fn init_jwt_secret(secret: &str) {
    let _ = JWT_SECRET.set(secret.to_string());
}

fn jwt_secret() -> &'static str {
    JWT_SECRET.get().expect("JWT secret not initialised")
}

/// Custom JWT payload for our application.
#[derive(Debug, Serialize, Deserialize)]
struct AppJwtClaims {
    /// Subject (user id)
    sub: String,
    /// User name
    name: String,
    /// User email
    email: String,
    /// User role
    role: String,
    /// Issued-at timestamp
    iat: u64,
    /// Expiration timestamp (24 h)
    exp: u64,
}

fn create_token(user: &UserModel) -> Result<String> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let claims = AppJwtClaims {
        sub: user.id.clone(),
        name: user.name.clone(),
        email: user.email.clone(),
        role: user.role.clone(),
        iat: now,
        exp: now + 86_400, // 24 hours
    };

    encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(jwt_secret().as_bytes()),
    )
    .map_err(|e| Error::Http(format!("Token creation failed: {}", e)))
}

// =========================================================================
// In-memory credential store (shared with UserRepository)
// =========================================================================

/// Find a user by email. Returns the user if found.
pub fn find_user_by_email(email: &str) -> Option<UserModel> {
    // Access the same static REPO from user.rs
    // We create a local finder that iterates the repo
    let users = crate::handlers::user::repo().list();
    users.into_iter().find(|u| u.email == email)
}

/// Find a user by id.
pub fn find_user_by_id(id: &str) -> Option<UserModel> {
    crate::handlers::user::repo().get(id)
}

// =========================================================================
// IRequestHandler implementations
// =========================================================================

#[derive(Default)]
pub struct RegisterHandler;

#[derive(Default)]
pub struct LoginHandler;

#[derive(Default)]
pub struct AuthMeHandler;

#[handler]
#[async_trait]
impl IRequestHandler<RegisterRequest, AuthResponse> for RegisterHandler {
    async fn handle(&self, req: RegisterRequest) -> Result<AuthResponse> {
        // Check if email already exists
        if find_user_by_email(&req.email).is_some() {
            return Err(Error::Http("Email already registered".into()));
        }

        // Hash password
        let hashed = hash(&req.password, DEFAULT_COST)
            .map_err(|e| Error::Http(format!("Password hash failed: {}", e)))?;

        // Create user with role "user" by default
        let user = crate::handlers::user::repo()
            .create_with_password(&req.name, &req.email, &hashed, "user");

        let token = create_token(&user)?;

        tracing::info!("[Auth] User registered: {} ({})", user.name, user.id);

        Ok(AuthResponse {
            token,
            user: UserView {
                id: user.id,
                name: user.name,
                email: user.email,
                role: user.role,
                created_at: user.created_at,
            },
        })
    }
}

#[handler]
#[async_trait]
impl IRequestHandler<LoginRequest, AuthResponse> for LoginHandler {
    async fn handle(&self, req: LoginRequest) -> Result<AuthResponse> {
        let user = find_user_by_email(&req.email)
            .ok_or_else(|| Error::Http("Invalid email or password".into()))?;

        // If user has no password (seed user) or password doesn't match
        if user.password_hash.is_empty()
            || !verify(&req.password, &user.password_hash)
                .map_err(|_| Error::Http("Authentication error".into()))?
        {
            return Err(Error::Http("Invalid email or password".into()));
        }

        let token = create_token(&user)?;

        tracing::info!("[Auth] User logged in: {} ({})", user.name, user.id);

        Ok(AuthResponse {
            token,
            user: UserView {
                id: user.id,
                name: user.name,
                email: user.email,
                role: user.role,
                created_at: user.created_at,
            },
        })
    }
}

#[handler]
#[async_trait]
impl IRequestHandler<AuthMeRequest, UserView> for AuthMeHandler {
    async fn handle(&self, _: AuthMeRequest) -> Result<UserView> {
        // The JWT middleware has already stored claims in the context.
        // For handler access, we extract from the HTTP context.
        // Since IRequestHandler doesn't have direct access to IHttpContext,
        // we use the mediator's context injection. For now, get user from
        // the token's sub claim stored in a thread-local / static.
        //
        // Actually, the #[handler] macro + mediator pipeline injects
        // IHttpContext as a method parameter when the request struct
        // implements FromHttpContext. But AuthMeRequest is a simple struct
        // with no fields.
        //
        // The simplest approach: use a thread-local set by the middleware.

        let user_id = CURRENT_USER_ID.with(|id| id.borrow().clone());
        if user_id.is_empty() {
            return Err(Error::Http("Not authenticated".into()));
        }

        let user = find_user_by_id(&user_id).ok_or_else(|| Error::Http("User not found".into()))?;

        Ok(UserView {
            id: user.id,
            name: user.name,
            email: user.email,
            role: user.role,
            created_at: user.created_at,
        })
    }
}

// =========================================================================
// Thread-local for passing current user ID from middleware to handler
// =========================================================================

thread_local! {
    static CURRENT_USER_ID: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
}

/// Set the current user ID for the duration of request handling.
/// Called by the auth middleware before invoking the handler.
pub fn set_current_user_id(id: &str) {
    CURRENT_USER_ID.with(|cell| *cell.borrow_mut() = id.to_string());
}

// =========================================================================
// JWT Authentication Middleware
// =========================================================================

/// Middleware that validates JWT Bearer tokens and stores claims.
pub struct JwtAuthzMiddleware;

#[async_trait]
impl IMiddleware for JwtAuthzMiddleware {
    async fn invoke(&self, ctx: &mut dyn IHttpContext) -> Result<()> {
        // Skip OPTIONS preflight
        if ctx.request().method().to_uppercase() == "OPTIONS" {
            return Ok(());
        }

        // Try to extract and validate Bearer token
        let token = match ctx.request().header("authorization") {
            Some(h) => h.strip_prefix("Bearer ").map(|t| t.trim().to_string()),
            None => None,
        };

        if let Some(token) = token {
            if !token.is_empty() {
                // Decode and validate
                match decode::<AppJwtClaims>(
                    &token,
                    &DecodingKey::from_secret(jwt_secret().as_bytes()),
                    &Validation::default(),
                ) {
                    Ok(data) => {
                        let claims = data.claims;
                        // Store in the HTTP context for downstream middleware
                        let jwt_claims = JwtClaims::new(&claims.sub, &claims.role);
                        ctx.set_claims(Box::new(jwt_claims));
                        // Store in thread-local for the handler
                        set_current_user_id(&claims.sub);
                    }
                    Err(e) => {
                        return Err(Error::Http(format!("Invalid token: {}", e)));
                    }
                }
            }
        }

        Ok(())
    }
}

/// Simple IClaims implementation backed by JWT data.
pub struct JwtClaims {
    sub: String,
    roles: Vec<String>,
    claims_map: HashMap<String, String>,
}

impl JwtClaims {
    fn new(sub: &str, role: &str) -> Self {
        let mut claims_map = HashMap::new();
        claims_map.insert("sub".into(), sub.to_string());
        claims_map.insert("role".into(), role.to_string());
        Self {
            sub: sub.to_string(),
            roles: vec![role.to_string()],
            claims_map,
        }
    }
}

impl IClaims for JwtClaims {
    fn subject(&self) -> &str {
        &self.sub
    }

    fn roles(&self) -> &[String] {
        &self.roles
    }

    fn permissions(&self) -> &[String] {
        &[]
    }

    fn claims(&self) -> &HashMap<String, String> {
        &self.claims_map
    }
}
