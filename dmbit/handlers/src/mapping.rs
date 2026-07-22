//! Shared goods → DTO mapping + component / uniqueness helpers.

use std::collections::HashSet;

use dmbit_contracts::goods::{ComponentModel, GoodsModel};
use dmbit_domain::entities::{Goods, GoodsComponent, Product};
use dmbit_domain::new_id;
use rust_ef::{db_context::DbContext, prelude::*};
use rust_webx::*;

use crate::db::EfResultExt;
use crate::util::{now_secs, operator_id};

/// Allowed goods statuses — must stay in sync with dashboard buckets / admin options.
pub const GOODS_STATUSES: &[&str] = &["运行中", "联调中", "待上架", "已交付"];

pub const MAX_PRODUCT_NAME: usize = 100;
pub const MAX_PRODUCT_CODE: usize = 50;
pub const MAX_PRODUCT_REMARK: usize = 500;
pub const MAX_BRAND: usize = 100;
pub const MAX_UNIT: usize = 20;
pub const MAX_STATUS: usize = 20;
pub const MAX_LOCATION: usize = 100;
pub const MAX_ASSET_CODE: usize = 50;
pub const MAX_COMP_MODEL: usize = 80;

pub fn normalize_status(raw: &str) -> Result<String> {
    let t = raw.trim();
    if t.is_empty() {
        return Ok("待上架".into());
    }
    if GOODS_STATUSES.contains(&t) {
        return Ok(t.to_string());
    }
    Err(Error::Validation(format!(
        "状态无效「{t}」（支持：{}）",
        GOODS_STATUSES.join(" / ")
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
        other if other.is_empty() => Err(Error::Validation("部件类型不能为空".into())),
        _ => Err(Error::Validation(format!(
            "部件类型无效: {raw}（支持：加速卡/硬盘，或 accelerator/disk）"
        ))),
    }
}

/// Format GB for display / CSV (decimal: 1000 GB = 1 TB, 1000 TB = 1 PB).
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

/// CSV / Excel boundary only: parse human labels like `8TB` into integer **GB**.
/// Persisted model uses `capacity_gb`; never use the label string for aggregation.
pub fn parse_capacity_label_to_gb(raw: &str) -> Result<i64> {
    let t = raw.trim();
    if t.is_empty() {
        return Err(Error::Validation("容量不能为空".into()));
    }
    // Bare integer → GB
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
        Error::Validation(format!(
            "容量数值无效「{t}」（示例：8TB / 960GB / 8000）"
        ))
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
        return Err(Error::Validation(format!(
            "容量必须能换算为整数 GB「{t}」"
        )));
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

/// When disk components exist, drop free-text「硬盘」parameter lines (avoid duplicate UI attrs).
pub fn strip_redundant_disk_params(parameters: &str, components: &[ComponentModel]) -> String {
    if !components.iter().any(|c| c.kind == "disk") {
        return parameters.to_string();
    }
    parameters
        .lines()
        .filter(|line| {
            let t = line.trim();
            if t.is_empty() {
                return false;
            }
            if let Some((k, _)) = t.split_once('：').or_else(|| t.split_once(':')) {
                if k.trim() == "硬盘" {
                    return false;
                }
            }
            true
        })
        .collect::<Vec<_>>()
        .join("\n")
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

pub fn component_to_model(c: &GoodsComponent) -> ComponentModel {
    ComponentModel {
        id: c.id.clone(),
        kind: c.kind.clone(),
        model: c.model.clone(),
        capacity_gb: c.capacity_gb,
        qty_per_unit: c.qty_per_unit,
        sort_order: c.sort_order,
    }
}

pub fn goods_to_model(g: Goods, product_name: &str, components: Vec<ComponentModel>) -> GoodsModel {
    GoodsModel {
        id: g.id,
        product_id: g.product_id,
        product_name: product_name.to_string(),
        brand: g.brand,
        parameters: g.parameters,
        unit: g.unit,
        quantity: g.quantity,
        status: g.status,
        location: g.location,
        asset_code: g.asset_code,
        sort_order: g.sort_order,
        created_at: g.created_at,
        updated_at: g.updated_at,
        components,
    }
}

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

pub async fn assert_goods_key_available(
    ctx: &mut DbContext,
    product_id: &str,
    brand: &str,
    asset_code: &str,
    location: &str,
    exclude_id: Option<&str>,
) -> Result<()> {
    let pid = product_id.to_string();
    let rows = linq!(ctx.set::<Goods>(), |g: Goods| g.product_id == pid)
        .to_list()
        .await
        .map_ef()?;
    for g in rows {
        if exclude_id.map(|id| id == g.id.as_str()).unwrap_or(false) {
            continue;
        }
        if g.brand == brand && g.asset_code == asset_code && g.location == location {
            return Err(Error::Conflict(format!(
                "台账已存在：{brand} / {asset_code} / {location}"
            )));
        }
    }
    Ok(())
}

pub async fn load_components_for(
    ctx: &mut DbContext,
    goods_ids: &[String],
) -> Result<std::collections::HashMap<String, Vec<ComponentModel>>> {
    let mut map: std::collections::HashMap<String, Vec<ComponentModel>> =
        std::collections::HashMap::new();
    if goods_ids.is_empty() {
        return Ok(map);
    }
    let all = linq!(ctx.set::<GoodsComponent>();).to_list().await.map_ef()?;
    for c in all {
        if !goods_ids.iter().any(|id| id == &c.goods_id) {
            continue;
        }
        map.entry(c.goods_id.clone())
            .or_default()
            .push(component_to_model(&c));
    }
    for list in map.values_mut() {
        list.sort_by(|a, b| a.sort_order.cmp(&b.sort_order).then(a.model.cmp(&b.model)));
    }
    Ok(map)
}

pub async fn replace_components(
    ctx: &mut DbContext,
    goods_id: &str,
    components: &[ComponentModel],
) -> Result<i32> {
    let now = now_secs();
    let op = operator_id();
    let gid = goods_id.to_string();
    let existing = linq!(ctx.set::<GoodsComponent>(), |c: GoodsComponent| c.goods_id == gid)
        .to_list()
        .await
        .map_ef()?;
    for mut old in existing {
        old.is_deleted = true;
        old.updated_at = now;
        old.updated_id = op.clone();
        ctx.set::<GoodsComponent>().update(old);
    }

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
                "同一台账下部件重复：{kind} / {model} / {capacity_gb}GB"
            )));
        }
        let entity = GoodsComponent {
            id: if raw.id.trim().is_empty() {
                new_id()
            } else {
                raw.id.trim().to_string()
            },
            goods_id: goods_id.to_string(),
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
            goods: BelongsTo::new(),
        };
        ctx.set::<GoodsComponent>().add(entity);
        written += 1;
    }
    // Caller owns the Unit of Work flush (single save_changes for atomic batches).
    Ok(written)
}

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
        // 388 台 × 36 块 × 8000 GB
        let blocks = 388i64 * 36;
        let gb = blocks * 8000;
        assert_eq!(gb, 111_744_000);
        assert_eq!(gb / 1_000_000, 111); // PB integer part
    }
}
