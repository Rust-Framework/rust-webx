#![allow(non_snake_case)] // rust-dix #[derive(Inject)] generates __rdi_construct_* symbols

//! Dmbit host library — inventory surface only.
//!
//! | File | Role |
//! |------|------|
//! | `main.rs` | Program.cs — host builder chain |
//! | [`startup`] | Extensions and hosted services |

pub mod startup;

extern crate dmbit_domain;
extern crate dmbit_handlers;

pub use startup::DbInitService;
