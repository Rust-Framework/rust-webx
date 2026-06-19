use lref::prelude::*;
use serde::{Deserialize, Serialize};

/// Product database entity.
#[derive(EntityType, Clone, Serialize, Deserialize, Debug)]
#[table("products")]
pub struct ProductEntity {
    #[primary_key]
    pub id: String,
    #[max_length(200)]
    pub name: String,
    pub price: f64,
    pub created_at: String,
}

/// DTO returned to API clients.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProductModel {
    pub id: String,
    pub name: String,
    pub price: f64,
    pub created_at: String,
}

impl From<ProductEntity> for ProductModel {
    fn from(e: ProductEntity) -> Self {
        Self {
            id: e.id,
            name: e.name,
            price: e.price,
            created_at: e.created_at,
        }
    }
}
