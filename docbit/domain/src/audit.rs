//! Audit field helpers — operator id from HTTP [`RequestContext`].

/// Current request operator id (JWT `sub`), set by the HTTP dispatch pipeline.
pub fn operator_id() -> Option<String> {
    rust_webx_core::request_context::RequestContext::operator_id()
}
