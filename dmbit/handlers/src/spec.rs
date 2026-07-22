//! Spec handlers — 设备规格 CRUD + components.

use rust_ef::{db_context::DbContext, prelude::*};
use rust_webx::*;

use dmbit_contracts::spec::*;
use dmbit_domain::entities::{Device, Product, Spec, SpecComponent};
use dmbit_domain::new_id;

use crate::db::{save_changes, EfResultExt};
use crate::mapping::{
    assert_spec_code_available, load_components_for_specs, load_device_counts,
    replace_spec_components, require_non_negative, require_text,
    spec_to_model, MAX_BRAND, MAX_SPEC_CODE, MAX_UNIT,
};
use crate::util::{now_secs, operator_id, parse_id};

async fn product_name(ctx: &mut DbContext, product_id: &str) -> Result<String> {
    let p = crate::ef_require_by_id!(
        ctx,
        Product,
        product_id,
        Error::NotFound("所属产品不存在".into())
    );
    Ok(p.name)
}

async fn spec_model(
    ctx: &mut DbContext,
    s: &Spec,
    name: &str,
) -> Result<SpecModel> {
    let cmap = load_components_for_specs(ctx, &[s.id.clone()]).await?;
    let comps = cmap.get(&s.id).cloned().unwrap_or_default();
    let dmap = load_device_counts(ctx, &[s.id.clone()]).await?;
    let dc = dmap.get(&s.id).copied().unwrap_or(0);
    Ok(spec_to_model(s, name, comps, dc))
}

#[derive(Inject)]
pub struct ListSpecsHandler {
    #[inject(owned)]
    ctx: DbContext,
}

#[derive(Inject)]
pub struct ListProductSpecsHandler {
    #[inject(owned)]
    ctx: DbContext,
}

#[derive(Inject)]
pub struct GetSpecHandler {
    #[inject(owned)]
    ctx: DbContext,
}

#[derive(Inject)]
pub struct CreateSpecHandler {
    #[inject(owned)]
    ctx: DbContext,
}

#[derive(Inject)]
pub struct UpdateSpecHandler {
    #[inject(owned)]
    ctx: DbContext,
}

