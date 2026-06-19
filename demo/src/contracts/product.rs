//! Product controller â€” demonstrates a controller-style module with doc comments.

use crate::domain::product::ProductModel;
use rust_webapp::*;

// â”€â”€ IRequest definitions â”€â”€

pub struct ListProductsRequest;

/// Returns all products in the catalog
#[get("/api/products")]
impl IRequest<Vec<ProductModel>> for ListProductsRequest {}

pub struct GetProductRequest {
    pub id: String,
}

/// Get a single product by its unique ID
/// Supports both numeric IDs and slug-based lookups
#[get("/api/products/{id}")]
impl IRequest<ProductModel> for GetProductRequest {}

#[derive(serde::Deserialize)]
pub struct CreateProductRequest {
    pub name: String,
    pub price: f64,
}

/// Add a new product to the catalog
/// Requires name and price in the request body
#[post("/api/products")]
#[authorize(role = "admin")]
impl IRequest<ProductModel> for CreateProductRequest {}

#[derive(serde::Deserialize)]
pub struct UpdateProductRequest {
    pub id: String,
    pub name: Option<String>,
    pub price: Option<f64>,
}

/// Update an existing product's name or price
/// Only provided fields will be changed; omitted fields are left unchanged
#[put("/api/products/{id}")]
#[authorize(role = "admin")]
impl IRequest<ProductModel> for UpdateProductRequest {}

pub struct DeleteProductRequest {
    pub id: String,
}

/// Remove a product from the catalog permanently
#[delete("/api/products/{id}")]
#[authorize(role = "admin")]
impl IRequest<String> for DeleteProductRequest {}
