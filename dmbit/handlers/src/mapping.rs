//! Shared goods → DTO mapping + component helpers.

use dmbit_contracts::goods::{ComponentModel, GoodsModel};
use dmbit_domain::entities::{Goods, GoodsComponent};
use dmbit_domain::new_id;
use rust_ef::{db_context::DbContext, prelude::*};
use rust_webx::*;

use crate::db::{save_changes, EfResultExt};
use crate::util::{now_secs, operator_id};

pub fn normalize_status(raw: &str) -> String {
    match raw.trim() {
        "运行中" | "联调中" | "待上架" | "已交付" => raw.trim().to_string(),
        "" => "待上架".into(),
        other => other.to_string(),
    }
}

pub fn normalize_category(raw: &str) -> String {
    let t = raw.trim();
    match t.to_ascii_lowercase().as_str() {
        "storage" | "存储" => "storage".into(),
        // compute / 算力 / empty / unknown → compute
        _ => "compute".into(),
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

pub fn component_to_model(c: &GoodsComponent) -> ComponentModel {
    ComponentModel {
        id: c.id.clone(),
        kind: c.kind.clone(),
        model: c.model.clone(),
        capacity: c.capacity.clone(),
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
                if c.capacity.is_empty() {
                    format!("{}×{}", c.model, c.qty_per_unit)
                } else {
                    format!("{} {}×{}", c.model, c.capacity, c.qty_per_unit)
                }
            } else {
                format!("{}×{}", c.model, c.qty_per_unit)
            }
        })
        .collect::<Vec<_>>()
        .join(" · ")
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
    for (idx, raw) in components.iter().enumerate() {
        let model = raw.model.trim();
        if model.is_empty() {
            continue;
        }
        let kind = normalize_comp_kind(&raw.kind)?;
        if raw.qty_per_unit < 1 {
            return Err(Error::Validation("部件单台数量至少为 1".into()));
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
            capacity: raw.capacity.trim().to_string(),
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
    save_changes(ctx).await?;
    Ok(written)
}

pub fn parse_tb(capacity: &str) -> f64 {
    let s = capacity.trim().to_ascii_uppercase().replace(' ', "");
    if s.is_empty() {
        return 0.0;
    }
    let num: String = s.chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
    let n: f64 = num.parse().unwrap_or(0.0);
    if s.contains("PB") {
        n * 1024.0
    } else if s.contains("TB") {
        n
    } else if s.contains("GB") {
        n / 1024.0
    } else {
        n
    }
}

/// MW per accelerator card (approx).
pub fn mw_per_card(model: &str) -> f64 {
    let m = model.to_ascii_uppercase();
    if m.contains("5090") {
        0.000533
    } else if m.contains("4090") {
        0.000533
    } else if m.contains("ASCEND") || m.contains("910") {
        0.0004
    } else {
        0.0003
    }
}
