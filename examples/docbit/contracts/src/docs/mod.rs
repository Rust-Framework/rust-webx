//! Docs context — filesystem-backed documentation reading plus the bundled
//! upload that installs a new work's `docs/` tree.

pub mod get_doc_content;
pub mod get_doc_index;
pub mod list_doc_works;
pub mod models;
pub mod service;
pub mod upload_work_docs;

pub use get_doc_content::GetDocContentRequest;
pub use get_doc_index::GetDocIndexRequest;
pub use list_doc_works::ListDocWorksRequest;
pub use models::{DocContent, DocIndex, DocIndexItem, ResyncSummary};
pub use service::IDocumentService;
pub use upload_work_docs::{UploadWorkDocsRequest, UploadWorkDocsResponse};
