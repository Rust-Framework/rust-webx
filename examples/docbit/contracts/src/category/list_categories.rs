//! `GET /api/categories` — the category forest.

use serde::Deserialize;
use webx::*;

use super::models::CategoryTreeNode;

#[derive(Default, Deserialize, WebxRequestMeta)]
pub struct ListCategoriesRequest;

#[get("/api/categories")]
impl IRequest<Vec<CategoryTreeNode>> for ListCategoriesRequest {}
