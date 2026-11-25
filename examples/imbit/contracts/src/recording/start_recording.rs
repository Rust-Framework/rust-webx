//! `POST /api/recordings` — start the control plane for a room recording.

use serde::Deserialize;
use webx::*;

use super::model::RecordingModel;

#[derive(Default, Deserialize, WebxRequestMeta)]
pub struct StartRecordingRequest {
    #[fromclaim]
    pub user: IdentityUser,
    /// Room to record; the caller must be a member.
    pub room_id: String,
    /// Media type of the segments (`audio/ogg` when absent).
    pub content_type: Option<String>,
}

#[post("/api/recordings")]
impl IRequest<RecordingModel> for StartRecordingRequest {}
