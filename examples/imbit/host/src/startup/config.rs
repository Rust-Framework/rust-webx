//! Runtime configuration defaults.
//!
//! The framework configuration sections stay the single source of truth
//! (`Jwt.Secret` / `Jwt.SecretFile`, `Turn.*`, `APP__Section__Key` overlays);
//! this module only fills in what the reference app promises to run with out of
//! the box.

use std::time::Duration;

use webx::{AppMode, AppOptions};

/// Default interval of the live-session credential re-check (`IMBIT_REVALIDATION_MS`).
///
/// 500 ms is demo-friendly: a logout closes the browser's WebSocket within one
/// second. A production deployment sets 10–30 s (the interval is the *longest*
/// window in which a revoked credential can still hold a live socket).
pub const DEFAULT_SESSION_REVALIDATION_MS: u64 = 500;

/// Development JWT secret (32+ bytes).
///
/// Production supplies `JWT_SECRET` or `Jwt.SecretFile` instead; the built-in
/// secret is never installed in Production mode.
pub const DEV_JWT_SECRET: &str = "imbit-development-secret-change-me-0123456789";

/// Default listen address (`IMBIT_ADDR` overrides it).
pub const DEFAULT_ADDRESS: &str = "127.0.0.1:8090";

/// Application mode.
///
/// An explicit `APP_ENV` always wins (`AppMode::from_env`: anything but
/// `Development`/`Dev` stays Production, so the guards cannot be disabled by
/// accident). An unset `APP_ENV` means the development defaults this reference
/// app documents — `cargo run -p imbit` must work with no configuration.
pub fn app_mode() -> AppMode {
    if std::env::var("APP_ENV").is_ok() {
        AppMode::from_env()
    } else {
        AppMode::Development
    }
}

/// Interval of the live-WebSocket credential re-check.
///
/// `IMBIT_REVALIDATION_MS` overrides the demo default; the framework clamps the
/// value to at least 100 ms (a zero interval would poll in a hot loop).
pub fn session_revalidation_interval() -> Duration {
    let millis = std::env::var("IMBIT_REVALIDATION_MS")
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .unwrap_or(DEFAULT_SESSION_REVALIDATION_MS);
    Duration::from_millis(millis)
}

/// Browser origins allowed to open a WebSocket session (`IMBIT_ORIGINS`).
///
/// Comma-separated, matched exactly (scheme + host + port). `None` — the
/// variable is unset or empty — disables the handshake Origin check, which is
/// what keeps non-browser clients (Bearer tokens, no `Origin` header) working.
/// A deployment that serves a browser must set it; see
/// [`WebSocketLimits`](webx::WebSocketLimits) and the framework's WebSocket
/// manual chapter.
pub fn websocket_origins() -> Option<Vec<String>> {
    let raw = std::env::var("IMBIT_ORIGINS").ok()?;
    let origins: Vec<String> = raw
        .split(',')
        .map(str::trim)
        .filter(|origin| !origin.is_empty())
        .map(str::to_string)
        .collect();
    (!origins.is_empty()).then_some(origins)
}

/// Options the reference app needs before the host validates them.
pub fn apply_options(options: &mut AppOptions, mode: AppMode) {
    if mode == AppMode::Development && options.jwt.secret.trim().is_empty() {
        options.jwt.secret = DEV_JWT_SECRET.to_string();
    }
    options.metrics.enabled = true;
    configure_turn_defaults(options);
}

/// Development defaults for the framework `Turn` section.
///
/// The section itself is the single source of truth (`Turn.Secret`,
/// `Turn.Urls`, `APP__Turn__*`); the `TURN_*` environment variables below are a
/// convenience so the reference app works without an appsettings file. Values
/// already present in the section are never overridden.
fn configure_turn_defaults(options: &mut AppOptions) {
    if options.turn.secret.trim().is_empty() {
        options.turn.secret = std::env::var("TURN_SECRET")
            .unwrap_or_else(|_| "imbit-turn-dev-secret-change-me".to_string());
    }
    if options.turn.urls.is_empty() {
        let urls = std::env::var("TURN_URLS")
            .unwrap_or_else(|_| "turn:127.0.0.1:3478,stun:127.0.0.1:3478".to_string());
        options.turn.urls = urls
            .split(',')
            .map(str::trim)
            .filter(|url| !url.is_empty())
            .map(str::to_string)
            .collect();
    }
    if options.turn.ttl_seconds == webx::DEFAULT_TURN_TTL_SECONDS {
        if let Some(ttl) = std::env::var("TURN_TTL_SECONDS")
            .ok()
            .and_then(|value| value.parse().ok())
        {
            options.turn.ttl_seconds = ttl;
        }
    }
}
