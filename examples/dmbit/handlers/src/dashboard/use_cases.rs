//! Dashboard aggregation.
//!
//! Priority: Device records (actual machines) > Spec.planned_quantity (the plan).
//! When no Device record exists the screen still shows the imported plan, so the
//! room is never blank right after a CSV import.
//!
//! Everything here is read-model assembly over four bounded queries; the pure
//! entity → model steps come from the domain mappers.

use std::collections::HashMap;

use rust_ef::{db_context::DbContext, prelude::*};
use webx::*;

use dmbit_contracts::dashboard::{
    DashboardModel, DashboardStats, DeviceOverviewRow, PartTotal, StatusBucket,
};
use dmbit_contracts::goods::ComponentModel;
use dmbit_contracts::product::ProductModel;
use dmbit_contracts::site::SiteConfig;
use dmbit_domain::entities::{Device, Product, Spec};
use dmbit_domain::mappers::product_mapper::product_with_specs;
use dmbit_domain::mappers::spec_mapper::{parts_summary, spec_model};

use crate::common::MAX_LIST_ROWS;
use crate::spec::queries::components_by_spec;

/// The whole dashboard payload for one request.
pub async fn build_dashboard(ctx: &mut DbContext, site: &SiteConfig) -> Result<DashboardModel> {
    let products = linq!(ctx.set::<Product>();)
        .take(MAX_LIST_ROWS)
        .to_list()
        .await?;
    let specs = linq!(ctx.set::<Spec>();)
        .take(MAX_LIST_ROWS)
        .to_list()
        .await?;
    let devices = linq!(ctx.set::<Device>();)
        .take(MAX_LIST_ROWS)
        .to_list()
        .await?;

    let spec_ids: Vec<String> = specs.iter().map(|s| s.id.clone()).collect();
    let components = components_by_spec(ctx, &spec_ids).await?;

    let spec_product: HashMap<String, (&Spec, &Product)> = specs
        .iter()
        .filter_map(|s| {
            products
                .iter()
                .find(|p| p.id == s.product_id)
                .map(|p| (s.id.clone(), (s, p)))
        })
        .collect();
    let mut devices_by_spec: HashMap<String, Vec<&Device>> = HashMap::new();
    for d in &devices {
        devices_by_spec
            .entry(d.spec_id.clone())
            .or_default()
            .push(d);
    }
    let has_devices = !devices.is_empty();

    let (product_models, device_rows) = build_rows(
        &products,
        &specs,
        &components,
        &devices_by_spec,
        has_devices,
    );
    let tally = tally(&specs, &devices, &spec_product, &components, has_devices);

    Ok(DashboardModel {
        title: pick(&site.title, "智算机房设备概览"),
        brand_name: site.brand_name.clone(),
        room_name: pick(&site.room_name, "直播数据智算机房"),
        stats: DashboardStats {
            product_count: products.len() as i32,
            goods_count: specs.len() as i32,
            total_quantity: tally.total_quantity,
            compute_quantity: tally.compute_quantity,
            storage_quantity: tally.storage_quantity,
            storage_capacity_gb: tally.storage_capacity_gb,
            running_quantity: tally.running,
            commissioning_quantity: tally.commissioning,
            pending_quantity: tally.pending,
            delivered_quantity: tally.delivered,
            status_buckets: vec![
                StatusBucket {
                    status: "运行中".into(),
                    quantity: tally.running,
                },
                StatusBucket {
                    status: "联调中".into(),
                    quantity: tally.commissioning,
                },
                StatusBucket {
                    status: "待上架".into(),
                    quantity: tally.pending,
                },
                StatusBucket {
                    status: "已交付".into(),
                    quantity: tally.delivered,
                },
            ],
        },
        accelerator_totals: tally.accelerators,
        disk_totals: tally.disks,
        products: product_models,
        devices: device_rows,
    })
}

fn pick(configured: &str, fallback: &str) -> String {
    if configured.is_empty() {
        fallback.into()
    } else {
        configured.to_string()
    }
}

