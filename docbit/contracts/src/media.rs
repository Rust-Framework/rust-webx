//! Blog media — image/attachment upload and public serving.
//!
//! Files live under `<app_base>/uploads/` (writable, beside the executable, and
//! deliberately **outside** the embedded `wwwroot`). They are served through
//! `/api/media/...` so the SPA middleware keeps serving only the frontend and
//! never has to special-case media paths.
//!
//! Layout: `uploads/{images|files}/{shard}/{id}.{ext}`, sharded two hex
//! characters deep so no single directory grows unbounded.

use serde::{Deserialize, Serialize};
use webx::*;

/// Upload one media file. Images and attachments are told apart by
/// `Content-Type`, so the client sends a single plain file part.
#[derive(Deserialize)]
pub struct UploadMediaRequest {
    pub file: FormFile,
}

#[post("/api/media")]
#[authorize]
impl IRequest<MediaAsset> for UploadMediaRequest {}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaAsset {
    /// Public URL to reference from content, e.g. `/api/media/images/ab/ab12….png`.
    pub url: String,
    /// Sanitised original file name.
    pub name: String,
    pub content_type: String,
    pub size: u64,
    /// Markdown ready to paste: an image tag for images, a link otherwise.
    pub markdown: String,
}

/// Serve a stored media file by its path relative to `uploads/`.
#[derive(Default, Deserialize, WebxRequestMeta)]
pub struct GetMediaRequest {
    #[from_route]
    pub path: String,
}

#[get("/api/media/{*path}")]
impl IRequest<ResponseData> for GetMediaRequest {}
