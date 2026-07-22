//! Soft-delete global query filters.

use rust_ef::prelude::*;

use crate::entities::{Goods, GoodsComponent, Product, Role, User};

pub fn register_soft_delete(ctx: &mut DbContext) {
    let model = ctx.model();
    model.has_query_filter::<User>(linq!(filter |u: User| !u.is_deleted));
    model.has_query_filter::<Role>(linq!(filter |r: Role| !r.is_deleted));
    model.has_query_filter::<Product>(linq!(filter |p: Product| !p.is_deleted));
    model.has_query_filter::<Goods>(linq!(filter |g: Goods| !g.is_deleted));
    model.has_query_filter::<GoodsComponent>(linq!(filter |c: GoodsComponent| !c.is_deleted));
}
