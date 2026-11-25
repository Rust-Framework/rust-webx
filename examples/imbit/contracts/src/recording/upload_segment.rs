//! `POST /api/recordings/{recording_id}/parts/{part}` — upload one segment.
//!
//! Uploading the same `part` again is the resume path: the server replaces its
//! index entry instead of appending a duplicate.

use serde::Deserialize;
use webx::*;

use super::model::RecordingModel;

#[derive(Default, Deserialize, WebxRequestMeta)]
pub struct UploadSegmentRequest {
    #[fromclaim]
    pub user: IdentityUser,
    #[frompath]
    pub recording_id: String,
    /// 1-based part number.
    #[frompath]
    pub part: u32,
    /// Segment bytes, base64 encoded. Reference transport only: production
    /// uploads multipart or pre-signed direct-to-store.
    pub data_base64: String,
    pub duration_ms: u64,
    /// Segment start (unix seconds); defaults to the server clock.
    pub started_at_unix: Option<u64>,
}

#[post("/api/recordings/{recording_id}/parts/{part}")]
impl IRequest<RecordingModel> for UploadSegmentRequest {}
