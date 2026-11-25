#![allow(non_snake_case)] // rust-dix #[derive(Inject)] generates __rdi_construct_* symbols

//! Dmbit handlers — one operation per file, one module per business context.
//!
//! | Module | Role |
//! |---|---|
//! | [`common`] | persistence / identity / clock helpers shared by handlers |
//! | [`authorization`] | dynamic authorizer backed by the endpoint declarations |
//! | `<context>/` | `<operation>.rs` (one Handler) + `queries.rs` / `use_cases.rs` |

pub mod auth;
pub mod authorization;
pub mod common;
pub mod dashboard;
pub mod device;
pub mod inventory;
pub mod product;
pub mod spec;
