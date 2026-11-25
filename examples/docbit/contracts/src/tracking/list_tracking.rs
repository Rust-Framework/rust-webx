//! `GET /api/tracking` — the visit log, newest first (admin).

use serde::Deserialize;
use webx::*;

use super::models::TrackingModel;

#[derive(Default, Deserialize, WebxRequestMeta)]
pub struct ListTrackingRequest;

#[get("/api/tracking")]
#[authorize(role = "admin")]
impl IRequest<Vec<TrackingModel>> for ListTrackingRequest {}
