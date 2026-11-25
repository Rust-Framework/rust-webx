//! Shared auth queries — user lookup and token minting.

use std::time::Duration;

use rust_ef::{db_context::DbContext, prelude::*};
use webx::*;

use docbit_contracts::auth::UserModel;
use docbit_domain::entities::User;

/// Mint a token through the framework port; the host stamps `iss`/`aud`
/// (when configured), `jti`, `iat` and `exp`.
pub(super) async fn create_token(issuer: &dyn ITokenIssuer, user: &UserModel) -> Result<String> {
    let issued = issuer
        .issue(
            TokenClaims::new(&user.id)
                .with_roles(user.roles.clone())
                .with_claim("name", user.name.clone())
                .with_claim("email", user.email.clone())
                .expires_in(Duration::from_secs(86_400)),
        )
        .await?;
    Ok(issued.token)
}

/// Load a user (roles included) by email; the soft-delete filter applies.
pub(super) async fn load_user_by_email(ctx: &mut DbContext, email: &str) -> Result<Option<User>> {
    let q = email.to_string();
    Ok(
        linq!(ctx.set::<User>(), |u: User| u.email == q; include u.roles)
            .first_or_default()
            .await?,
    )
}

/// Load a user (roles included) by id; the soft-delete filter applies.
pub(super) async fn load_user_by_id(ctx: &mut DbContext, id: &str) -> Result<Option<User>> {
    let q = id.to_string();
    Ok(
        linq!(ctx.set::<User>(), |u: User| u.id == q; include u.roles)
            .first_or_default()
            .await?,
    )
}
