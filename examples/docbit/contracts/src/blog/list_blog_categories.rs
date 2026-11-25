//! `GET /api/blog/categories` — categories with their post counts.

use serde::Deserialize;
use webx::*;

use super::models::BlogCategoryCount;

#[derive(Default, Deserialize, WebxRequestMeta)]
pub struct ListBlogCategoriesRequest;

#[get("/api/blog/categories")]
impl IRequest<Vec<BlogCategoryCount>> for ListBlogCategoriesRequest {}
