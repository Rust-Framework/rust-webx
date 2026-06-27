//! Documentation API handlers — read-only over `IDocumentService`.
//!
//! `IDocumentService` 由 host crate 实现（需要文件系统访问与 `AppPaths`），
//! 此处仅注入 `Arc<dyn IDocumentService>` 并提供 HTTP 包装。

use std::sync::Arc;

use rust_webapp::*;

use docbit_contracts::docs::{
    DocContent, DocIndex, GetDocContentRequest, GetDocIndexRequest, IDocumentService,
    ListDocWorksRequest,
};

#[rust_dicore::inject_attr(singleton, as = dyn IRequestHandler<ListDocWorksRequest, Vec<String>>)]
pub struct ListDocWorksHandler {
    docs: Arc<dyn IDocumentService>,
}

#[rust_dicore::inject_attr(singleton, as = dyn IRequestHandler<GetDocIndexRequest, DocIndex>)]
pub struct GetDocIndexHandler {
    docs: Arc<dyn IDocumentService>,
}

#[rust_dicore::inject_attr(singleton, as = dyn IRequestHandler<GetDocContentRequest, DocContent>)]
pub struct GetDocContentHandler {
    docs: Arc<dyn IDocumentService>,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<ListDocWorksRequest, Vec<String>> for ListDocWorksHandler {
    async fn handle(&self, _: ListDocWorksRequest) -> Result<Vec<String>> {
        self.docs.list_works().map_err(Error::Internal)
    }
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<GetDocIndexRequest, DocIndex> for GetDocIndexHandler {
    async fn handle(&self, req: GetDocIndexRequest) -> Result<DocIndex> {
        self.docs.index(&req.work).map_err(Error::NotFound)
    }
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<GetDocContentRequest, DocContent> for GetDocContentHandler {
    async fn handle(&self, req: GetDocContentRequest) -> Result<DocContent> {
        // 路径编码约定：`/` 在 URL 路径段中以 `:` 替代，这里还原。
        let path = percent_decode(&req.path).replace(':', "/");
        self.docs.content(&req.work, &path).map_err(Error::NotFound)
    }
}

/// 简易 percent-decoding：把 `%XX` 与 `+` 还原为原始字符。
fn percent_decode(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '%' {
            let hex: String = chars.by_ref().take(2).collect();
            if let Ok(byte) = u8::from_str_radix(&hex, 16) {
                result.push(byte as char);
            } else {
                result.push('%');
            }
        } else if c == '+' {
            result.push(' ');
        } else {
            result.push(c);
        }
    }
    result
}
