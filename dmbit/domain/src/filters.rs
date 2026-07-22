//! Soft-delete global query filters.

use rust_ef::prelude::*;

use crate::entities::{Device, Product, Role, Spec, SpecComponent, User};

pub fn register_soft_delete(ctx: &mut DbContext) {
    let model = ctx.model();
    model.has_query_filter::<User>(linq!(filter |u: User| !u.is_deleted));
    model.has_query_filter::<Role>(linq!(filter |r: Role| !r.is_deleted));
    model.has_query_filter::<Product>(linq!(filter |p: Product| !p.is_deleted));
    model.has_query_filter::<Spec>(linq!(filter |s: Spec| !s.is_deleted));
    model.has_query_filter::<SpecComponent>(linq!(filter |c: SpecComponent| !c.is_deleted));
    model.has_query_filter::<Device>(linq!(filter |d: Device| !d.is_deleted));
}
