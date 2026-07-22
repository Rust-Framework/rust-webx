//! Inventory CSV import / export — Spec + SpecComponent.
//!
//! Simplified format (14 columns):
//!   产品名称,产品编码,类别,规格编码,品牌,参数,单位,计划数量,
//!   部件类型,部件型号,容量,单台数量,备注,排序
//!
//! Sparse continuation: extra components repeat 规格编码 + 部件* columns only.
//! Product-only rows (no 规格编码 / brand / components) update or create product master.
//!
//! Import conflict: if any product code or spec code already exists and
//! `confirm_update` is false, return conflict lists without writing.

use std::collections::{BTreeSet, HashMap};

use rust_ef::{db_context::DbContext, prelude::*};
use rust_webx::*;

use dmbit_contracts::goods::ComponentModel;
use dmbit_contracts::inventory::*;
use dmbit_domain::entities::{Product, Spec};
use dmbit_domain::new_id;

use crate::db::{save_changes, EfResultExt};
use crate::mapping::{
    category_label, format_capacity_label, kind_label,
    load_components_for_specs, normalize_category, normalize_comp_kind,
    optional_text, parse_capacity_label_to_gb,
    replace_spec_components, require_text,
    MAX_PRODUCT_CODE, MAX_PRODUCT_NAME,
    MAX_PRODUCT_REMARK, MAX_UNIT,
};
use crate::util::{now_secs, operator_id};

const CSV_HEADERS: &[&str] = &[
    "产品名称",
    "产品编码",
    "类别",
    "规格编码",
    "品牌",
    "参数",
    "单位",
    "计划数量",
    "部件类型",
    "部件型号",
    "容量",
    "单台数量",
    "备注",
    "排序",
];

fn csv_header_line() -> String {
    CSV_HEADERS.join(",")
}

fn headers_match(header: &str) -> bool {
    let cols = parse_csv_line(header);
    if cols.len() < CSV_HEADERS.len() {
        return false;
    }
    if cols[CSV_HEADERS.len()..]
        .iter()
        .any(|c| !c.trim().is_empty())
    {
        return false;
    }
    cols.iter()
        .take(CSV_HEADERS.len())
        .zip(CSV_HEADERS.iter())
        .all(|(a, b)| a.trim() == *b)
}

fn csv_escape(s: &str) -> String {
    if s.contains(',') || s.contains('"') || s.contains('\n') || s.contains('\r') {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

fn parse_csv_line(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_q = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if in_q {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    cur.push('"');
                    chars.next();
                } else {
                    in_q = false;
                }
            } else {
                cur.push(c);
            }
        } else if c == '"' {
            in_q = true;
        } else if c == ',' {
            out.push(cur);
            cur = String::new();
        } else {
            cur.push(c);
        }
    }
    out.push(cur);
    out
}

fn col<'a>(cols: &'a [String], i: usize) -> &'a str {
    cols.get(i).map(|s| s.as_str()).unwrap_or("").trim()
}

fn annotate_line(line_no: usize, err: Error) -> Error {
    match err {
        Error::Validation(msg) => {
            if msg.starts_with("第 ") {
                Error::Validation(msg)
            } else {
                Error::Validation(format!("第 {line_no} 行：{msg}"))
            }
        }
        other => other,
    }
}

// ── Export ─────────────────────────────────────────────────────────

fn push_spec_header_row(
    lines: &mut Vec<String>,
    p: &Product,
    s: &Spec,
    kind: &str,
    model: &str,
    capacity: &str,
    qty_per_unit: &str,
) {
    lines.push(format!(
        "{},{},{},{},{},{},{},{},{},{},{},{},{},{}",
        csv_escape(&p.name),
        csv_escape(&p.code),
        csv_escape(category_label(&p.category)),
        csv_escape(&s.code),
        csv_escape(&s.brand),
        csv_escape(&s.parameters),
        csv_escape(&s.unit),
        s.planned_quantity,
        csv_escape(kind),
        csv_escape(model),
        csv_escape(capacity),
        qty_per_unit,
        csv_escape(&p.remark),
        s.sort_order,
    ));
}

