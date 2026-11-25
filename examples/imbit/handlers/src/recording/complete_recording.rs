//! `POST /api/recordings/{recording_id}/complete` — finish and publish.

use std::sync::Arc;

use webx::*;

use imbit_contracts::recording::{CompleteRecordingRequest, RecordingModel};
use imbit_domain::mappers::ToModel;
use imbit_domain::{ImStore, RecordingRegistry};

#[derive(Inject)]
pub struct CompleteRecordingHandler {
    #[inject]
    store: Arc<ImStore>,
    #[inject]
    recordings: Arc<RecordingRegistry>,
    #[inject]
    clock: Arc<dyn IClock>,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<CompleteRecordingRequest, RecordingModel> for CompleteRecordingHandler {
    async fn handle(&mut self, request: CompleteRecordingRequest) -> ActionResult<RecordingModel> {
        let recording = self.recordings.find(&request.recording_id)?;
        self.store
            .require_member(recording.room_id(), &request.user.id)?;

        let recording = self
            .recordings
            .complete(&request.recording_id, self.clock.now_unix())?;

        Ok(recording.to_model())
    }
}
