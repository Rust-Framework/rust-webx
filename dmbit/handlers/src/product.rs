//! Product handlers — device type (master) CRUD.

use rust_ef::{db_context::DbContext, prelude::*};
use rust_webx::*;

use dmbit_contracts::goods::GoodsModel;
use dmbit_contracts::product::*;
use dmbit_contracts::spec::SpecModel;
use dmbit_domain::entities::{Device, Product, Spec, SpecComponent};
use dmbit_domain::new_id;

use crate::db::{save_changes, EfResultExt};
use crate::mapping::{
    assert_product_code_available, load_components_for_specs, load_device_counts,
    normalize_category, optional_text, require_text, spec_to_model, MAX_PRODUCT_CODE,
    MAX_PRODUCT_NAME, MAX_PRODUCT_REMARK,
};
use crate::util::{now_secs, operator_id, parse_id};

/// Convert SpecModel to GoodsModel for backward compatibility with admin panel.
fn spec_to_goods(s: &SpecModel) -> GoodsModel {
    GoodsModel {
        id: s.id.clone(),
        product_id: s.product_id.clone(),
        product_name: s.product_name.clone(),
        code: s.code.clone(),
        brand: s.brand.clone(),
        parameters: s.parameters.clone(),
        unit: s.unit.clone(),
        quantity: s.planned_quantity,
        planned_quantity: s.planned_quantity,
        status: String::new(),
        location: String::new(),
        asset_code: String::new(),
        sort_order: s.sort_order,
        created_at: s.created_at,
        updated_at: s.updated_at,
        components: s.components.clone(),
        device_count: s.device_count,
    }
}

fn product_to_model(p: Product, specs: Vec<SpecModel>) -> ProductModel {
    ProductModel {
        id: p.id,
        name: p.name,
        code: p.code,
        category: p.category,
        remark: p.remark,
        sort_order: p.sort_order,
        created_at: p.created_at,
        updated_at: p.updated_at,
        goods: specs.iter().map(spec_to_goods).collect(),
    }
}

#[derive(Inject)]
pub struct ListProductsHandler {
    #[inject(owned)]
    ctx: DbContext,
}

#[derive(Inject)]
pub struct GetProductHandler {
    #[inject(owned)]
    ctx: DbContext,
}

#[derive(Inject)]
pub struct CreateProductHandler {
    #[inject(owned)]
    ctx: DbContext,
}

#[derive(Inject)]
pub struct UpdateProductHandler {
    #[inject(owned)]
    ctx: DbContext,
}

