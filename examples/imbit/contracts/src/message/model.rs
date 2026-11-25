//! Read model of a stored chat message.

use serde::Serialize;

/// One message of a room log, as the HTTP catch-up and the WebSocket
/// push/history payloads render it.
#[derive(Debug, Clone, Serialize)]
pub struct MessageModel {
    pub room_id: String,
    /// Per-room sequence number assigned by the server.
    pub seq: u64,
    /// Sender display name, stamped from the connection identity.
    pub from: String,
    pub text: String,
    /// Client idempotency key; a retry echoes the original `seq`.
    pub client_msg_id: String,
    pub at_unix: u64,
}
