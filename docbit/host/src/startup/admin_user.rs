//! Default admin user bootstrap.

use bcrypt::{hash, DEFAULT_COST};
use rust_ef::{db_context::DbContext, prelude::*};
use rust_webx::*;
use docbit_handlers::db::{save_changes, EfResultExt};

use docbit_domain::entities::User;
use docbit_domain::{new_id, seed_ids};

const ADMIN_EMAIL: &str = "admin@docbit.local";
const ADMIN_DEFAULT_PASSWORD: &str = "admin123";

/// Create the default admin account when missing.
pub async fn ensure_admin_user(ctx: &mut DbContext) -> Result<()> {
    let q = ADMIN_EMAIL.to_string();
    let existing = linq!(ctx.set::<User>(), |u: User| u.email == q)
        .first_or_default()
        .await
        .map_ef()?;

    if existing.is_some() {
        return Ok(());
    }

    let user_id = new_id();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let password_hash =
        hash(ADMIN_DEFAULT_PASSWORD, DEFAULT_COST).map_err(|e| Error::Internal(e.to_string()))?;

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

    tracing::info!(
        "[DbInit] Created default admin user: {} / {}",
        ADMIN_EMAIL,
        ADMIN_DEFAULT_PASSWORD
    );
    Ok(())
}
