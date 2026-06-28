//! Tracking contracts — site visit statistics.

use rust_webapp::*;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrackingModel {
    pub id: i32,
    pub path: String,
    pub method: String,
    pub ip: String,
    pub user_agent: String,
    pub referer: Option<String>,
    pub status: i32,
    pub duration_ms: i32,
    pub visited_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrackingSummary {
    pub total_visits: i64,
    pub unique_paths: i64,
    pub today_visits: i64,
}

#[derive(Default)]
pub struct GetTrackingSummaryRequest;

#[get("/api/tracking/summary")]
#[authorize(role = "admin")]
impl IRequest<TrackingSummary> for GetTrackingSummaryRequest {}

#[derive(Default)]
pub struct ListTrackingRequest;

#[get("/api/tracking")]
#[authorize(role = "admin")]
impl IRequest<Vec<TrackingModel>> for ListTrackingRequest {}
