//! Inventory CSV import / export.
//!
//! Flat row model: one CSV row per component line; product + goods fields are
//! repeated when a goods has multiple components. Goods without components
//! emit a single row with empty 部件* columns. Product-only rows (no brand /
//! qty / components) update or create the product master.
//!
//! Import conflict: if any product code (or matching goods) already exists and
//! `confirm_update` is false, return conflict lists without writing.

use std::collections::{BTreeSet, HashMap};

use rust_ef::{db_context::DbContext, prelude::*};
use rust_webx::*;

use dmbit_contracts::goods::ComponentModel;
use dmbit_contracts::inventory::*;
use dmbit_domain::entities::{Goods, Product};
use dmbit_domain::new_id;

use crate::db::{save_changes, EfResultExt};
use crate::mapping::{
    load_components_for, normalize_category, normalize_comp_kind, normalize_status,
    replace_components,
};
use crate::util::{now_secs, operator_id};

/// Must stay in sync with export output and admin import expectations.
const CSV_HEADERS: &[&str] = &[
    "产品名称",
    "产品编码",
    "类别",
    "品牌短码",
    "状态",
    "数量",
    "单位",
    "机位",
    "资产编码",
    "机箱",
    "主板",
    "内存",
    "接口",
    "扩展",
    "电源",
    "光模块",
    "其他参数",
    "部件类型",
    "部件型号",
    "容量",
    "单台数量",
];

const PARAM_KEYS: &[&str] = &["机箱", "主板", "内存", "接口", "扩展", "电源", "光模块"];

fn csv_header_line() -> String {
    CSV_HEADERS.join(",")
}

fn headers_match(header: &str) -> bool {
    let cols = parse_csv_line(header);
    if cols.len() != CSV_HEADERS.len() {
        return false;
    }
    cols.iter()
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

fn category_label(c: &str) -> &'static str {
    match c.trim() {
        "storage" => "存储",
        _ => "算力",
    }
}

fn kind_label(k: &str) -> &'static str {
    match k.trim() {
        "disk" => "硬盘",
        _ => "加速卡",
    }
}

fn parse_param_map(text: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some((k, v)) = line.split_once('：').or_else(|| line.split_once(':')) {
            map.insert(k.trim().to_string(), v.trim().to_string());
        }
    }
    map
}

fn param_value(map: &HashMap<String, String>, key: &str) -> String {
    map.get(key).cloned().unwrap_or_default()
}

fn extras_text(map: &HashMap<String, String>) -> String {
    let mut lines: Vec<String> = map
        .iter()
        .filter(|(k, _)| !PARAM_KEYS.iter().any(|pk| *pk == k.as_str()))
        .map(|(k, v)| format!("{k}：{v}"))
        .collect();
    lines.sort();
    // Semicolon-separated so CSV stays single-line (parser is line-based).
    lines.join("；")
}

fn build_parameters(
    chassis: &str,
    board: &str,
    memory: &str,
    iface: &str,
    expand: &str,
    power: &str,
    optic: &str,
    extras: &str,
) -> String {
    let mut lines = Vec::new();
    let pairs = [
        ("机箱", chassis),
        ("主板", board),
        ("内存", memory),
        ("接口", iface),
        ("扩展", expand),
        ("电源", power),
        ("光模块", optic),
    ];
    for (k, v) in pairs {
        let v = v.trim();
        if !v.is_empty() {
            lines.push(format!("{k}：{v}"));
        }
    }
    for part in extras.split(|c| c == '；' || c == ';' || c == '\n') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        if let Some((k, _)) = part.split_once('：').or_else(|| part.split_once(':')) {
            let key = k.trim();
            if PARAM_KEYS.iter().any(|pk| *pk == key) {
                continue;
            }
        }
        lines.push(part.to_string());
    }
    lines.join("\n")
}

