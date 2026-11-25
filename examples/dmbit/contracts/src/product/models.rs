//! Product read model.

use serde::{Deserialize, Serialize};

use crate::goods::models::GoodsModel;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProductModel {
    pub id: String,
    pub name: String,
    pub code: String,
    /// compute | storage
    pub category: String,
    pub remark: String,
    pub sort_order: i32,
    pub created_at: i64,
    pub updated_at: i64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub goods: Vec<GoodsModel>,
}
