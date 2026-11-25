//! `GET /api/recordings/{recording_id}` — metadata plus the playback index.

use std::sync::Arc;

use webx::*;

use imbit_contracts::recording::{GetRecordingRequest, RecordingModel};
use imbit_domain::mappers::ToModel;
use imbit_domain::{ImStore, RecordingRegistry};

#[derive(Inject)]
pub struct GetRecordingHandler {
    #[inject]
    store: Arc<ImStore>,
    #[inject]
    recordings: Arc<RecordingRegistry>,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<GetRecordingRequest, RecordingModel> for GetRecordingHandler {
    async fn handle(&mut self, request: GetRecordingRequest) -> ActionResult<RecordingModel> {
        let recording = self.recordings.find(&request.recording_id)?;
        self.store
            .require_member(recording.room_id(), &request.user.id)?;

        Ok(recording.to_model())
    }
}
