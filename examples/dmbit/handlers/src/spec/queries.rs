//! Spec queries — the joins the spec read models need, plus the write helpers
//! shared with the product and inventory handlers.

use std::collections::{HashMap, HashSet};

use rust_ef::{db_context::DbContext, prelude::*};
use webx::*;

use dmbit_contracts::goods::ComponentModel;
use dmbit_contracts::spec::SpecModel;
use dmbit_domain::entities::{Device, Product, Spec, SpecComponent};
use dmbit_domain::mappers::component_mapper::component_seeds;
use dmbit_domain::mappers::spec_mapper::spec_model;

use crate::common::{now_secs, operator_id, MAX_LIST_ROWS};

/// `ComponentModel` list per spec id.
///
/// Component rows are scanned once per request and filtered by the id set: the
/// query builder has no portable "id in this slice" predicate, and one query per
/// spec would turn the product tree into an N+1 round trip. The scan is bounded
/// by [`MAX_LIST_ROWS`].
pub async fn components_by_spec(
    ctx: &mut DbContext,
    spec_ids: &[String],
) -> Result<HashMap<String, Vec<ComponentModel>>> {
    let mut map: HashMap<String, Vec<ComponentModel>> = HashMap::new();
    if spec_ids.is_empty() {
        return Ok(map);
    }
    let wanted: HashSet<&str> = spec_ids.iter().map(|s| s.as_str()).collect();
    let rows = linq!(ctx.set::<SpecComponent>();)
        .take(MAX_LIST_ROWS)
        .to_list()
        .await?;
    for c in rows {
        if !wanted.contains(c.spec_id.as_str()) {
            continue;
        }
        map.entry(c.spec_id.clone())
            .or_default()
            .push(ComponentModel::from(&c));
    }
    for list in map.values_mut() {
        list.sort_by(|a, b| a.sort_order.cmp(&b.sort_order).then(a.model.cmp(&b.model)));
    }
    Ok(map)
}

/// Device count per spec id (same bounded-scan trade-off as
/// [`components_by_spec`]).
pub async fn device_counts(
    ctx: &mut DbContext,
    spec_ids: &[String],
) -> Result<HashMap<String, i32>> {
    let mut map: HashMap<String, i32> = HashMap::new();
    if spec_ids.is_empty() {
        return Ok(map);
    }
    let wanted: HashSet<&str> = spec_ids.iter().map(|s| s.as_str()).collect();
    let rows = linq!(ctx.set::<Device>();)
        .take(MAX_LIST_ROWS)
        .to_list()
        .await?;
    for d in rows {
        if !wanted.contains(d.spec_id.as_str()) {
            continue;
        }
        *map.entry(d.spec_id).or_default() += 1;
    }
    Ok(map)
}

/// Read models for a product's spec rows, sorted by `sort_order`.
pub async fn spec_models_for(
    ctx: &mut DbContext,
    specs: &[&Spec],
    product_name: &str,
) -> Result<Vec<SpecModel>> {
    let ids: Vec<String> = specs.iter().map(|s| s.id.clone()).collect();
    let cmap = components_by_spec(ctx, &ids).await?;
    let dmap = device_counts(ctx, &ids).await?;
    let mut models: Vec<SpecModel> = specs
        .iter()
        .map(|s| {
            let components = cmap.get(&s.id).cloned().unwrap_or_default();
            let count = dmap.get(&s.id).copied().unwrap_or(0);
            spec_model(s, product_name, components, count)
        })
        .collect();
    models.sort_by_key(|m| m.sort_order);
    Ok(models)
}

/// Read model for one spec row.
pub async fn spec_model_for(
    ctx: &mut DbContext,
    s: &Spec,
    product_name: &str,
) -> Result<SpecModel> {
    let spec_id = s.id.clone();
    spec_models_for(ctx, &[s], product_name)
        .await?
        .into_iter()
        .next()
        .ok_or_else(|| Error::internal(format!("spec {spec_id} produced no read model")))
}

/// Load one spec or `404`.
pub async fn require_spec(ctx: &mut DbContext, id: &str) -> Result<Spec> {
    let q = id.to_string();
    linq!(ctx.set::<Spec>(), |s: Spec| s.id == q)
        .first_or_default()
        .await?
        .ok_or_else(|| Error::not_found("规格不存在"))
}

