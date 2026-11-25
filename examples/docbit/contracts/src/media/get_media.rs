//! `GET /api/media/{*path}` — serve a stored file back.

use serde::Deserialize;
use webx::*;

/// Serve a stored media file by its path relative to `uploads/`.
#[derive(Default, Deserialize, WebxRequestMeta)]
pub struct GetMediaRequest {
    #[frompath]
    pub path: String,
}

#[get("/api/media/{*path}")]
impl IRequest<Response> for GetMediaRequest {}
