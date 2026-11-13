//! Soft-delete global query filters.

use rust_ef::prelude::*;

use crate::entities::{Device, Product, Role, Spec, SpecComponent, User};

pub fn register_soft_delete(ctx: &mut DbContext) {
    let model = ctx.model();
    model.has_query_filter::<User>(linq!(|u: User| !u.is_deleted));
    model.has_query_filter::<Role>(linq!(|r: Role| !r.is_deleted));
    model.has_query_filter::<Product>(linq!(|p: Product| !p.is_deleted));
    model.has_query_filter::<Spec>(linq!(|s: Spec| !s.is_deleted));
    model.has_query_filter::<SpecComponent>(linq!(|c: SpecComponent| !c.is_deleted));
    model.has_query_filter::<Device>(linq!(|d: Device| !d.is_deleted));
}
