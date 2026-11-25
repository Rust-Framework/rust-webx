//! Device queries.

use rust_ef::{db_context::DbContext, prelude::*};
use webx::*;

use dmbit_domain::entities::Device;

/// Load one device or `404`.
pub async fn require_device(ctx: &mut DbContext, id: &str) -> Result<Device> {
    let q = id.to_string();
    linq!(ctx.set::<Device>(), |d: Device| d.id == q)
        .first_or_default()
        .await?
        .ok_or_else(|| Error::not_found("设备不存在"))
}

/// Reject a duplicate asset code (`409`) unless it belongs to `exclude_id`.
pub async fn assert_asset_code_available(
    ctx: &mut DbContext,
    asset_code: &str,
    exclude_id: Option<&str>,
) -> Result<()> {
    let q = asset_code.to_string();
    let existing = linq!(ctx.set::<Device>(), |d: Device| d.asset_code == q)
        .first_or_default()
        .await?;
    if let Some(d) = existing {
        if exclude_id.map(|id| id != d.id.as_str()).unwrap_or(true) {
            return Err(Error::conflict(format!("资产编码「{asset_code}」已存在")));
        }
    }
    Ok(())
}

/// Whether a device with this asset code already exists (batch generation skips
/// the ones that do instead of failing the whole batch).
pub async fn asset_code_taken(ctx: &mut DbContext, asset_code: &str) -> Result<bool> {
    let q = asset_code.to_string();
    Ok(linq!(ctx.set::<Device>(), |d: Device| d.asset_code == q)
        .first_or_default()
        .await?
        .is_some())
}
