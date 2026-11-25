//! CSV → desired state.
//!
//! Parsing stops at a validated in-memory picture of what the file *wants*: one
//! slot per spec code, one per product code. Nothing here touches the database,
//! so a malformed row can never leave a half-applied import behind.

use webx::*;

use dmbit_contracts::goods::ComponentModel;
use dmbit_domain::mappers::inventory_mapper::{build_parameters, ParameterColumns};
use dmbit_domain::rules::{
    normalize_category, normalize_comp_kind, optional_text, parse_capacity_label_to_gb,
    require_text, MAX_PRODUCT_CODE, MAX_PRODUCT_NAME, MAX_PRODUCT_REMARK, MAX_UNIT,
};

use super::csv;

/// One CSV line, validated but not yet merged into the desired state.
struct CsvRow {
    line_no: usize,
    product_name: String,
    product_code: String,
    category_raw: String,
    spec_code: String,
    brand: String,
    parameters: String,
    unit_raw: String,
    planned_quantity: i32,
    planned_quantity_provided: bool,
    product_sort: Option<i32>,
    remark: String,
    component: Option<ComponentModel>,
}

/// The merged desired state of one spec (or of one product-only row).
#[derive(Default)]
pub struct AccSpec {
    pub product_name: String,
    pub product_code: String,
    pub category: String,
    pub remark: String,
    pub product_sort: i32,
    pub spec_code: String,
    pub brand: String,
    pub parameters: String,
    pub unit: String,
    pub planned_quantity: i32,
    pub spec_sort: i32,
    pub components: Vec<ComponentModel>,
    pub components_touched: bool,
}

/// The merged desired state of one product master.
#[derive(Default)]
pub struct AccProduct {
    pub code: String,
    pub name: String,
    pub category: String,
    pub remark: String,
    pub sort_order: i32,
}

/// The parsed file: per-spec rows in file order plus the product masters.
#[derive(Default)]
pub struct ParsedImport {
    pub rows: Vec<AccSpec>,
    pub products: Vec<AccProduct>,
}

/// Fold every data line into [`ParsedImport`].
pub fn accumulate(lines: impl Iterator<Item = String>) -> Result<ParsedImport> {
    let mut import = ParsedImport::default();
    for (idx, line) in lines.enumerate() {
        let line_no = idx + 2;
        let cols = csv::parse_line(&line);
        if cols.iter().all(|c| c.trim().is_empty()) {
            continue;
        }
        if cols.len() < 4 {
            return Err(Error::validation(format!(
                "第 {line_no} 行：只有 {} 列数据，至少需要 4 列（产品名称、产品编码、类别、规格编码）。\n可能是：1) 该行是空行请删除；2) CSV 格式被破坏，请用 Excel 重新保存为 UTF-8 CSV",
                cols.len()
            )));
        }
        let row = parse_row(line_no, &cols)?;
        accumulate_product(&mut import.products, &row);
        merge_row(&mut import.rows, row)?;
    }
    Ok(import)
}

fn parse_row(line_no: usize, cols: &[String]) -> Result<CsvRow> {
    let product_name = optional_text(
        "产品名称",
        csv::col(cols, csv::COL_PRODUCT_NAME),
        MAX_PRODUCT_NAME,
    )
    .map_err(|e| csv::annotate_line(line_no, e))?;
    let product_code = require_text(
        "产品编码",
        csv::col(cols, csv::COL_PRODUCT_CODE),
        MAX_PRODUCT_CODE,
    )
    .map_err(|e| csv::annotate_line(line_no, e))?;
    let remark = optional_text("备注", csv::col(cols, csv::COL_REMARK), MAX_PRODUCT_REMARK)
        .map_err(|e| csv::annotate_line(line_no, e))?;

    // Reconstruct the `parameters` text from the seven structured columns.
    let parameters = build_parameters(&ParameterColumns {
        chassis: csv::col(cols, csv::COL_CHASSIS),
        board: csv::col(cols, csv::COL_BOARD),
        memory: csv::col(cols, csv::COL_MEMORY),
        iface: csv::col(cols, csv::COL_IFACE),
        expand: csv::col(cols, csv::COL_EXPAND),
        power: csv::col(cols, csv::COL_POWER),
        optic: csv::col(cols, csv::COL_OPTIC),
        extras: csv::col(cols, csv::COL_EXTRAS),
    });

    let planned_raw = csv::col(cols, csv::COL_PLANNED_QTY);
    let planned_quantity: i32 = if planned_raw.is_empty() {
        0
    } else {
        planned_raw
            .parse()
            .map_err(|_| Error::validation(format!("第 {line_no} 行：数量必须是整数")))?
    };
    if planned_quantity < 0 {
        return Err(Error::validation(format!("第 {line_no} 行：数量不能为负")));
    }

    let sort_raw = csv::col(cols, csv::COL_SORT);
    let product_sort: Option<i32> = if sort_raw.is_empty() {
        None
    } else {
        Some(
            sort_raw
                .parse()
                .map_err(|_| Error::validation(format!("第 {line_no} 行：排序必须是整数")))?,
        )
    };

    Ok(CsvRow {
        line_no,
        product_name,
        product_code,
        category_raw: csv::col(cols, csv::COL_CATEGORY).to_string(),
        spec_code: csv::col(cols, csv::COL_SPEC_CODE).to_string(),
        brand: csv::col(cols, csv::COL_BRAND).to_string(),
        parameters,
        unit_raw: csv::col(cols, csv::COL_UNIT).to_string(),
        planned_quantity,
        planned_quantity_provided: !planned_raw.is_empty(),
        product_sort,
        remark,
        component: parse_component(line_no, cols)?,
    })
}

