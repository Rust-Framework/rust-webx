//! DbContext model configuration entry points.

use rust_ef::db_context::DbContext;

use crate::filters;
use crate::seed;

pub fn prepare_context(ctx: &mut DbContext) {
    filters::register_soft_delete(ctx);
}

pub fn register_seed(ctx: &mut DbContext) {
    seed::register(ctx);
}

pub fn configure_for_init(ctx: &mut DbContext) {
    prepare_context(ctx);
    register_seed(ctx);
}
