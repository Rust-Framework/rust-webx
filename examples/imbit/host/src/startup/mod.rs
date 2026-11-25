//! Composition-root building blocks: configuration defaults and services.

pub mod config;
pub mod services;

pub use config::{app_mode, apply_options, DEFAULT_ADDRESS, DEV_JWT_SECRET};
pub use services::{add_imbit_services, object_store, WS_PATH};
