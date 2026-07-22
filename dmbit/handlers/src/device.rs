//! Device handlers — 设备实例 CRUD + 批量生成.

use rust_ef::{db_context::DbContext, prelude::*};
use rust_webx::*;

use dmbit_contracts::device::*;
use dmbit_domain::entities::{Device, Product, Spec};
use dmbit_domain::new_id;

use crate::db::{save_changes, EfResultExt};
use crate::mapping::{
    assert_device_asset_code_available, device_to_model, normalize_status,
    optional_text, require_text, MAX_ASSET_CODE, MAX_LOCATION, MAX_SERIAL_NO,
};
use crate::util::{now_secs, operator_id, parse_id};

async fn spec_and_product(
    ctx: &mut DbContext,
    spec_id: &str,
) -> Result<(Spec, String)> {
    let s = linq!(ctx.set::<Spec>(), |s: Spec| s.id == spec_id)
        .first_or_default()
        .await
        .map_ef()?
        .ok_or_else(|| Error::NotFound("规格不存在".into()))?;
    let pid = s.product_id.clone();
    let p = linq!(ctx.set::<Product>(), |p: Product| p.id == pid)
        .first_or_default()
        .await
        .map_ef()?
        .ok_or_else(|| Error::NotFound("所属产品不存在".into()))?;
    Ok((s, p.name))
}

#[derive(Inject)]
pub struct ListDevicesHandler {
    #[inject(owned)]
    ctx: DbContext,
}

#[derive(Inject)]
pub struct ListSpecDevicesHandler {
    #[inject(owned)]
    ctx: DbContext,
}

#[derive(Inject)]
pub struct GetDeviceHandler {
    #[inject(owned)]
    ctx: DbContext,
}

#[derive(Inject)]
pub struct CreateDeviceHandler {
    #[inject(owned)]
    ctx: DbContext,
}

#[derive(Inject)]
pub struct GenerateDevicesHandler {
    #[inject(owned)]
    ctx: DbContext,
}

#[derive(Inject)]
pub struct UpdateDeviceHandler {
    #[inject(owned)]
    ctx: DbContext,
}

#[derive(Inject)]
pub struct DeleteDeviceHandler {
    #[inject(owned)]
    ctx: DbContext,
}

