//! Application startup — database initialization and documentation sync.

mod admin_user;
mod db_init;
mod exhibition_seed;

pub use db_init::DbInitService;
