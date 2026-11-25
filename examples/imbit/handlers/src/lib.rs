#![allow(non_snake_case)] // rust-dix #[derive(Inject)] generates __rdi_construct_* symbols

//! imbit handlers — one operation per file, one `IRequestHandler` per file.
//!
//! | Module | Role |
//! |--------|------|
//! | [`auth`] | password login, token revocation |
//! | [`room`] | create / join / list |
//! | [`message`] | HTTP offline catch-up |
//! | [`recording`] | control plane, resumable segments, playback |
//! | [`media`] | TURN credentials |
//! | [`realtime`] | `/ws/im` message-as-request handler |
//! | [`common`] | helpers shared by more than one handler |
//!
//! Handlers only orchestrate: identity comes from the request (`#[fromclaim]` on
//! HTTP, the authenticated upgrade on WebSocket), rules and typed errors live in
//! `imbit-domain`, and a domain failure becomes the framework error through a
//! plain `?` — no handler inspects error text.

pub mod auth;
pub mod common;
pub mod media;
pub mod message;
pub mod realtime;
pub mod recording;
pub mod room;
