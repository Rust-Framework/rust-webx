//! Product queries.

use rust_ef::{db_context::DbContext, prelude::*};
use webx::*;

use dmbit_contracts::product::ProductModel;
use dmbit_domain::entities::{Product, Spec};
use dmbit_domain::mappers::product_mapper::product_with_specs;

use crate::common::MAX_LIST_ROWS;
use crate::spec::queries::spec_models_for;

/// Load one product or `404`.
pub async fn require_product(ctx: &mut DbContext, id: &str) -> Result<Product> {
    let q = id.to_string();
    linq!(ctx.set::<Product>(), |p: Product| p.id == q)
        .first_or_default()
        .await?
        .ok_or_else(|| Error::not_found("产品不存在"))
}

/// Reject a duplicate product code (`409`) unless it belongs to `exclude_id`.
pub async fn assert_code_available(
    ctx: &mut DbContext,
    code: &str,
    exclude_id: Option<&str>,
) -> Result<()> {
    let q = code.to_string();
    let existing = linq!(ctx.set::<Product>(), |p: Product| p.code == q)
        .first_or_default()
        .await?;
    if let Some(p) = existing {
        if exclude_id.map(|id| id != p.id.as_str()).unwrap_or(true) {
            return Err(Error::conflict(format!("产品编码「{code}」已存在")));
        }
    }
    Ok(())
}

/// The specs of a product, ordered by `sort_order`.
pub async fn specs_of_product(ctx: &mut DbContext, product_id: &str) -> Result<Vec<Spec>> {
    let q = product_id.to_string();
    let mut specs = linq!(ctx.set::<Spec>(), |s: Spec| s.product_id == q)
        .take(MAX_LIST_ROWS)
        .to_list()
        .await?;
    specs.sort_by_key(|s| s.sort_order);
    Ok(specs)
}

/// Product read model with its spec tree.
pub async fn product_model_for(ctx: &mut DbContext, p: &Product) -> Result<ProductModel> {
    let specs = specs_of_product(ctx, &p.id).await?;
    let refs: Vec<&Spec> = specs.iter().collect();
    let models = spec_models_for(ctx, &refs, &p.name).await?;
    Ok(product_with_specs(p, &models))
}
