//! HTTP and real-time DTOs for the IM reference app.

use serde::{Deserialize, Serialize};
use webx::*;

// ---------------------------------------------------------------------------
// HTTP API
// ---------------------------------------------------------------------------

#[derive(Default, Deserialize, WebxRequestMeta)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
}

#[post("/api/login")]
impl IRequest<LoginResponse> for LoginRequest {}

#[derive(Serialize)]
pub struct LoginResponse {
    pub token: String,
    pub user_id: String,
    pub username: String,
}

#[derive(Default, Deserialize, WebxRequestMeta)]
pub struct CreateRoomRequest {
    #[fromclaim]
    pub user: IdentityUser,
    pub name: String,
}

#[post("/api/rooms")]
impl IRequest<RoomView> for CreateRoomRequest {}

#[derive(Default, Deserialize, WebxRequestMeta)]
pub struct JoinRoomRequest {
    #[fromclaim]
    pub user: IdentityUser,
    #[frompath]
    pub room_id: String,
}

#[post("/api/rooms/{room_id}/join")]
impl IRequest<RoomView> for JoinRoomRequest {}

#[derive(Default, Deserialize, WebxRequestMeta)]
pub struct ListRoomsRequest {
    #[fromclaim]
    pub user: IdentityUser,
}

#[get("/api/rooms")]
impl IRequest<Vec<RoomView>> for ListRoomsRequest {}

#[derive(Default, Deserialize, WebxRequestMeta)]
pub struct HistoryRequest {
    #[fromclaim]
    pub user: IdentityUser,
    #[frompath]
    pub room_id: String,
    #[fromquery]
    pub after_seq: Option<u64>,
}

#[get("/api/rooms/{room_id}/messages")]
impl IRequest<HistoryResponse> for HistoryRequest {}

/// Ephemeral TURN/STUN credentials for the authenticated caller (2.4-1).
#[derive(Default, Deserialize, WebxRequestMeta)]
pub struct TurnRequest {
    #[fromclaim]
    pub user: IdentityUser,
    #[fromquery]
    pub ttl_seconds: Option<u64>,
}

#[get("/api/turn")]
impl IRequest<TurnCredentials> for TurnRequest {}

// ---------------------------------------------------------------------------
// Recordings (2.4-2): control-plane metadata + segment upload/playback
// ---------------------------------------------------------------------------

#[derive(Default, Deserialize, WebxRequestMeta)]
pub struct StartRecordingRequest {
    #[fromclaim]
    pub user: IdentityUser,
    pub room_id: String,
    pub content_type: Option<String>,
}

#[post("/api/recordings")]
impl IRequest<RecordingView> for StartRecordingRequest {}

#[derive(Default, Deserialize, WebxRequestMeta)]
pub struct UploadSegmentRequest {
    #[fromclaim]
    pub user: IdentityUser,
    #[frompath]
    pub recording_id: String,
    #[frompath]
    pub part: u32,
    /// Segment bytes, base64 encoded (reference transport; production uses
    /// multipart or pre-signed direct upload).
    pub data_base64: String,
    pub duration_ms: u64,
    pub started_at_unix: Option<u64>,
}

#[post("/api/recordings/{recording_id}/parts/{part}")]
impl IRequest<RecordingView> for UploadSegmentRequest {}

#[derive(Default, Deserialize, WebxRequestMeta)]
pub struct CompleteRecordingRequest {
    #[fromclaim]
    pub user: IdentityUser,
    #[frompath]
    pub recording_id: String,
}

#[post("/api/recordings/{recording_id}/complete")]
impl IRequest<RecordingView> for CompleteRecordingRequest {}

#[derive(Default, Deserialize, WebxRequestMeta)]
pub struct GetRecordingRequest {
    #[fromclaim]
    pub user: IdentityUser,
    #[frompath]
    pub recording_id: String,
}

