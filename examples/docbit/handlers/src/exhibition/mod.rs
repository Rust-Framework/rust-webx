//! Exhibition context — portfolio works.
//!
//! Public list/get serve DB rows only; INDEX.json is synced into the DB by
//! [`crate::catalog`] (at boot and after a documentation upload), never overlaid
//! at request time.

pub mod delete_exhibition;
pub mod get_exhibition;
pub mod list_exhibitions;
pub mod upsert_exhibition;

pub use delete_exhibition::DeleteExhibitionHandler;
pub use get_exhibition::GetExhibitionHandler;
pub use list_exhibitions::ListExhibitionsHandler;
pub use upsert_exhibition::UpsertExhibitionHandler;
