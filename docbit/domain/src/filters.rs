//! Global query filters — soft-delete visibility for audited entities.

use rust_ef::prelude::*;

use crate::entities::{Blog, Category, Comment, Exhibition, Resource, Role, User};

/// Register `is_deleted = false` filters. Must run before any `set::<T>()`.
pub fn register_soft_delete(ctx: &mut DbContext) {
    let model = ctx.model();
    model.has_query_filter::<Blog>(linq!(filter |b: Blog| !b.is_deleted));
    model.has_query_filter::<Category>(linq!(filter |c: Category| !c.is_deleted));
    model.has_query_filter::<Comment>(linq!(filter |c: Comment| !c.is_deleted));
    model.has_query_filter::<Exhibition>(linq!(filter |e: Exhibition| !e.is_deleted));
    model.has_query_filter::<Resource>(linq!(filter |r: Resource| !r.is_deleted));
    model.has_query_filter::<Role>(linq!(filter |r: Role| !r.is_deleted));
    model.has_query_filter::<User>(linq!(filter |u: User| !u.is_deleted));
}
