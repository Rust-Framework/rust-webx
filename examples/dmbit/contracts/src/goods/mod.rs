//! Goods context — component / goods read models left over from the old
//! goods routes.
//!
//! The routes themselves moved to `/api/specs`; the read models stay because
//! the admin panel and the CSV export still speak them.

pub mod models;
pub use models::*;
