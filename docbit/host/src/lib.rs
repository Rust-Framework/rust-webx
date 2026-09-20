#![allow(non_snake_case)] // rust-dix #[derive(Inject)] generates __rdi_construct_* symbols

//! Docbit host library — package surface for the binary and e2e tests.
//!
//! | Module / file | Role |
//! |---------------|------|
//! | `main.rs` | Program.cs — `#[webx::main(embed)]` + host builder |
//! | [`startup`] | Add* / Use* extensions, hosted services, seed |
//! | `build.rs` | Compile-time web root |
//!
//! E2E builds a host via `tests/support.rs` (mirrors `main.rs`).

pub mod startup;

extern crate docbit_domain;
extern crate docbit_handlers;

pub use startup::DbInitService;
