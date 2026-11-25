//! `GET /api/inventory/export` — download the inventory as CSV.

use serde::Deserialize;
use webx::*;

#[derive(Default, Deserialize, WebxRequestMeta)]
pub struct ExportInventoryRequest;

/// A self-controlled response: the handler returns a real `text/csv` body with a
/// `Content-Disposition` filename (`ContentResult`), not JSON.
#[get("/api/inventory/export")]
#[authorize(role = "admin")]
impl IRequest<Response> for ExportInventoryRequest {}