fn push_goods_row(
    lines: &mut Vec<String>,
    p: &Product,
    g: &Goods,
    kind: &str,
    model: &str,
    capacity: &str,
    qty_per_unit: &str,
) {
    let pm = parse_param_map(&g.parameters);
    lines.push(format!(
        "{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{}",
        csv_escape(&p.name),
        csv_escape(&p.code),
        csv_escape(category_label(&p.category)),
        csv_escape(&g.brand),
        csv_escape(&g.status),
        g.quantity,
        csv_escape(&g.unit),
        csv_escape(&g.location),
        csv_escape(&g.asset_code),
        csv_escape(&param_value(&pm, "机箱")),
        csv_escape(&param_value(&pm, "主板")),
        csv_escape(&param_value(&pm, "内存")),
        csv_escape(&param_value(&pm, "接口")),
        csv_escape(&param_value(&pm, "扩展")),
        csv_escape(&param_value(&pm, "电源")),
        csv_escape(&param_value(&pm, "光模块")),
        csv_escape(&extras_text(&pm)),
        csv_escape(kind),
        csv_escape(model),
        csv_escape(capacity),
        qty_per_unit,
    ));
}

fn goods_label(brand: &str, asset_code: &str, location: &str) -> String {
    format!("{brand} / {asset_code} / {location}")
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
        let goods = linq!(self.ctx.set::<Goods>();).to_list().await.map_ef()?;
        let ids: Vec<String> = goods.iter().map(|g| g.id.clone()).collect();
        let cmap = load_components_for(&mut self.ctx, &ids).await?;

        let mut lines = vec![csv_header_line()];
        for p in &products {
            let mut rows: Vec<_> = goods.iter().filter(|g| g.product_id == p.id).collect();
            rows.sort_by(|a, b| a.sort_order.cmp(&b.sort_order));
            if rows.is_empty() {
                lines.push(format!(
                    "{},{},{},,,,,,,,,,,,,,,,,,",
                    csv_escape(&p.name),
                    csv_escape(&p.code),
                    csv_escape(category_label(&p.category)),
                ));
                continue;
            }
            for g in rows {
                let comps = cmap.get(&g.id).cloned().unwrap_or_default();
                if comps.is_empty() {
                    push_goods_row(lines.as_mut(), p, g, "", "", "", "");
                } else {
                    for c in comps {
                        push_goods_row(
                            lines.as_mut(),
                            p,
                            g,
                            kind_label(&c.kind),
                            &c.model,
                            &c.capacity,
                            &c.qty_per_unit.to_string(),
                        );
                    }
                }
            }
        }

        // UTF-8 BOM for Excel
        let mut csv = String::from("\u{FEFF}");
        csv.push_str(&lines.join("\n"));
        csv.push('\n');
        Ok(InventoryCsvModel { csv })
    }
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

        #[derive(Default)]
        struct AccGoods {
            product_name: String,
            product_code: String,
            category: String,
            brand: String,
            quantity: i32,
            unit: String,
            status: String,
            location: String,
            asset_code: String,
            parameters: String,
            components: Vec<ComponentModel>,
        }

        let mut acc: Vec<AccGoods> = Vec::new();
        for (idx, line) in lines.enumerate() {
            let line_no = idx + 2; // header is line 1
            let cols = parse_csv_line(line);
            if cols.iter().all(|c| c.trim().is_empty()) {
                continue;
            }
            if cols.len() < 9 {
                return Err(Error::Validation(format!(
                    "第 {line_no} 行列数不足（至少需要产品/台账基础列）"
                )));
            }

            let product_name = col(&cols, 0).to_string();
            let product_code = col(&cols, 1).to_string();
            if product_code.is_empty() {
                return Err(Error::Validation(format!(
                    "第 {line_no} 行：产品编码不能为空"
                )));
            }

            let category = normalize_category(col(&cols, 2));
            let brand = col(&cols, 3).to_string();
            let status = normalize_status(col(&cols, 4));
            let quantity: i32 = col(&cols, 5).parse().unwrap_or(0);
            if quantity < 0 {
                return Err(Error::Validation(format!(
                    "第 {line_no} 行：数量不能为负"
                )));
            }
            let unit = {
                let u = col(&cols, 6);
                if u.is_empty() {
                    "台".into()
                } else {
                    u.to_string()
                }
            };
            let location = col(&cols, 7).to_string();
            let asset_code = col(&cols, 8).to_string();
            let parameters = build_parameters(
                col(&cols, 9),
                col(&cols, 10),
                col(&cols, 11),
                col(&cols, 12),
                col(&cols, 13),
                col(&cols, 14),
                col(&cols, 15),
                col(&cols, 16),
            );
            let comp_kind_raw = col(&cols, 17);
            let comp_model = col(&cols, 18).to_string();
            let comp_capacity = col(&cols, 19).to_string();
            let comp_qty_raw = col(&cols, 20);

            let has_comp =
                !comp_kind_raw.is_empty() || !comp_model.is_empty() || !comp_qty_raw.is_empty();
            let mut component: Option<ComponentModel> = None;
            if has_comp {
                if comp_model.is_empty() {
                    return Err(Error::Validation(format!(
                        "第 {line_no} 行：部件型号不能为空"
                    )));
                }
                let kind = normalize_comp_kind(comp_kind_raw).map_err(|_| {
                    Error::Validation(format!(
                        "第 {line_no} 行：部件类型无效「{comp_kind_raw}」（支持：加速卡/硬盘）"
                    ))
                })?;
                if kind == "disk" && comp_capacity.trim().is_empty() {
                    return Err(Error::Validation(format!(
                        "第 {line_no} 行：部件类型为硬盘时容量不能为空"
                    )));
                }
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
                    model: comp_model,
                    capacity: comp_capacity,
                    qty_per_unit: comp_qty,
                    sort_order: 0,
                });
            }

            let key_brand = brand.clone();
            let key_asset = asset_code.clone();
            let key_loc = location.clone();
            let idx = acc.iter().position(|a| {
                a.product_code == product_code
                    && a.brand == key_brand
                    && a.asset_code == key_asset
                    && a.location == key_loc
            });

            let slot = if let Some(i) = idx {
                &mut acc[i]
            } else {
                acc.push(AccGoods {
                    product_name,
                    product_code: product_code.clone(),
                    category,
                    brand,
                    quantity,
                    unit,
                    status,
                    location,
                    asset_code,
                    parameters,
                    components: Vec::new(),
                });
                acc.last_mut().unwrap()
            };

            if quantity > 0 {
                slot.quantity = quantity;
            }
            if let Some(c) = component {
                let sort_order = (slot.components.len() as i32) + 1;
                slot.components.push(ComponentModel {
                    sort_order,
                    ..c
                });
            }
        }

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
        let existing_goods = linq!(self.ctx.set::<Goods>();).to_list().await.map_ef()?;

        let mut conflict_product_codes: BTreeSet<String> = BTreeSet::new();
        let mut conflict_goods_labels: BTreeSet<String> = BTreeSet::new();
        for row in &acc {
            if product_by_code.contains_key(&row.product_code) {
                conflict_product_codes.insert(row.product_code.clone());
            }
            let product_only =
                row.brand.is_empty() && row.quantity == 0 && row.components.is_empty();
            if product_only {
                continue;
            }
            if let Some(pid) = product_by_code.get(&row.product_code) {
                if existing_goods.iter().any(|g| {
                    &g.product_id == pid
                        && g.brand == row.brand
                        && g.asset_code == row.asset_code
                        && g.location == row.location
                }) {
                    conflict_goods_labels.insert(goods_label(
                        &row.brand,
                        &row.asset_code,
                        &row.location,
                    ));
                }
            }
        }

        if (!conflict_product_codes.is_empty() || !conflict_goods_labels.is_empty())
            && !req.confirm_update
        {
            let n_p = conflict_product_codes.len();
            let n_g = conflict_goods_labels.len();
            return Ok(ImportInventoryResult {
                products_upserted: 0,
                goods_upserted: 0,
                components_written: 0,
                message: format!(
                    "检测到编号冲突（产品编码 {n_p} 个、台账 {n_g} 条）。确认后将覆盖更新，取消则不写入。"
                ),
                needs_confirm: true,
                conflict_product_codes: conflict_product_codes.into_iter().collect(),
                conflict_goods_labels: conflict_goods_labels.into_iter().collect(),
            });
        }

        let now = now_secs();
        let op = operator_id();
        let mut products_upserted = 0i32;
        let mut goods_upserted = 0i32;
        let mut components_written = 0i32;

        for row in acc {
            let product_only =
                row.brand.is_empty() && row.quantity == 0 && row.components.is_empty();

            if product_only {
                if let Some(pid) = product_by_code.get(&row.product_code).cloned() {
                    if let Some(mut p) = product_entities.get(&pid).cloned() {
                        if !row.product_name.is_empty() {
                            p.name = row.product_name;
                        }
                        p.category = row.category;
                        p.updated_at = now;
                        p.updated_id = op.clone();
                        product_entities.insert(pid, p.clone());
                        self.ctx.set::<Product>().update(p);
                        products_upserted += 1;
                    }
                } else if !row.product_name.is_empty() {
                    let id = new_id();
                    let entity = Product {
                        id: id.clone(),
                        name: row.product_name,
                        code: row.product_code.clone(),
                        category: row.category,
                        remark: String::new(),
                        sort_order: 0,
                        created_id: op.clone(),
                        created_at: now,
                        updated_id: op.clone(),
                        updated_at: now,
                        is_deleted: false,
                        goods: HasMany::new(),
                    };
                    product_by_code.insert(row.product_code, id.clone());
                    product_entities.insert(id, entity.clone());
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

            if row.brand.is_empty() {
                return Err(Error::Validation(format!(
                    "产品「{}」的台账行缺少品牌短码",
                    row.product_code
                )));
            }

            let product_id = if let Some(id) = product_by_code.get(&row.product_code).cloned() {
                if let Some(mut p) = product_entities.get(&id).cloned() {
                    let mut changed = false;
                    if !row.product_name.is_empty() && p.name != row.product_name {
                        p.name = row.product_name.clone();
                        changed = true;
                    }
                    if p.category != row.category {
                        p.category = row.category.clone();
                        changed = true;
                    }
                    if changed {
                        p.updated_at = now;
                        p.updated_id = op.clone();
                        product_entities.insert(id.clone(), p.clone());
                        self.ctx.set::<Product>().update(p);
                        products_upserted += 1;
                    }
                }
                id
            } else {
                let id = new_id();
                let name = if row.product_name.is_empty() {
                    row.product_code.clone()
                } else {
                    row.product_name.clone()
                };
                let entity = Product {
                    id: id.clone(),
                    name,
                    code: row.product_code.clone(),
                    category: row.category.clone(),
                    remark: String::new(),
                    sort_order: 0,
                    created_id: op.clone(),
                    created_at: now,
                    updated_id: op.clone(),
                    updated_at: now,
                    is_deleted: false,
                    goods: HasMany::new(),
                };
                product_by_code.insert(row.product_code.clone(), id.clone());
                product_entities.insert(id.clone(), entity.clone());
                self.ctx.set::<Product>().add(entity);
                products_upserted += 1;
                id
            };

            let goods_id = if let Some(mut g) = existing_goods.iter().cloned().find(|g| {
                g.product_id == product_id
                    && g.brand == row.brand
                    && g.asset_code == row.asset_code
                    && g.location == row.location
            }) {
                g.quantity = row.quantity;
                g.unit = row.unit.clone();
                g.status = row.status.clone();
                g.parameters = row.parameters.clone();
                g.updated_at = now;
                g.updated_id = op.clone();
                let id = g.id.clone();
                self.ctx.set::<Goods>().update(g);
                goods_upserted += 1;
                id
            } else {
                let id = new_id();
                let entity = Goods {
                    id: id.clone(),
                    product_id,
                    brand: row.brand,
                    parameters: row.parameters,
                    unit: row.unit,
                    quantity: row.quantity,
                    status: row.status,
                    location: row.location,
                    asset_code: row.asset_code,
                    sort_order: 0,
                    created_id: op.clone(),
                    created_at: now,
                    updated_id: op.clone(),
                    updated_at: now,
                    is_deleted: false,
                    product: BelongsTo::new(),
                };
                self.ctx.set::<Goods>().add(entity);
                goods_upserted += 1;
                id
            };

            // Always replace components for goods rows (empty list clears on update).
            save_changes(&mut self.ctx).await?;
            components_written +=
                replace_components(&mut self.ctx, &goods_id, &row.components).await?;
        }

        save_changes(&mut self.ctx).await?;

        Ok(ImportInventoryResult {
            products_upserted,
            goods_upserted,
            components_written,
            message: format!(
                "导入完成：产品 {products_upserted}，台账 {goods_upserted}，部件 {components_written}"
            ),
            needs_confirm: false,
            conflict_product_codes: Vec::new(),
            conflict_goods_labels: Vec::new(),
        })
    }
}
