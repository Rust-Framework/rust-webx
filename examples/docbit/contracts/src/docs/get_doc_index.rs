//! `GET /api/docs/{work}/index` — one work's navigation index.

use serde::Deserialize;
use webx::*;

use super::models::DocIndex;

#[derive(Default, Deserialize, WebxRequestMeta)]
pub struct GetDocIndexRequest {
    #[frompath]
    pub work: String,
}

#[get("/api/docs/{work}/index")]
impl IRequest<DocIndex> for GetDocIndexRequest {}
