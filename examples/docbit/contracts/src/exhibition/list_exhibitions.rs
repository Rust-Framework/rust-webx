//! `GET /api/exhibitions` — every portfolio work, in display order.

use serde::Deserialize;
use webx::*;

use super::models::ExhibitionModel;

#[derive(Default, Deserialize, WebxRequestMeta)]
pub struct ListExhibitionsRequest;

#[get("/api/exhibitions")]
impl IRequest<Vec<ExhibitionModel>> for ListExhibitionsRequest {}
