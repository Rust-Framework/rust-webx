//! Dashboard handler — 智算机房管理.
//!
//! Aggregates are sums of persisted goods + components only.
//! Storage capacity = Σ(disk blocks × capacity_gb) — integer GB on the ledger.

use std::collections::HashMap;
use std::sync::Arc;

use rust_ef::{db_context::DbContext, prelude::*};
use rust_webx::*;

use dmbit_contracts::dashboard::*;
use dmbit_contracts::product::ProductModel;
use dmbit_contracts::site::SiteConfig;
use dmbit_domain::entities::{Goods, Product};

use crate::db::EfResultExt;
use crate::mapping::{
    format_capacity_label, goods_to_model, load_components_for, parts_summary,
};

#[derive(Inject)]
pub struct GetDashboardHandler {
    #[inject(owned)]
    ctx: DbContext,
    #[inject]
    site: Arc<SiteConfig>,
}

fn push_part_total(
    map: &mut HashMap<String, PartTotal>,
    kind: &str,
    model: &str,
    capacity_gb: i64,
    add: i32,
    unit: &str,
) {
    let capacity_label = if kind == "disk" && capacity_gb > 0 {
        format_capacity_label(capacity_gb)
    } else {
        String::new()
    };
    let key = format!("{kind}|{model}|{capacity_gb}");
    let label = if kind == "disk" && !capacity_label.is_empty() {
        format!("{model} · {capacity_label}")
    } else {
        model.to_string()
    };
    let entry = map.entry(key).or_insert_with(|| PartTotal {
        kind: kind.into(),
        model: model.into(),
        capacity: capacity_label,
        label,
        count: 0,
        unit: unit.into(),
    });
    entry.count += add;
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<GetDashboardRequest, DashboardModel> for GetDashboardHandler {
    async fn handle(&mut self, _: GetDashboardRequest) -> Result<DashboardModel> {
        let mut products = linq!(self.ctx.set::<Product>();)
            .to_list()
            .await
            .map_ef()?;
        products.sort_by(|a, b| a.sort_order.cmp(&b.sort_order));

        let all_goods = linq!(self.ctx.set::<Goods>();).to_list().await.map_ef()?;
        let ids: Vec<String> = all_goods.iter().map(|g| g.id.clone()).collect();
        let cmap = load_components_for(&mut self.ctx, &ids).await?;

        let mut models = Vec::with_capacity(products.len());
        let mut devices = Vec::new();
        let mut total_quantity = 0i32;
        let mut compute_quantity = 0i32;
        let mut storage_quantity = 0i32;
        let mut running = 0i32;
        let mut commissioning = 0i32;
        let mut pending = 0i32;
        let mut delivered = 0i32;
        let mut accel_map: HashMap<String, PartTotal> = HashMap::new();
        let mut disk_map: HashMap<String, PartTotal> = HashMap::new();
        let mut storage_capacity_gb = 0i64;

        for p in &products {
            let mut goods_models = Vec::new();
            for g in all_goods.iter().filter(|g| g.product_id == p.id) {
                total_quantity += g.quantity;
                match g.status.as_str() {
                    "运行中" => running += g.quantity,
                    "联调中" => commissioning += g.quantity,
                    "待上架" => pending += g.quantity,
                    "已交付" => delivered += g.quantity,
                    _ => {}
                }

                if p.category.eq_ignore_ascii_case("storage") {
                    storage_quantity += g.quantity;
                } else {
                    compute_quantity += g.quantity;
                }

                let comps = cmap.get(&g.id).cloned().unwrap_or_default();
                for c in &comps {
                    let total = g.quantity.saturating_mul(c.qty_per_unit);
                    if c.kind == "disk" {
                        push_part_total(
                            &mut disk_map,
                            "disk",
                            &c.model,
                            c.capacity_gb,
                            total,
                            "块",
                        );
                        storage_capacity_gb = storage_capacity_gb
                            .saturating_add(i64::from(total).saturating_mul(c.capacity_gb));
                    } else if c.kind == "accelerator" {
                        push_part_total(
                            &mut accel_map,
                            "accelerator",
                            &c.model,
                            0,
                            total,
                            "张",
                        );
                    }
                }

                devices.push(DeviceOverviewRow {
                    id: g.id.clone(),
                    product_id: g.product_id.clone(),
                    product_name: p.name.clone(),
                    product_category: p.category.clone(),
                    brand: g.brand.clone(),
                    parts_summary: parts_summary(&comps),
                    quantity: g.quantity,
                    unit: g.unit.clone(),
                    status: g.status.clone(),
                    location: g.location.clone(),
                    asset_code: g.asset_code.clone(),
                    parameters: g.parameters.clone(),
                    sort_order: g.sort_order,
                });

                goods_models.push(goods_to_model(g.clone(), &p.name, comps));
            }
            goods_models.sort_by(|a, b| a.sort_order.cmp(&b.sort_order));
            models.push(ProductModel {
                id: p.id.clone(),
                name: p.name.clone(),
                code: p.code.clone(),
                category: p.category.clone(),
                remark: p.remark.clone(),
                sort_order: p.sort_order,
                created_at: p.created_at,
                updated_at: p.updated_at,
                goods: goods_models,
            });
        }

        devices.sort_by(|a, b| {
            a.sort_order
                .cmp(&b.sort_order)
                .then(a.brand.cmp(&b.brand))
        });

        let mut accelerator_totals: Vec<PartTotal> = accel_map.into_values().collect();
        accelerator_totals.sort_by(|a, b| b.count.cmp(&a.count).then(a.label.cmp(&b.label)));
        let mut disk_totals: Vec<PartTotal> = disk_map.into_values().collect();
        disk_totals.sort_by(|a, b| b.count.cmp(&a.count).then(a.label.cmp(&b.label)));

        Ok(DashboardModel {
            title: if self.site.title.is_empty() {
                "智算机房设备概览".into()
            } else {
                self.site.title.clone()
            },
            brand_name: self.site.brand_name.clone(),
            room_name: if self.site.room_name.is_empty() {
                "直播数据智算机房".into()
            } else {
                self.site.room_name.clone()
            },
            stats: DashboardStats {
                product_count: models.len() as i32,
                goods_count: all_goods.len() as i32,
                total_quantity,
                compute_quantity,
                storage_quantity,
                storage_capacity_gb,
                running_quantity: running,
                commissioning_quantity: commissioning,
                pending_quantity: pending,
                delivered_quantity: delivered,
                status_buckets: vec![
                    StatusBucket {
                        status: "运行中".into(),
                        quantity: running,
                    },
                    StatusBucket {
                        status: "联调中".into(),
                        quantity: commissioning,
                    },
                    StatusBucket {
                        status: "待上架".into(),
                        quantity: pending,
                    },
                    StatusBucket {
                        status: "已交付".into(),
                        quantity: delivered,
                    },
                ],
            },
            accelerator_totals,
            disk_totals,
            products: models,
            devices,
        })
    }
}
