//! `POST /api/specs/{id}/devices/generate` — batch-generate devices for a spec.

use rust_ef::db_context::DbContext;
use webx::*;

use dmbit_contracts::device::{GenerateDevicesRequest, GenerateDevicesResult};
use dmbit_domain::entities::Device;
use dmbit_domain::mappers::device_mapper::DeviceSeed;
use dmbit_domain::new_id;
use dmbit_domain::rules::DEFAULT_STATUS;

use super::queries::asset_code_taken;
use crate::common::{now_secs, operator_id, parse_id, save_changes};
use crate::spec::queries::spec_with_product;

#[derive(Inject)]
pub struct GenerateDevicesHandler {
    #[inject(owned)]
    ctx: DbContext,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<GenerateDevicesRequest, GenerateDevicesResult> for GenerateDevicesHandler {
    async fn handle(&mut self, req: GenerateDevicesRequest) -> ActionResult<GenerateDevicesResult> {
        let spec_id = parse_id(&req.id)?;
        let (spec, _product_name) = spec_with_product(&mut self.ctx, &spec_id).await?;

        let count = req.count.unwrap_or(spec.planned_quantity);
        if count <= 0 {
            return Err(Error::validation("生成数量必须大于 0"));
        }

        let prefix = if req.asset_prefix.is_empty() {
            format!("{}-", spec.code)
        } else {
            req.asset_prefix.clone()
        };

        let now = now_secs();
        let actor = operator_id();
        let mut created = 0i32;

        for i in 0..count {
            let index = req.start_index + i;
            let asset_code = format!("{prefix}{index:04}");
            // Skip codes that already exist instead of failing the whole batch.
            if asset_code_taken(&mut self.ctx, &asset_code).await? {
                continue;
            }
            self.ctx.add(Device::from(DeviceSeed {
                id: new_id(),
                spec_id: spec_id.clone(),
                status: DEFAULT_STATUS.into(),
                location: String::new(),
                asset_code,
                serial_no: String::new(),
                sort_order: index,
                actor: actor.clone(),
                now,
            }));
            created += 1;
        }

        save_changes(&mut self.ctx).await?;

        Ok(GenerateDevicesResult {
            created,
            message: format!("成功生成 {created} 台设备（规格「{}」）", spec.code),
        })
    }
}