fn parse_component(line_no: usize, cols: &[String]) -> Result<Option<ComponentModel>> {
    let kind_raw = csv::col(cols, csv::COL_COMP_KIND);
    let model_raw = csv::col(cols, csv::COL_COMP_MODEL);
    let capacity_raw = csv::col(cols, csv::COL_COMP_CAPACITY);
    let qty_raw = csv::col(cols, csv::COL_COMP_QTY);

    if kind_raw.is_empty() && model_raw.is_empty() && qty_raw.is_empty() {
        return Ok(None);
    }
    if model_raw.is_empty() {
        return Err(Error::validation(format!(
            "第 {line_no} 行：部件类型已填写但部件型号为空。\n如果该行不需要部件，请把「部件类型」列也留空。"
        )));
    }

    let kind = normalize_comp_kind(kind_raw).map_err(|_| {
        Error::validation(format!(
            "第 {line_no} 行：部件类型「{kind_raw}」无法识别。\n支持的类型：加速卡 / 硬盘（或 accelerator / disk / gpu / npu / hdd / ssd）"
        ))
    })?;
    let capacity_gb = if kind == "disk" {
        parse_capacity_label_to_gb(capacity_raw).map_err(|e| csv::annotate_line(line_no, e))?
    } else if !capacity_raw.is_empty() {
        return Err(Error::validation(format!(
            "第 {line_no} 行：加速卡部件不应填写容量"
        )));
    } else {
        0
    };
    let qty: i32 = if qty_raw.is_empty() {
        0
    } else {
        qty_raw
            .parse()
            .map_err(|_| Error::validation(format!("第 {line_no} 行：单台数量必须是整数")))?
    };
    if qty < 1 {
        return Err(Error::validation(format!(
            "第 {line_no} 行：单台数量至少为 1"
        )));
    }

    Ok(Some(ComponentModel {
        id: String::new(),
        kind,
        model: model_raw.to_string(),
        capacity_gb,
        qty_per_unit: qty,
        sort_order: 0,
    }))
}

/// Fold a row's product-level columns into the desired product master.
fn accumulate_product(products: &mut Vec<AccProduct>, row: &CsvRow) {
    let index = match products.iter().position(|p| p.code == row.product_code) {
        Some(i) => i,
        None => {
            products.push(AccProduct {
                code: row.product_code.clone(),
                ..AccProduct::default()
            });
            products.len() - 1
        }
    };
    let Some(product) = products.get_mut(index) else {
        return;
    };
    if !row.product_name.is_empty() {
        product.name = row.product_name.clone();
    }
    // A blank category cell must not flip an existing storage product back to
    // `compute`, so only an explicit value moves it.
    if !row.category_raw.trim().is_empty() {
        if let Ok(category) = normalize_category(&row.category_raw) {
            product.category = category;
        }
    }
    if !row.remark.is_empty() {
        product.remark = row.remark.clone();
    }
    if let Some(sort) = row.product_sort {
        product.sort_order = sort;
    }
}

