//! Inventory context — CSV import / export.
//!
//! | File | Role |
//! |---|---|
//! | `export_inventory.rs` | `GET /api/inventory/export` (a real `text/csv` response) |
//! | `import_inventory.rs` | `POST /api/inventory/import` (thin handler) |
//! | `use_cases.rs` | the import use case: conflicts → upsert → one commit |
//! | `parse.rs` | CSV rows → the desired in-memory state |
//! | `csv.rs` | CSV mechanics + the column contract shared by both directions |
//! | `queries.rs` | the code-keyed lookups the import needs |

pub mod csv;
mod export_inventory;
mod import_inventory;
mod parse;
pub mod queries;
pub mod use_cases;

pub use export_inventory::ExportInventoryHandler;
pub use import_inventory::ImportInventoryHandler;
