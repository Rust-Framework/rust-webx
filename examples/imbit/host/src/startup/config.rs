//! Runtime configuration defaults.
//!
//! The framework configuration sections stay the single source of truth
//! (`Jwt.Secret` / `Jwt.SecretFile`, `Turn.*`, `APP__Section__Key` overlays);
//! this module only fills in what the reference app promises to run with out of
//! the box.

use webx::{AppMode, AppOptions};

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
