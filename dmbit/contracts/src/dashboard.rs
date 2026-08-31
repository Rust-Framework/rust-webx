//! Dashboard — 智算机房台账.
//!
//! Aggregates are derived only from persisted product / goods / component rows.

use rust_webx::*;
use serde::{Deserialize, Serialize};

use crate::product::ProductModel;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatusBucket {
    pub status: String,
    pub quantity: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PartTotal {
    pub kind: String,
    pub model: String,
    pub capacity: String,
    pub label: String,
    pub count: i32,
    pub unit: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceOverviewRow {
    pub id: String,
    pub product_id: String,
    pub product_name: String,
    pub product_category: String,
    pub brand: String,
    pub parts_summary: String,
    pub quantity: i32,
    pub unit: String,
    pub status: String,
    pub location: String,
    pub asset_code: String,
    pub parameters: String,
    pub sort_order: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DashboardStats {
    pub product_count: i32,
    pub goods_count: i32,
    pub total_quantity: i32,
    pub compute_quantity: i32,
    pub storage_quantity: i32,
    /// Σ(disk block count × capacity_gb). Integer GB; no string parsing when aggregating.
    pub storage_capacity_gb: i64,
    pub running_quantity: i32,
    pub commissioning_quantity: i32,
    pub pending_quantity: i32,
    pub delivered_quantity: i32,
    pub status_buckets: Vec<StatusBucket>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DashboardModel {
    pub title: String,
    pub brand_name: String,
    pub room_name: String,
    pub stats: DashboardStats,
    pub accelerator_totals: Vec<PartTotal>,
    pub disk_totals: Vec<PartTotal>,
    pub products: Vec<ProductModel>,
    pub devices: Vec<DeviceOverviewRow>,
}

#[derive(Default, Deserialize)]
pub struct GetDashboardRequest;

#[get("/api/dashboard")]
impl IRequest<DashboardModel> for GetDashboardRequest {}