#[derive(Inject)]
pub struct DeleteSpecHandler {
    #[inject(owned)]
    ctx: DbContext,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<ListSpecsRequest, Vec<SpecModel>> for ListSpecsHandler {
    async fn handle(&mut self, _: ListSpecsRequest) -> Result<Vec<SpecModel>> {
        let products = linq!(self.ctx.set::<Product>();)
            .to_list()
            .await
            .map_ef()?;
        let mut specs = linq!(self.ctx.set::<Spec>();).to_list().await.map_ef()?;
        specs.sort_by(|a, b| a.sort_order.cmp(&b.sort_order).then(a.id.cmp(&b.id)));
        let ids: Vec<String> = specs.iter().map(|s| s.id.clone()).collect();
        let cmap = load_components_for_specs(&mut self.ctx, &ids).await?;
        let dmap = load_device_counts(&mut self.ctx, &ids).await?;

        Ok(specs
            .into_iter()
            .map(|s| {
                let name = products
                    .iter()
                    .find(|p| p.id == s.product_id)
                    .map(|p| p.name.as_str())
                    .unwrap_or("");
                let comps = cmap.get(&s.id).cloned().unwrap_or_default();
                let dc = dmap.get(&s.id).copied().unwrap_or(0);
                spec_to_model(&s, name, comps, dc)
            })
            .collect())
    }
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<ListProductSpecsRequest, Vec<SpecModel>> for ListProductSpecsHandler {
    async fn handle(&mut self, req: ListProductSpecsRequest) -> Result<Vec<SpecModel>> {
        let product_id = parse_id(&req.id)?;
        let name = product_name(&mut self.ctx, &product_id).await?;

        let mut specs = linq!(self.ctx.set::<Spec>(), |s: Spec| s.product_id == product_id)
            .to_list()
            .await
            .map_ef()?;
        specs.sort_by(|a, b| a.sort_order.cmp(&b.sort_order));
        let ids: Vec<String> = specs.iter().map(|s| s.id.clone()).collect();
        let cmap = load_components_for_specs(&mut self.ctx, &ids).await?;
        let dmap = load_device_counts(&mut self.ctx, &ids).await?;

        Ok(specs
            .into_iter()
            .map(|s| {
                let comps = cmap.get(&s.id).cloned().unwrap_or_default();
                let dc = dmap.get(&s.id).copied().unwrap_or(0);
                spec_to_model(&s, &name, comps, dc)
            })
            .collect())
    }
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<GetSpecRequest, SpecModel> for GetSpecHandler {
    async fn handle(&mut self, req: GetSpecRequest) -> Result<SpecModel> {
        let id = parse_id(&req.id)?;
        let s = crate::ef_require_by_id!(
            self.ctx,
            Spec,
            id,
            Error::NotFound("规格不存在".into())
        );
        let name = product_name(&mut self.ctx, &s.product_id).await?;
        spec_model(&mut self.ctx, &s, &name).await
    }
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<CreateSpecRequest, SpecModel> for CreateSpecHandler {
    async fn handle(&mut self, req: CreateSpecRequest) -> Result<SpecModel> {
        let code = require_text("规格编码", &req.code, MAX_SPEC_CODE)?;
        let brand = require_text("品牌", &req.brand, MAX_BRAND)?;
        let unit = require_text("单位", &req.unit, MAX_UNIT)?;
        let planned_quantity = require_non_negative("计划数量", req.planned_quantity)?;

        let product_id = parse_id(&req.product_id)?;
        let name = product_name(&mut self.ctx, &product_id).await?;
        assert_spec_code_available(&mut self.ctx, &code, None).await?;

        let now = now_secs();
        let op = operator_id();
        let id = new_id();
        let entity = Spec {
            id: id.clone(),
            product_id,
            code,
            brand,
            parameters: req.parameters,
            unit,
            planned_quantity,
            sort_order: req.sort_order,
            created_id: op.clone(),
            created_at: now,
            updated_id: op.clone(),
            updated_at: now,
            is_deleted: false,
            product: BelongsTo::new(),
            components: HasMany::new(),
            devices: HasMany::new(),
        };

        self.ctx.set::<Spec>().add(entity.clone());
        replace_spec_components(&mut self.ctx, &id, &req.components).await?;
        save_changes(&mut self.ctx).await?;

        spec_model(&mut self.ctx, &entity, &name).await
    }
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<UpdateSpecRequest, SpecModel> for UpdateSpecHandler {
    async fn handle(&mut self, req: UpdateSpecRequest) -> Result<SpecModel> {
        let id = parse_id(&req.id)?;
        let mut s = crate::ef_require_by_id!(
            self.ctx,
            Spec,
            id,
            Error::NotFound("规格不存在".into())
        );

        if let Some(product_id) = req.product_id {
            let product_id = parse_id(&product_id)?;
            let _ = product_name(&mut self.ctx, &product_id).await?;
            s.product_id = product_id;
        }
        if let Some(code) = req.code {
            let code = require_text("规格编码", &code, MAX_SPEC_CODE)?;
            assert_spec_code_available(&mut self.ctx, &code, Some(&s.id)).await?;
            s.code = code;
        }
        if let Some(brand) = req.brand {
            s.brand = require_text("品牌", &brand, MAX_BRAND)?;
        }
        if let Some(parameters) = req.parameters {
            s.parameters = parameters;
        }
        if let Some(unit) = req.unit {
            s.unit = require_text("单位", &unit, MAX_UNIT)?;
        }
        if let Some(planned_quantity) = req.planned_quantity {
            s.planned_quantity = require_non_negative("计划数量", planned_quantity)?;
        }
        if let Some(sort_order) = req.sort_order {
            s.sort_order = sort_order;
        }

        s.updated_at = now_secs();
        s.updated_id = operator_id();

        let name = product_name(&mut self.ctx, &s.product_id).await?;

        self.ctx.set::<Spec>().update(s.clone());

        if let Some(components) = req.components {
            replace_spec_components(&mut self.ctx, &id, &components).await?;
        }
        save_changes(&mut self.ctx).await?;

        spec_model(&mut self.ctx, &s, &name).await
    }
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<DeleteSpecRequest, String> for DeleteSpecHandler {
    async fn handle(&mut self, req: DeleteSpecRequest) -> Result<String> {
        let id = parse_id(&req.id)?;
        let mut s = crate::ef_require_by_id!(
            self.ctx,
            Spec,
            id,
            Error::NotFound("规格不存在".into())
        );

        let now = now_secs();
        let op = operator_id();
        s.is_deleted = true;
        s.updated_at = now;
        s.updated_id = op.clone();

        // Physical delete components
        let sid = s.id.clone();
        let sid2 = sid.clone();
        let comps =
            linq!(self.ctx.set::<SpecComponent>(), |c: SpecComponent| c.spec_id == sid2)
                .to_list()
                .await
                .map_ef()?;
        for mut c in comps {
            c.is_deleted = true;
            c.updated_at = now;
            c.updated_id = op.clone();
            self.ctx.set::<SpecComponent>().update(c);
        }

        // Mark devices as "已淘汰" instead of cascade delete
        let devices =
            linq!(self.ctx.set::<Device>(), |d: Device| d.spec_id == sid)
                .to_list()
                .await
                .map_ef()?;
        for mut d in devices {
            d.status = "已淘汰".into();
            d.updated_at = now;
            d.updated_id = op.clone();
            self.ctx.set::<Device>().update(d);
        }

        self.ctx.set::<Spec>().update(s);
        save_changes(&mut self.ctx).await?;

        Ok(format!("已删除规格 {}", id))
    }
}
