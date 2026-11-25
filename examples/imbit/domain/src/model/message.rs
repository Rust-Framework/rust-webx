//! Stored chat message (immutable value object).

/// One message appended to a room log.
///
/// The server stamps `seq` (per-room, gapless) and `at_unix`; `client_msg_id`
/// is the client's idempotency key. `from_user` / `from_name` come from the
/// connection identity, never from the frame payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub room_id: String,
    pub seq: u64,
    pub from_user: String,
    pub from_name: String,
    pub client_msg_id: String,
    pub text: String,
    pub at_unix: u64,
}
