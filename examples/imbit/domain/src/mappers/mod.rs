//! Aggregate → read model mapping, one file per context.
//!
//! `ToModel` is the fixed read direction. The write direction of this reference
//! app goes through the aggregate constructors and methods (`Room::create`,
//! `Recording::start`, `ImStore::register_user`) instead of a mapper, because
//! there is no ORM entity to build yet: the durable `rust-ef` variant adds
//! `ToEntity` for the same request types without changing these models.

pub mod message_mapper;
pub mod recording_mapper;
pub mod room_mapper;

/// Convert an aggregate into the read model the contracts use.
pub trait ToModel<M> {
    fn to_model(self) -> M;
}
