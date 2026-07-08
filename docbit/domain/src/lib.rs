//! Docbit domain crate — entities, model configuration, and mapping.

pub mod audit;
pub mod configure;
pub mod conversions;
pub mod entities;
pub mod filters;
pub mod ids;
pub mod mapper;
pub mod seed;

pub use audit::operator_id as audit_operator_id;
pub use configure::{configure_for_init, prepare_context, register_seed};
pub use ids::{new_id, seed as seed_ids};
pub use mapper::{ApplyTo, ToEntity, ToModel};
