//! Tracking entity — site visit statistics.

use rust_ef::prelude::*;

#[derive(Debug, Clone, EntityType)]
#[table("tracking")]
pub struct Tracking {
    #[primary_key]
    #[auto_increment]
    pub id: i32,
    #[required]
    #[max_length(500)]
    #[index]
    pub path: String,
    #[required]
    #[max_length(10)]
    pub method: String,
    #[max_length(50)]
    pub ip: String,
    #[max_length(500)]
    pub user_agent: String,
    #[max_length(500)]
    pub referer: Option<String>,
    #[required]
    pub status: i32,
    #[required]
    pub duration_ms: i32,
    #[required]
    #[index]
    pub visited_at: i64,
}
