//! Dmbit contracts — one module per business context, one file per operation.
//!
//! Each `<context>/` module holds `<operation>.rs` (one request + its route) and
//! `models.rs` (the shared `<聚合>Model` read models). Request → Handler → file
//! names line up across the four layers.

pub mod auth;
pub mod dashboard;
pub mod device;
pub mod goods;
pub mod inventory;
pub mod product;
pub mod site;
pub mod spec;
