//! Pipeline behavior orchestration.
//!
//! Pipeline behaviors wrap around request handling, forming a chain
//! similar to ASP.NET Core middleware but for the mediator pipeline.
//!
//! ```ignore
//! Request -> [ValidationBehavior] -> [LoggingBehavior] -> [Handler]
//! ```
//!
//! Each behavior can inspect/modify the request or response,
//! or short-circuit the chain entirely.

// Pipeline behavior chain execution will be fully implemented
// in a future version with proper type-erased request/response handling.
