//! Server → client push frames (no request): room fan-out and signaling.

use serde::Serialize;
use webx::SignalingMessage;

use crate::message::MessageModel;

/// Frame pushed to room members without a matching request.
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

impl ImEvent {
    /// Broadcast frame of a stored message, built from the shared read model so
    /// the push payload and the HTTP catch-up payload cannot drift.
    pub fn message(model: &MessageModel) -> Self {
        Self::Message {
            room_id: model.room_id.clone(),
            seq: model.seq,
            from: model.from.clone(),
            text: model.text.clone(),
            client_msg_id: model.client_msg_id.clone(),
            at_unix: model.at_unix,
        }
    }
}