/// Merge a parsed row into the desired state, one slot per spec code.
fn merge_row(rows: &mut Vec<AccSpec>, row: CsvRow) -> Result<()> {
    let key = row.spec_code.trim().to_string();
    let existing = if key.is_empty() {
        None
    } else {
        rows.iter().position(|a| a.spec_code == key)
    };

    let index = match existing {
        Some(i) => {
            check_continuation(&rows[i], &row)?;
            if let Some(slot) = rows.get_mut(i) {
                if row.component.is_some() {
                    slot.components_touched = true;
                }
            }
            i
        }
        None => {
            let slot = new_slot(&row, key.is_empty())?;
            rows.push(slot);
            rows.len() - 1
        }
    };

    let Some(component) = row.component else {
        return Ok(());
    };
    let line_no = row.line_no;
    let slot = rows
        .get_mut(index)
        .ok_or_else(|| Error::internal("导入行未归位"))?;
    let duplicate = slot.components.iter().any(|x| {
        x.kind == component.kind
            && x.model == component.model
            && x.capacity_gb == component.capacity_gb
    });
    if duplicate {
        return Err(Error::validation(format!(
            "第 {line_no} 行：部件重复。\n该规格已存在：{}/{}/{}GB，请检查是否误填了重复的部件行。",
            component.kind, component.model, component.capacity_gb
        )));
    }
    let sort_order = (slot.components.len() as i32) + 1;
    slot.components.push(ComponentModel {
        sort_order,
        ..component
    });
    Ok(())
}

fn check_continuation(slot: &AccSpec, row: &CsvRow) -> Result<()> {
    let line_no = row.line_no;
    if !row.product_name.is_empty() && slot.product_name != row.product_name {
        return Err(Error::validation(format!(
            "第 {line_no} 行：同一规格的产品名称与首行不一致（续行请留空继承）"
        )));
    }
    if !row.category_raw.trim().is_empty() {
        let category =
            normalize_category(&row.category_raw).map_err(|e| csv::annotate_line(line_no, e))?;
        if category != slot.category {
            return Err(Error::validation(format!(
                "第 {line_no} 行：同一规格的类别与首行不一致（续行请留空继承）"
            )));
        }
    }
    if !row.brand.trim().is_empty() && row.brand.trim() != slot.brand {
        return Err(Error::validation(format!(
            "第 {line_no} 行：同一规格的品牌与首行不一致（续行请留空继承）"
        )));
    }
    if !row.parameters.is_empty() && row.parameters != slot.parameters {
        return Err(Error::validation(format!(
            "第 {line_no} 行：同一规格的参数与首行不一致（续行请留空继承）"
        )));
    }
    if !row.unit_raw.trim().is_empty() {
        let unit = require_text("单位", &row.unit_raw, MAX_UNIT)
            .map_err(|e| csv::annotate_line(line_no, e))?;
        if unit != slot.unit {
            return Err(Error::validation(format!(
                "第 {line_no} 行：同一规格的单位与首行不一致"
            )));
        }
    }
    if row.planned_quantity_provided && row.planned_quantity != slot.planned_quantity {
        return Err(Error::validation(format!(
            "第 {line_no} 行：同一规格的数量与首行不一致"
        )));
    }
    if !row.remark.is_empty() && row.remark != slot.remark {
        return Err(Error::validation(format!(
            "第 {line_no} 行：同一规格的备注与首行不一致"
        )));
    }
    if let Some(sort) = row.product_sort {
        if sort != slot.product_sort {
            return Err(Error::validation(format!("第 {line_no} 行：排序不一致")));
        }
    }
    Ok(())
}

/// Build the initial desired state for a row that starts a new slot.
fn new_slot(row: &CsvRow, product_only: bool) -> Result<AccSpec> {
    let category =
        normalize_category(&row.category_raw).map_err(|e| csv::annotate_line(row.line_no, e))?;
    let mut slot = AccSpec {
        product_name: row.product_name.clone(),
        product_code: row.product_code.clone(),
        category,
        remark: row.remark.clone(),
        product_sort: row.product_sort.unwrap_or(0),
        parameters: row.parameters.clone(),
        ..AccSpec::default()
    };
    if product_only {
        // A product master row carries no spec data at all.
        slot.planned_quantity = 0;
        return Ok(slot);
    }
    slot.spec_code = row.spec_code.trim().to_string();
    slot.brand = row.brand.trim().to_string();
    slot.unit = if row.unit_raw.trim().is_empty() {
        "台".into()
    } else {
        require_text("单位", &row.unit_raw, MAX_UNIT)
            .map_err(|e| csv::annotate_line(row.line_no, e))?
    };
    slot.planned_quantity = row.planned_quantity;
    slot.components_touched = row.component.is_some();
    Ok(slot)
}
