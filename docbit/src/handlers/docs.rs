//! Documentation API handlers.

use std::sync::Arc;

use rust_webapp::*;

use crate::contracts::docs::*;
use crate::services::docs::{DocContent, DocIndex, DocService};

#[rust_dicore::inject_attr(singleton, as = dyn IRequestHandler<ListDocWorksRequest, Vec<String>>)]
pub struct ListDocWorksHandler {
    docs: Arc<DocService>,
}

#[rust_dicore::inject_attr(singleton, as = dyn IRequestHandler<GetDocIndexRequest, DocIndex>)]
pub struct GetDocIndexHandler {
    docs: Arc<DocService>,
}

#[rust_dicore::inject_attr(singleton, as = dyn IRequestHandler<GetDocContentRequest, DocContent>)]
pub struct GetDocContentHandler {
    docs: Arc<DocService>,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<ListDocWorksRequest, Vec<String>> for ListDocWorksHandler {
    async fn handle(&self, _req: ListDocWorksRequest) -> Result<Vec<String>> {
        self.docs
            .list_works()
            .map_err(|e| Error::Internal(e))
    }
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<GetDocIndexRequest, DocIndex> for GetDocIndexHandler {
    async fn handle(&self, req: GetDocIndexRequest) -> Result<DocIndex> {
        self.docs
            .index(&req.work)
            .map_err(|e| Error::NotFound(e))
    }
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<GetDocContentRequest, DocContent> for GetDocContentHandler {
    async fn handle(&self, req: GetDocContentRequest) -> Result<DocContent> {
        let path = req.path.replace(':', "/");
        self.docs
            .content(&req.work, &path)
            .map_err(|e| Error::NotFound(e))
    }
}