#[derive(Inject)]
pub struct DeleteProductHandler {
    #[inject(owned)]
    ctx: DbContext,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<ListProductsRequest, Vec<ProductModel>> for ListProductsHandler {
    async fn handle(&mut self, _: ListProductsRequest) -> Result<Vec<ProductModel>> {
        let mut products = linq!(self.ctx.set::<Product>();).to_list().await.map_ef()?;
        products.sort_by(|a, b| a.sort_order.cmp(&b.sort_order).then(a.name.cmp(&b.name)));

        let all_specs = linq!(self.ctx.set::<Spec>();).to_list().await.map_ef()?;
        let ids: Vec<String> = all_specs.iter().map(|s| s.id.clone()).collect();
        let cmap = load_components_for_specs(&mut self.ctx, &ids).await?;
        let dmap = load_device_counts(&mut self.ctx, &ids).await?;

        let mut result = Vec::with_capacity(products.len());
        for p in products {
            let mut specs: Vec<SpecModel> = all_specs
                .iter()
                .filter(|s| s.product_id == p.id)
                .map(|s| {
                    let comps = cmap.get(&s.id).cloned().unwrap_or_default();
                    let dc = dmap.get(&s.id).copied().unwrap_or(0);
                    spec_to_model(s, &p.name, comps, dc)
                })
                .collect();
            specs.sort_by_key(|a| a.sort_order);
            result.push(product_to_model(p, specs));
        }
        Ok(result)
    }
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<GetProductRequest, ProductModel> for GetProductHandler {
    async fn handle(&mut self, req: GetProductRequest) -> Result<ProductModel> {
        let id = parse_id(&req.id)?;
        let p =
            crate::ef_require_by_id!(self.ctx, Product, id, Error::NotFound("产品不存在".into()));

        let pid = p.id.clone();
        let specs = linq!(self.ctx.set::<Spec>(), |s: Spec| s.product_id == pid)
            .to_list()
            .await
            .map_ef()?;
        let ids: Vec<String> = specs.iter().map(|s| s.id.clone()).collect();
        let cmap = load_components_for_specs(&mut self.ctx, &ids).await?;
        let dmap = load_device_counts(&mut self.ctx, &ids).await?;

        let mut spec_models: Vec<SpecModel> = specs
            .into_iter()
            .map(|s| {
                let comps = cmap.get(&s.id).cloned().unwrap_or_default();
                let dc = dmap.get(&s.id).copied().unwrap_or(0);
                spec_to_model(&s, &p.name, comps, dc)
            })
            .collect();
        spec_models.sort_by_key(|a| a.sort_order);

        Ok(product_to_model(p, spec_models))
    }
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<CreateProductRequest, ProductModel> for CreateProductHandler {
    async fn handle(&mut self, req: CreateProductRequest) -> Result<ProductModel> {
        let name = require_text("产品名称", &req.name, MAX_PRODUCT_NAME)?;
        let code = require_text("产品编码", &req.code, MAX_PRODUCT_CODE)?;
        let remark = optional_text("备注", &req.remark, MAX_PRODUCT_REMARK)?;
        let category = normalize_category(&req.category)?;
        assert_product_code_available(&mut self.ctx, &code, None).await?;

        let now = now_secs();
        let op = operator_id();
        let entity = Product {
            id: new_id(),
            name,
            code,
            category,
            remark,
            sort_order: req.sort_order,
            created_id: op.clone(),
            created_at: now,
            updated_id: op,
            updated_at: now,
            is_deleted: false,
            specs: HasMany::new(),
        };

        self.ctx.add(entity.clone());
        save_changes(&mut self.ctx).await?;

        Ok(product_to_model(entity, Vec::new()))
    }
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<UpdateProductRequest, ProductModel> for UpdateProductHandler {
    async fn handle(&mut self, req: UpdateProductRequest) -> Result<ProductModel> {
        let id = parse_id(&req.id)?;
        let mut p =
            crate::ef_require_by_id!(self.ctx, Product, id, Error::NotFound("产品不存在".into()));

        if let Some(name) = req.name {
            p.name = require_text("产品名称", &name, MAX_PRODUCT_NAME)?;
        }
        if let Some(code) = req.code {
            let code = require_text("产品编码", &code, MAX_PRODUCT_CODE)?;
            assert_product_code_available(&mut self.ctx, &code, Some(&p.id)).await?;
            p.code = code;
        }
        if let Some(category) = req.category {
            p.category = normalize_category(&category)?;
        }
        if let Some(remark) = req.remark {
            p.remark = optional_text("备注", &remark, MAX_PRODUCT_REMARK)?;
        }
        if let Some(sort_order) = req.sort_order {
            p.sort_order = sort_order;
        }
        p.updated_at = now_secs();
        p.updated_id = operator_id();

        self.ctx.update(p.clone());
        save_changes(&mut self.ctx).await?;

        // Load specs for response
        let pid = p.id.clone();
        let specs = linq!(self.ctx.set::<Spec>(), |s: Spec| s.product_id == pid)
            .to_list()
            .await
            .map_ef()?;
        let ids: Vec<String> = specs.iter().map(|s| s.id.clone()).collect();
        let cmap = load_components_for_specs(&mut self.ctx, &ids).await?;
        let dmap = load_device_counts(&mut self.ctx, &ids).await?;
        let mut spec_models: Vec<SpecModel> = specs
            .into_iter()
            .map(|s| {
                let comps = cmap.get(&s.id).cloned().unwrap_or_default();
                let dc = dmap.get(&s.id).copied().unwrap_or(0);
                spec_to_model(&s, &p.name, comps, dc)
            })
            .collect();
        spec_models.sort_by_key(|a| a.sort_order);

        Ok(product_to_model(p, spec_models))
    }
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<DeleteProductRequest, String> for DeleteProductHandler {
    async fn handle(&mut self, req: DeleteProductRequest) -> Result<String> {
        let id = parse_id(&req.id)?;
        let mut p =
            crate::ef_require_by_id!(self.ctx, Product, id, Error::NotFound("产品不存在".into()));

        let now = now_secs();
        let op = operator_id();
        p.is_deleted = true;
        p.updated_at = now;
        p.updated_id = op.clone();

        let pid = p.id.clone();
        // Cascade: delete specs → components + mark devices as 已淘汰
        let child_specs = linq!(self.ctx.set::<Spec>(), |s: Spec| s.product_id == pid)
            .to_list()
            .await
            .map_ef()?;
        for mut s in child_specs {
            let sid = s.id.clone();
            // Physical delete components
            let sid2 = sid.clone();
            let comps = linq!(self.ctx.set::<SpecComponent>(), |c: SpecComponent| c
                .spec_id
                == sid2)
            .to_list()
            .await
            .map_ef()?;
            for mut c in comps {
                c.is_deleted = true;
                c.updated_at = now;
                c.updated_id = op.clone();
                self.ctx.update(c);
            }
            // Mark devices as 已淘汰
            let devs = linq!(self.ctx.set::<Device>(), |d: Device| d.spec_id == sid)
                .to_list()
                .await
                .map_ef()?;
            for mut d in devs {
                d.status = "已淘汰".into();
                d.updated_at = now;
                d.updated_id = op.clone();
                self.ctx.update(d);
            }
            s.is_deleted = true;
            s.updated_at = now;
            s.updated_id = op.clone();
            self.ctx.update(s);
        }

        self.ctx.update(p);
        save_changes(&mut self.ctx).await?;

        Ok(format!("已删除产品 {}", id))
    }
}