/// Product models plus the big-screen device table.
fn build_rows(
    products: &[Product],
    specs: &[Spec],
    components: &HashMap<String, Vec<ComponentModel>>,
    devices_by_spec: &HashMap<String, Vec<&Device>>,
    has_devices: bool,
) -> (Vec<ProductModel>, Vec<DeviceOverviewRow>) {
    let mut product_models = Vec::with_capacity(products.len());
    let mut device_rows = Vec::new();
    let mut placeholder_order = 0i32;

    for p in products {
        let mut spec_models = Vec::new();
        for s in specs.iter().filter(|s| s.product_id == p.id) {
            let spec_components = components.get(&s.id).cloned().unwrap_or_default();
            if has_devices {
                let rows = devices_by_spec.get(&s.id).cloned().unwrap_or_default();
                let count = rows.len() as i32;
                for d in rows {
                    device_rows.push(DeviceOverviewRow {
                        id: d.id.clone(),
                        product_id: p.id.clone(),
                        product_name: p.name.clone(),
                        product_category: p.category.clone(),
                        brand: s.brand.clone(),
                        parts_summary: parts_summary(&spec_components),
                        quantity: 1,
                        unit: s.unit.clone(),
                        status: d.status.clone(),
                        location: d.location.clone(),
                        asset_code: d.asset_code.clone(),
                        parameters: s.parameters.clone(),
                        sort_order: d.sort_order,
                    });
                }
                spec_models.push(spec_model(s, &p.name, spec_components, count));
            } else {
                // Plan-driven: one placeholder row per spec.
                placeholder_order += 1;
                device_rows.push(DeviceOverviewRow {
                    id: s.id.clone(),
                    product_id: p.id.clone(),
                    product_name: p.name.clone(),
                    product_category: p.category.clone(),
                    brand: s.brand.clone(),
                    parts_summary: parts_summary(&spec_components),
                    quantity: s.planned_quantity,
                    unit: s.unit.clone(),
                    status: "待上架".to_string(),
                    location: String::new(),
                    asset_code: String::new(),
                    parameters: s.parameters.clone(),
                    sort_order: placeholder_order,
                });
                spec_models.push(spec_model(s, &p.name, spec_components, 0));
            }
        }
        spec_models.sort_by_key(|m| m.sort_order);
        product_models.push(product_with_specs(p, &spec_models));
    }

    device_rows.sort_by(|a, b| a.sort_order.cmp(&b.sort_order).then(a.brand.cmp(&b.brand)));
    (product_models, device_rows)
}

/// The aggregate counters and part totals.
struct Tally {
    total_quantity: i32,
    compute_quantity: i32,
    storage_quantity: i32,
    storage_capacity_gb: i64,
    running: i32,
    commissioning: i32,
    pending: i32,
    delivered: i32,
    accelerators: Vec<PartTotal>,
    disks: Vec<PartTotal>,
}

fn tally(
    specs: &[Spec],
    devices: &[Device],
    spec_product: &HashMap<String, (&Spec, &Product)>,
    components: &HashMap<String, Vec<ComponentModel>>,
    has_devices: bool,
) -> Tally {
    let mut t = Tally {
        total_quantity: 0,
        compute_quantity: 0,
        storage_quantity: 0,
        storage_capacity_gb: 0,
        running: 0,
        commissioning: 0,
        pending: 0,
        delivered: 0,
        accelerators: Vec::new(),
        disks: Vec::new(),
    };
    let mut accel_map: HashMap<String, PartTotal> = HashMap::new();
    let mut disk_map: HashMap<String, PartTotal> = HashMap::new();

    if has_devices {
        for d in devices {
            t.total_quantity += 1;
            match d.status.as_str() {
                "运行中" => t.running += 1,
                "联调中" => t.commissioning += 1,
                "待上架" => t.pending += 1,
                "已交付" => t.delivered += 1,
                _ => {}
            }
            if let Some((_, p)) = spec_product.get(&d.spec_id) {
                if p.category == "storage" {
                    t.storage_quantity += 1;
                } else {
                    t.compute_quantity += 1;
                }
            }
            if let Some(list) = components.get(&d.spec_id) {
                add_parts(&mut t, &mut accel_map, &mut disk_map, list, 1);
            }
        }
    } else {
        for s in specs {
            let planned = s.planned_quantity;
            if planned <= 0 {
                continue;
            }
            t.total_quantity += planned;
            t.pending += planned;
            if let Some((_, p)) = spec_product.get(&s.id) {
                if p.category == "storage" {
                    t.storage_quantity += planned;
                } else {
                    t.compute_quantity += planned;
                }
            }
            if let Some(list) = components.get(&s.id) {
                add_parts(&mut t, &mut accel_map, &mut disk_map, list, planned);
            }
        }
    }

    t.accelerators = sorted_totals(accel_map);
    t.disks = sorted_totals(disk_map);
    t
}

/// Fold one spec's components into the totals, `units` times.
fn add_parts(
    t: &mut Tally,
    accel_map: &mut HashMap<String, PartTotal>,
    disk_map: &mut HashMap<String, PartTotal>,
    components: &[ComponentModel],
    units: i32,
) {
    for c in components {
        if c.kind == "disk" {
            push_part_total(
                disk_map,
                "disk",
                &c.model,
                c.capacity_gb,
                c.qty_per_unit * units,
                "块",
            );
            t.storage_capacity_gb = t.storage_capacity_gb.saturating_add(
                i64::from(c.qty_per_unit)
                    .saturating_mul(i64::from(units))
                    .saturating_mul(c.capacity_gb),
            );
        } else {
            push_part_total(
                accel_map,
                "accelerator",
                &c.model,
                0,
                c.qty_per_unit * units,
                "张",
            );
        }
    }
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
        dmbit_domain::rules::format_capacity_label(capacity_gb)
    } else {
        String::new()
    };
    let key = format!("{kind}|{model}|{capacity_gb}");
    let label = if capacity_label.is_empty() {
        model.to_string()
    } else {
        format!("{model} · {capacity_label}")
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

fn sorted_totals(map: HashMap<String, PartTotal>) -> Vec<PartTotal> {
    let mut totals: Vec<PartTotal> = map.into_values().collect();
    totals.sort_by(|a, b| b.count.cmp(&a.count).then(a.label.cmp(&b.label)));
    totals
}
