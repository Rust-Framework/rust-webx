//! Compatibility re-export for the extracted idempotency store.
//!
//! The durable `Idempotency-Key` store now lives in the standalone
//! `rust-webx-idempotency-ef` crate; this package keeps the historical
//! `webx_ef::*` paths working for one release. New code should depend on
//! `rust-webx-idempotency-ef` directly.
//!
//! Persistence errors are no longer translated here: `rust-ef` 2.9.1
//! implements `From<EFError> for rust_core::AppError`, so handlers use `?`.

/// Historical module path; the items live in `rust-webx-idempotency-ef` now.
pub mod idempotency {
    pub use rust_webx_idempotency_ef::*;
}

pub use rust_webx_idempotency_ef::*;