fn push_component_continuation_row(
    lines: &mut Vec<String>,
    s: &Spec,
    kind: &str,
    model: &str,
    capacity: &str,
    qty_per_unit: &str,
) {
    lines.push(format!(
        ",,,{},,,,,,{},{},{},{},",
        csv_escape(&s.code),
        csv_escape(kind),
        csv_escape(model),
        csv_escape(capacity),
        qty_per_unit,
    ));
}

fn push_product_only_row(lines: &mut Vec<String>, p: &Product) {
    lines.push(format!(
        "{},{},{},,,,,,,,,,,,{},{}",
        csv_escape(&p.name),
        csv_escape(&p.code),
        csv_escape(category_label(&p.category)),
        csv_escape(&p.remark),
        p.sort_order,
    ));
}

#[derive(Inject)]
pub struct ExportInventoryHandler {
    #[inject(owned)]
    ctx: DbContext,
}

#[derive(Inject)]
pub struct ImportInventoryHandler {
    #[inject(owned)]
    ctx: DbContext,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<ExportInventoryRequest, InventoryCsvModel> for ExportInventoryHandler {
    async fn handle(&mut self, _: ExportInventoryRequest) -> Result<InventoryCsvModel> {
        let mut products = linq!(self.ctx.set::<Product>();)
            .to_list()
            .await
            .map_ef()?;
        products.sort_by(|a, b| a.sort_order.cmp(&b.sort_order));

        let specs = linq!(self.ctx.set::<Spec>();).to_list().await.map_ef()?;
        let ids: Vec<String> = specs.iter().map(|s| s.id.clone()).collect();
        let cmap = load_components_for_specs(&mut self.ctx, &ids).await?;

        let mut lines = vec![csv_header_line()];
        for p in &products {
            let mut rows: Vec<_> = specs.iter().filter(|s| s.product_id == p.id).collect();
            rows.sort_by(|a, b| a.sort_order.cmp(&b.sort_order));
            if rows.is_empty() {
                push_product_only_row(lines.as_mut(), p);
                continue;
            }
            for s in rows {
                let comps = cmap.get(&s.id).cloned().unwrap_or_default();
                if comps.is_empty() {
                    push_spec_header_row(lines.as_mut(), p, s, "", "", "", "");
                } else {
                    let mut iter = comps.into_iter();
                    if let Some(first) = iter.next() {
                        push_spec_header_row(
                            lines.as_mut(),
                            p,
                            s,
                            kind_label(&first.kind),
                            &first.model,
                            &format_capacity_label(first.capacity_gb),
                            &first.qty_per_unit.to_string(),
                        );
                    }
                    for c in iter {
                        push_component_continuation_row(
                            lines.as_mut(),
                            s,
                            kind_label(&c.kind),
                            &c.model,
                            &format_capacity_label(c.capacity_gb),
                            &c.qty_per_unit.to_string(),
                        );
                    }
                }
            }
        }

        let mut csv = String::from("\u{FEFF}");
        csv.push_str(&lines.join("\n"));
        csv.push('\n');
        Ok(InventoryCsvModel { csv })
    }
}

// ── Import ─────────────────────────────────────────────────────────

#[derive(Default)]
struct AccSpec {
    product_name: String,
    product_code: String,
    category: String,
    remark: String,
    product_sort: i32,
    spec_code: String,
    brand: String,
    parameters: String,
    unit: String,
    planned_quantity: i32,
    spec_sort: i32,
    components: Vec<ComponentModel>,
    components_touched: bool,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<ImportInventoryRequest, ImportInventoryResult> for ImportInventoryHandler {
    async fn handle(&mut self, req: ImportInventoryRequest) -> Result<ImportInventoryResult> {
        let text = req.csv.trim_start_matches('\u{FEFF}');
        let mut lines = text.lines().filter(|l| !l.trim().is_empty());
        let header = lines.next().unwrap_or("");
        if !headers_match(header) {
            return Err(Error::Validation(format!(
                "CSV 表头不正确，须与导出文件一致：{}",
                CSV_HEADERS.join(",")
            )));
        }

        let mut acc: Vec<AccSpec> = Vec::new();
        for (idx, line) in lines.enumerate() {
            let line_no = idx + 2;
            let cols = parse_csv_line(line);
            if cols.iter().all(|c| c.trim().is_empty()) {
                continue;
            }
            if cols.len() < 4 {
                return Err(Error::Validation(format!(
                    "第 {line_no} 行列数不足"
                )));
            }

            let product_name = optional_text("产品名称", col(&cols, 0), MAX_PRODUCT_NAME)
                .map_err(|e| annotate_line(line_no, e))?;
            let product_code = require_text("产品编码", col(&cols, 1), MAX_PRODUCT_CODE)
                .map_err(|e| annotate_line(line_no, e))?;
            let category_raw = col(&cols, 2);
            let spec_code_raw = col(&cols, 3);
            let brand_raw = col(&cols, 4);
            let parameters = col(&cols, 5).to_string();
            let unit_raw = col(&cols, 6);
            let planned_qty_raw = col(&cols, 7);
            let comp_kind_raw = col(&cols, 8);
            let comp_model_raw = col(&cols, 9);
            let comp_capacity_raw = col(&cols, 10);
            let comp_qty_raw = col(&cols, 11);
            let remark = optional_text("备注", col(&cols, 12), MAX_PRODUCT_REMARK)
                .map_err(|e| annotate_line(line_no, e))?;
            let product_sort_raw = col(&cols, 13);

            let planned_quantity: i32 = if planned_qty_raw.is_empty() {
                0
            } else {
                planned_qty_raw.parse().map_err(|_| {
                    Error::Validation(format!("第 {line_no} 行：计划数量必须是整数"))
                })?
            };
            if planned_quantity < 0 {
                return Err(Error::Validation(format!(
                    "第 {line_no} 行：计划数量不能为负"
                )));
            }
            let product_sort: Option<i32> = if product_sort_raw.is_empty() {
                None
            } else {
                Some(product_sort_raw.parse().map_err(|_| {
                    Error::Validation(format!("第 {line_no} 行：排序必须是整数"))
                })?)
            };

            // Parse component if present
            let has_comp =
                !comp_kind_raw.is_empty() || !comp_model_raw.is_empty() || !comp_qty_raw.is_empty();
            let mut component: Option<ComponentModel> = None;
            if has_comp {
                if comp_model_raw.is_empty() {
                    return Err(Error::Validation(format!(
                        "第 {line_no} 行：部件型号不能为空"
                    )));
                }
                let kind = normalize_comp_kind(comp_kind_raw).map_err(|_| {
                    Error::Validation(format!(
                        "第 {line_no} 行：部件类型无效「{comp_kind_raw}」（支持：加速卡/硬盘）"
                    ))
                })?;
                let capacity_gb = if kind == "disk" {
                    parse_capacity_label_to_gb(comp_capacity_raw)
                        .map_err(|e| annotate_line(line_no, e))?
                } else if !comp_capacity_raw.is_empty() {
                    return Err(Error::Validation(format!(
                        "第 {line_no} 行：加速卡部件不应填写容量"
                    )));
                } else {
                    0
                };
                let comp_qty: i32 = if comp_qty_raw.is_empty() {
                    0
                } else {
                    comp_qty_raw.parse().map_err(|_| {
                        Error::Validation(format!("第 {line_no} 行：单台数量必须是整数"))
                    })?
                };
                if comp_qty < 1 {
                    return Err(Error::Validation(format!(
                        "第 {line_no} 行：单台数量至少为 1"
                    )));
                }
                component = Some(ComponentModel {
                    id: String::new(),
                    kind,
                    model: comp_model_raw.to_string(),
                    capacity_gb,
                    qty_per_unit: comp_qty,
                    sort_order: 0,
                });
            }

            let key_code = spec_code_raw.trim().to_string();
            let existing_idx = if key_code.is_empty() {
                None
            } else {
                acc.iter().position(|a| a.spec_code == key_code)
            };

            if let Some(i) = existing_idx {
                // Continuation row for an existing spec
                let slot = &mut acc[i];
                if !product_name.is_empty() && slot.product_name != product_name {
                    return Err(Error::Validation(format!(
                        "第 {line_no} 行：同一规格的产品名称与首行不一致（续行请留空继承）"
                    )));
                }
                if !category_raw.is_empty() {
                    let cat = normalize_category(category_raw)
                        .map_err(|e| annotate_line(line_no, e))?;
                    if cat != slot.category {
                        return Err(Error::Validation(format!(
                            "第 {line_no} 行：同一规格的类别与首行不一致（续行请留空继承）"
                        )));
                    }
                }
                if !brand_raw.is_empty() && brand_raw.trim() != slot.brand {
                    return Err(Error::Validation(format!(
                        "第 {line_no} 行：同一规格的品牌与首行不一致（续行请留空继承）"
                    )));
                }
                if !parameters.is_empty() && parameters != slot.parameters {
                    return Err(Error::Validation(format!(
                        "第 {line_no} 行：同一规格的参数与首行不一致（续行请留空继承）"
                    )));
                }
                if !unit_raw.is_empty() {
                    let unit = require_text("单位", unit_raw, MAX_UNIT)
                        .map_err(|e| annotate_line(line_no, e))?;
                    if unit != slot.unit {
                        return Err(Error::Validation(format!(
                            "第 {line_no} 行：同一规格的单位与首行不一致（续行请留空继承）"
                        )));
                    }
                }
                if planned_qty_raw.is_empty() == false && planned_quantity != slot.planned_quantity {
                    return Err(Error::Validation(format!(
                        "第 {line_no} 行：同一规格的计划数量与首行不一致（续行请留空继承）"
                    )));
                }
                if !remark.is_empty() && remark != slot.remark {
                    return Err(Error::Validation(format!(
                        "第 {line_no} 行：同一规格的备注与首行不一致（续行请留空继承）"
                    )));
                }
                if let Some(ps) = product_sort {
                    if ps != slot.product_sort {
                        return Err(Error::Validation(format!(
                            "第 {line_no} 行：同一规格的产品排序与首行不一致（续行请留空继承）"
                        )));
                    }
                }
                if has_comp {
                    slot.components_touched = true;
                }
            } else if key_code.is_empty() {
                // Product-only row (no spec_code)
                let category = normalize_category(category_raw)
                    .map_err(|e| annotate_line(line_no, e))?;
                acc.push(AccSpec {
                    product_name,
                    product_code,
                    category,
                    remark,
                    product_sort: product_sort.unwrap_or(0),
                    spec_code: String::new(),
                    brand: String::new(),
                    parameters,
                    unit: String::new(),
                    planned_quantity: 0,
                    spec_sort: 0,
                    components: Vec::new(),
                    components_touched: false,
                });
            } else {
                // New spec header row
                let category = normalize_category(category_raw)
                    .map_err(|e| annotate_line(line_no, e))?;
                let unit = if unit_raw.is_empty() {
                    "台".into()
                } else {
                    require_text("单位", unit_raw, MAX_UNIT)
                        .map_err(|e| annotate_line(line_no, e))?
                };
                acc.push(AccSpec {
                    product_name,
                    product_code,
                    category,
                    remark,
                    product_sort: product_sort.unwrap_or(0),
                    spec_code: key_code.clone(),
                    brand: brand_raw.trim().to_string(),
                    parameters,
                    unit,
                    planned_quantity,
                    spec_sort: 0,
                    components: Vec::new(),
                    components_touched: has_comp,
                });
            }

            let slot = acc.last_mut().unwrap();
            if let Some(c) = component {
                let dup = slot.components.iter().any(|x| {
                    x.kind == c.kind && x.model == c.model && x.capacity_gb == c.capacity_gb
                });
                if dup {
                    return Err(Error::Validation(format!(
                        "第 {line_no} 行：同一规格下部件重复（{} / {} / {}GB）",
                        c.kind, c.model, c.capacity_gb
                    )));
                }
                let sort_order = (slot.components.len() as i32) + 1;
                slot.components.push(ComponentModel { sort_order, ..c });
            }
        }

        // ── Conflict detection ──
        let existing_products = linq!(self.ctx.set::<Product>();)
            .to_list()
            .await
            .map_ef()?;
        let mut product_by_code: HashMap<String, String> = existing_products
            .iter()
            .map(|p| (p.code.clone(), p.id.clone()))
            .collect();
        let mut product_entities: HashMap<String, Product> = existing_products
            .into_iter()
            .map(|p| (p.id.clone(), p))
            .collect();

        let existing_specs = linq!(self.ctx.set::<Spec>();).to_list().await.map_ef()?;
        let mut spec_by_code: HashMap<String, String> = existing_specs
            .iter()
            .map(|s| (s.code.clone(), s.id.clone()))
            .collect();
        let mut spec_entities: HashMap<String, Spec> = existing_specs
            .into_iter()
            .map(|s| (s.id.clone(), s))
            .collect();

        let mut conflict_product_codes: BTreeSet<String> = BTreeSet::new();
        let mut conflict_spec_codes: BTreeSet<String> = BTreeSet::new();
        for row in &acc {
            if row.spec_code.is_empty() {
                // Product-only
                if product_by_code.contains_key(&row.product_code) {
                    conflict_product_codes.insert(row.product_code.clone());
                }
            } else {
                if product_by_code.contains_key(&row.product_code) {
                    conflict_product_codes.insert(row.product_code.clone());
                }
                if spec_by_code.contains_key(&row.spec_code) {
                    conflict_spec_codes.insert(row.spec_code.clone());
                }
            }
        }

        if (!conflict_product_codes.is_empty() || !conflict_spec_codes.is_empty())
            && !req.confirm_update
        {
            let n_p = conflict_product_codes.len();
            let n_s = conflict_spec_codes.len();
            return Ok(ImportInventoryResult {
                products_upserted: 0,
                goods_upserted: 0,
                components_written: 0,
                message: format!(
                    "检测到编号冲突（产品编码 {n_p} 个、规格编码 {n_s} 个）。确认后将覆盖更新，取消则不写入。"
                ),
                needs_confirm: true,
                conflict_product_codes: conflict_product_codes.into_iter().collect(),
                conflict_goods_labels: conflict_spec_codes.into_iter().collect(),
            });
        }

        let now = now_secs();
        let op = operator_id();
        let mut products_upserted = 0i32;
        let mut specs_upserted = 0i32;
        let mut components_written = 0i32;

        for row in acc {
            // ── Product-only rows ──
            if row.spec_code.is_empty() {
                if let Some(pid) = product_by_code.get(&row.product_code).cloned() {
                    if let Some(mut p) = product_entities.get(&pid).cloned() {
                        update_product(&mut p, &row, now, &op);
                        product_entities.insert(pid, p.clone());
                        self.ctx.set::<Product>().update(p);
                        products_upserted += 1;
                    }
                } else if !row.product_name.is_empty() {
                    let id = new_id();
                    let entity = new_product(&row, &id, now, &op);
                    product_by_code.insert(row.product_code.clone(), id.clone());
                    product_entities.insert(id.clone(), entity.clone());
                    self.ctx.set::<Product>().add(entity);
                    products_upserted += 1;
                } else {
                    return Err(Error::Validation(format!(
                        "产品编码「{}」新建时需要产品名称",
                        row.product_code
                    )));
                }
                continue;
            }

            // ── Spec rows ──
            if row.brand.is_empty() && !row.components_touched {
                continue;
            }

            // Ensure product exists
            let product_id = if let Some(id) = product_by_code.get(&row.product_code).cloned() {
                if let Some(mut p) = product_entities.get(&id).cloned() {
                    let changed = update_product(&mut p, &row, now, &op);
                    if changed {
                        product_entities.insert(id.clone(), p.clone());
                        self.ctx.set::<Product>().update(p);
                        products_upserted += 1;
                    }
                }
                id
            } else {
                let id = new_id();
                let entity = new_product(&row, &id, now, &op);
                product_by_code.insert(row.product_code.clone(), id.clone());
                product_entities.insert(id.clone(), entity.clone());
                self.ctx.set::<Product>().add(entity);
                products_upserted += 1;
                id
            };

            // Upsert spec
            let spec_id = if let Some(sid) = spec_by_code.get(&row.spec_code).cloned() {
                if let Some(mut s) = spec_entities.get(&sid).cloned() {
                    s.parameters = row.parameters.clone();
                    s.unit = row.unit.clone();
                    s.planned_quantity = row.planned_quantity;
                    s.brand = row.brand.clone();
                    s.product_id = product_id.clone();
                    s.sort_order = row.spec_sort;
                    s.updated_at = now;
                    s.updated_id = op.clone();
                    spec_entities.insert(sid.clone(), s.clone());
                    self.ctx.set::<Spec>().update(s);
                    specs_upserted += 1;
                    sid
                } else {
                    continue;
                }
            } else {
                let id = new_id();
                let entity = Spec {
                    id: id.clone(),
                    product_id: product_id.clone(),
                    code: row.spec_code.clone(),
                    brand: row.brand.clone(),
                    parameters: row.parameters.clone(),
                    unit: row.unit.clone(),
                    planned_quantity: row.planned_quantity,
                    sort_order: row.spec_sort,
                    created_id: op.clone(),
                    created_at: now,
                    updated_id: op.clone(),
                    updated_at: now,
                    is_deleted: false,
                    product: BelongsTo::new(),
                    components: HasMany::new(),
                    devices: HasMany::new(),
                };
                spec_by_code.insert(row.spec_code.clone(), id.clone());
                spec_entities.insert(id.clone(), entity.clone());
                self.ctx.set::<Spec>().add(entity);
                specs_upserted += 1;
                id
            };

            if row.components_touched {
                components_written +=
                    replace_spec_components(&mut self.ctx, &spec_id, &row.components).await?;
            }
        }

        save_changes(&mut self.ctx).await?;

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
}

fn update_product(p: &mut Product, row: &AccSpec, now: i64, op: &Option<String>) -> bool {
    let mut changed = false;
    if !row.product_name.is_empty() && p.name != row.product_name {
        p.name = row.product_name.clone();
        changed = true;
    }
    if p.category != row.category {
        p.category = row.category.clone();
        changed = true;
    }
    if !row.remark.is_empty() && p.remark != row.remark {
        p.remark = row.remark.clone();
        changed = true;
    }
    if p.sort_order != row.product_sort {
        p.sort_order = row.product_sort;
        changed = true;
    }
    if changed {
        p.updated_at = now;
        p.updated_id = op.clone();
    }
    changed
}

fn new_product(row: &AccSpec, id: &str, now: i64, op: &Option<String>) -> Product {
    let name = if row.product_name.is_empty() {
        row.product_code.clone()
    } else {
        row.product_name.clone()
    };
    Product {
        id: id.to_string(),
        name,
        code: row.product_code.clone(),
        category: row.category.clone(),
        remark: row.remark.clone(),
        sort_order: row.product_sort,
        created_id: op.clone(),
        created_at: now,
        updated_id: op.clone(),
        updated_at: now,
        is_deleted: false,
        specs: HasMany::new(),
    }
}