#[get("/api/recordings/{recording_id}")]
impl IRequest<RecordingView> for GetRecordingRequest {}

#[derive(Default, Deserialize, WebxRequestMeta)]
pub struct GetSegmentRequest {
    #[fromclaim]
    pub user: IdentityUser,
    #[frompath]
    pub recording_id: String,
    #[frompath]
    pub part: u32,
}

/// Playback returns raw bytes through the self-controlled response type.
#[get("/api/recordings/{recording_id}/parts/{part}")]
impl IRequest<Response> for GetSegmentRequest {}

#[derive(Debug, Serialize)]
pub struct RecordingView {
    pub id: String,
    pub room_id: String,
    pub owner_id: String,
    pub status: String,
    pub content_type: String,
    pub total_size: u64,
    pub total_duration_ms: u64,
    pub created_at_unix: u64,
    pub completed_at_unix: Option<u64>,
    pub segments: Vec<RecordingSegmentView>,
}

#[derive(Debug, Serialize)]
pub struct RecordingSegmentView {
    pub part: u32,
    pub size: u64,
    pub duration_ms: u64,
    pub started_at_unix: u64,
}

#[derive(Serialize, Clone)]
pub struct RoomView {
    pub id: String,
    pub name: String,
    pub owner_id: String,
    pub members: Vec<String>,
}

#[derive(Serialize)]
pub struct HistoryResponse {
    pub room_id: String,
    pub messages: Vec<MessageView>,
}

#[derive(Debug, Serialize, Clone)]
pub struct MessageView {
    pub room_id: String,
    pub seq: u64,
    pub from: String,
    pub text: String,
    pub client_msg_id: String,
    pub at_unix: u64,
}

impl From<crate::store::StoredMessage> for MessageView {
    fn from(message: crate::store::StoredMessage) -> Self {
        Self {
            room_id: message.room_id,
            seq: message.seq,
            from: message.from_name,
            text: message.text,
            client_msg_id: message.client_msg_id,
            at_unix: message.at_unix,
        }
    }
}

// ---------------------------------------------------------------------------
// WebSocket protocol (message-as-request)
// ---------------------------------------------------------------------------

/// One client -> server frame.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ImRequest {
    /// Join a room (also registers this connection for fan-out).
    Join { room_id: String },
    /// Send a message; `client_msg_id` makes retries idempotent.
    Send {
        room_id: String,
        client_msg_id: String,
        text: String,
    },
    /// Offline catch-up: messages after `after_seq`.
    History { room_id: String, after_seq: u64 },
    /// Online members of a room.
    Presence { room_id: String },
    /// WebRTC signaling (SDP offer/answer, trickle ICE).
    ///
    /// `to` is the target **username** for 1:1 negotiation; omitted = broadcast
    /// to the room (group call). Only room members may signal.
    Signal {
        room_id: String,
        to: Option<String>,
        kind: SignalingKind,
        payload: serde_json::Value,
    },
}

impl IRequest<ImReply> for ImRequest {}

/// One server -> client reply (request/response frame).
#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ImReply {
    Joined {
        room_id: String,
        members: usize,
    },
    Ack {
        room_id: String,
        seq: u64,
        duplicate: bool,
    },
    History {
        room_id: String,
        messages: Vec<MessageView>,
    },
    Presence {
        room_id: String,
        online: Vec<String>,
    },
    /// Signaling frame delivered to `delivered` members (0 when the target is
    /// offline; signaling is best-effort and clients renegotiate on reconnect).
    Signaled {
        room_id: String,
        delivered: usize,
    },
}

/// Broadcast frame pushed to room members when a message is stored.
#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ImEvent {
    Message {
        room_id: String,
        seq: u64,
        from: String,
        text: String,
        client_msg_id: String,
        at_unix: u64,
    },
    /// Server-stamped signaling frame (`from` is the sender's user id).
    Signal {
        room_id: String,
        message: SignalingMessage,
    },
}
