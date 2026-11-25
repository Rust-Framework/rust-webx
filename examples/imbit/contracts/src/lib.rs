//! imbit contracts — request/response DTOs, routes and the WebSocket protocol.
//!
//! Innermost layer: `rust-webx` (binding, route macros, identity) and serde
//! only. The read models (`<aggregate>Model`) live here because the HTTP API,
//! the WebSocket protocol and the domain mappers all speak them — a handler
//! never defines a DTO.
//!
//! | Context | Operations |
//! |---------|------------|
//! | [`auth`] | login, logout |
//! | [`session`] | bearer → browser cookie session exchange |
//! | [`room`] | create, join, list |
//! | [`message`] | offline catch-up history |
//! | [`recording`] | start, upload segment, complete, get, play segment |
//! | [`media`] | TURN/STUN credentials |
//! | [`realtime`] | `/ws/im` message-as-request protocol (requests, replies, events) |

pub mod auth;
pub mod media;
pub mod message;
pub mod realtime;
pub mod recording;
pub mod room;
pub mod session;
