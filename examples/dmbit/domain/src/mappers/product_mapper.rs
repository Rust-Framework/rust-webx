//! Product ↔ `ProductModel` mapping (plus the legacy `GoodsModel` view the
//! admin panel still reads).

use dmbit_contracts::goods::GoodsModel;
use dmbit_contracts::product::{CreateProductRequest, ProductModel, UpdateProductRequest};
use dmbit_contracts::spec::SpecModel;
use rust_ef::prelude::*;
use webx_core::{Error, IPatchable, Result};

use crate::entities::Product;
use crate::ids::new_id;
use crate::mappers::{now_secs, operator_id};
use crate::rules::{
    normalize_category, optional_text, require_text, MAX_PRODUCT_CODE, MAX_PRODUCT_NAME,
    MAX_PRODUCT_REMARK,
};

impl From<&Product> for ProductModel {
    /// The product row alone; `goods` stays empty until the spec tree is known.
    fn from(p: &Product) -> Self {
        ProductModel {
            id: p.id.clone(),
            name: p.name.clone(),
            code: p.code.clone(),
            category: p.category.clone(),
            remark: p.remark.clone(),
            sort_order: p.sort_order,
            created_at: p.created_at,
            updated_at: p.updated_at,
            goods: Vec::new(),
        }
    }
}

/// Product + its specs flattened into the legacy `goods` list.
pub fn product_with_specs(p: &Product, specs: &[SpecModel]) -> ProductModel {
    let mut model = ProductModel::from(p);
    model.goods = specs.iter().map(spec_to_goods).collect();
    model
}

/// Legacy admin-panel view of one spec as a goods row.
pub fn spec_to_goods(s: &SpecModel) -> GoodsModel {
    GoodsModel {
        id: s.id.clone(),
        product_id: s.product_id.clone(),
        product_name: s.product_name.clone(),
        code: s.code.clone(),
        brand: s.brand.clone(),
        parameters: s.parameters.clone(),
        unit: s.unit.clone(),
        quantity: s.planned_quantity,
        planned_quantity: s.planned_quantity,
        status: String::new(),
        location: String::new(),
        asset_code: String::new(),
        sort_order: s.sort_order,
        created_at: s.created_at,
        updated_at: s.updated_at,
        components: s.components.clone(),
        device_count: s.device_count,
    }
}

impl TryFrom<CreateProductRequest> for Product {
    type Error = Error;

    fn try_from(req: CreateProductRequest) -> Result<Self> {
        let name = require_text("产品名称", &req.name, MAX_PRODUCT_NAME)?;
        let code = require_text("产品编码", &req.code, MAX_PRODUCT_CODE)?;
        let remark = optional_text("备注", &req.remark, MAX_PRODUCT_REMARK)?;
        let category = normalize_category(&req.category)?;

        let now = now_secs();
        let actor = operator_id();
        Ok(Product {
            id: new_id(),
            name,
            code,
            category,
            remark,
            sort_order: req.sort_order,
            created_id: actor.clone(),
            created_at: now,
            updated_id: actor,
            updated_at: now,
            is_deleted: false,
            specs: HasMany::new(),
        })
    }
}

/// Validate and normalize a partial product update before [`IPatchable::patch`].
pub fn normalize_update_product(req: &mut UpdateProductRequest) -> Result<()> {
    if let Some(v) = req.name.take() {
        req.name = Some(require_text("产品名称", &v, MAX_PRODUCT_NAME)?);
    }
    if let Some(v) = req.code.take() {
        req.code = Some(require_text("产品编码", &v, MAX_PRODUCT_CODE)?);
    }
    if let Some(v) = req.category.take() {
        req.category = Some(normalize_category(&v)?);
    }
    if let Some(v) = req.remark.take() {
        req.remark = Some(optional_text("备注", &v, MAX_PRODUCT_REMARK)?);
    }
    Ok(())
}

impl IPatchable<UpdateProductRequest> for Product {
    fn patch(&mut self, patch: UpdateProductRequest) {
        if let Some(v) = patch.name {
            self.name = v;
        }
        if let Some(v) = patch.code {
            self.code = v;
        }
        if let Some(v) = patch.category {
            self.category = v;
        }
        if let Some(v) = patch.remark {
            self.remark = v;
        }
        if let Some(v) = patch.sort_order {
            self.sort_order = v;
        }
    }
}

/// Server-trusted values for a new product row.
///
/// `id`, `now` and `actor` are chosen by the application layer; the seed exists
/// so the entity is still built through one `From` conversion.
pub struct ProductSeed {
    pub id: String,
    pub name: String,
    pub code: String,
    pub category: String,
    pub remark: String,
    pub sort_order: i32,
    pub actor: Option<String>,
    pub now: i64,
}

impl From<ProductSeed> for Product {
    fn from(seed: ProductSeed) -> Self {
        Product {
            id: seed.id,
            name: seed.name,
            code: seed.code,
            category: seed.category,
            remark: seed.remark,
            sort_order: seed.sort_order,
            created_id: seed.actor.clone(),
            created_at: seed.now,
            updated_id: seed.actor,
            updated_at: seed.now,
            is_deleted: false,
            specs: HasMany::new(),
        }
    }
}
