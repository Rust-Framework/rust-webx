use bcrypt::{hash, verify, DEFAULT_COST};
use jsonwebtoken::{encode, EncodingKey, Header};
use lrwf::*;
use serde::Serialize;

use crate::contracts::auth::*;
use crate::domain::user::UserModel;

// =========================================================================
// JWT helpers  (encoding secret is set by use_auth() at startup)
// =========================================================================

/// Custom JWT payload for our application.
/// Fields match the framework's `RawClaims` so `use_auth()` middleware can
/// extract roles without a custom handler.
#[derive(Debug, Serialize, serde::Deserialize)]
struct AppJwtClaims {
    /// Subject (user id)
    sub: String,
    /// User display name (custom claim)
    name: String,
    /// User email (custom claim)
    email: String,
    /// User roles — required by framework JwtAuth
    #[serde(default)]
    roles: Vec<String>,
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
        roles: vec![user.role.clone()],
        iat: now,
        exp: now + 86_400, // 24 hours
    };

    encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(lrwf::jwt_secret().as_bytes()),
    )
    .map_err(|e| Error::Http(format!("Token creation failed: {}", e)))
}

// =========================================================================
// In-memory credential store (shared with UserRepository)
// =========================================================================

/// Find a user by email. Returns the user if found.
pub fn find_user_by_email(email: &str) -> Option<UserModel> {
    crate::handlers::user::repo().find_by_email(email)
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
        // The endpoint-level #[authorize] guarantees the user is authenticated.
        // take_current_user() is set by StubEndpoint::handle before dispatching.
        let (user_id, _) =
            lrwf::take_current_user().ok_or_else(|| Error::Http("Not authenticated".into()))?;

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
// Seed admin user
// =========================================================================

/// Ensure a default admin user exists in the repository.
/// Called once at startup.
pub fn ensure_admin_user() {
    let existing = find_user_by_email("admin@lrwf.dev");
    if existing.is_some() {
        return;
    }

    let hashed = bcrypt::hash("admin123", DEFAULT_COST).expect("Failed to hash admin password");
    crate::handlers::user::repo().create_with_password("Admin", "admin@lrwf.dev", &hashed, "admin");
    tracing::info!("[Auth] Default admin created: admin@lrwf.dev / admin123");
}
