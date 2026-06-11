use lref::provider::DbValue;
use lref::query::QueryBuilder;
use lrwf::*;

use crate::contracts::product::*;
use crate::domain::product::{ProductEntity, ProductModel};

// ── IRequestHandler implementations — backed by lref ORM (SQLite) ──

#[derive(Default)]
pub struct ListProductsHandler;

#[derive(Default)]
pub struct GetProductHandler;

#[derive(Default)]
pub struct CreateProductHandler;

#[derive(Default)]
pub struct UpdateProductHandler;

#[derive(Default)]
pub struct DeleteProductHandler;

fn qb() -> QueryBuilder<ProductEntity> {
    QueryBuilder::<ProductEntity>::with_provider(
        "products",
        crate::handlers::startup::provider_dyn(),
    )
}

#[handler]
#[async_trait]
impl IRequestHandler<ListProductsRequest, Vec<ProductModel>> for ListProductsHandler {
    async fn handle(&self, _: ListProductsRequest) -> Result<Vec<ProductModel>> {
        let entities = qb()
            .to_list()
            .await
            .map_err(|e| Error::Internal(e.to_string()))?;
        Ok(entities.into_iter().map(ProductModel::from).collect())
    }
}

#[handler]
#[async_trait]
impl IRequestHandler<GetProductRequest, ProductModel> for GetProductHandler {
    async fn handle(&self, req: GetProductRequest) -> Result<ProductModel> {
        let entity = qb()
            .filter_column("id", "=", DbValue::String(req.id.clone()))
            .first_or_default()
            .await
            .map_err(|e| Error::Internal(e.to_string()))?
            .ok_or_else(|| Error::NotFound(format!("Product not found: {}", req.id)))?;
        Ok(ProductModel::from(entity))
    }
}

#[handler]
#[async_trait]
impl IRequestHandler<CreateProductRequest, ProductModel> for CreateProductHandler {
    async fn handle(&self, req: CreateProductRequest) -> Result<ProductModel> {
        let id = uuid();
        let now = now_secs();
        let sql = format!(
            "INSERT INTO products (id, name, price, created_at) VALUES ('{}', '{}', {}, '{}')",
            id, req.name.replace('\'', "''"), req.price, now
        );
        crate::handlers::startup::exec(&sql).await
            .map_err(|e| Error::Internal(format!("Failed to create product: {}", e)))?;
        let model = ProductModel { id, name: req.name, price: req.price, created_at: now };
        tracing::info!("Product created: {} (id: {})", model.name, model.id);
        Ok(model)
    }
}

#[handler]
#[async_trait]
impl IRequestHandler<UpdateProductRequest, ProductModel> for UpdateProductHandler {
    async fn handle(&self, req: UpdateProductRequest) -> Result<ProductModel> {
        let existing = qb()
            .filter_column("id", "=", DbValue::String(req.id.clone()))
            .first_or_default()
            .await
            .map_err(|e| Error::Internal(e.to_string()))?
            .ok_or_else(|| Error::NotFound(format!("Product not found: {}", req.id)))?;

        let new_name = req.name.clone().unwrap_or(existing.name.clone());
        let new_price = req.price.unwrap_or(existing.price);

        let mut set_parts: Vec<String> = Vec::new();
        if req.name.is_some() {
            set_parts.push(format!("name = '{}'", new_name.replace('\'', "''")));
        }
        if req.price.is_some() {
            set_parts.push(format!("price = {}", new_price));
        }
        if set_parts.is_empty() {
            return Ok(ProductModel {
                id: existing.id.clone(),
                name: existing.name.clone(),
                price: existing.price,
                created_at: existing.created_at,
            });
        }
        let sql = format!(
            "UPDATE products SET {} WHERE id = '{}'",
            set_parts.join(", "),
            req.id
        );
        crate::handlers::startup::exec(&sql).await
            .map_err(|e| Error::Internal(format!("Failed to update product: {}", e)))?;
        Ok(ProductModel {
            id: req.id,
            name: new_name,
            price: new_price,
            created_at: existing.created_at,
        })
    }
}

#[handler]
#[async_trait]
impl IRequestHandler<DeleteProductRequest, String> for DeleteProductHandler {
    async fn handle(&self, req: DeleteProductRequest) -> Result<String> {
        let sql = format!("DELETE FROM products WHERE id = '{}'", req.id);
        crate::handlers::startup::exec(&sql).await
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
