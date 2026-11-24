//! DbContext model configuration entry points.

use rust_ef::db_context::DbContext;

use crate::filters;
use crate::seed;

/// Register global query filters. Call on every new `DbContext` instance.
pub fn prepare_context(ctx: &mut DbContext) {
    filters::register_soft_delete(ctx);
}

/// Register seed metadata. Call once during application startup.
pub fn register_seed(ctx: &mut DbContext) {
    seed::register(ctx);
}

/// Startup-only: filters plus seed before `ensure_created`.
pub fn configure_for_init(ctx: &mut DbContext) {
    prepare_context(ctx);
    register_seed(ctx);
}
