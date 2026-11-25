//! Auth queries.

use rust_ef::{db_context::DbContext, prelude::*};
use webx::Result;

use dmbit_domain::entities::User;

/// User by email with roles eager-loaded — the login credential lookup.
pub async fn user_by_email(ctx: &mut DbContext, email: &str) -> Result<Option<User>> {
    let q = email.to_string();
    Ok(
        linq!(ctx.set::<User>(), |u: User| u.email == q; include u.roles)
            .first_or_default()
            .await?,
    )
}

/// User by id with roles eager-loaded — the current-principal lookup.
pub async fn user_by_id(ctx: &mut DbContext, id: &str) -> Result<Option<User>> {
    let q = id.to_string();
    Ok(
        linq!(ctx.set::<User>(), |u: User| u.id == q; include u.roles)
            .first_or_default()
            .await?,
    )
}
