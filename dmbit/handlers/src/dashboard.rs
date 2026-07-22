//! Dashboard handler — 智算机房管理.

use std::collections::HashMap;
use std::sync::Arc;

use rust_ef::{db_context::DbContext, prelude::*};
use rust_webx::*;

use dmbit_contracts::dashboard::*;
use dmbit_contracts::goods::ComponentModel;
use dmbit_contracts::product::ProductModel;
use dmbit_contracts::site::SiteConfig;
use dmbit_domain::entities::{Goods, Product};

use crate::db::EfResultExt;
use crate::mapping::{
    goods_to_model, load_components_for, mw_per_card, parse_tb, parts_summary,
};

#[derive(Inject)]
pub struct GetDashboardHandler {
    #[inject(owned)]
    ctx: DbContext,
    #[inject]
    site: Arc<SiteConfig>,
}

fn after_colon(line: &str) -> String {
    line.split(['：', ':'])
        .nth(1)
        .unwrap_or("")
        .trim()
        .to_string()
}

fn config_summary(parameters: &str) -> String {
    let lines: Vec<&str> = parameters
        .lines()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect();

    let chassis = lines
        .iter()
        .find(|l| l.contains("机箱") || l.contains("4U"))
        .map(|l| {
            if l.contains("4U") {
                "4U".into()
            } else {
                after_colon(l)
            }
        })
        .unwrap_or_else(|| "4U".into());

    let optic = lines
        .iter()
        .find(|l| l.contains("光模块"))
        .map(|l| after_colon(l))
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "双光千兆".into());

    format!("{chassis} / {optic}")
}

fn goods_metrics(quantity: i32, components: &[ComponentModel]) -> (f64, f64, String, bool) {
    let qty = quantity as f64;
    let mut storage_pb = 0.0;
    let mut power_mw = 0.0;
    let mut has_disk = false;
    let mut has_accel = false;
    let mut visual = "compute".to_string();

    for c in components {
        let total_parts = qty * c.qty_per_unit as f64;
        if c.kind == "disk" {
            has_disk = true;
            storage_pb += total_parts * parse_tb(&c.capacity) / 1024.0;
            visual = "storage".into();
        } else if c.kind == "accelerator" {
            has_accel = true;
            power_mw += total_parts * mw_per_card(&c.model);
            let m = c.model.to_ascii_uppercase();
            if m.contains("5090") {
                visual = "gpu5090".into();
            } else if m.contains("4090") {
                visual = "gpu4090".into();
            }
        }
    }

    let featured = has_disk || has_accel;
    (storage_pb, power_mw, visual, featured)
}

fn push_part_total(
    map: &mut HashMap<String, PartTotal>,
    kind: &str,
    model: &str,
    capacity: &str,
    add: i32,
    unit: &str,
) {
    let key = format!("{kind}|{model}|{capacity}");
    let label = if kind == "disk" && !capacity.is_empty() {
        format!("{model} · {capacity}")
    } else {
        model.to_string()
    };
    let entry = map.entry(key).or_insert_with(|| PartTotal {
        kind: kind.into(),
        model: model.into(),
        capacity: capacity.into(),
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
        let mut storage_pb = 0.0;
        let mut power_mw = 0.0;
        let mut accel_map: HashMap<String, PartTotal> = HashMap::new();
        let mut disk_map: HashMap<String, PartTotal> = HashMap::new();

        for p in &products {
            let mut goods_models = Vec::new();
            for g in all_goods.iter().filter(|g| g.product_id == p.id) {
                total_quantity += g.quantity;
                match g.status.as_str() {
                    "运行中" => running += g.quantity,
                    "联调中" => commissioning += g.quantity,
                    "已交付" => delivered += g.quantity,
                    _ => pending += g.quantity,
                }

                if p.category.eq_ignore_ascii_case("storage") {
                    storage_quantity += g.quantity;
                } else {
                    compute_quantity += g.quantity;
                }

                let comps = cmap.get(&g.id).cloned().unwrap_or_default();
                let (pb, mw, visual, featured) = goods_metrics(g.quantity, &comps);
                storage_pb += pb;
                power_mw += mw;

                for c in &comps {
                    let total = g.quantity.saturating_mul(c.qty_per_unit);
                    if c.kind == "disk" {
                        push_part_total(
                            &mut disk_map,
                            "disk",
                            &c.model,
                            &c.capacity,
                            total,
                            "块",
                        );
                    } else if c.kind == "accelerator" {
                        push_part_total(
                            &mut accel_map,
                            "accelerator",
                            &c.model,
                            "",
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
                    config_summary: config_summary(&g.parameters),
                    parts_summary: parts_summary(&comps),
                    quantity: g.quantity,
                    unit: g.unit.clone(),
                    status: g.status.clone(),
                    location: g.location.clone(),
                    asset_code: g.asset_code.clone(),
                    parameters: g.parameters.clone(),
                    storage_pb: pb,
                    power_mw: mw,
                    sort_order: g.sort_order,
                    featured,
                    visual,
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
            b.featured
                .cmp(&a.featured)
                .then(a.sort_order.cmp(&b.sort_order))
                .then(a.brand.cmp(&b.brand))
        });

        let mut accelerator_totals: Vec<PartTotal> = accel_map.into_values().collect();
        accelerator_totals.sort_by(|a, b| b.count.cmp(&a.count).then(a.label.cmp(&b.label)));
        let mut disk_totals: Vec<PartTotal> = disk_map.into_values().collect();
        disk_totals.sort_by(|a, b| b.count.cmp(&a.count).then(a.label.cmp(&b.label)));

        let rack_count = if total_quantity > 0 {
            ((total_quantity as f64) / 59.24).round() as i32
        } else {
            0
        };

        let health_percent = if total_quantity > 0 {
            ((running as i64 * 100) / total_quantity as i64) as i32
        } else {
            0
        };

        let storage_pb = (storage_pb * 10.0).round() / 10.0;
        let power_mw = (power_mw * 100.0).round() / 100.0;

        Ok(DashboardModel {
            title: if self.site.title.is_empty() {
                "智算机房台账".into()
            } else {
                self.site.title.clone()
            },
            brand_name: self.site.brand_name.clone(),
            tagline: self.site.tagline.clone(),
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
                rack_count,
                storage_pb,
                power_mw,
                running_quantity: running,
                commissioning_quantity: commissioning,
                pending_quantity: pending,
                delivered_quantity: delivered,
                health_percent,
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