/// The product name of a spec's product, or `404` when the product is gone.
pub async fn require_product_name(ctx: &mut DbContext, product_id: &str) -> Result<String> {
    let q = product_id.to_string();
    let product = linq!(ctx.set::<Product>(), |p: Product| p.id == q)
        .first_or_default()
        .await?
        .ok_or_else(|| Error::not_found("所属产品不存在"))?;
    Ok(product.name)
}

/// A spec together with its product name (device read models need both).
pub async fn spec_with_product(ctx: &mut DbContext, spec_id: &str) -> Result<(Spec, String)> {
    let spec = require_spec(ctx, spec_id).await?;
    let name = require_product_name(ctx, &spec.product_id).await?;
    Ok((spec, name))
}

/// Reject a duplicate spec code (`409`) unless it belongs to `exclude_id`.
pub async fn assert_code_available(
    ctx: &mut DbContext,
    code: &str,
    exclude_id: Option<&str>,
) -> Result<()> {
    let q = code.to_string();
    let existing = linq!(ctx.set::<Spec>(), |s: Spec| s.code == q)
        .first_or_default()
        .await?;
    if let Some(s) = existing {
        if exclude_id.map(|id| id != s.id.as_str()).unwrap_or(true) {
            return Err(Error::conflict(format!("规格编码「{code}」已存在")));
        }
    }
    Ok(())
}

/// Replace a spec's component list: soft-delete the old rows, insert the new ones.
///
/// Old rows are soft-deleted because the component list is a historical record;
/// the tuple may therefore repeat, which is why the uniqueness rule lives in the
/// mapper instead of a unique index.
pub async fn replace_components(
    ctx: &mut DbContext,
    spec_id: &str,
    components: &[ComponentModel],
) -> Result<i32> {
    let sid = spec_id.to_string();
    let existing = linq!(ctx.set::<SpecComponent>(), |c: SpecComponent| c.spec_id
        == sid)
    .take(MAX_LIST_ROWS)
    .to_list()
    .await?;

    let now = now_secs();
    let actor = operator_id();
    for old in existing {
        ctx.edit(old, |row| {
            row.is_deleted = true;
            row.updated_at = now;
            row.updated_id = actor.clone();
        });
    }
    ctx.detect_changes();

    let seeds = component_seeds(spec_id, components, now, actor)?;
    let written = seeds.len() as i32;
    for seed in seeds {
        ctx.add(SpecComponent::from(seed));
    }
    Ok(written)
}

/// `spec_id → (spec code, product name)` for the device read models.
pub async fn spec_name_map(ctx: &mut DbContext) -> Result<HashMap<String, (String, String)>> {
    let specs = linq!(ctx.set::<Spec>();)
        .take(MAX_LIST_ROWS)
        .to_list()
        .await?;
    let products = linq!(ctx.set::<Product>();)
        .take(MAX_LIST_ROWS)
        .to_list()
        .await?;
    let mut map = HashMap::new();
    for s in specs {
        let product_name = products
            .iter()
            .find(|p| p.id == s.product_id)
            .map(|p| p.name.as_str())
            .unwrap_or("");
        map.insert(s.id, (s.code, product_name.to_string()));
    }
    Ok(map)
}

/// Soft-delete a spec's components and retire its devices (`已淘汰`).
///
/// Shared by the spec delete and the product cascade so both leave the fleet in
/// the same shape.
pub async fn retire_spec_children(ctx: &mut DbContext, spec_id: &str) -> Result<()> {
    let now = now_secs();
    let actor = operator_id();

    let sid = spec_id.to_string();
    let components = linq!(ctx.set::<SpecComponent>(), |c: SpecComponent| c.spec_id
        == sid)
    .take(MAX_LIST_ROWS)
    .to_list()
    .await?;
    for c in components {
        ctx.edit(c, |row| {
            row.is_deleted = true;
            row.updated_at = now;
            row.updated_id = actor.clone();
        });
    }

    let q = spec_id.to_string();
    let devices = linq!(ctx.set::<Device>(), |d: Device| d.spec_id == q)
        .take(MAX_LIST_ROWS)
        .to_list()
        .await?;
    for d in devices {
        ctx.edit(d, |row| {
            row.status = "已淘汰".into();
            row.updated_at = now;
            row.updated_id = actor.clone();
        });
    }
    Ok(())
}
