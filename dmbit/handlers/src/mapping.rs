//! Shared spec / device → DTO mapping + validation helpers.

use std::collections::HashSet;

use dmbit_contracts::goods::ComponentModel;
use dmbit_domain::entities::{Device, Product, Spec, SpecComponent};
use dmbit_domain::new_id;
use rust_ef::{db_context::DbContext, prelude::*};
use rust_webx::*;

use crate::db::EfResultExt;
use crate::util::{now_secs, operator_id};

// ── Constants ──────────────────────────────────────────────────────

pub const STATUSES: &[&str] = &["运行中", "联调中", "待上架", "已交付", "已淘汰"];

pub const MAX_PRODUCT_NAME: usize = 100;
pub const MAX_PRODUCT_CODE: usize = 50;
pub const MAX_PRODUCT_REMARK: usize = 500;
pub const MAX_BRAND: usize = 100;
pub const MAX_UNIT: usize = 20;
pub const MAX_STATUS: usize = 20;
pub const MAX_LOCATION: usize = 100;
pub const MAX_ASSET_CODE: usize = 50;
pub const MAX_COMP_MODEL: usize = 80;
pub const MAX_SPEC_CODE: usize = 50;
pub const MAX_SERIAL_NO: usize = 100;

// ── Validation ─────────────────────────────────────────────────────

pub fn normalize_status(raw: &str) -> Result<String> {
    let t = raw.trim();
    if t.is_empty() {
        return Ok("待上架".into());
    }
    if STATUSES.contains(&t) {
        return Ok(t.to_string());
    }
    Err(Error::Validation(format!(
        "状态无效「{t}」（支持：{}）",
        STATUSES.join(" / ")
    )))
}

pub fn normalize_category(raw: &str) -> Result<String> {
    let t = raw.trim();
    if t.is_empty() {
        return Ok("compute".into());
    }
    match t.to_ascii_lowercase().as_str() {
        "storage" | "存储" => Ok("storage".into()),
        "compute" | "算力" => Ok("compute".into()),
        _ => Err(Error::Validation(format!(
            "类别无效「{t}」（支持：算力/存储，或 compute/storage）"
        ))),
    }
}

pub fn normalize_comp_kind(raw: &str) -> Result<String> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "accelerator" | "gpu" | "npu" | "加速卡" | "卡" => Ok("accelerator".into()),
        "disk" | "hdd" | "ssd" | "硬盘" | "盘" => Ok("disk".into()),
        "" => Err(Error::Validation("部件类型不能为空".into())),
        _ => Err(Error::Validation(format!(
            "部件类型无效: {raw}（支持：加速卡/硬盘，或 accelerator/disk）"
        ))),
    }
}

pub fn require_text(field: &str, raw: &str, max: usize) -> Result<String> {
    let t = raw.trim();
    if t.is_empty() {
        return Err(Error::Validation(format!("{field}不能为空")));
    }
    if t.chars().count() > max {
        return Err(Error::Validation(format!("{field}长度不能超过 {max}")));
    }
    Ok(t.to_string())
}

pub fn optional_text(field: &str, raw: &str, max: usize) -> Result<String> {
    let t = raw.trim();
    if t.chars().count() > max {
        return Err(Error::Validation(format!("{field}长度不能超过 {max}")));
    }
    Ok(t.to_string())
}

pub fn require_non_negative(field: &str, n: i32) -> Result<i32> {
    if n < 0 {
        return Err(Error::Validation(format!("{field}不能为负")));
    }
    Ok(n)
}

// ── Capacity helpers ───────────────────────────────────────────────

pub fn format_capacity_label(gb: i64) -> String {
    if gb <= 0 {
        return String::new();
    }
    if gb % 1_000_000 == 0 {
        format!("{}PB", gb / 1_000_000)
    } else if gb % 1000 == 0 {
        format!("{}TB", gb / 1000)
    } else {
        format!("{gb}GB")
    }
}

