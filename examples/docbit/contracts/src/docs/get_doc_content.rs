//! `GET /api/docs/{work}/content/{*path}` — one markdown page.

use serde::Deserialize;
use webx::*;

use super::models::DocContent;

#[derive(Default, Deserialize, WebxRequestMeta)]
pub struct GetDocContentRequest {
    #[frompath]
    pub work: String,
    /// Remaining document path (slashes allowed via `{*path}`).
    /// Legacy colon encoding (`01-introduction:hello.md`) is still accepted.
    #[frompath]
    pub path: String,
}

#[get("/api/docs/{work}/content/{*path}")]
impl IRequest<DocContent> for GetDocContentRequest {}
