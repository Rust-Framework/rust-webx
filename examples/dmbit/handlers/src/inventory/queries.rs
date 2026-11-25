//! Inventory queries — the code-keyed lookups the import needs.

use std::collections::HashMap;

use rust_ef::{db_context::DbContext, prelude::*};
use webx::Result;

use dmbit_domain::entities::{Product, Spec};

use crate::common::MAX_LIST_ROWS;

/// Existing products indexed by their business code.
pub async fn products_by_code(ctx: &mut DbContext) -> Result<HashMap<String, Product>> {
    let rows = linq!(ctx.set::<Product>();)
        .take(MAX_LIST_ROWS)
        .to_list()
        .await?;
    Ok(rows.into_iter().map(|p| (p.code.clone(), p)).collect())
}

/// Existing specs indexed by their business code.
pub async fn specs_by_code(ctx: &mut DbContext) -> Result<HashMap<String, Spec>> {
    let rows = linq!(ctx.set::<Spec>();)
        .take(MAX_LIST_ROWS)
        .to_list()
        .await?;
    Ok(rows.into_iter().map(|s| (s.code.clone(), s)).collect())
}
