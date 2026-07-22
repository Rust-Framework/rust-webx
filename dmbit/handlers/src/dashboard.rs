//! Dashboard handler — 智算机房管理.
//!
//! Aggregation priority: Device records (actual machines) > Spec.planned_quantity (plan).
//! When 0 Device records exist, falls back to Spec.planned_quantity so the big screen
//! shows meaningful data immediately after CSV import.

use std::collections::HashMap;
use std::sync::Arc;

use rust_ef::{db_context::DbContext, prelude::*};
use rust_webx::*;

use dmbit_contracts::dashboard::*;
use dmbit_contracts::goods::ComponentModel;
use dmbit_contracts::site::SiteConfig;
use dmbit_domain::entities::{Device, Product, Spec, SpecComponent};

use crate::db::EfResultExt;
use crate::mapping::{
    component_to_model, format_capacity_label, parts_summary, spec_to_model,
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
    let capacity_label = if (kind == "disk" || kind == "hdd" || kind == "ssd") && capacity_gb > 0 {
        format_capacity_label(capacity_gb)
    } else {
        String::new()
    };
    let key = format!("{kind}|{model}|{capacity_gb}");
    let label = if !capacity_label.is_empty() {
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
        let products = linq!(self.ctx.set::<Product>();)
            .to_list()
            .await
            .map_ef()?;
        let specs = linq!(self.ctx.set::<Spec>();).to_list().await.map_ef()?;
        let devices = linq!(self.ctx.set::<Device>();).to_list().await.map_ef()?;
        let comps = linq!(self.ctx.set::<SpecComponent>();).to_list().await.map_ef()?;

        // Index: spec_id → (spec, product)
        let spec_product: HashMap<String, (&Spec, &Product)> = specs
            .iter()
            .filter_map(|s| {
                products
                    .iter()
                    .find(|p| p.id == s.product_id)
                    .map(|p| (s.id.clone(), (s, p)))
            })
            .collect();

        // Index: spec_id → Vec<ComponentModel>
        let mut comp_map: HashMap<String, Vec<ComponentModel>> = HashMap::new();
        for c in &comps {
            comp_map
                .entry(c.spec_id.clone())
                .or_default()
                .push(component_to_model(c));
        }

        // Index: spec_id → Vec<&Device> (only if devices exist)
        let mut dev_map: HashMap<String, Vec<&Device>> = HashMap::new();
        for d in &devices {
            dev_map.entry(d.spec_id.clone()).or_default().push(d);
        }

        let has_devices = !devices.is_empty();

        // ── Build product models & devices_rows ──
        let mut product_models = Vec::with_capacity(products.len());
        let mut devices_rows = Vec::new();
        let mut sort_order_counter: i32 = 0;

        for p in &products {
            let mut spec_models = Vec::new();
            let product_specs: Vec<&Spec> = specs.iter().filter(|s| s.product_id == p.id).collect();
            for s in &product_specs {
                let scomps = comp_map.get(&s.id).cloned().unwrap_or_default();

                if has_devices {
                    // ── Device-driven: each device = one row ──
                    let sdevs = dev_map.get(&s.id).cloned().unwrap_or_default();
                    let dc = sdevs.len() as i32;
                    for d in &sdevs {
                        devices_rows.push(DeviceOverviewRow {
                            id: d.id.clone(),
                            product_id: p.id.clone(),
                            product_name: p.name.clone(),
                            product_category: p.category.clone(),
                            brand: s.brand.clone(),
                            parts_summary: parts_summary(&scomps),
                            quantity: 1,
                            unit: s.unit.clone(),
                            status: d.status.clone(),
                            location: d.location.clone(),
                            asset_code: d.asset_code.clone(),
                            parameters: s.parameters.clone(),
                            sort_order: d.sort_order,
                        });
                    }
                    spec_models.push(spec_to_model(s, &p.name, scomps, dc));
                } else {
                    // ── Plan-driven: one placeholder row per spec ──
                    sort_order_counter += 1;
                    devices_rows.push(DeviceOverviewRow {
                        id: s.id.clone(),
                        product_id: p.id.clone(),
                        product_name: p.name.clone(),
                        product_category: p.category.clone(),
                        brand: s.brand.clone(),
                        parts_summary: parts_summary(&scomps),
                        quantity: s.planned_quantity,
                        unit: s.unit.clone(),
                        status: "待上架".to_string(),
                        location: String::new(),
                        asset_code: String::new(),
                        parameters: s.parameters.clone(),
                        sort_order: sort_order_counter,
                    });
                    spec_models.push(spec_to_model(s, &p.name, scomps, 0));
                }
            }
            spec_models.sort_by(|a, b| a.sort_order.cmp(&b.sort_order));
            product_models.push(dmbit_contracts::product::ProductModel {
                id: p.id.clone(),
                name: p.name.clone(),
                code: p.code.clone(),
                category: p.category.clone(),
                remark: p.remark.clone(),
                sort_order: p.sort_order,
                created_at: p.created_at,
                updated_at: p.updated_at,
                goods: Vec::new(),
            });
        }

        // ── Aggregate stats ──
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

        if has_devices {
            // ── Aggregate from Device records ──
            for d in &devices {
                total_quantity += 1;
                match d.status.as_str() {
                    "运行中" => running += 1,
                    "联调中" => commissioning += 1,
                    "待上架" => pending += 1,
                    "已交付" => delivered += 1,
                    _ => {}
                }

                if let Some((_s, p)) = spec_product.get(&d.spec_id) {
                    match p.category.as_str() {
                        "storage" => storage_quantity += 1,
                        _ => compute_quantity += 1,
                    };
                }

                if let Some(scomps) = comp_map.get(&d.spec_id) {
                    for c in scomps {
                        if c.kind == "disk" {
                            push_part_total(
                                &mut disk_map, "disk", &c.model, c.capacity_gb,
                                c.qty_per_unit, "块",
                            );
                            storage_capacity_gb = storage_capacity_gb
                                .saturating_add(i64::from(c.qty_per_unit).saturating_mul(c.capacity_gb));
                        } else {
                            push_part_total(
                                &mut accel_map, "accelerator", &c.model, 0,
                                c.qty_per_unit, "张",
                            );
                        }
                    }
                }
            }
        } else {
            // ── Fallback: aggregate from Spec.planned_quantity ──
            for s in &specs {
                let pq = s.planned_quantity;
                if pq <= 0 {
                    continue;
                }
                total_quantity += pq;
                pending += pq; // all planned devices are "待上架"

                if let Some((_s, p)) = spec_product.get(&s.id) {
                    match p.category.as_str() {
                        "storage" => storage_quantity += pq,
                        _ => compute_quantity += pq,
                    };
                }

                if let Some(scomps) = comp_map.get(&s.id) {
                    for c in scomps {
                        let per_spec = i64::from(pq).saturating_mul(i64::from(c.qty_per_unit));
                        if c.kind == "disk" {
                            push_part_total(
                                &mut disk_map, "disk", &c.model, c.capacity_gb,
                                (pq * c.qty_per_unit) as i32, "块",
                            );
                            storage_capacity_gb = storage_capacity_gb
                                .saturating_add(per_spec.saturating_mul(c.capacity_gb));
                        } else {
                            push_part_total(
                                &mut accel_map, "accelerator", &c.model, 0,
                                (pq * c.qty_per_unit) as i32, "张",
                            );
                        }
                    }
                }
            }
        }

        devices_rows.sort_by(|a, b| {
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
                product_count: products.len() as i32,
                goods_count: specs.len() as i32,
                total_quantity,
                compute_quantity,
                storage_quantity,
                storage_capacity_gb,
                running_quantity: running,
                commissioning_quantity: commissioning,
                pending_quantity: pending,
                delivered_quantity: delivered,
                status_buckets: vec![
                    StatusBucket { status: "运行中".into(), quantity: running },
                    StatusBucket { status: "联调中".into(), quantity: commissioning },
                    StatusBucket { status: "待上架".into(), quantity: pending },
                    StatusBucket { status: "已交付".into(), quantity: delivered },
                ],
            },
            accelerator_totals,
            disk_totals,
            products: product_models,
            devices: devices_rows,
        })
    }
}
