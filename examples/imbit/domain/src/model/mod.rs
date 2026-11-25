//! Aggregates and value objects of the IM domain.

pub mod message;
pub mod recording;
pub mod room;
pub mod user;

pub use message::Message;
pub use recording::{recording_prefix, segment_key, Recording};
pub use room::Room;
pub use user::User;
