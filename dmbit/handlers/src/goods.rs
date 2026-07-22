//! Goods handlers — inventory CRUD + components.

use rust_ef::{db_context::DbContext, prelude::*};
use rust_webx::*;

use dmbit_contracts::goods::*;
use dmbit_domain::entities::{Goods, GoodsComponent, Product};
use dmbit_domain::new_id;

use crate::db::{save_changes, EfResultExt};
use crate::mapping::{
    assert_goods_key_available, goods_to_model, load_components_for, normalize_status,
    optional_text, replace_components, require_non_negative, require_text,
    strip_redundant_disk_params, MAX_ASSET_CODE, MAX_BRAND, MAX_LOCATION, MAX_UNIT,
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

async fn goods_model(ctx: &mut DbContext, g: Goods, name: &str) -> Result<GoodsModel> {
    let map = load_components_for(ctx, &[g.id.clone()]).await?;
    let comps = map.get(&g.id).cloned().unwrap_or_default();
    Ok(goods_to_model(g, name, comps))
}

#[derive(Inject)]
pub struct ListGoodsHandler {
    #[inject(owned)]
    ctx: DbContext,
}

#[derive(Inject)]
pub struct ListProductGoodsHandler {
    #[inject(owned)]
    ctx: DbContext,
}

#[derive(Inject)]
pub struct GetGoodsHandler {
    #[inject(owned)]
    ctx: DbContext,
}

#[derive(Inject)]
pub struct CreateGoodsHandler {
    #[inject(owned)]
    ctx: DbContext,
}

#[derive(Inject)]
pub struct UpdateGoodsHandler {
    #[inject(owned)]
    ctx: DbContext,
}

#[derive(Inject)]
pub struct DeleteGoodsHandler {
    #[inject(owned)]
    ctx: DbContext,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<ListGoodsRequest, Vec<GoodsModel>> for ListGoodsHandler {
    async fn handle(&mut self, _: ListGoodsRequest) -> Result<Vec<GoodsModel>> {
        let products = linq!(self.ctx.set::<Product>();)
            .to_list()
            .await
            .map_ef()?;
        let mut goods = linq!(self.ctx.set::<Goods>();).to_list().await.map_ef()?;
        goods.sort_by(|a, b| a.sort_order.cmp(&b.sort_order).then(a.id.cmp(&b.id)));
        let ids: Vec<String> = goods.iter().map(|g| g.id.clone()).collect();
        let cmap = load_components_for(&mut self.ctx, &ids).await?;

        Ok(goods
            .into_iter()
            .map(|g| {
                let name = products
                    .iter()
                    .find(|p| p.id == g.product_id)
                    .map(|p| p.name.as_str())
                    .unwrap_or("");
                let comps = cmap.get(&g.id).cloned().unwrap_or_default();
                goods_to_model(g, name, comps)
            })
            .collect())
    }
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<ListProductGoodsRequest, Vec<GoodsModel>> for ListProductGoodsHandler {
    async fn handle(&mut self, req: ListProductGoodsRequest) -> Result<Vec<GoodsModel>> {
        let product_id = parse_id(&req.id)?;
        let name = product_name(&mut self.ctx, &product_id).await?;

        let mut goods = linq!(self.ctx.set::<Goods>(), |g: Goods| g.product_id == product_id)
            .to_list()
            .await
            .map_ef()?;
        goods.sort_by(|a, b| a.sort_order.cmp(&b.sort_order));
        let ids: Vec<String> = goods.iter().map(|g| g.id.clone()).collect();
        let cmap = load_components_for(&mut self.ctx, &ids).await?;

        Ok(goods
            .into_iter()
            .map(|g| {
                let comps = cmap.get(&g.id).cloned().unwrap_or_default();
                goods_to_model(g, &name, comps)
            })
            .collect())
    }
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<GetGoodsRequest, GoodsModel> for GetGoodsHandler {
    async fn handle(&mut self, req: GetGoodsRequest) -> Result<GoodsModel> {
        let id = parse_id(&req.id)?;
        let g = crate::ef_require_by_id!(
            self.ctx,
            Goods,
            id,
            Error::NotFound("台账不存在".into())
        );
        let name = product_name(&mut self.ctx, &g.product_id).await?;
        goods_model(&mut self.ctx, g, &name).await
    }
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<CreateGoodsRequest, GoodsModel> for CreateGoodsHandler {
    async fn handle(&mut self, req: CreateGoodsRequest) -> Result<GoodsModel> {
        let brand = require_text("品牌短码", &req.brand, MAX_BRAND)?;
        let unit = require_text("单位", &req.unit, MAX_UNIT)?;
        let quantity = require_non_negative("数量", req.quantity)?;
        let location = optional_text("机位", &req.location, MAX_LOCATION)?;
        let asset_code = optional_text("资产编码", &req.asset_code, MAX_ASSET_CODE)?;
        let status = normalize_status(&req.status)?;

        let product_id = parse_id(&req.product_id)?;
        let name = product_name(&mut self.ctx, &product_id).await?;
        assert_goods_key_available(
            &mut self.ctx,
            &product_id,
            &brand,
            &asset_code,
            &location,
            None,
        )
        .await?;

        let now = now_secs();
        let op = operator_id();
        let id = new_id();
        let entity = Goods {
            id: id.clone(),
            product_id,
            brand,
            parameters: strip_redundant_disk_params(&req.parameters, &req.components),
            unit,
            quantity,
            status,
            location,
            asset_code,
            sort_order: req.sort_order,
            created_id: op.clone(),
            created_at: now,
            updated_id: op,
            updated_at: now,
            is_deleted: false,
            product: BelongsTo::new(),
        };

        self.ctx.set::<Goods>().add(entity.clone());
        replace_components(&mut self.ctx, &id, &req.components).await?;
        save_changes(&mut self.ctx).await?;

        goods_model(&mut self.ctx, entity, &name).await
    }
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<UpdateGoodsRequest, GoodsModel> for UpdateGoodsHandler {
    async fn handle(&mut self, req: UpdateGoodsRequest) -> Result<GoodsModel> {
        let id = parse_id(&req.id)?;
        let mut g = crate::ef_require_by_id!(
            self.ctx,
            Goods,
            id,
            Error::NotFound("台账不存在".into())
        );

        if let Some(product_id) = req.product_id {
            let product_id = parse_id(&product_id)?;
            let _ = product_name(&mut self.ctx, &product_id).await?;
            g.product_id = product_id;
        }
        if let Some(brand) = req.brand {
            g.brand = require_text("品牌短码", &brand, MAX_BRAND)?;
        }
        if let Some(parameters) = req.parameters {
            g.parameters = parameters;
        }
        if let Some(unit) = req.unit {
            g.unit = require_text("单位", &unit, MAX_UNIT)?;
        }
        if let Some(quantity) = req.quantity {
            g.quantity = require_non_negative("数量", quantity)?;
        }
        if let Some(status) = req.status {
            g.status = normalize_status(&status)?;
        }
        if let Some(location) = req.location {
            g.location = optional_text("机位", &location, MAX_LOCATION)?;
        }
        if let Some(asset_code) = req.asset_code {
            g.asset_code = optional_text("资产编码", &asset_code, MAX_ASSET_CODE)?;
        }
        if let Some(sort_order) = req.sort_order {
            g.sort_order = sort_order;
        }

        assert_goods_key_available(
            &mut self.ctx,
            &g.product_id,
            &g.brand,
            &g.asset_code,
            &g.location,
            Some(&g.id),
        )
        .await?;

        g.updated_at = now_secs();
        g.updated_id = operator_id();

        let name = product_name(&mut self.ctx, &g.product_id).await?;

        let comps_for_strip = if let Some(ref components) = req.components {
            components.clone()
        } else {
            load_components_for(&mut self.ctx, &[id.clone()])
                .await?
                .remove(&id)
                .unwrap_or_default()
        };
        g.parameters = strip_redundant_disk_params(&g.parameters, &comps_for_strip);

        self.ctx.set::<Goods>().update(g.clone());

        if let Some(components) = req.components {
            replace_components(&mut self.ctx, &id, &components).await?;
        }
        save_changes(&mut self.ctx).await?;

        goods_model(&mut self.ctx, g, &name).await
    }
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<DeleteGoodsRequest, String> for DeleteGoodsHandler {
    async fn handle(&mut self, req: DeleteGoodsRequest) -> Result<String> {
        let id = parse_id(&req.id)?;
        let mut g = crate::ef_require_by_id!(
            self.ctx,
            Goods,
            id,
            Error::NotFound("台账不存在".into())
        );

        let now = now_secs();
        let op = operator_id();
        g.is_deleted = true;
        g.updated_at = now;
        g.updated_id = op.clone();

        let gid = g.id.clone();
        let children =
            linq!(self.ctx.set::<GoodsComponent>(), |c: GoodsComponent| c.goods_id == gid)
                .to_list()
                .await
                .map_ef()?;
        for mut c in children {
            c.is_deleted = true;
            c.updated_at = now;
            c.updated_id = op.clone();
            self.ctx.set::<GoodsComponent>().update(c);
        }

        self.ctx.set::<Goods>().update(g);
        save_changes(&mut self.ctx).await?;

        Ok(format!("已删除台账 {}", id))
    }
}
