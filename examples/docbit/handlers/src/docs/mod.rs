//! Docs context — reading documentation over HTTP plus the bundle upload that
//! installs a new work's `docs/` tree.
//!
//! `IDocumentService` is implemented in [`crate::doc_service`] (it needs the
//! filesystem and the app base path); these handlers only inject the port.

pub mod extract;
pub mod get_content;
pub mod get_index;
pub mod list_works;
pub mod upload_work_docs;

pub use get_content::GetDocContentHandler;
pub use get_index::GetDocIndexHandler;
pub use list_works::ListDocWorksHandler;
pub use upload_work_docs::UploadWorkDocsHandler;
