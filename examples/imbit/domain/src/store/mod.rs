//! In-process repositories.
//!
//! The reference app keeps its state in process memory so it runs without a
//! database. A durable variant replaces these implementations with a `rust-ef`
//! `DbContext` **behind the same method surface** — same names, same
//! [`StoreError`](crate::StoreError) classifications — so handlers and contracts
//! do not change.

mod im_store;
mod recording_registry;

pub use im_store::{ImStore, PostedMessage};
pub use recording_registry::RecordingRegistry;
