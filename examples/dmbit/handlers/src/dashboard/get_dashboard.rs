//! `GET /api/dashboard` — the 智算机房 big-screen payload.

use std::sync::Arc;

use rust_ef::db_context::DbContext;
use webx::*;

use dmbit_contracts::dashboard::{DashboardModel, GetDashboardRequest};
use dmbit_contracts::site::SiteConfig;

use super::use_cases::build_dashboard;

#[derive(Inject)]
pub struct GetDashboardHandler {
    #[inject(owned)]
    ctx: DbContext,
    #[inject]
    site: Arc<SiteConfig>,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<GetDashboardRequest, DashboardModel> for GetDashboardHandler {
    async fn handle(&mut self, _: GetDashboardRequest) -> ActionResult<DashboardModel> {
        build_dashboard(&mut self.ctx, &self.site).await
    }
}
