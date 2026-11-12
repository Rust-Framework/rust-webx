//! The `IActionResult` family — results that write the HTTP response themselves.
//!
//! A normal endpoint returns a serializable value: the framework serializes it
//! as JSON and answers `200`. When an action must control the response — a file
//! download, a non-JSON body, a different status code, extra headers — it
//! returns an **action result** instead, mirroring ASP.NET Core:
//!
//! ```ignore
//! #[get("/reports/{id}")]
//! impl IRequest<Response> for DownloadReportRequest {}
//!
//! impl IRequestHandler<DownloadReportRequest, Response> for DownloadReportHandler {
//!     async fn handle(&mut self, req: DownloadReportRequest) -> ActionResult<Response> {
//!         Ok(Box::new(
//!             FileResult::path(self.reports.path_for(&req.id)?)
//!                 .content_type("text/csv")
//!                 .download_name("report.csv"),
//!         ))
//!     }
//! }
//! ```
//!
//! [`Response`] is the response type for such an action: `Box<dyn IActionResult>`
//! (Rust needs the box; ASP.NET Core writes `IActionResult` directly). A value
//! that needs no response control stays a plain DTO and never touches this
//! module.

use crate::error::Result;
use crate::http::{content_disposition_value, Disposition, FileBody, IHttpContext, ResponseBody};
use serde::Serialize;

/// ASP.NET Core's `IActionResult`: the action writes the response directly.
///
/// Implement this when none of the built-in results fit; return it boxed as
/// [`Response`]. The transport applies whatever the result sets — status,
/// headers, byte body or file body (the host then serves ranges, validators and
/// chunked streams for file bodies).
#[async_trait::async_trait]
pub trait IActionResult: Send {
    /// Write this result into the response.
    async fn execute(self: Box<Self>, ctx: &mut dyn IHttpContext) -> Result<()>;
}

/// The response type of an endpoint that controls the HTTP response itself:
/// ASP.NET Core's `IActionResult` return type.
pub type Response = Box<dyn IActionResult>;

/// `200 OK` + a JSON body — the result the framework uses for ordinary
/// `Serialize` DTO responses.
pub struct JsonResult<T> {
    /// The value written as `application/json`.
    pub value: T,
    /// HTTP status to use instead of `200`.
    pub status: u16,
}

impl<T> JsonResult<T> {
    /// A `200 OK` JSON result.
    pub fn new(value: T) -> Self {
        Self { value, status: 200 }
    }

    /// Override the HTTP status (e.g. `201` after a create).
    pub fn status(mut self, status: u16) -> Self {
        self.status = status;
        self
    }
}

#[async_trait::async_trait]
impl<T: Serialize + Send> IActionResult for JsonResult<T> {
    async fn execute(self: Box<Self>, ctx: &mut dyn IHttpContext) -> Result<()> {
        // Serialize before touching the response: the body must be a Send
        // buffer, and the DTO itself does not need to be `Sync` to be written.
        let json =
            serde_json::to_vec(&self.value).map_err(crate::error::Error::ResponseSerialization)?;
        let response = ctx.response_mut();
        response.set_status(self.status);
        response.set_header("content-type", "application/json");
        response.write_bytes(json).await
    }
}

/// A body sent as-is with an explicit media type — ASP.NET Core's
/// `ContentResult` (`Content(...)`, `Text(...)`, `Results.Bytes(...)`).
pub struct ContentResult {
    body: Vec<u8>,
    content_type: String,
    status: u16,
    headers: Vec<(String, String)>,
}

impl ContentResult {
    /// UTF-8 `text/plain; charset=utf-8`.
    pub fn text(body: impl Into<String>) -> Self {
        Self::bytes(body.into().into_bytes(), "text/plain; charset=utf-8")
    }

    /// UTF-8 `text/html; charset=utf-8`.
    pub fn html(body: impl Into<String>) -> Self {
        Self::bytes(body.into().into_bytes(), "text/html; charset=utf-8")
    }

    /// Raw bytes with an explicit media type.
    pub fn bytes(body: impl Into<Vec<u8>>, content_type: impl Into<String>) -> Self {
        Self {
            body: body.into(),
            content_type: content_type.into(),
            status: 200,
            headers: Vec::new(),
        }
    }

    /// Override the media type (e.g. `text/csv; charset=utf-8`).
    pub fn content_type(mut self, content_type: impl Into<String>) -> Self {
        self.content_type = content_type.into();
        self
    }

    /// Override the HTTP status.
    pub fn status(mut self, status: u16) -> Self {
        self.status = status;
        self
    }

    /// Add a response header.
    pub fn header(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((key.into(), value.into()));
        self
    }

    /// Send the body as a download named `file_name`
    /// (`Content-Disposition: attachment`).
    pub fn download_name(self, file_name: impl Into<String>) -> Self {
        self.with_disposition(Disposition::Attachment, file_name.into())
    }

    /// Send the body inline under `file_name`
    /// (`Content-Disposition: inline`).
    pub fn inline_name(self, file_name: impl Into<String>) -> Self {
        self.with_disposition(Disposition::Inline, file_name.into())
    }

