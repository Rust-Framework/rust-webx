//! SpecComponent ↔ `ComponentModel` mapping and the validated component list.

use std::collections::HashSet;

use dmbit_contracts::goods::ComponentModel;
use rust_ef::prelude::*;
use webx_core::{Error, Result};

use crate::entities::SpecComponent;
use crate::ids::new_id;
use crate::rules::{normalize_comp_kind, require_disk_capacity_gb, MAX_COMP_MODEL};

impl From<&SpecComponent> for ComponentModel {
    fn from(c: &SpecComponent) -> Self {
        ComponentModel {
            id: c.id.clone(),
            kind: c.kind.clone(),
            model: c.model.clone(),
            capacity_gb: c.capacity_gb,
            qty_per_unit: c.qty_per_unit,
            sort_order: c.sort_order,
        }
    }
}

/// A validated component row ready to be inserted.
pub struct SpecComponentSeed {
    pub id: String,
    pub spec_id: String,
    pub kind: String,
    pub model: String,
    pub capacity_gb: i64,
    pub qty_per_unit: i32,
    pub sort_order: i32,
    pub actor: Option<String>,
    pub now: i64,
}

impl From<SpecComponentSeed> for SpecComponent {
    fn from(seed: SpecComponentSeed) -> Self {
        SpecComponent {
            id: seed.id,
            spec_id: seed.spec_id,
            kind: seed.kind,
            model: seed.model,
            capacity_gb: seed.capacity_gb,
            qty_per_unit: seed.qty_per_unit,
            sort_order: seed.sort_order,
            created_id: seed.actor.clone(),
            created_at: seed.now,
            updated_id: seed.actor,
            updated_at: seed.now,
            is_deleted: false,
            spec: BelongsTo::new(),
        }
    }
}

/// Validate a request's component list into insertable seeds.
///
/// Duplicate `(kind, model, capacity_gb)` rows are rejected here, before any
/// database write, so the failure is a `400` field error instead of a driver
/// constraint violation.
pub fn component_seeds(
    spec_id: &str,
    components: &[ComponentModel],
    now: i64,
    actor: Option<String>,
) -> Result<Vec<SpecComponentSeed>> {
    let mut seeds = Vec::with_capacity(components.len());
    let mut seen: HashSet<String> = HashSet::new();
    for (idx, raw) in components.iter().enumerate() {
        let model = raw.model.trim();
        if model.is_empty() {
            continue;
        }
        if model.chars().count() > MAX_COMP_MODEL {
            return Err(Error::validation(format!(
                "部件型号长度不能超过 {MAX_COMP_MODEL}"
            )));
        }
        let kind = normalize_comp_kind(&raw.kind)?;
        if raw.qty_per_unit < 1 {
            return Err(Error::validation("部件单台数量至少为 1"));
        }
        let capacity_gb = if kind == "disk" {
            require_disk_capacity_gb(raw.capacity_gb)?
        } else if raw.capacity_gb != 0 {
            return Err(Error::validation(
                "加速卡部件不应填写容量（capacity_gb 须为 0）",
            ));
        } else {
            0
        };
        let dedupe = format!("{kind}|{model}|{capacity_gb}");
        if !seen.insert(dedupe) {
            return Err(Error::validation(format!(
                "同一规格下部件重复：{kind} / {model} / {capacity_gb}GB"
            )));
        }
        seeds.push(SpecComponentSeed {
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
            actor: actor.clone(),
            now,
        });
    }
    Ok(seeds)
}
