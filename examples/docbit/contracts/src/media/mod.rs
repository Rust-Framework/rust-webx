//! Media context — image/attachment upload and public serving.
//!
//! Files live under `<app_base>/uploads/` (writable, beside the executable, and
//! deliberately **outside** the embedded `wwwroot`) and are served through
//! `/api/media/...`, so the SPA middleware only ever serves the frontend.
//!
//! Layout: `uploads/{images|files}/{shard}/{id}.{ext}`, sharded two hex
//! characters deep so no single directory grows unbounded.

pub mod get_media;
pub mod models;
pub mod upload_media;

pub use get_media::GetMediaRequest;
pub use models::MediaAsset;
pub use upload_media::UploadMediaRequest;
