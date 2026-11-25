#![allow(non_snake_case)] // rust-dix #[derive(Inject)] generates __rdi_construct_* symbols

//! Docbit handlers crate — one `IRequestHandler` per file under its context
//! directory, plus the shared services the host composes.
//!
//! | Area | Files |
//! |------|-------|
//! | Handler contexts | `auth/`, `blog/`, `category/`, `comment/`, `docs/`, `exhibition/`, `media/`, `rbac/`, `site/`, `tracking/`, `user/` |
//! | Shared support | [`common`] (persistence, list bounds, time), [`authorization`] (dynamic authorizer) |
//! | Services and middleware | [`doc_service`] (the `IDocumentService` port impl), [`catalog`] (INDEX.json → exhibition sync), [`seo`] (SSR shell, robots/sitemap) |
//! | Demo | [`cache`] (framework `MemoryCache` get-or-create) |
//!
//! ## Handler wiring
//!
//! `#[derive(Inject)]` on the struct generates `__rdi_construct_<Handler>`;
//! `#[handler(inject)]` on the impl block submits a `HandlerRegistration` to
//! `inventory` with a per-request factory. The factory resolves the owned
//! `DbContext` via `get_owned`, so `handle(&mut self, …)` never needs
//! `Arc<Mutex>`.
//!
//! ## Write paths
//!
//! - new rows: `ctx.add(Entity::try_from(request)?)` then [`common::save_changes`]
//! - mutated rows loaded by a query: `ctx.edit(row, |r| r.patch(request))`,
//!   then `ctx.detect_changes()` and [`common::save_changes`]
//! - persistence errors: rely on rust-ef's `From<EFError> for webx::Error` and `?`
//!
//! Soft delete is a global query filter (`docbit_domain::prepare_context`), so
//! handlers never repeat `!is_deleted`; audit fields are stamped by the mappers
//! from `RequestContext`, so handlers never pass the operator explicitly.

pub mod auth;
pub mod authorization;
pub mod blog;
pub mod cache;
pub mod catalog;
pub mod category;
pub mod comment;
pub mod common;
pub mod doc_service;
pub mod docs;
pub mod exhibition;
pub mod media;
pub mod rbac;
pub mod seo;
pub mod site;
pub mod tracking;
pub mod user;
