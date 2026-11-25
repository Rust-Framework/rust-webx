//! `GET /api/docs` — the documentation works served from disk.

use std::sync::Arc;

use webx::*;

use docbit_contracts::docs::{IDocumentService, ListDocWorksRequest};

use crate::common::MAX_LIST_ROWS;

#[derive(Inject)]
pub struct ListDocWorksHandler {
    #[inject]
    docs: Arc<dyn IDocumentService>,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<ListDocWorksRequest, Vec<String>> for ListDocWorksHandler {
    async fn handle(&mut self, _: ListDocWorksRequest) -> ActionResult<Vec<String>> {
        let mut works = self.docs.list_works().map_err(Error::internal)?;
        works.truncate(MAX_LIST_ROWS);
        Ok(works)
    }
}