    fn with_disposition(mut self, disposition: Disposition, file_name: String) -> Self {
        self.headers.push((
            "content-disposition".into(),
            content_disposition_value(disposition, &file_name),
        ));
        self
    }
}

#[async_trait::async_trait]
impl IActionResult for ContentResult {
    async fn execute(self: Box<Self>, ctx: &mut dyn IHttpContext) -> Result<()> {
        let response = ctx.response_mut();
        response.set_status(self.status);
        response.set_header("content-type", &self.content_type);
        for (key, value) in &self.headers {
            response.set_header(key, value);
        }
        response.write_bytes(self.body).await
    }
}

/// `204 No Content`.
pub struct NoContentResult;

#[async_trait::async_trait]
impl IActionResult for NoContentResult {
    async fn execute(self: Box<Self>, ctx: &mut dyn IHttpContext) -> Result<()> {
        ctx.response_mut().set_status(204);
        Ok(())
    }
}

/// A status code with no body (e.g. `202 Accepted`).
pub struct StatusCodeResult(pub u16);

#[async_trait::async_trait]
impl IActionResult for StatusCodeResult {
    async fn execute(self: Box<Self>, ctx: &mut dyn IHttpContext) -> Result<()> {
        ctx.response_mut().set_status(self.0);
        Ok(())
    }
}

/// A file response — ASP.NET Core's `PhysicalFile(...)` / `File(...)` /
/// `Results.File(...)`.
///
/// The host streams the source and fills in the HTTP metadata: `Content-Length`,
/// `ETag` / `Last-Modified` validators, `Accept-Ranges` and `Range` handling
/// (`206` / `416`), plus conditional requests (`304`). Files opened from a path
/// derive their validators from size and mtime; streams become conditional when
/// an [`entity_tag`](FileResult::entity_tag) or
/// [`last_modified`](FileResult::last_modified) is supplied.
pub struct FileResult {
    body: FileBody,
    status: u16,
    headers: Vec<(String, String)>,
}

impl FileResult {
    /// Send the file at `path` (ASP.NET Core's `PhysicalFile`).
    pub fn path(path: impl Into<std::path::PathBuf>) -> Self {
        Self::with_body(FileBody::path(path))
    }

    /// Send an async reader of unknown length (chunked, no `Range`).
    pub fn stream(reader: impl tokio::io::AsyncRead + Send + Unpin + 'static) -> Self {
        Self::with_body(FileBody::stream(reader))
    }

    /// Send an async reader of known length that supports seeking, so clients
    /// can resume and cache (`Content-Length`, `Range`).
    pub fn seekable_stream(
        reader: impl tokio::io::AsyncRead + tokio::io::AsyncSeek + Send + Unpin + 'static,
        len: u64,
    ) -> Self {
        Self::with_body(FileBody::seekable_stream(reader, len))
    }

    /// Send a fully-specified [`FileBody`].
    pub fn with_body(body: FileBody) -> Self {
        Self {
            body,
            status: 200,
            headers: Vec::new(),
        }
    }

    /// Override the media type instead of deriving it from the extension.
    pub fn content_type(mut self, content_type: impl Into<String>) -> Self {
        self.body = self.body.content_type(content_type);
        self
    }

    /// Send the file as a download named `file_name`.
    pub fn download_name(mut self, file_name: impl Into<String>) -> Self {
        self.body = self.body.download_name(file_name);
        self
    }

    /// Present the file inline under `file_name`.
    pub fn inline_name(mut self, file_name: impl Into<String>) -> Self {
        self.body = self.body.inline_name(file_name);
        self
    }

    /// Enable or disable `Range` processing for the file source.
    pub fn enable_range_processing(mut self, enabled: bool) -> Self {
        self.body = self.body.enable_range_processing(enabled);
        self
    }

    /// Advertise an explicit `Last-Modified` (required for a stream to support
    /// `If-Modified-Since`).
    pub fn last_modified(mut self, last_modified: std::time::SystemTime) -> Self {
        self.body = self.body.last_modified(last_modified);
        self
    }

    /// Advertise an explicit `ETag` (required for a stream to support
    /// `If-None-Match`).
    pub fn entity_tag(mut self, entity_tag: impl Into<String>) -> Self {
        self.body = self.body.entity_tag(entity_tag);
        self
    }

    /// Override the HTTP status.
    pub fn status(mut self, status: u16) -> Self {
        self.status = status;
        self
    }

    /// Add a response header.
    pub fn header(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((key.into(), value.into()));
        self
    }
}

#[async_trait::async_trait]
impl IActionResult for FileResult {
    async fn execute(self: Box<Self>, ctx: &mut dyn IHttpContext) -> Result<()> {
        let response = ctx.response_mut();
        response.set_status(self.status);
        for (key, value) in &self.headers {
            response.set_header(key, value);
        }
        response.write_body(ResponseBody::File(self.body)).await
    }
}
