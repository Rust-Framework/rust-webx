//! Tracking read models — one visit row and the aggregated summary.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrackingModel {
    pub id: String,
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
