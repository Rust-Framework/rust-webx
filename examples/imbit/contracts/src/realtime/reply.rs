//! Server → client replies of the `/ws/im` protocol (one per request frame).

use serde::Serialize;

use crate::message::MessageModel;

/// The reply frame of the request that was just answered.
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
        /// `true` when the `client_msg_id` was already stored (retry).
        duplicate: bool,
    },
    History {
        room_id: String,
        messages: Vec<MessageModel>,
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
