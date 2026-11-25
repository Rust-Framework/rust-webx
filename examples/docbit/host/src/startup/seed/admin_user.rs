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

use docbit_handlers::common::{now_secs, save_changes};
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
pub async fn ensure_admin_user(ctx: &mut DbContext, hasher: &dyn IPasswordHasher) -> Result<()> {
    let q = ADMIN_EMAIL.to_string();
    let existing = linq!(ctx.set::<User>(), |u: User| u.email == q)
        .first_or_default()
        .await?;

    let mode = AppMode::from_env();
    let password = resolve_password(mode, configured_password());

    if let Some(existing) = existing {
        let admin_id = existing.id.clone();
        // 2.5 moved the sample from bcrypt to the framework Argon2 hasher.
        // Existing development databases keep working by re-hashing the
        // configured/default admin password once; without a password we leave
        // the row alone and warn instead of silently locking the account.
        if existing.password_hash.starts_with("$2") {
            if let Some(password) = password.as_deref() {
                let hashed = hasher.hash(password).await?;
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0);
                ctx.edit(existing, |u| {
                    u.password_hash = hashed;
                    u.updated_at = now;
                });
                ctx.detect_changes();
                save_changes(ctx).await?;
                tracing::warn!(
                    "[DbInit] Re-hashed legacy bcrypt admin to Argon2id; set {ADMIN_PASSWORD_ENV} \
                     to rotate the password."
                );
            } else {
                tracing::warn!(
                    "[DbInit] Admin still has a legacy bcrypt hash and no {ADMIN_PASSWORD_ENV} \
                     is set; set it once to migrate to Argon2id."
                );
            }
        }
        ensure_admin_role(ctx, &admin_id).await?;
        return Ok(());
    }

    let Some(password) = password else {
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
    let password_hash = hasher.hash(&password).await?;

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

    // Two flushes: the account first, then the `admin` role link (see
    // `ensure_admin_role` — `role_users` needs its parent rows to exist).
    ctx.add(user);
    save_changes(ctx).await?;

    ensure_admin_role(ctx, &user_id).await?;

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

/// Ensure the admin account carries the `admin` role.
///
/// `role_users` is a foreign-key-dependent table, so it is **not** part of
/// `has_data` (that batch runs in metadata order, not dependency order). The
/// link is granted here instead — principals (`users`, `roles`) already exist by
/// the time the startup seed runs, and the check keeps repeat boots idempotent.
async fn ensure_admin_role(ctx: &mut DbContext, user_id: &str) -> Result<()> {
    let uid = user_id.to_string();
    let rid = seed_ids::ROLE_ADMIN.to_string();
    let (uid_q, rid_q) = (uid.clone(), rid.clone());

    let linked = linq!(
        ctx.set::<docbit_domain::entities::RoleUser>(),
        |r: docbit_domain::entities::RoleUser| r.user_id == uid_q && r.role_id == rid_q
    )
    .any()
    .await?;
    if linked {
        return Ok(());
    }

    ctx.add(docbit_domain::entities::RoleUser {
        id: new_id(),
        user_id: uid,
        role_id: rid,
        created_at: now_secs(),
    });
    save_changes(ctx).await?;

    tracing::info!("[DbInit] Granted the `admin` role to the existing administrator.");
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
