use bcrypt::{hash, verify, DEFAULT_COST};
use jsonwebtoken::{encode, EncodingKey, Header};
use lref::provider::DbValue;
use lref::query::QueryBuilder;
use lrwf::*;
use serde::Serialize;

use crate::contracts::auth::*;
use crate::domain::user::{UserEntity, UserModel};

// =========================================================================
// JWT helpers
// =========================================================================

#[derive(Debug, Serialize, serde::Deserialize)]
struct AppJwtClaims {
    sub: String,
    name: String,
    email: String,
    #[serde(default)]
    roles: Vec<String>,
    iat: u64,
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
        exp: now + 86_400,
    };

    encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(lrwf::jwt_secret().as_bytes()),
    )
    .map_err(|e| Error::Http(format!("Token creation failed: {}", e)))
}

fn qb() -> QueryBuilder<UserEntity> {
    QueryBuilder::<UserEntity>::with_provider(
        "users",
        crate::handlers::startup::provider_dyn(),
    )
}

fn find_user_by_email(email: &str) -> Option<UserEntity> {
    let rt = tokio::runtime::Handle::current();
    let qb = qb()
        .filter_column("email", "=", DbValue::String(email.to_string()));
    rt.block_on(qb.first_or_default())
        .ok()
        .flatten()
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
        if find_user_by_email(&req.email).is_some() {
            return Err(Error::Http("Email already registered".into()));
        }

        let hashed = hash(&req.password, DEFAULT_COST)
            .map_err(|e| Error::Http(format!("Password hash failed: {}", e)))?;

        let id = uuid();
        let now = now_secs();
        let sql = format!(
            "INSERT INTO users (id, name, email, password_hash, role, created_at) VALUES ('{}', '{}', '{}', '{}', 'user', '{}')",
            id,
            req.name.replace('\'', "''"),
            req.email.replace('\'', "''"),
            hashed.replace('\'', "''"),
            now
        );
        crate::handlers::startup::exec(&sql).await
            .map_err(|e| Error::Internal(format!("Failed to create user: {}", e)))?;

        let model = UserModel {
            id: id.clone(),
            name: req.name.clone(),
            email: req.email.clone(),
            password_hash: hashed,
            role: "user".to_string(),
            created_at: now.clone(),
        };
        let token = create_token(&model)?;

        tracing::info!("[Auth] User registered: {} ({})", model.name, model.id);

        Ok(AuthResponse {
            token,
            user: UserView {
                id: model.id,
                name: model.name,
                email: model.email,
                role: model.role,
                created_at: model.created_at,
            },
        })
    }
}

#[handler]
#[async_trait]
impl IRequestHandler<LoginRequest, AuthResponse> for LoginHandler {
    async fn handle(&self, req: LoginRequest) -> Result<AuthResponse> {
        let user = qb()
            .filter_column("email", "=", DbValue::String(req.email.clone()))
            .first_or_default()
            .await
            .map_err(|e| Error::Internal(e.to_string()))?
            .ok_or_else(|| Error::Http("Invalid email or password".into()))?;

        if user.password_hash.is_empty()
            || !verify(&req.password, &user.password_hash)
                .map_err(|_| Error::Http("Authentication error".into()))?
        {
            return Err(Error::Http("Invalid email or password".into()));
        }

        let model = UserModel::from(user);
        let token = create_token(&model)?;

        tracing::info!("[Auth] User logged in: {} ({})", model.name, model.id);

        Ok(AuthResponse {
            token,
            user: UserView {
                id: model.id,
                name: model.name,
                email: model.email,
                role: model.role,
                created_at: model.created_at,
            },
        })
    }
}

#[handler]
#[async_trait]
impl IRequestHandler<AuthMeRequest, UserView> for AuthMeHandler {
    async fn handle(&self, _: AuthMeRequest) -> Result<UserView> {
        unreachable!("handle_with_claims is always called by the dispatcher")
    }

    async fn handle_with_claims(
        &self,
        _: AuthMeRequest,
        claims: Option<&dyn IClaims>,
    ) -> Result<UserView> {
        let user_id = claims
            .map(|c| c.subject().to_string())
            .ok_or_else(|| Error::Http("Not authenticated".into()))?;

        let user = qb()
            .filter_column("id", "=", DbValue::String(user_id))
            .first_or_default()
            .await
            .map_err(|e| Error::Internal(e.to_string()))?
            .ok_or_else(|| Error::Http("User not found".into()))?;

        Ok(UserView {
            id: user.id,
            name: user.name,
            email: user.email,
            role: user.role,
            created_at: user.created_at,
        })
    }
}

fn uuid() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| format!("{:x}", d.as_nanos()))
        .unwrap_or_else(|_| "0".to_string())
}

fn now_secs() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs().to_string())
        .unwrap_or_else(|_| "0".to_string())
}
