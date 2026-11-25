//! Inventory CSV import use case.
//!
//! Two phases, on purpose:
//!
//! 1. **Parse** the whole file into the desired state ([`super::parse`]). A
//!    malformed row fails there, before anything is written, so an import is
//!    all-or-nothing.
//! 2. **Write** one row per product and per spec through the tracked
//!    `detect_changes` path, then commit once.
//!
//! Conflict detection (`needs_confirm`) sits between the phases: existing
//! business codes are reported back to the caller instead of being silently
//! overwritten.

use std::collections::{BTreeSet, HashMap};

use rust_ef::db_context::DbContext;
use webx::*;

use dmbit_contracts::inventory::{ImportInventoryRequest, ImportInventoryResult};
use dmbit_domain::entities::{Product, Spec};
use dmbit_domain::mappers::inventory_mapper::InventoryProductValues;
use dmbit_domain::mappers::product_mapper::ProductSeed;
use dmbit_domain::mappers::spec_mapper::SpecSeed;
use dmbit_domain::new_id;

use super::csv;
use super::parse::{self, ParsedImport};
use super::queries::{products_by_code, specs_by_code};
use crate::common::{now_secs, operator_id, save_changes};
use crate::spec::queries::replace_components;

/// Run the import and return the counters the UI reports.
pub async fn import_inventory(
    ctx: &mut DbContext,
    req: ImportInventoryRequest,
) -> Result<ImportInventoryResult> {
    // ── 1. Parse ──
    let bytes = req.file.read_bytes().await?;
    let raw = String::from_utf8(bytes).map_err(|_| {
        Error::validation("导入文件不是有效的 UTF-8 文本，请用 Excel 另存为 UTF-8 CSV".to_string())
    })?;
    let text = raw.trim_start_matches('\u{FEFF}');
    let rows = csv::parse_rows(text);
    let mut lines = rows.into_iter();
    let header = lines.next().unwrap_or_default();
    csv::check_headers(&header)?;
    let parsed = parse::accumulate(lines)?;

    // ── 2. Conflicts ──
    let existing_products = products_by_code(ctx).await?;
    let existing_specs = specs_by_code(ctx).await?;
    if !req.confirm_update.unwrap_or(false) {
        if let Some(report) = conflict_report(&parsed, &existing_products, &existing_specs) {
            return Ok(report);
        }
    }

    // ── 3. Write ──
    write_rows(ctx, &parsed, existing_products, existing_specs).await
}

/// Report the colliding business codes instead of overwriting them.
fn conflict_report(
    parsed: &ParsedImport,
    products: &HashMap<String, Product>,
    specs: &HashMap<String, Spec>,
) -> Option<ImportInventoryResult> {
    let mut conflict_products: BTreeSet<String> = BTreeSet::new();
    let mut conflict_specs: BTreeSet<String> = BTreeSet::new();
    for row in &parsed.rows {
        if products.contains_key(&row.product_code) {
            conflict_products.insert(row.product_code.clone());
        }
        if !row.spec_code.is_empty() && specs.contains_key(&row.spec_code) {
            conflict_specs.insert(row.spec_code.clone());
        }
    }
    if conflict_products.is_empty() && conflict_specs.is_empty() {
        return None;
    }

    let product_list = conflict_products
        .iter()
        .map(|s| s.as_str())
        .collect::<Vec<_>>()
        .join("、");
    let spec_list = conflict_specs
        .iter()
        .map(|s| s.as_str())
        .collect::<Vec<_>>()
        .join("、");
    Some(ImportInventoryResult {
        products_upserted: 0,
        goods_upserted: 0,
        components_written: 0,
        message: format!(
            "以下编码与现有数据冲突：\n产品（{} 个）：{}\n规格（{} 个）：{}\n\n点「确认更新」会用CSV覆盖旧数据；点「取消」则不导入任何内容。",
            conflict_products.len(),
            product_list,
            conflict_specs.len(),
            spec_list,
        ),
        needs_confirm: true,
        conflict_product_codes: conflict_products.into_iter().collect(),
        conflict_goods_labels: conflict_specs.into_iter().collect(),
    })
}

