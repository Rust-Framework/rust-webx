//! Client → server request frames of the `/ws/im` protocol.
//!
//! The frame is **untrusted client JSON**, so the contract deliberately has no
//! `#[fromclaim]` / `#[fromheader]` / `#[fromclient]` field: a client could
//! otherwise choose its own identity. The handler reads the identity captured
//! by the authenticated upgrade instead (`RequestContext::identity()`).

use serde::Deserialize;
use webx::*;

use super::reply::ImReply;

/// One client frame.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ImRequest {
    /// Join a room (also registers this connection for its fan-out).
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

/// Message-as-request registration marker.
///
/// `use_websocket_request` requires `Req: IRequest<Rsp>` because the frame is
/// dispatched through the mediator like any other request, but a WebSocket
/// frame has **no HTTP route**: the endpoint is registered once, by path, in
/// the host (`use_websocket_request("/ws/im")`). The marker is therefore
/// spelled with the qualified trait path — it exists for the dispatcher, not
/// for endpoint discovery.
impl webx::IRequest<ImReply> for ImRequest {}
