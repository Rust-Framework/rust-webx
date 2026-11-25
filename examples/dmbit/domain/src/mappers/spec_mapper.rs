//! Spec ↔ `SpecModel` mapping.

use dmbit_contracts::goods::ComponentModel;
use dmbit_contracts::spec::{CreateSpecRequest, SpecModel, UpdateSpecRequest};
use rust_ef::prelude::*;
use webx_core::{Error, IPatchable, Result};

use crate::entities::Spec;
use crate::ids::new_id;
use crate::mappers::{now_secs, operator_id};
use crate::rules::{require_non_negative, require_text, MAX_BRAND, MAX_SPEC_CODE, MAX_UNIT};

/// One spec row with the joined values the read model needs.
///
/// `device_count` is a count over `Device` rows, so it is supplied by the
/// application layer rather than fetched by the mapper.
pub fn spec_model(
    s: &Spec,
    product_name: &str,
    components: Vec<ComponentModel>,
    device_count: i32,
) -> SpecModel {
    SpecModel {
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

/// One-line component summary for the big-screen device rows.
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
                        crate::rules::format_capacity_label(c.capacity_gb),
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

impl TryFrom<CreateSpecRequest> for Spec {
    type Error = Error;

    fn try_from(req: CreateSpecRequest) -> Result<Self> {
        let product_id = req.product_id.trim();
        if product_id.is_empty() {
            return Err(Error::validation("标识不能为空"));
        }
        if product_id.chars().count() > 36 {
            return Err(Error::validation("标识格式无效"));
        }
        let code = require_text("规格编码", &req.code, MAX_SPEC_CODE)?;
        let brand = require_text("品牌", &req.brand, MAX_BRAND)?;
        let unit = require_text("单位", &req.unit, MAX_UNIT)?;
        let planned_quantity = require_non_negative("计划数量", req.planned_quantity)?;

        let now = now_secs();
        let actor = operator_id();
        Ok(Spec {
            id: new_id(),
            product_id: product_id.to_string(),
            code,
            brand,
            parameters: req.parameters,
            unit,
            planned_quantity,
            sort_order: req.sort_order,
            created_id: actor.clone(),
            created_at: now,
            updated_id: actor,
            updated_at: now,
            is_deleted: false,
            product: BelongsTo::new(),
            components: HasMany::new(),
            devices: HasMany::new(),
        })
    }
}

/// Validate and normalize a partial spec update before [`IPatchable::patch`].
///
/// `product_id` and `components` are left to the handler: moving a spec needs the
/// context, and the component list is a delete-then-insert round trip.
pub fn normalize_update_spec(req: &mut UpdateSpecRequest) -> Result<()> {
    if let Some(v) = req.code.take() {
        req.code = Some(require_text("规格编码", &v, MAX_SPEC_CODE)?);
    }
    if let Some(v) = req.brand.take() {
        req.brand = Some(require_text("品牌", &v, MAX_BRAND)?);
    }
    if let Some(v) = req.unit.take() {
        req.unit = Some(require_text("单位", &v, MAX_UNIT)?);
    }
    if let Some(v) = req.planned_quantity {
        req.planned_quantity = Some(require_non_negative("计划数量", v)?);
    }
    Ok(())
}

impl IPatchable<UpdateSpecRequest> for Spec {
    fn patch(&mut self, patch: UpdateSpecRequest) {
        if let Some(v) = patch.code {
            self.code = v;
        }
        if let Some(v) = patch.brand {
            self.brand = v;
        }
        if let Some(v) = patch.parameters {
            self.parameters = v;
        }
        if let Some(v) = patch.unit {
            self.unit = v;
        }
        if let Some(v) = patch.planned_quantity {
            self.planned_quantity = v;
        }
        if let Some(v) = patch.sort_order {
            self.sort_order = v;
        }
    }
}

/// Server-trusted values for a new spec row.
pub struct SpecSeed {
    pub id: String,
    pub product_id: String,
    pub code: String,
    pub brand: String,
    pub parameters: String,
    pub unit: String,
    pub planned_quantity: i32,
    pub sort_order: i32,
    pub actor: Option<String>,
    pub now: i64,
}

impl From<SpecSeed> for Spec {
    fn from(seed: SpecSeed) -> Self {
        Spec {
            id: seed.id,
            product_id: seed.product_id,
            code: seed.code,
            brand: seed.brand,
            parameters: seed.parameters,
            unit: seed.unit,
            planned_quantity: seed.planned_quantity,
            sort_order: seed.sort_order,
            created_id: seed.actor.clone(),
            created_at: seed.now,
            updated_id: seed.actor,
            updated_at: seed.now,
            is_deleted: false,
            product: BelongsTo::new(),
            components: HasMany::new(),
            devices: HasMany::new(),
        }
    }
}