/// Load spec+product names into a cache map.
async fn spec_name_map(ctx: &mut DbContext) -> Result<std::collections::HashMap<String, (String, String)>> {
    let specs = linq!(ctx.set::<Spec>();).to_list().await.map_ef()?;
    let products = linq!(ctx.set::<Product>();).to_list().await.map_ef()?;
    let mut map = std::collections::HashMap::new();
    for s in specs {
        let pname = products.iter().find(|p| p.id == s.product_id).map(|p| p.name.as_str()).unwrap_or("");
        map.insert(s.id, (s.code, pname.to_string()));
    }
    Ok(map)
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<ListDevicesRequest, Vec<DeviceModel>> for ListDevicesHandler {
    async fn handle(&mut self, _: ListDevicesRequest) -> Result<Vec<DeviceModel>> {
        let names = spec_name_map(&mut self.ctx).await?;
        let mut devices = linq!(self.ctx.set::<Device>();).to_list().await.map_ef()?;
        devices.sort_by(|a, b| a.sort_order.cmp(&b.sort_order).then(a.asset_code.cmp(&b.asset_code)));

        Ok(devices
            .into_iter()
            .map(|d| {
                let (code, name) = names.get(&d.spec_id).cloned().unwrap_or_default();
                device_to_model(&d, &code, &name)
            })
            .collect())
    }
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<ListSpecDevicesRequest, Vec<DeviceModel>> for ListSpecDevicesHandler {
    async fn handle(&mut self, req: ListSpecDevicesRequest) -> Result<Vec<DeviceModel>> {
        let spec_id = parse_id(&req.id)?;
        let (spec, product_name) = spec_and_product(&mut self.ctx, &spec_id).await?;

        let mut devices =
            linq!(self.ctx.set::<Device>(), |d: Device| d.spec_id == spec_id)
                .to_list()
                .await
                .map_ef()?;
        devices.sort_by(|a, b| a.sort_order.cmp(&b.sort_order).then(a.asset_code.cmp(&b.asset_code)));

        Ok(devices
            .into_iter()
            .map(|d| device_to_model(&d, &spec.code, &product_name))
            .collect())
    }
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<GetDeviceRequest, DeviceModel> for GetDeviceHandler {
    async fn handle(&mut self, req: GetDeviceRequest) -> Result<DeviceModel> {
        let id = parse_id(&req.id)?;
        let d = crate::ef_require_by_id!(
            self.ctx,
            Device,
            id,
            Error::NotFound("设备不存在".into())
        );
        let (spec, product_name) = spec_and_product(&mut self.ctx, &d.spec_id).await?;
        Ok(device_to_model(&d, &spec.code, &product_name))
    }
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<CreateDeviceRequest, DeviceModel> for CreateDeviceHandler {
    async fn handle(&mut self, req: CreateDeviceRequest) -> Result<DeviceModel> {
        let spec_id = parse_id(&req.spec_id)?;
        let asset_code = require_text("资产编码", &req.asset_code, MAX_ASSET_CODE)?;
        let status = normalize_status(&req.status)?;
        let location = optional_text("机位", &req.location, MAX_LOCATION)?;
        let serial_no = optional_text("序列号", &req.serial_no, MAX_SERIAL_NO)?;

        let (spec, product_name) = spec_and_product(&mut self.ctx, &spec_id).await?;
        assert_device_asset_code_available(&mut self.ctx, &asset_code, None).await?;

        let now = now_secs();
        let op = operator_id();
        let entity = Device {
            id: new_id(),
            spec_id,
            status,
            location,
            asset_code,
            serial_no,
            sort_order: req.sort_order,
            created_id: op.clone(),
            created_at: now,
            updated_id: op,
            updated_at: now,
            is_deleted: false,
            spec: BelongsTo::new(),
        };

        self.ctx.set::<Device>().add(entity.clone());
        save_changes(&mut self.ctx).await?;

        Ok(device_to_model(&entity, &spec.code, &product_name))
    }
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<GenerateDevicesRequest, GenerateDevicesResult> for GenerateDevicesHandler {
    async fn handle(&mut self, req: GenerateDevicesRequest) -> Result<GenerateDevicesResult> {
        let spec_id = parse_id(&req.id)?;
        let (_spec, _product_name) = spec_and_product(&mut self.ctx, &spec_id).await?;

        let count = req.count.unwrap_or(_spec.planned_quantity);
        if count <= 0 {
            return Err(Error::Validation("生成数量必须大于 0".into()));
        }

        let prefix = if req.asset_prefix.is_empty() {
            format!("{}-", _spec.code)
        } else {
            req.asset_prefix.clone()
        };

        let now = now_secs();
        let op = operator_id();
        let mut created = 0i32;

        for i in 0..count {
            let idx = req.start_index + i;
            let asset_code = format!("{prefix}{idx:04}");
            // Check asset_code availability
            let q = asset_code.clone();
            let existing = linq!(self.ctx.set::<Device>(), |d: Device| d.asset_code == q)
                .first_or_default()
                .await
                .map_ef()?;
            if existing.is_some() {
                continue;
            }

            let entity = Device {
                id: new_id(),
                spec_id: spec_id.clone(),
                status: "待上架".into(),
                location: String::new(),
                asset_code,
                serial_no: String::new(),
                sort_order: idx,
                created_id: op.clone(),
                created_at: now,
                updated_id: op.clone(),
                updated_at: now,
                is_deleted: false,
                spec: BelongsTo::new(),
            };
            self.ctx.set::<Device>().add(entity);
            created += 1;
        }

        save_changes(&mut self.ctx).await?;

        Ok(GenerateDevicesResult {
            created,
            message: format!("成功生成 {created} 台设备（规格「{}」）", _spec.code),
        })
    }
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<UpdateDeviceRequest, DeviceModel> for UpdateDeviceHandler {
    async fn handle(&mut self, req: UpdateDeviceRequest) -> Result<DeviceModel> {
        let id = parse_id(&req.id)?;
        let mut d = crate::ef_require_by_id!(
            self.ctx,
            Device,
            id,
            Error::NotFound("设备不存在".into())
        );

        if let Some(spec_id) = req.spec_id {
            let spec_id = parse_id(&spec_id)?;
            let _ = spec_and_product(&mut self.ctx, &spec_id).await?;
            d.spec_id = spec_id;
        }
        if let Some(status) = req.status {
            d.status = normalize_status(&status)?;
        }
        if let Some(location) = req.location {
            d.location = optional_text("机位", &location, MAX_LOCATION)?;
        }
        if let Some(asset_code) = req.asset_code {
            let asset_code = require_text("资产编码", &asset_code, MAX_ASSET_CODE)?;
            assert_device_asset_code_available(&mut self.ctx, &asset_code, Some(&d.id)).await?;
            d.asset_code = asset_code;
        }
        if let Some(serial_no) = req.serial_no {
            d.serial_no = optional_text("序列号", &serial_no, MAX_SERIAL_NO)?;
        }
        if let Some(sort_order) = req.sort_order {
            d.sort_order = sort_order;
        }

        d.updated_at = now_secs();
        d.updated_id = operator_id();

        self.ctx.set::<Device>().update(d.clone());
        save_changes(&mut self.ctx).await?;

        let (spec, product_name) = spec_and_product(&mut self.ctx, &d.spec_id).await?;
        Ok(device_to_model(&d, &spec.code, &product_name))
    }
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<DeleteDeviceRequest, String> for DeleteDeviceHandler {
    async fn handle(&mut self, req: DeleteDeviceRequest) -> Result<String> {
        let id = parse_id(&req.id)?;
        let mut d = crate::ef_require_by_id!(
            self.ctx,
            Device,
            id,
            Error::NotFound("设备不存在".into())
        );

        d.is_deleted = true;
        d.updated_at = now_secs();
        d.updated_id = operator_id();

        self.ctx.set::<Device>().update(d);
        save_changes(&mut self.ctx).await?;

        Ok(format!("已删除设备 {}", id))
    }
}
