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
    /// Signaling frame accepted for `delivered` **local** connections: the
    /// target user's connections for a 1:1 signal, the local room connections
    /// other than the sender for a room broadcast. Delivery itself is
    /// asynchronous (event transport, at-most-once), so a multi-instance
    /// deployment cannot count other nodes; clients renegotiate on reconnect.
    ///
    /// A room broadcast is written to every local room connection, including the
    /// sender's; signal receivers filter their own frames by `message.from`.
    Signaled {
        room_id: String,
        delivered: usize,
    },
}
