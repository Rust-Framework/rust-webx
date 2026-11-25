//! Browser session exchange: bearer token → `HttpOnly` session cookie + CSRF cookie.

pub mod create_session;

pub use create_session::{CreateSessionRequest, SessionResponse};
