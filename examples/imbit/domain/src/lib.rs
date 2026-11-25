//! imbit domain — aggregates, typed errors, in-process repositories, mappers.
//!
//! The crate touches the framework only through `rust-webx-core` value types
//! ([`webx_core::RecordingSegment`], [`webx_core::RecordingStatus`]) and the
//! error contract ([`webx_core::Error`]). There is no HTTP, no transport and no
//! handler in here: the domain decides, the application layer orchestrates.
//!
//! ## In-process state
//!
//! [`ImStore`] and [`RecordingRegistry`] keep the reference app runnable without
//! a database. The durable variant replaces their bodies with a `rust-ef`
//! `DbContext` **behind the same method surface** (same names, same
//! [`StoreError`] classifications), so handlers do not change.

pub mod error;
pub mod mappers;
pub mod model;
pub mod store;

pub use error::StoreError;
pub use store::{ImStore, PostedMessage, RecordingRegistry};
