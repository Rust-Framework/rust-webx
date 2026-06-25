use crate::services::docs::{DocContent, DocIndex};
use rust_webapp::*;

pub struct ListDocWorksRequest;

#[get("/api/docs")]
impl IRequest<Vec<String>> for ListDocWorksRequest {}

pub struct GetDocIndexRequest {
    pub work: String,
}

#[get("/api/docs/{work}/index")]
impl IRequest<DocIndex> for GetDocIndexRequest {}

pub struct GetDocContentRequest {
    pub work: String,
    /// Document path with `/` encoded as `:` (e.g. `01-introduction:hello-world.md`)
    pub path: String,
}

#[get("/api/docs/{work}/content/{path}")]
impl IRequest<DocContent> for GetDocContentRequest {}
