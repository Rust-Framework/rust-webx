//! `GET /api/tracking/summary` — visit counters (admin).

use serde::Deserialize;
use webx::*;

use super::models::TrackingSummary;

#[derive(Default, Deserialize, WebxRequestMeta)]
pub struct GetTrackingSummaryRequest;

#[get("/api/tracking/summary")]
#[authorize(role = "admin")]
impl IRequest<TrackingSummary> for GetTrackingSummaryRequest {}
