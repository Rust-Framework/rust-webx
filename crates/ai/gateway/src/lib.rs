//! OpenAI-compatible HTTP surface for `rust-webx-ai` gateways.
//!
//! This crate is the HTTP adapter around [`GatewayExecutor`](webx_ai::GatewayExecutor):
//! it owns the `/v1/chat/completions` request/response shapes and SSE encoding,
//! while routing, providers, quota and usage stay in `rust-webx-ai`.
//!
//! Call [`install`] once in `main` (or reference any type from this crate) so the
//! compile-time route inventory is linked into the binary.

pub mod auth;
pub mod handler;
pub mod health;
pub mod request;
pub mod response;
pub mod sse;

pub use auth::StoreApiKeyValidator;
pub use handler::{OpenAiChatHandler, OpenAiModelsHandler, OpenAiModelsRequest};
pub use health::GatewayHealth;
pub use request::{
    OpenAiChatRequest, OpenAiFunction, OpenAiMessage, OpenAiTool, OpenAiToolCall,
    OpenAiToolDefinition, StopValue,
};
pub use response::OpenAiErrorResult;
pub use sse::OpenAiSseEncoder;

/// Force this crate's inventory (the `/v1/chat/completions` route and handler)
/// into the final binary.
pub fn install() {}
