//! Composition-root building blocks: configuration defaults and services.

pub mod config;
pub mod services;

pub use config::{
    app_mode, apply_options, session_revalidation_interval, websocket_origins, DEFAULT_ADDRESS,
    DEFAULT_SESSION_REVALIDATION_MS, DEV_JWT_SECRET,
};
pub use services::{add_imbit_services, csrf_config, object_store, session_cookie_scheme, WS_PATH};
