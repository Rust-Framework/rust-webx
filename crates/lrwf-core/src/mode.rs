//! Application mode: Development vs Production.
//!
//! Controls framework behavior such as startup banner verbosity
//! and built-in request logging.

/// Controls how the framework behaves at startup and at runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppMode {
    /// Full startup banner, per-request console logging, verbose diagnostics.
    Development,
    /// Minimal one-line startup message, no framework-initiated console logging.
    Production,
}

impl Default for AppMode {
    fn default() -> Self {
        AppMode::Development
    }
}
