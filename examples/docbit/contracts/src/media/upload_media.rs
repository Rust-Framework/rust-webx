//! `POST /api/media` — upload one image or attachment.

use webx::*;

use super::models::MediaAsset;

/// Upload one media file. Images and attachments are told apart by
/// `Content-Type`, so the client sends a single plain file part.
#[derive(WebxRequestMeta)]
pub struct UploadMediaRequest {
    pub file: FormFile,
}

#[post("/api/media")]
#[authorize]
impl IRequest<MediaAsset> for UploadMediaRequest {}
