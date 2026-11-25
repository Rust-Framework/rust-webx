//! `GET /api/exhibitions/{slug}` — one portfolio work.

use serde::Deserialize;
use webx::*;

use super::models::ExhibitionModel;

#[derive(Default, Deserialize, WebxRequestMeta)]
pub struct GetExhibitionRequest {
    #[frompath]
    pub slug: String,
}

#[get("/api/exhibitions/{slug}")]
impl IRequest<ExhibitionModel> for GetExhibitionRequest {}