/// Write the desired state: one write per product, one per spec, one commit.
async fn write_rows(
    ctx: &mut DbContext,
    parsed: &ParsedImport,
    mut products: HashMap<String, Product>,
    mut specs: HashMap<String, Spec>,
) -> Result<ImportInventoryResult> {
    let now = now_secs();
    let actor = operator_id();
    let mut products_upserted = 0i32;
    let mut specs_upserted = 0i32;
    let mut components_written = 0i32;

    // ── Product masters ──
    for acc in &parsed.products {
        let values = InventoryProductValues {
            name: acc.name.clone(),
            category: acc.category.clone(),
            remark: acc.remark.clone(),
            sort_order: acc.sort_order,
        };
        match products.get(&acc.code).cloned() {
            Some(loaded) => {
                let changed = values.differs(&loaded);
                ctx.edit(loaded, |product| {
                    product.patch(values);
                    if changed {
                        product.updated_at = now;
                        product.updated_id = actor.clone();
                    }
                });
                if changed {
                    products_upserted += 1;
                }
            }
            None => {
                if acc.name.is_empty() {
                    return Err(Error::validation(format!(
                        "产品编码「{}」在系统中不存在。新建产品时需要同时填写「产品名称」列。\n当前CSV中该列值为空。",
                        acc.code
                    )));
                }
                let entity = Product::from(ProductSeed {
                    id: new_id(),
                    name: acc.name.clone(),
                    code: acc.code.clone(),
                    category: if acc.category.is_empty() {
                        "compute".into()
                    } else {
                        acc.category.clone()
                    },
                    remark: acc.remark.clone(),
                    sort_order: acc.sort_order,
                    actor: actor.clone(),
                    now,
                });
                products.insert(acc.code.clone(), entity.clone());
                ctx.add(entity);
                products_upserted += 1;
            }
        }
    }

    // ── Specs ──
    for row in &parsed.rows {
        if row.spec_code.is_empty() {
            continue;
        }
        if row.brand.is_empty() && !row.components_touched {
            continue;
        }
        let product_id = products
            .get(&row.product_code)
            .map(|p| p.id.clone())
            .ok_or_else(|| Error::validation(format!("产品编码「{}」不存在", row.product_code)))?;

        let spec_id = match specs.get(&row.spec_code).cloned() {
            Some(loaded) => {
                let updated_id = loaded.id.clone();
                ctx.edit(loaded, |spec| {
                    spec.parameters = row.parameters.clone();
                    spec.unit = row.unit.clone();
                    spec.planned_quantity = row.planned_quantity;
                    spec.brand = row.brand.clone();
                    spec.product_id = product_id;
                    spec.sort_order = row.spec_sort;
                    spec.updated_at = now;
                    spec.updated_id = actor.clone();
                });
                specs_upserted += 1;
                updated_id
            }
            None => {
                let entity = Spec::from(SpecSeed {
                    id: new_id(),
                    product_id,
                    code: row.spec_code.clone(),
                    brand: row.brand.clone(),
                    parameters: row.parameters.clone(),
                    unit: row.unit.clone(),
                    planned_quantity: row.planned_quantity,
                    sort_order: row.spec_sort,
                    actor: actor.clone(),
                    now,
                });
                let id = entity.id.clone();
                specs.insert(row.spec_code.clone(), entity.clone());
                ctx.add(entity);
                specs_upserted += 1;
                id
            }
        };

        if row.components_touched {
            components_written += replace_components(ctx, &spec_id, &row.components).await?;
        }
    }

    ctx.detect_changes();
    save_changes(ctx).await?;

    Ok(ImportInventoryResult {
        products_upserted,
        goods_upserted: specs_upserted,
        components_written,
        message: format!(
            "导入完成：产品 {products_upserted}，规格 {specs_upserted}，部件 {components_written}"
        ),
        needs_confirm: false,
        conflict_product_codes: Vec::new(),
        conflict_goods_labels: Vec::new(),
    })
}
