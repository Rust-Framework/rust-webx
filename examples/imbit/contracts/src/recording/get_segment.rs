//! `GET /api/recordings/{recording_id}/parts/{part}` — play one segment.
//!
//! Playback returns the stored bytes, not JSON, so the endpoint uses the
//! self-controlled response type (`IRequest<Response>`) and the handler returns
//! a `ContentResult` with the recording's media type.

use serde::Deserialize;
use webx::*;

#[derive(Default, Deserialize, WebxRequestMeta)]
pub struct GetSegmentRequest {
    #[fromclaim]
    pub user: IdentityUser,
    #[frompath]
    pub recording_id: String,
    #[frompath]
    pub part: u32,
}

#[get("/api/recordings/{recording_id}/parts/{part}")]
impl IRequest<Response> for GetSegmentRequest {}
