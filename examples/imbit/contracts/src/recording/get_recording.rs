//! `GET /api/recordings/{recording_id}` — metadata + playback index.

use serde::Deserialize;
use webx::*;

use super::model::RecordingModel;

#[derive(Default, Deserialize, WebxRequestMeta)]
pub struct GetRecordingRequest {
    #[fromclaim]
    pub user: IdentityUser,
    #[frompath]
    pub recording_id: String,
}

#[get("/api/recordings/{recording_id}")]
impl IRequest<RecordingModel> for GetRecordingRequest {}
