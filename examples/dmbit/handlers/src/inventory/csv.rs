//! CSV mechanics for the inventory import / export.
//!
//! The column contract (21 columns) is declared once here; the parser, the
//! header check and the row builders all read it, so an export always re-imports.

use webx::{Error, Result};

use dmbit_domain::entities::{Product, Spec};
use dmbit_domain::mappers::inventory_mapper::{extract_extras, parse_param_map};
use dmbit_domain::rules::{category_label, format_capacity_label, kind_label};

/// Structured 21-column format for operations-friendly maintenance.
pub const CSV_HEADERS: &[&str] = &[
    "产品名称",
    "产品编码",
    "类别",
    "规格编码",
    "品牌",
    "机箱",
    "主板",
    "内存",
    "接口",
    "扩展",
    "电源",
    "光模块",
    "附加参数",
    "单位",
    "数量",
    "部件类型",
    "部件型号",
    "容量",
    "单台数量",
    "备注",
    "排序",
];

pub const COL_PRODUCT_NAME: usize = 0;
pub const COL_PRODUCT_CODE: usize = 1;
pub const COL_CATEGORY: usize = 2;
pub const COL_SPEC_CODE: usize = 3;
pub const COL_BRAND: usize = 4;
pub const COL_CHASSIS: usize = 5;
pub const COL_BOARD: usize = 6;
pub const COL_MEMORY: usize = 7;
pub const COL_IFACE: usize = 8;
pub const COL_EXPAND: usize = 9;
pub const COL_POWER: usize = 10;
pub const COL_OPTIC: usize = 11;
pub const COL_EXTRAS: usize = 12;
pub const COL_UNIT: usize = 13;
pub const COL_PLANNED_QTY: usize = 14;
pub const COL_COMP_KIND: usize = 15;
pub const COL_COMP_MODEL: usize = 16;
pub const COL_COMP_CAPACITY: usize = 17;
pub const COL_COMP_QTY: usize = 18;
pub const COL_REMARK: usize = 19;
pub const COL_SORT: usize = 20;

/// The header line of an exported file.
pub fn header_line() -> String {
    CSV_HEADERS.join(",")
}

/// Reject a header that does not match the export contract, naming the columns.
pub fn check_headers(header: &str) -> Result<()> {
    let cols = parse_line(header);
    if cols.len() < CSV_HEADERS.len() {
        return Err(Error::validation(format!(
            "CSV 表头只有 {} 列，需要 {} 列。请用导出功能获取正确格式的CSV文件。",
            cols.len(),
            CSV_HEADERS.len()
        )));
    }
    if cols[CSV_HEADERS.len()..]
        .iter()
        .any(|c| !c.trim().is_empty())
    {
        return Err(Error::validation(
            "CSV 表头有多余列。请确保列数与导出文件一致，不要添加额外列。",
        ));
    }
    let mut mismatches = Vec::new();
    for (i, (actual, expected)) in cols
        .iter()
        .take(CSV_HEADERS.len())
        .zip(CSV_HEADERS.iter())
        .enumerate()
    {
        if actual.trim() != *expected {
            mismatches.push(format!(
                "第{}列：CSV中是「{}」，应为「{}」",
                i + 1,
                actual.trim(),
                expected
            ));
        }
    }
    if mismatches.is_empty() {
        Ok(())
    } else {
        Err(Error::validation(format!(
            "CSV 表头不匹配（共{}处）：\n{}\n\n提示：请用系统的「导出」功能获取正确格式的CSV，在导出的基础上修改数据。",
            mismatches.len(),
            mismatches.join("\n")
        )))
    }
}

/// Quote a field when it carries a delimiter, a quote or a newline.
pub fn escape(s: &str) -> String {
    if s.contains(',') || s.contains('"') || s.contains('\n') || s.contains('\r') {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

/// Split CSV text into rows, respecting quoted fields that span lines.
pub fn parse_rows(text: &str) -> Vec<String> {
    let mut rows = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if in_quotes {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    current.push('"');
                    current.push('"');
                    chars.next();
                } else {
                    in_quotes = false;
                }
                current.push(c);
            } else {
                current.push(c);
            }
        } else if c == '"' {
            in_quotes = true;
            current.push(c);
        } else if c == '\n' {
            let row = current.trim().to_string();
            if !row.is_empty() {
                rows.push(row);
            }
            current = String::new();
        } else if c == '\r' {
            // normalize CRLF
        } else {
            current.push(c);
        }
    }
    let row = current.trim().to_string();
    if !row.is_empty() {
        rows.push(row);
    }
    rows
}

