//! Docbit domain crate — entities, conversions, and seed data.
//!
//! Depends on `docbit-contracts` and `rust-ef`. Entities use simplified naming
//! (e.g. `Blog` instead of `BlogEntity`) to avoid redundancy.

pub mod conversions;
pub mod entities;
pub mod mapper;
pub mod seed;

pub use mapper::{ApplyTo, ToEntity, ToModel};
