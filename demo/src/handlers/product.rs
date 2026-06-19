//! Product handlers â€” auto-registered via `#[rust_dicore::inject_attr]` + `#[handler(inject)]`.

use std::sync::Arc;

use lref::{db_context::DbContext, prelude::*, provider::DbValue};
use rust_webapp::*;
use tokio::sync::Mutex;

use crate::contracts::product::*;
use crate::domain::product::{ProductEntity, ProductModel};

#[rust_dicore::inject_attr(singleton, as = dyn IRequestHandler<ListProductsRequest, Vec<ProductModel>>)]
pub struct ListProductsHandler {
    ctx: Arc<Mutex<DbContext>>,
}

#[rust_dicore::inject_attr(singleton, as = dyn IRequestHandler<GetProductRequest, ProductModel>)]
pub struct GetProductHandler {
    ctx: Arc<Mutex<DbContext>>,
}

#[rust_dicore::inject_attr(singleton, as = dyn IRequestHandler<CreateProductRequest, ProductModel>)]
pub struct CreateProductHandler {
    ctx: Arc<Mutex<DbContext>>,
}

#[rust_dicore::inject_attr(singleton, as = dyn IRequestHandler<UpdateProductRequest, ProductModel>)]
pub struct UpdateProductHandler {
    ctx: Arc<Mutex<DbContext>>,
}

#[rust_dicore::inject_attr(singleton, as = dyn IRequestHandler<DeleteProductRequest, String>)]
pub struct DeleteProductHandler {
    ctx: Arc<Mutex<DbContext>>,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<ListProductsRequest, Vec<ProductModel>> for ListProductsHandler {
    async fn handle(&self, _req: ListProductsRequest) -> Result<Vec<ProductModel>> {
        let query = {
            let mut ctx = self.ctx.lock().await;
            ctx.set::<ProductEntity>()
                .query()
                .order_by_desc_column("created_at")
        };
        let products = query
            .to_list()
            .await
            .map_err(|e| Error::Internal(e.to_string()))?;
        Ok(products.into_iter().map(ProductModel::from).collect())
    }
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<GetProductRequest, ProductModel> for GetProductHandler {
    async fn handle(&self, req: GetProductRequest) -> Result<ProductModel> {
        let query = {
            let mut ctx = self.ctx.lock().await;
            ctx.set::<ProductEntity>()
                .query()
                .filter_column("id", "=", DbValue::String(req.id))
        };
        let product = query
            .first_or_default()
            .await
            .map_err(|e| Error::Internal(e.to_string()))?
            .ok_or_else(|| Error::NotFound("Product not found".into()))?;
        Ok(ProductModel::from(product))
    }
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<CreateProductRequest, ProductModel> for CreateProductHandler {
    async fn handle(&self, req: CreateProductRequest) -> Result<ProductModel> {
        let id = format!(
            "{:x}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        );
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs().to_string())
            .unwrap_or_else(|_| "0".to_string());
        let sql = format!(
            "INSERT INTO products (id, name, price, created_at) VALUES ('{}', '{}', {}, '{}')",
            id,
            crate::common::escape_sql(&req.name),
            req.price,
            now
        );
        {
            let ctx = self.ctx.lock().await;
            ctx.provider()
                .execute_migration_command(&sql)
                .await
                .map_err(|e| Error::Internal(format!("Failed to create product: {}", e)))?;
        }
        tracing::info!("[Product] Created: {}", req.name);
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
        let query = {
            let mut ctx = self.ctx.lock().await;
            ctx.set::<ProductEntity>().query().filter_column(
                "id",
                "=",
                DbValue::String(req.id.clone()),
            )
        };
        let existing = query
            .first_or_default()
            .await
            .map_err(|e| Error::Internal(e.to_string()))?
            .ok_or_else(|| Error::NotFound("Product not found".into()))?;

        let new_name = req.name.unwrap_or(existing.name);
        let new_price = req.price.unwrap_or(existing.price);
        let sql = format!(
            "UPDATE products SET name='{}', price={} WHERE id='{}'",
            crate::common::escape_sql(&new_name),
            new_price,
            crate::common::escape_sql(&req.id)
        );
        {
            let ctx = self.ctx.lock().await;
            ctx.provider()
                .execute_migration_command(&sql)
                .await
                .map_err(|e| Error::Internal(format!("Failed to update product: {}", e)))?;
        }
        tracing::info!("[Product] Updated: {} ({})", new_name, req.id);
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
        {
            let mut ctx = self.ctx.lock().await;
            let query = ctx.set::<ProductEntity>().query().filter_column(
                "id",
                "=",
                DbValue::String(req.id.clone()),
            );
            drop(ctx);
            let exists = query
                .first_or_default()
                .await
                .map_err(|e| Error::Internal(e.to_string()))?;
            if exists.is_none() {
                return Err(Error::NotFound("Product not found".into()));
            }
        }
        let sql = format!(
            "DELETE FROM products WHERE id='{}'",
            crate::common::escape_sql(&req.id)
        );
        {
            let ctx = self.ctx.lock().await;
            ctx.provider()
                .execute_migration_command(&sql)
                .await
                .map_err(|e| Error::Internal(format!("Failed to delete product: {}", e)))?;
        }
        tracing::info!("[Product] Deleted: {}", req.id);
        Ok(format!("Deleted product {}", req.id))
    }
}