/// Split one CSV line into its fields.
pub fn parse_line(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if in_quotes {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    current.push('"');
                    chars.next();
                } else {
                    in_quotes = false;
                }
            } else {
                current.push(c);
            }
        } else if c == '"' {
            in_quotes = true;
        } else if c == ',' {
            out.push(current);
            current = String::new();
        } else {
            current.push(c);
        }
    }
    out.push(current);
    out
}

/// One trimmed field, empty when the row is shorter than the header.
pub fn col(cols: &[String], i: usize) -> &str {
    cols.get(i).map(|s| s.as_str()).unwrap_or("").trim()
}

/// Prefix an error with the CSV line number (once).
pub fn annotate_line(line_no: usize, err: Error) -> Error {
    let Some(msg) = err.public_detail().map(str::to_string) else {
        return err;
    };
    if msg.starts_with("第") {
        Error::validation(msg)
    } else {
        Error::validation(format!("第{line_no} 行：{msg}"))
    }
}

// ── Export row builders ─────────────────────────────────────────────
//
// Fields are written by column index, so the 21-column contract has exactly one
// owner: adding a column touches `CSV_HEADERS` and its `COL_*` index only.

/// A spec's header row: product master + spec + its first component.
#[allow(clippy::too_many_arguments)]
pub fn push_spec_row(
    lines: &mut Vec<String>,
    product: &Product,
    spec: &Spec,
    kind: &str,
    model: &str,
    capacity_gb: i64,
    qty_per_unit: i32,
) {
    let params = parse_param_map(&spec.parameters);
    let mut fields = blank_row();
    fields[COL_PRODUCT_NAME] = escape(&product.name);
    fields[COL_PRODUCT_CODE] = escape(&product.code);
    fields[COL_CATEGORY] = escape(category_label(&product.category));
    fields[COL_SPEC_CODE] = escape(&spec.code);
    fields[COL_BRAND] = escape(&spec.brand);
    fields[COL_CHASSIS] = field(&params, "机箱");
    fields[COL_BOARD] = field(&params, "主板");
    fields[COL_MEMORY] = field(&params, "内存");
    fields[COL_IFACE] = field(&params, "接口");
    fields[COL_EXPAND] = field(&params, "扩展");
    fields[COL_POWER] = field(&params, "电源");
    fields[COL_OPTIC] = field(&params, "光模块");
    fields[COL_EXTRAS] = escape(&extract_extras(&params));
    fields[COL_UNIT] = escape(&spec.unit);
    fields[COL_PLANNED_QTY] = spec.planned_quantity.to_string();
    fields[COL_COMP_KIND] = escape(kind_label(kind));
    fields[COL_COMP_MODEL] = escape(model);
    fields[COL_COMP_CAPACITY] = escape(&format_capacity_label(capacity_gb));
    fields[COL_COMP_QTY] = qty_per_unit.to_string();
    fields[COL_REMARK] = escape(&product.remark);
    fields[COL_SORT] = spec.sort_order.to_string();
    lines.push(fields.join(","));
}

/// A continuation row: only the spec code + the next component.
pub fn push_component_row(
    lines: &mut Vec<String>,
    spec: &Spec,
    kind: &str,
    model: &str,
    capacity_gb: i64,
    qty_per_unit: i32,
) {
    let mut fields = blank_row();
    fields[COL_SPEC_CODE] = escape(&spec.code);
    fields[COL_COMP_KIND] = escape(kind_label(kind));
    fields[COL_COMP_MODEL] = escape(model);
    fields[COL_COMP_CAPACITY] = escape(&format_capacity_label(capacity_gb));
    fields[COL_COMP_QTY] = qty_per_unit.to_string();
    lines.push(fields.join(","));
}

/// A product-only row (no spec yet).
pub fn push_product_row(lines: &mut Vec<String>, product: &Product) {
    let mut fields = blank_row();
    fields[COL_PRODUCT_NAME] = escape(&product.name);
    fields[COL_PRODUCT_CODE] = escape(&product.code);
    fields[COL_CATEGORY] = escape(category_label(&product.category));
    fields[COL_REMARK] = escape(&product.remark);
    fields[COL_SORT] = product.sort_order.to_string();
    lines.push(fields.join(","));
}

/// One empty field per column, so a builder only sets what it carries.
fn blank_row() -> Vec<String> {
    vec![String::new(); CSV_HEADERS.len()]
}

fn field(params: &std::collections::HashMap<String, String>, key: &str) -> String {
    escape(params.get(key).map(|s| s.as_str()).unwrap_or(""))
}
