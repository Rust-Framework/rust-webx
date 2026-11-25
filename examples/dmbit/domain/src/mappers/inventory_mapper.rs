//! CSV ⇄ `parameters` text mapping for the inventory import / export.
//!
//! The database stores a spec's chassis description as one `键：值` line per
//! parameter; the CSV flattens the seven fixed keys into their own columns and
//! keeps everything else in `附加参数`. Both directions live here so the export
//! and the import can never drift apart.

use std::collections::HashMap;

use webx_core::IPatchable;

use crate::entities::Product;

/// The seven fixed `parameters` keys, in column order.
pub const PARAM_KEYS: &[&str] = &["机箱", "主板", "内存", "接口", "扩展", "电源", "光模块"];

/// One CSV row's parameter columns; `extras` carries the non-standard keys.
#[derive(Debug, Default, Clone, Copy)]
pub struct ParameterColumns<'a> {
    pub chassis: &'a str,
    pub board: &'a str,
    pub memory: &'a str,
    pub iface: &'a str,
    pub expand: &'a str,
    pub power: &'a str,
    pub optic: &'a str,
    pub extras: &'a str,
}

/// Parse `键：值` lines into a map (accepts both the Chinese and ASCII colon).
pub fn parse_param_map(text: &str) -> HashMap<String, String> {
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

/// Build the `parameters` text from the seven columns + extras.
///
/// CONSISTENCY: always `：` (Chinese colon), one `键：值` per line.
pub fn build_parameters(cols: &ParameterColumns<'_>) -> String {
    let pairs = [
        ("机箱", cols.chassis),
        ("主板", cols.board),
        ("内存", cols.memory),
        ("接口", cols.iface),
        ("扩展", cols.expand),
        ("电源", cols.power),
        ("光模块", cols.optic),
    ];
    let mut lines = Vec::new();
    for (k, v) in pairs {
        let v = v.trim();
        if !v.is_empty() {
            lines.push(format!("{k}：{v}"));
        }
    }
    for part in cols.extras.split(['；', ';']) {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        if let Some((k, _)) = part.split_once('：').or_else(|| part.split_once(':')) {
            if PARAM_KEYS.contains(&k.trim()) {
                continue;
            }
        }
        lines.push(part.to_string());
    }
    lines.join("\n")
}

/// Extract the non-standard keys of a parsed map for the `附加参数` column.
pub fn extract_extras(map: &HashMap<String, String>) -> String {
    let mut lines: Vec<String> = map
        .iter()
        .filter(|(k, _)| !PARAM_KEYS.contains(&k.as_str()))
        .map(|(k, v)| format!("{k}：{v}"))
        .collect();
    lines.sort();
    lines.join("；")
}

/// The product-level fields one CSV row contributes to its product master.
#[derive(Debug, Default, Clone)]
pub struct InventoryProductValues {
    pub name: String,
    pub category: String,
    pub remark: String,
    pub sort_order: i32,
}

impl InventoryProductValues {
    /// Whether applying this row would change the stored product.
    ///
    /// A blank CSV cell keeps the stored value (the import is an upsert), so
    /// only non-empty cells and a different sort order count as a change. The
    /// import uses this to report writes instead of rows.
    pub fn differs(&self, p: &Product) -> bool {
        (!self.name.is_empty() && p.name != self.name)
            || (!self.category.is_empty() && p.category != self.category)
            || (!self.remark.is_empty() && p.remark != self.remark)
            || p.sort_order != self.sort_order
    }
}

impl IPatchable<InventoryProductValues> for Product {
    /// The import keeps every stored value a blank CSV cell did not provide.
    fn patch(&mut self, patch: InventoryProductValues) {
        if !patch.name.is_empty() {
            self.name = patch.name;
        }
        if !patch.category.is_empty() {
            self.category = patch.category;
        }
        if !patch.remark.is_empty() {
            self.remark = patch.remark;
        }
        self.sort_order = patch.sort_order;
    }
}
