//! `GET /api/inventory/export` — download the inventory as CSV.
//!
//! A self-controlled response: the handler owns status, content type and
//! filename, and returns a `ContentResult` instead of a JSON body.

use rust_ef::{db_context::DbContext, prelude::*};
use webx::*;

use dmbit_contracts::inventory::models::INVENTORY_EXPORT_FILE_NAME;
use dmbit_contracts::inventory::ExportInventoryRequest;
use dmbit_domain::entities::{Product, Spec};

use super::csv;
use crate::common::MAX_LIST_ROWS;
use crate::spec::queries::components_by_spec;

#[derive(Inject)]
pub struct ExportInventoryHandler {
    #[inject(owned)]
    ctx: DbContext,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<ExportInventoryRequest, Response> for ExportInventoryHandler {
    async fn handle(&mut self, _: ExportInventoryRequest) -> ActionResult<Response> {
        let mut products = linq!(self.ctx.set::<Product>();)
            .take(MAX_LIST_ROWS)
            .to_list()
            .await?;
        products.sort_by_key(|p| p.sort_order);
        let specs = linq!(self.ctx.set::<Spec>();)
            .take(MAX_LIST_ROWS)
            .to_list()
            .await?;
        let ids: Vec<String> = specs.iter().map(|s| s.id.clone()).collect();
        let components = components_by_spec(&mut self.ctx, &ids).await?;

        let mut lines = vec![csv::header_line()];
        for product in &products {
            let mut rows: Vec<&Spec> = specs
                .iter()
                .filter(|s| s.product_id == product.id)
                .collect();
            if rows.is_empty() {
                csv::push_product_row(&mut lines, product);
                continue;
            }
            rows.sort_by_key(|s| s.sort_order);
            for spec in rows {
                let list = components.get(&spec.id).cloned().unwrap_or_default();
                match list.split_first() {
                    None => csv::push_spec_row(&mut lines, product, spec, "", "", 0, 0),
                    Some((first, rest)) => {
                        csv::push_spec_row(
                            &mut lines,
                            product,
                            spec,
                            &first.kind,
                            &first.model,
                            first.capacity_gb,
                            first.qty_per_unit,
                        );
                        for c in rest {
                            csv::push_component_row(
                                &mut lines,
                                spec,
                                &c.kind,
                                &c.model,
                                c.capacity_gb,
                                c.qty_per_unit,
                            );
                        }
                    }
                }
            }
        }

        // A BOM keeps Excel on Windows from mangling the Chinese headers.
        let mut body = String::from("\u{FEFF}");
        body.push_str(&lines.join("\n"));
        body.push('\n');

        Ok(Box::new(
            ContentResult::bytes(body.into_bytes(), "text/csv; charset=utf-8")
                .download_name(INVENTORY_EXPORT_FILE_NAME),
        ))
    }
}
