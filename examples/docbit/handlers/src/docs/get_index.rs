//! `GET /api/docs/{work}/index` — one work's navigation index.

use std::sync::Arc;

use webx::*;

use docbit_contracts::docs::{DocIndex, GetDocIndexRequest, IDocumentService};

#[derive(Inject)]
pub struct GetDocIndexHandler {
    #[inject]
    docs: Arc<dyn IDocumentService>,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<GetDocIndexRequest, DocIndex> for GetDocIndexHandler {
    async fn handle(&mut self, req: GetDocIndexRequest) -> ActionResult<DocIndex> {
        self.docs.index(&req.work).map_err(Error::not_found)
    }
}
