//! Portfolio handlers — read works from `docs/{slug}/INDEX.json` via DocService.

use std::sync::Arc;

use rust_webapp::*;

use crate::contracts::docs::IDocumentService;
use crate::contracts::work::*;

#[rust_dicore::inject_attr(singleton, as = dyn IRequestHandler<ListWorksRequest, Vec<WorkModel>>)]
pub struct ListWorksHandler {
    docs: Arc<dyn IDocumentService>,
}

#[rust_dicore::inject_attr(singleton, as = dyn IRequestHandler<GetWorkRequest, WorkModel>)]
pub struct GetWorkHandler {
    docs: Arc<dyn IDocumentService>,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<ListWorksRequest, Vec<WorkModel>> for ListWorksHandler {
    async fn handle(&self, _req: ListWorksRequest) -> Result<Vec<WorkModel>> {
        self.docs
            .list_portfolio()
            .map_err(|e| Error::Internal(e))
    }
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<GetWorkRequest, WorkModel> for GetWorkHandler {
    async fn handle(&self, req: GetWorkRequest) -> Result<WorkModel> {
        self.docs
            .get_portfolio(&req.slug)
            .map_err(|e| Error::NotFound(e))
    }
}
