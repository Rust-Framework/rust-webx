//! `DELETE /api/exhibitions/{slug}` — soft delete (admin).

use serde::Deserialize;
use webx::*;

#[derive(Default, Deserialize, WebxRequestMeta)]
pub struct DeleteExhibitionRequest {
    pub slug: String,
}

#[delete("/api/exhibitions/{slug}")]
#[authorize(role = "admin")]
impl IRequest<String> for DeleteExhibitionRequest {}