pub fn parse_capacity_label_to_gb(raw: &str) -> Result<i64> {
    let t = raw.trim();
    if t.is_empty() {
        return Err(Error::Validation("容量不能为空".into()));
    }
    if let Ok(n) = t.parse::<i64>() {
        if n <= 0 {
            return Err(Error::Validation(format!("容量必须为正整数 GB「{t}」")));
        }
        return Ok(n);
    }
    let split = t
        .char_indices()
        .find(|(_, c)| c.is_alphabetic())
        .map(|(i, _)| i)
        .unwrap_or(t.len());
    let (num_part, unit_part) = t.split_at(split);
    let num: f64 = num_part.trim().parse().map_err(|_| {
        Error::Validation(format!("容量数值无效「{t}」（示例：8TB / 960GB / 8000）"))
    })?;
    if !num.is_finite() || num <= 0.0 {
        return Err(Error::Validation(format!("容量必须为正数「{t}」")));
    }
    let unit = unit_part.trim().to_ascii_uppercase();
    let gb_f = match unit.as_str() {
        "GB" | "G" => num,
        "TB" | "T" => num * 1000.0,
        "PB" | "P" => num * 1_000_000.0,
        "" => {
            return Err(Error::Validation(format!(
                "容量缺少单位「{t}」（需带 TB/GB/PB，或直接写整数 GB）"
            )));
        }
        _ => {
            return Err(Error::Validation(format!(
                "容量单位无效「{t}」（支持：TB / GB / PB）"
            )));
        }
    };
    let gb = gb_f.round();
    if (gb_f - gb).abs() > 1e-6 {
        return Err(Error::Validation(format!("容量必须能换算为整数 GB「{t}」")));
    }
    let gb = gb as i64;
    if gb <= 0 {
        return Err(Error::Validation(format!("容量必须为正「{t}」")));
    }
    Ok(gb)
}

pub fn require_disk_capacity_gb(gb: i64) -> Result<i64> {
    if gb <= 0 {
        return Err(Error::Validation(
            "硬盘容量必须为正整数（单位 GB，例如 8TB 请填 8000）".into(),
        ));
    }
    Ok(gb)
}

// ── Category / kind labels (for CSV export) ────────────────────────

pub fn category_label(c: &str) -> &'static str {
    match c.trim() {
        "storage" => "存储",
        _ => "算力",
    }
}

pub fn kind_label(k: &str) -> &'static str {
    match k.trim() {
        "disk" => "硬盘",
        _ => "加速卡",
    }
}

// ── Spec ↔ DTO mapping ────────────────────────────────────────────

pub fn component_to_model(c: &SpecComponent) -> ComponentModel {
    ComponentModel {
        id: c.id.clone(),
        kind: c.kind.clone(),
        model: c.model.clone(),
        capacity_gb: c.capacity_gb,
        qty_per_unit: c.qty_per_unit,
        sort_order: c.sort_order,
    }
}

pub fn spec_to_model(
    s: &Spec,
    product_name: &str,
    components: Vec<ComponentModel>,
    device_count: i32,
) -> dmbit_contracts::spec::SpecModel {
    dmbit_contracts::spec::SpecModel {
        id: s.id.clone(),
        product_id: s.product_id.clone(),
        product_name: product_name.to_string(),
        code: s.code.clone(),
        brand: s.brand.clone(),
        parameters: s.parameters.clone(),
        unit: s.unit.clone(),
        planned_quantity: s.planned_quantity,
        sort_order: s.sort_order,
        created_at: s.created_at,
        updated_at: s.updated_at,
        components,
        device_count,
    }
}

/// Summary string for dashboard device rows.
pub fn parts_summary(components: &[ComponentModel]) -> String {
    if components.is_empty() {
        return "—".into();
    }
    components
        .iter()
        .map(|c| {
            if c.kind == "disk" {
                if c.capacity_gb <= 0 {
                    format!("{}×{}", c.model, c.qty_per_unit)
                } else {
                    format!(
                        "{} {}×{}",
                        c.model,
                        format_capacity_label(c.capacity_gb),
                        c.qty_per_unit
                    )
                }
            } else {
                format!("{}×{}", c.model, c.qty_per_unit)
            }
        })
        .collect::<Vec<_>>()
        .join(" · ")
}

