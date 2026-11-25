//! Recording context: control plane, resumable segment upload, playback.

pub mod complete_recording;
pub mod get_recording;
pub mod get_segment;
pub mod model;
pub mod start_recording;
pub mod upload_segment;

pub use complete_recording::CompleteRecordingRequest;
pub use get_recording::GetRecordingRequest;
pub use get_segment::GetSegmentRequest;
pub use model::{RecordingModel, RecordingSegmentModel};
pub use start_recording::StartRecordingRequest;
pub use upload_segment::UploadSegmentRequest;
