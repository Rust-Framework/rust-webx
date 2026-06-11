//! Product handlers — auto-registered via `#[lrdi::inject_attr]` + `#[handler(inject)]`.
//!
//! EF Core pattern: `ctx.set::<ProductEntity>()` for queries,
//! `ctx.execute(&sql)` for writes (REF has no change tracking).

use std::sync::Arc;

use lref::provider::DbValue;
use lrwf::*;

use crate::contracts::product::*;
use crate::domain::db_context::AppDbContext;
use crate::domain::product::{ProductEntity, ProductModel};

// ── Handlers — auto-injected via #[inject_attr] ──

#[lrdi::inject_attr(singleton, as = dyn IRequestHandler<ListProductsRequest, Vec<ProductModel>>)]
pub struct ListProductsHandler {
    ctx: Arc<AppDbContext>,
}

#[lrdi::inject_attr(singleton, as = dyn IRequestHandler<GetProductRequest, ProductModel>)]
pub struct GetProductHandler {
    ctx: Arc<AppDbContext>,
}

#[lrdi::inject_attr(singleton, as = dyn IRequestHandler<CreateProductRequest, ProductModel>)]
pub struct CreateProductHandler {
    ctx: Arc<AppDbContext>,
}

#[lrdi::inject_attr(singleton, as = dyn IRequestHandler<UpdateProductRequest, ProductModel>)]
pub struct UpdateProductHandler {
    ctx: Arc<AppDbContext>,
}

#[lrdi::inject_attr(singleton, as = dyn IRequestHandler<DeleteProductRequest, String>)]
pub struct DeleteProductHandler {
    ctx: Arc<AppDbContext>,
}

// ── Handler implementations ──

#[handler(inject)]
#[async_trait]
impl IRequestHandler<ListProductsRequest, Vec<ProductModel>> for ListProductsHandler {
    async fn handle(&self, _: ListProductsRequest) -> Result<Vec<ProductModel>> {
        let entities = self
            .ctx
            .set::<ProductEntity>()
            .to_list()
            .await
            .map_err(|e| Error::Internal(e.to_string()))?;
        Ok(entities.into_iter().map(ProductModel::from).collect())
    }
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<GetProductRequest, ProductModel> for GetProductHandler {
    async fn handle(&self, req: GetProductRequest) -> Result<ProductModel> {
        let entity = self
            .ctx
            .set::<ProductEntity>()
            .filter_column("id", "=", DbValue::String(req.id.clone()))
            .first_or_default()
            .await
            .map_err(|e| Error::Internal(e.to_string()))?
            .ok_or_else(|| Error::NotFound(format!("Product not found: {}", req.id)))?;
        Ok(ProductModel::from(entity))
    }
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<CreateProductRequest, ProductModel> for CreateProductHandler {
    async fn handle(&self, req: CreateProductRequest) -> Result<ProductModel> {
        let id = uuid();
        let now = now_secs();
        let sql = format!(
            "INSERT INTO products (id, name, price, created_at) VALUES ('{}', '{}', {}, '{}')",
            id,
            req.name.replace('\'', "''"),
            req.price,
            now
        );
        self.ctx
            .execute(&sql)
            .await
            .map_err(|e| Error::Internal(format!("Failed to create product: {}", e)))?;
        tracing::info!("Product created: {} (id: {})", req.name, id);
        Ok(ProductModel {
            id,
            name: req.name,
            price: req.price,
            created_at: now,
        })
    }
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<UpdateProductRequest, ProductModel> for UpdateProductHandler {
    async fn handle(&self, req: UpdateProductRequest) -> Result<ProductModel> {
        let existing = self
            .ctx
            .set::<ProductEntity>()
            .filter_column("id", "=", DbValue::String(req.id.clone()))
            .first_or_default()
            .await
            .map_err(|e| Error::Internal(e.to_string()))?
            .ok_or_else(|| Error::NotFound(format!("Product not found: {}", req.id)))?;

        let new_name = req.name.clone().unwrap_or(existing.name);
        let new_price = req.price.unwrap_or(existing.price);
        let mut set_parts: Vec<String> = Vec::new();
        if req.name.is_some() {
            set_parts.push(format!("name = '{}'", new_name.replace('\'', "''")));
        }
        if req.price.is_some() {
            set_parts.push(format!("price = {}", new_price));
        }
        if !set_parts.is_empty() {
            let sql = format!(
                "UPDATE products SET {} WHERE id = '{}'",
                set_parts.join(", "),
                req.id
            );
            self.ctx
                .execute(&sql)
                .await
                .map_err(|e| Error::Internal(format!("Failed to update product: {}", e)))?;
        }
        Ok(ProductModel {
            id: req.id,
            name: new_name,
            price: new_price,
            created_at: existing.created_at,
        })
    }
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<DeleteProductRequest, String> for DeleteProductHandler {
    async fn handle(&self, req: DeleteProductRequest) -> Result<String> {
        let sql = format!("DELETE FROM products WHERE id = '{}'", req.id);
        self.ctx
            .execute(&sql)
            .await
            .map_err(|e| Error::Internal(e.to_string()))?;
        tracing::info!("Product deleted: {}", req.id);
        Ok(format!("Product {} deleted", req.id))
    }
}

fn uuid() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| format!("{:x}", d.as_nanos()))
        .unwrap_or_else(|_| "0".to_string())
}

fn now_secs() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs().to_string())
        .unwrap_or_else(|_| "0".to_string())
}