// ── Device ↔ DTO mapping ──────────────────────────────────────────

pub fn device_to_model(
    d: &Device,
    spec_code: &str,
    product_name: &str,
) -> dmbit_contracts::device::DeviceModel {
    dmbit_contracts::device::DeviceModel {
        id: d.id.clone(),
        spec_id: d.spec_id.clone(),
        spec_code: spec_code.to_string(),
        product_name: product_name.to_string(),
        status: d.status.clone(),
        location: d.location.clone(),
        asset_code: d.asset_code.clone(),
        serial_no: d.serial_no.clone(),
        sort_order: d.sort_order,
        created_at: d.created_at,
        updated_at: d.updated_at,
    }
}

// ── Uniqueness checks ──────────────────────────────────────────────

pub async fn assert_product_code_available(
    ctx: &mut DbContext,
    code: &str,
    exclude_id: Option<&str>,
) -> Result<()> {
    let q = code.to_string();
    let rows = linq!(ctx.set::<Product>(), |p: Product| p.code == q)
        .to_list()
        .await
        .map_ef()?;
    if let Some(p) = rows.into_iter().next() {
        if exclude_id.map(|id| id != p.id.as_str()).unwrap_or(true) {
            return Err(Error::Conflict(format!("产品编码「{code}」已存在")));
        }
    }
    Ok(())
}

pub async fn assert_spec_code_available(
    ctx: &mut DbContext,
    code: &str,
    exclude_id: Option<&str>,
) -> Result<()> {
    let q = code.to_string();
    let rows = linq!(ctx.set::<Spec>(), |s: Spec| s.code == q)
        .to_list()
        .await
        .map_ef()?;
    if let Some(s) = rows.into_iter().next() {
        if exclude_id.map(|id| id != s.id.as_str()).unwrap_or(true) {
            return Err(Error::Conflict(format!("规格编码「{code}」已存在")));
        }
    }
    Ok(())
}

pub async fn assert_device_asset_code_available(
    ctx: &mut DbContext,
    asset_code: &str,
    exclude_id: Option<&str>,
) -> Result<()> {
    let q = asset_code.to_string();
    let rows = linq!(ctx.set::<Device>(), |d: Device| d.asset_code == q)
        .to_list()
        .await
        .map_ef()?;
    if let Some(d) = rows.into_iter().next() {
        if exclude_id.map(|id| id != d.id.as_str()).unwrap_or(true) {
            return Err(Error::Conflict(format!("资产编码「{asset_code}」已存在")));
        }
    }
    Ok(())
}

// ── Component loading ──────────────────────────────────────────────

/// Load components for given spec IDs using HashSet for O(1) lookup.
pub async fn load_components_for_specs(
    ctx: &mut DbContext,
    spec_ids: &[String],
) -> Result<std::collections::HashMap<String, Vec<ComponentModel>>> {
    let mut map: std::collections::HashMap<String, Vec<ComponentModel>> =
        std::collections::HashMap::new();
    if spec_ids.is_empty() {
        return Ok(map);
    }
    let id_set: HashSet<&str> = spec_ids.iter().map(|s| s.as_str()).collect();
    let all = linq!(ctx.set::<SpecComponent>();)
        .to_list()
        .await
        .map_ef()?;
    for c in all {
        if !id_set.contains(c.spec_id.as_str()) {
            continue;
        }
        map.entry(c.spec_id.clone())
            .or_default()
            .push(component_to_model(&c));
    }
    for list in map.values_mut() {
        list.sort_by(|a, b| a.sort_order.cmp(&b.sort_order).then(a.model.cmp(&b.model)));
    }
    Ok(map)
}

