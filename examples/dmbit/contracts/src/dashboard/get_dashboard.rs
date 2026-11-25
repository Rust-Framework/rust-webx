//! `GET /api/dashboard` — the 智算机房 big-screen payload.

use serde::Deserialize;
use webx::*;

use super::models::DashboardModel;

#[derive(Default, Deserialize, WebxRequestMeta)]
pub struct GetDashboardRequest;

#[get("/api/dashboard")]
impl IRequest<DashboardModel> for GetDashboardRequest {}
