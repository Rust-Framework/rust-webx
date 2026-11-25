//! Pure domain rules — field constraints, enumerated vocabulary, capacity units.
//!
//! No HTTP, no DbContext, no I/O: every function here is a decision the service
//! makes about its own data. Handlers call them and propagate the framework
//! error as-is; request-field rules that the binding layer can express are
//! declared with `#[validate]` in the contracts instead of being repeated here.

use webx_core::{Error, Result};

/// The device status vocabulary (also the big-screen buckets).
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

/// Default status for a device that has not been racked yet.
pub const DEFAULT_STATUS: &str = "待上架";

/// Normalize a device status; blank means `待上架`.
pub fn normalize_status(raw: &str) -> Result<String> {
    let t = raw.trim();
    if t.is_empty() {
        return Ok(DEFAULT_STATUS.into());
    }
    if STATUSES.contains(&t) {
        return Ok(t.to_string());
    }
    Err(Error::validation(format!(
        "状态无效「{t}」（支持：{}）",
        STATUSES.join(" / ")
    )))
}

/// Normalize a product category; blank means `compute`.
pub fn normalize_category(raw: &str) -> Result<String> {
    let t = raw.trim();
    if t.is_empty() {
        return Ok("compute".into());
    }
    match t.to_ascii_lowercase().as_str() {
        "storage" | "存储" => Ok("storage".into()),
        "compute" | "算力" => Ok("compute".into()),
        _ => Err(Error::validation(format!(
            "类别无效「{t}」（支持：算力/存储，或 compute/storage）"
        ))),
    }
}

/// Normalize a component kind; only `accelerator` / `disk` reach the database.
pub fn normalize_comp_kind(raw: &str) -> Result<String> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "accelerator" | "gpu" | "npu" | "加速卡" | "卡" => Ok("accelerator".into()),
        "disk" | "hdd" | "ssd" | "硬盘" | "盘" => Ok("disk".into()),
        "" => Err(Error::validation("部件类型不能为空")),
        _ => Err(Error::validation(format!(
            "部件类型无效: {raw}（支持：加速卡/硬盘，或 accelerator/disk）"
        ))),
    }
}

/// Trimmed, non-empty text no longer than `max` characters.
pub fn require_text(field: &str, raw: &str, max: usize) -> Result<String> {
    let t = raw.trim();
    if t.is_empty() {
        return Err(Error::validation(format!("{field}不能为空")));
    }
    if t.chars().count() > max {
        return Err(Error::validation(format!("{field}长度不能超过 {max}")));
    }
    Ok(t.to_string())
}

/// Trimmed text no longer than `max` characters; may be empty.
pub fn optional_text(field: &str, raw: &str, max: usize) -> Result<String> {
    let t = raw.trim();
    if t.chars().count() > max {
        return Err(Error::validation(format!("{field}长度不能超过 {max}")));
    }
    Ok(t.to_string())
}

pub fn require_non_negative(field: &str, n: i32) -> Result<i32> {
    if n < 0 {
        return Err(Error::validation(format!("{field}不能为负")));
    }
    Ok(n)
}

/// Human label for an integer GB capacity (`8000` → `8TB`).
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

/// Parse a human capacity label into integer GB (`8TB` → `8000`).
///
/// Money/inventory style: the stored value is always an integer number of GB,
/// so aggregations never go through `f64`.
pub fn parse_capacity_label_to_gb(raw: &str) -> Result<i64> {
    let t = raw.trim();
    if t.is_empty() {
        return Err(Error::validation("容量不能为空"));
    }
    if let Ok(n) = t.parse::<i64>() {
        if n <= 0 {
            return Err(Error::validation(format!("容量必须为正整数 GB「{t}」")));
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
        Error::validation(format!("容量数值无效「{t}」（示例：8TB / 960GB / 8000）"))
    })?;
    if !num.is_finite() || num <= 0.0 {
        return Err(Error::validation(format!("容量必须为正数「{t}」")));
    }
    let unit = unit_part.trim().to_ascii_uppercase();
    let gb_f = match unit.as_str() {
        "GB" | "G" => num,
        "TB" | "T" => num * 1000.0,
        "PB" | "P" => num * 1_000_000.0,
        "" => {
            return Err(Error::validation(format!(
                "容量缺少单位「{t}」（需带 TB/GB/PB，或直接写整数 GB）"
            )));
        }
        _ => {
            return Err(Error::validation(format!(
                "容量单位无效「{t}」（支持：TB / GB / PB）"
            )));
        }
    };
    let gb = gb_f.round();
    if (gb_f - gb).abs() > 1e-6 {
        return Err(Error::validation(format!("容量必须能换算为整数 GB「{t}」")));
    }
    let gb = gb as i64;
    if gb <= 0 {
        return Err(Error::validation(format!("容量必须为正「{t}」")));
    }
    Ok(gb)
}

/// A disk component must carry a positive integer GB capacity.
pub fn require_disk_capacity_gb(gb: i64) -> Result<i64> {
    if gb <= 0 {
        return Err(Error::validation(
            "硬盘容量必须为正整数（单位 GB，例如 8TB 请填 8000）",
        ));
    }
    Ok(gb)
}

/// Display label for a product category (CSV export).
pub fn category_label(c: &str) -> &'static str {
    match c.trim() {
        "storage" => "存储",
        _ => "算力",
    }
}

/// Display label for a component kind (CSV export).
pub fn kind_label(k: &str) -> &'static str {
    match k.trim() {
        "disk" => "硬盘",
        _ => "加速卡",
    }
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
        let blocks = 388i64 * 36;
        let gb = blocks * 8000;
        assert_eq!(gb, 111_744_000);
        assert_eq!(gb / 1_000_000, 111);
    }

    #[test]
    fn status_and_category_vocabularies_are_closed() {
        assert_eq!(normalize_status("  ").unwrap(), DEFAULT_STATUS);
        assert!(normalize_status("已报废").is_err());
        assert_eq!(normalize_category("存储").unwrap(), "storage");
        assert!(normalize_category("网络").is_err());
    }
}
