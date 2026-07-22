//! Dmbit domain — entities, configuration, and default-admin seed.

pub mod configure;
pub mod entities;
pub mod filters;
pub mod ids;
pub mod seed;

pub use configure::{configure_for_init, prepare_context};
pub use ids::{new_id, seed as seed_ids};
