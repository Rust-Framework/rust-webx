//! `GET /api/docs/{work}/content/{*path}` — one markdown page.

use std::sync::Arc;

use webx::*;

use docbit_contracts::docs::{DocContent, GetDocContentRequest, IDocumentService};

#[derive(Inject)]
pub struct GetDocContentHandler {
    #[inject]
    docs: Arc<dyn IDocumentService>,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<GetDocContentRequest, DocContent> for GetDocContentHandler {
    async fn handle(&mut self, req: GetDocContentRequest) -> ActionResult<DocContent> {
        // `{*path}` captures real slashes; legacy clients used `:` as `/`.
        let path = percent_decode(&req.path).replace(':', "/");

        self.docs
            .content(&req.work, &path)
            .map_err(Error::not_found)
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
