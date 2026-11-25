//! Startup seed helpers (admin account).
//!
//! Catalog synchronisation now lives in `docbit_handlers::catalog`, so it can run
//! at boot *and* after a documentation upload.

pub mod admin_user;
