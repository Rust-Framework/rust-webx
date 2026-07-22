//! Product handlers — device type (master) CRUD.

use rust_ef::{db_context::DbContext, prelude::*};
use rust_webx::*;

use dmbit_contracts::goods::GoodsModel;
use dmbit_contracts::product::*;
use dmbit_domain::entities::{Goods, GoodsComponent, Product};
use dmbit_domain::new_id;

use crate::db::{save_changes, EfResultExt};
use crate::mapping::{goods_to_model, load_components_for, normalize_category};
use crate::util::{now_secs, operator_id, parse_id};

fn product_to_model(p: Product, goods: Vec<GoodsModel>) -> ProductModel {
    ProductModel {
        id: p.id,
        name: p.name,
        code: p.code,
        category: p.category,
        remark: p.remark,
        sort_order: p.sort_order,
        created_at: p.created_at,
        updated_at: p.updated_at,
        goods,
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
        let mut products = linq!(self.ctx.set::<Product>();)
            .to_list()
            .await
            .map_ef()?;
        products.sort_by(|a, b| a.sort_order.cmp(&b.sort_order).then(a.name.cmp(&b.name)));

        let all_goods = linq!(self.ctx.set::<Goods>();).to_list().await.map_ef()?;
        let ids: Vec<String> = all_goods.iter().map(|g| g.id.clone()).collect();
        let cmap = load_components_for(&mut self.ctx, &ids).await?;

        let mut result = Vec::with_capacity(products.len());
        for p in products {
            let mut goods: Vec<GoodsModel> = all_goods
                .iter()
                .filter(|g| g.product_id == p.id)
                .cloned()
                .map(|g| {
                    let comps = cmap.get(&g.id).cloned().unwrap_or_default();
                    goods_to_model(g, &p.name, comps)
                })
                .collect();
            goods.sort_by(|a, b| a.sort_order.cmp(&b.sort_order));
            result.push(product_to_model(p, goods));
        }
        Ok(result)
    }
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<GetProductRequest, ProductModel> for GetProductHandler {
    async fn handle(&mut self, req: GetProductRequest) -> Result<ProductModel> {
        let id = parse_id(&req.id)?;
        let p = crate::ef_require_by_id!(
            self.ctx,
            Product,
            id,
            Error::NotFound("产品不存在".into())
        );

        let pid = p.id.clone();
        let goods_rows = linq!(self.ctx.set::<Goods>(), |g: Goods| g.product_id == pid)
            .to_list()
            .await
            .map_ef()?;
        let ids: Vec<String> = goods_rows.iter().map(|g| g.id.clone()).collect();
        let cmap = load_components_for(&mut self.ctx, &ids).await?;

        let mut goods: Vec<GoodsModel> = goods_rows
            .into_iter()
            .map(|g| {
                let comps = cmap.get(&g.id).cloned().unwrap_or_default();
                goods_to_model(g, &p.name, comps)
            })
            .collect();
        goods.sort_by(|a, b| a.sort_order.cmp(&b.sort_order));

        Ok(product_to_model(p, goods))
    }
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<CreateProductRequest, ProductModel> for CreateProductHandler {
    async fn handle(&mut self, req: CreateProductRequest) -> Result<ProductModel> {
        if req.name.trim().is_empty() {
            return Err(Error::Validation("产品名称不能为空".into()));
        }
        if req.code.trim().is_empty() {
            return Err(Error::Validation("编码不能为空".into()));
        }

        let now = now_secs();
        let op = operator_id();
        let entity = Product {
            id: new_id(),
            name: req.name.trim().to_string(),
            code: req.code.trim().to_string(),
            category: normalize_category(&req.category),
            remark: req.remark,
            sort_order: req.sort_order,
            created_id: op.clone(),
            created_at: now,
            updated_id: op,
            updated_at: now,
            is_deleted: false,
            goods: HasMany::new(),
        };

        self.ctx.set::<Product>().add(entity.clone());
        save_changes(&mut self.ctx).await?;

        Ok(product_to_model(entity, Vec::new()))
    }
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<UpdateProductRequest, ProductModel> for UpdateProductHandler {
    async fn handle(&mut self, req: UpdateProductRequest) -> Result<ProductModel> {
        let id = parse_id(&req.id)?;
        let mut p = crate::ef_require_by_id!(
            self.ctx,
            Product,
            id,
            Error::NotFound("产品不存在".into())
        );

        if let Some(name) = req.name {
            if name.trim().is_empty() {
                return Err(Error::Validation("产品名称不能为空".into()));
            }
            p.name = name.trim().to_string();
        }
        if let Some(code) = req.code {
            if code.trim().is_empty() {
                return Err(Error::Validation("编码不能为空".into()));
            }
            p.code = code.trim().to_string();
        }
        if let Some(category) = req.category {
            p.category = normalize_category(&category);
        }
        if let Some(remark) = req.remark {
            p.remark = remark;
        }
        if let Some(sort_order) = req.sort_order {
            p.sort_order = sort_order;
        }
        p.updated_at = now_secs();
        p.updated_id = operator_id();

        self.ctx.set::<Product>().update(p.clone());
        save_changes(&mut self.ctx).await?;

        Ok(product_to_model(p, Vec::new()))
    }
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<DeleteProductRequest, String> for DeleteProductHandler {
    async fn handle(&mut self, req: DeleteProductRequest) -> Result<String> {
        let id = parse_id(&req.id)?;
        let mut p = crate::ef_require_by_id!(
            self.ctx,
            Product,
            id,
            Error::NotFound("产品不存在".into())
        );

        let now = now_secs();
        let op = operator_id();
        p.is_deleted = true;
        p.updated_at = now;
        p.updated_id = op.clone();

        let pid = p.id.clone();
        let children = linq!(self.ctx.set::<Goods>(), |g: Goods| g.product_id == pid)
            .to_list()
            .await
            .map_ef()?;
        for mut g in children {
            let gid = g.id.clone();
            let comps =
                linq!(self.ctx.set::<GoodsComponent>(), |c: GoodsComponent| c.goods_id == gid)
                    .to_list()
                    .await
                    .map_ef()?;
            for mut c in comps {
                c.is_deleted = true;
                c.updated_at = now;
                c.updated_id = op.clone();
                self.ctx.set::<GoodsComponent>().update(c);
            }
            g.is_deleted = true;
            g.updated_at = now;
            g.updated_id = op.clone();
            self.ctx.set::<Goods>().update(g);
        }

        self.ctx.set::<Product>().update(p);
        save_changes(&mut self.ctx).await?;

        Ok(format!("Deleted product {}", id))
    }
}
