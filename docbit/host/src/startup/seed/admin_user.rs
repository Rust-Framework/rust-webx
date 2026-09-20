//! Default admin account bootstrap.
//!
//! # Password policy
//!
//! | Mode | Source | Behaviour without it |
//! |------|--------|----------------------|
//! | Development | `DOCBIT_ADMIN_PASSWORD`, else the well-known `admin123` | created, logged as a warning |
//! | Production | `DOCBIT_ADMIN_PASSWORD` only | **no account is created**, a warning is logged |
//!
//! A built-in password is a development convenience, never a deployment
//! default: shipping `admin123` to a public host hands over the admin panel.
//! Operator-supplied passwords are never written to the log.

use bcrypt::{hash, DEFAULT_COST};
use docbit_handlers::db::{save_changes, EfResultExt};
use rust_ef::{db_context::DbContext, prelude::*};
use webx::*;

use docbit_domain::entities::User;
use docbit_domain::{new_id, seed_ids};

const ADMIN_EMAIL: &str = "admin@docbit.local";
/// Development-only fallback so `cargo run` works with zero setup.
const DEV_DEFAULT_PASSWORD: &str = "admin123";
/// Set this to provision the admin account in any environment.
pub const ADMIN_PASSWORD_ENV: &str = "DOCBIT_ADMIN_PASSWORD";

/// Password to provision the admin with, or `None` when the environment has not
/// supplied one and the built-in default is not allowed (Production).
///
/// Pure so the policy can be tested without touching process-global env state.
fn resolve_password(mode: AppMode, configured: Option<String>) -> Option<String> {
    match configured {
        Some(value) if !value.is_empty() => Some(value),
        _ if mode == AppMode::Production => None,
        _ => Some(DEV_DEFAULT_PASSWORD.to_string()),
    }
}

/// Read `DOCBIT_ADMIN_PASSWORD` from the environment.
fn configured_password() -> Option<String> {
    std::env::var(ADMIN_PASSWORD_ENV).ok()
}

/// Create the default admin account when it is missing and a password is available.
pub async fn ensure_admin_user(ctx: &mut DbContext) -> Result<()> {
    let q = ADMIN_EMAIL.to_string();
    let existing = linq!(ctx.set::<User>(), |u: User| u.email == q)
        .first_or_default()
        .await
        .map_ef()?;

    if existing.is_some() {
        return Ok(());
    }

    let mode = AppMode::from_env();
    let Some(password) = resolve_password(mode, configured_password()) else {
        tracing::warn!(
            "[DbInit] No admin account created. Set {ADMIN_PASSWORD_ENV} to provision \
             `{ADMIN_EMAIL}`; Production never falls back to a built-in password."
        );
        return Ok(());
    };

    let user_id = new_id();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let password_hash =
        hash(&password, DEFAULT_COST).map_err(|e| Error::Internal(e.to_string()))?;

    let user = User {
        id: user_id.clone(),
        name: "Administrator".into(),
        email: ADMIN_EMAIL.into(),
        password_hash,
        created_id: None,
        created_at: now,
        updated_id: None,
        updated_at: now,
        is_deleted: false,
        roles: HasMany::new(),
    };
    let role_user = docbit_domain::entities::RoleUser {
        id: new_id(),
        user_id,
        role_id: seed_ids::ROLE_ADMIN.into(),
        created_at: now,
    };

    ctx.add(user);
    ctx.add(role_user);

    save_changes(ctx).await?;

    if mode == AppMode::Production {
        tracing::info!("[DbInit] Created admin `{ADMIN_EMAIL}` from {ADMIN_PASSWORD_ENV}.");
    } else {
        tracing::warn!(
            "[DbInit] Created DEVELOPMENT admin `{ADMIN_EMAIL}` / `{DEV_DEFAULT_PASSWORD}`. \
             Set {ADMIN_PASSWORD_ENV} before deploying."
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{resolve_password, DEV_DEFAULT_PASSWORD};
    use webx::AppMode;

    #[test]
    fn development_falls_back_to_the_built_in_password() {
        assert_eq!(
            resolve_password(AppMode::Development, None).as_deref(),
            Some(DEV_DEFAULT_PASSWORD)
        );
    }

    #[test]
    fn production_refuses_to_invent_a_password() {
        assert_eq!(resolve_password(AppMode::Production, None), None);
    }

    #[test]
    fn an_explicit_password_wins_in_every_mode() {
        let configured = Some("s3cret-from-operator".to_string());
        assert_eq!(
            resolve_password(AppMode::Production, configured.clone()).as_deref(),
            Some("s3cret-from-operator")
        );
        assert_eq!(
            resolve_password(AppMode::Development, configured).as_deref(),
            Some("s3cret-from-operator")
        );
    }

    #[test]
    fn an_empty_password_is_treated_as_unset() {
        let empty = Some(String::new());
        assert_eq!(resolve_password(AppMode::Production, empty.clone()), None);
        assert_eq!(
            resolve_password(AppMode::Development, empty).as_deref(),
            Some(DEV_DEFAULT_PASSWORD)
        );
    }
}
