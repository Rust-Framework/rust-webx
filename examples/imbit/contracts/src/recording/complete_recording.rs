//! `POST /api/recordings/{recording_id}/complete` — finish and publish.

use serde::Deserialize;
use webx::*;

use super::model::RecordingModel;

#[derive(Default, Deserialize, WebxRequestMeta)]
pub struct CompleteRecordingRequest {
    #[fromclaim]
    pub user: IdentityUser,
    #[frompath]
    pub recording_id: String,
}

#[post("/api/recordings/{recording_id}/complete")]
impl IRequest<RecordingModel> for CompleteRecordingRequest {}
