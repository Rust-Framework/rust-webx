//! rust-webx + rust-ef integration layer.
//!
//! Optional crate: applications without a database never depend on this module.
//! Core `rust-webx` remains ORM-free.

mod error;
mod interceptor;
mod persistence;
mod registration;

pub use error::{map_ef_error, EfResultExt};
pub use interceptor::SaveChangesLogInterceptor;
pub use persistence::save_changes;
pub use registration::EfServiceCollectionExt;

pub use rust_ef::di::DbContextServiceCollectionExt;