/// Load device counts per spec_id.
pub async fn load_device_counts(
    ctx: &mut DbContext,
    spec_ids: &[String],
) -> Result<std::collections::HashMap<String, i32>> {
    let mut map = std::collections::HashMap::new();
    if spec_ids.is_empty() {
        return Ok(map);
    }
    let id_set: HashSet<&str> = spec_ids.iter().map(|s| s.as_str()).collect();
    let all = linq!(ctx.set::<Device>();).to_list().await.map_ef()?;
    for d in all {
        if !id_set.contains(d.spec_id.as_str()) {
            continue;
        }
        *map.entry(d.spec_id.clone()).or_default() += 1;
    }
    Ok(map)
}

// ── Component replacement ─────────────────────────────────────────

/// Replace all components for a spec — physical delete old, insert new.
pub async fn replace_spec_components(
    ctx: &mut DbContext,
    spec_id: &str,
    components: &[ComponentModel],
) -> Result<i32> {
    let sid = spec_id.to_string();
    let existing = linq!(ctx.set::<SpecComponent>(), |c: SpecComponent| c.spec_id
        == sid)
    .to_list()
    .await
    .map_ef()?;
    // Soft-delete old records (ORM does not support physical remove on DbSet)
    for mut old in existing {
        old.is_deleted = true;
        old.updated_at = now_secs();
        old.updated_id = operator_id();
        ctx.update(old);
    }

    let now = now_secs();
    let op = operator_id();
    let mut written = 0i32;
    let mut seen: HashSet<String> = HashSet::new();
    for (idx, raw) in components.iter().enumerate() {
        let model = raw.model.trim();
        if model.is_empty() {
            continue;
        }
        if model.chars().count() > MAX_COMP_MODEL {
            return Err(Error::Validation(format!(
                "部件型号长度不能超过 {MAX_COMP_MODEL}"
            )));
        }
        let kind = normalize_comp_kind(&raw.kind)?;
        if raw.qty_per_unit < 1 {
            return Err(Error::Validation("部件单台数量至少为 1".into()));
        }
        let capacity_gb = if kind == "disk" {
            require_disk_capacity_gb(raw.capacity_gb)?
        } else if raw.capacity_gb != 0 {
            return Err(Error::Validation(
                "加速卡部件不应填写容量（capacity_gb 须为 0）".into(),
            ));
        } else {
            0
        };
        let dedupe = format!("{kind}|{model}|{capacity_gb}");
        if !seen.insert(dedupe) {
            return Err(Error::Validation(format!(
                "同一规格下部件重复：{kind} / {model} / {capacity_gb}GB"
            )));
        }
        let entity = SpecComponent {
            id: if raw.id.trim().is_empty() {
                new_id()
            } else {
                raw.id.trim().to_string()
            },
            spec_id: spec_id.to_string(),
            kind,
            model: model.to_string(),
            capacity_gb,
            qty_per_unit: raw.qty_per_unit,
            sort_order: if raw.sort_order != 0 {
                raw.sort_order
            } else {
                (idx as i32) + 1
            },
            created_id: op.clone(),
            created_at: now,
            updated_id: op.clone(),
            updated_at: now,
            is_deleted: false,
            spec: BelongsTo::new(),
        };
        ctx.add(entity);
        written += 1;
    }
    Ok(written)
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod capacity_tests {
    use super::*;

    #[test]
    fn label_roundtrip_to_gb() {
        assert_eq!(parse_capacity_label_to_gb("8TB").unwrap(), 8000);
        assert_eq!(parse_capacity_label_to_gb("8 TB").unwrap(), 8000);
        assert_eq!(parse_capacity_label_to_gb("960GB").unwrap(), 960);
        assert_eq!(parse_capacity_label_to_gb("8000").unwrap(), 8000);
        assert_eq!(parse_capacity_label_to_gb("1.2PB").unwrap(), 1_200_000);
        assert_eq!(format_capacity_label(8000), "8TB");
        assert_eq!(format_capacity_label(960), "960GB");
        assert!(parse_capacity_label_to_gb("HC320").is_err());
    }

    #[test]
    fn storage_sum_uses_integer_gb() {
        let blocks = 388i64 * 36;
        let gb = blocks * 8000;
        assert_eq!(gb, 111_744_000);
        assert_eq!(gb / 1_000_000, 111);
    }
}
