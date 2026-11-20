//! Engine configuration derived from the application's merged `AppOptions`.
//!
//! The engine owns no separate configuration file: [`EngineConfig::from_options`]
//! projects the sections the host already loads (`App`, `Server`, `Tls`) onto
//! the values the worker pool needs, plus the worker count from
//! `Server.Workers`.
//!
//! # Worker count
//!
//! `Server.Workers` is read from the merged appsettings (so
//! `APP__Server__Workers` wins over the JSON value) because the current
//! `webx_core::config::ServerSection` has no `workers` field yet. `0` means
//! **auto**: `available_parallelism()`, clamped to `1..=64`. An explicit value
//! is clamped the same way, so a typo cannot spawn thousands of threads. When
//! `webx_core` exposes `ServerSection::workers`, the JSON lookup below collapses
//! into `options.server.workers`.
//!
//! # Connection limit
//!
//! `App.MaxConnections` stays the documented **global** limit. The acceptor
//! splits it into one budget per worker
//! (`max_connections / workers`, floored at 1), so the effective global cap is
//! `budget * workers <= max_connections`. The limiter is not applied per client
//! address: per-IP throttling belongs to the rate-limit middleware.

use std::time::Duration;

use webx_core::config::AppOptions;
use webx_core::mode::AppMode;

/// Upper bound for the worker count, including an explicit `Server.Workers`.
///
/// Every worker is an OS thread plus a current-thread runtime; past a few dozen
/// the accept/handoff overhead and scheduler contention outweigh the extra
/// runtimes (the measured prototype peaks around 22 workers on a 32-connection
/// benchmark).
pub const MAX_WORKERS: usize = 64;

/// Default per-worker limit for Tokio's blocking pool.
///
/// The pool is created lazily, so this only bounds handlers that call
/// `spawn_blocking` (hashing, sync file IO); request futures themselves run on
/// the worker's current-thread runtime.
pub const DEFAULT_BLOCKING_THREADS: usize = 8;

/// How long hyper may wait for a request head before closing the connection.
///
/// hyper only enforces its `header_read_timeout` when a timer is installed; a
/// missing timer disables it and leaves the server open to slowloris.
pub const DEFAULT_HEADER_READ_TIMEOUT: Duration = Duration::from_secs(30);

/// Upper bound for hyper's per-connection read/header buffer.
///
/// hyper defaults to ~400 KiB per connection; 64 KiB covers normal request
/// heads while keeping a 10k-connection deployment's memory bounded.
pub const DEFAULT_MAX_HEADER_BUF: usize = 64 * 1024;

/// TLS material paths, resolved by the host from `Tls.CertPath` / `Tls.KeyPath`.
#[derive(Debug, Clone)]
pub struct TlsConfig {
    pub cert_path: String,
    pub key_path: String,
}

/// Everything one [`Server`](crate::Server) run needs from configuration.
#[derive(Debug, Clone)]
pub struct EngineConfig {
    /// Number of worker threads. `0` = auto (see [`Self::resolved_workers`]).
    pub workers: usize,
    /// Documented global connection limit; split into per-worker budgets.
    pub max_connections: usize,
    /// hyper HTTP/1 `header_read_timeout`.
    pub header_read_timeout: Duration,
    /// hyper HTTP/1 `max_buf_size`.
    pub max_header_buf: usize,
    /// Drain budget for ordinary connections during shutdown.
    pub drain_timeout: Duration,
    /// Drain budget used when at least one SSE response is active.
    pub stream_drain_timeout: Duration,
    /// Per-request deadline from `Server.RequestTimeoutSeconds`; `None` disables.
    pub request_timeout: Option<Duration>,
    /// TLS material when the listener terminates TLS.
    pub tls: Option<TlsConfig>,
    /// Runtime mode, forwarded from `APP_ENV` (spelled `mode` in core).
    pub mode: AppMode,
}

impl EngineConfig {
    /// Project `options` onto the engine's configuration.
    ///
    /// `workers` comes from `Server.Workers` (appsettings or
    /// `APP__Server__Workers`) and falls back to `0` = auto. `Tls` is only
    /// enabled when **both** paths are configured; a half-configured pair is
    /// logged and ignored, mirroring the host's "HTTPS URLs require both" rule
    /// without failing a plain-HTTP run.
    pub fn from_options(options: &AppOptions) -> Self {
        let mode = AppMode::from_env();
        let tls = match (
            options.tls.cert_path.trim().is_empty(),
            options.tls.key_path.trim().is_empty(),
        ) {
            (true, true) => None,
            (false, false) => Some(TlsConfig {
                cert_path: options.tls.cert_path.clone(),
                key_path: options.tls.key_path.clone(),
            }),
            _ => {
                tracing::warn!(
                    "[Engine] Tls.CertPath and Tls.KeyPath must both be set; TLS is disabled for this run"
                );
                None
            }
        };

        Self {
            workers: configured_workers(mode).unwrap_or(0),
            max_connections: options.app.max_connections.max(1),
            header_read_timeout: DEFAULT_HEADER_READ_TIMEOUT,
            max_header_buf: DEFAULT_MAX_HEADER_BUF,
            drain_timeout: Duration::from_secs(options.server.drain_timeout_seconds),
            stream_drain_timeout: Duration::from_secs(options.server.stream_drain_timeout_seconds),
            request_timeout: (options.server.request_timeout_seconds > 0)
                .then(|| Duration::from_secs(options.server.request_timeout_seconds)),
            tls,
            mode,
        }
    }

    /// Number of worker threads to spawn: explicit value or auto, clamped to
    /// `1..=MAX_WORKERS`.
    pub fn resolved_workers(&self) -> usize {
        if self.workers == 0 {
            let available = std::thread::available_parallelism()
                .map(|parallelism| parallelism.get())
                .unwrap_or(1);
            available.clamp(1, MAX_WORKERS)
        } else {
            self.workers.clamp(1, MAX_WORKERS)
        }
    }
}

impl Default for EngineConfig {
    /// Test/host default: auto workers, 10k connections, 30s/300s drain.
    fn default() -> Self {
        Self {
            workers: 0,
            max_connections: 10_000,
            header_read_timeout: DEFAULT_HEADER_READ_TIMEOUT,
            max_header_buf: DEFAULT_MAX_HEADER_BUF,
            drain_timeout: Duration::from_secs(30),
            stream_drain_timeout: Duration::from_secs(300),
            request_timeout: None,
            tls: None,
            mode: AppMode::Production,
        }
    }
}

/// Environment override for `Server.Workers`.
const WORKERS_ENV: &str = "APP__Server__Workers";

/// Read `Server.Workers` from the environment or the merged appsettings.
///
/// Returns `None` when the key is absent or not a valid non-negative integer;
/// a malformed value is logged rather than silently treated as auto so a typo
/// in a deployment file stays visible.
fn configured_workers(mode: AppMode) -> Option<usize> {
    if let Ok(raw) = std::env::var(WORKERS_ENV) {
        let parsed = parse_workers(&raw);
        if parsed.is_none() {
            tracing::warn!(
                value = raw.as_str(),
                "[Engine] {WORKERS_ENV} is not a valid worker count; using auto"
            );
        }
        return parsed;
    }

    let appsettings = webx_core::config::load_appsettings(mode)?;
    let value = appsettings.get("Server")?.get("Workers")?;
    let parsed = value
        .as_u64()
        .map(|value| value as usize)
        .or_else(|| value.as_str().and_then(parse_workers));
    if parsed.is_none() {
        tracing::warn!(
            value = %value,
            "[Engine] Server.Workers is not a valid worker count; using auto"
        );
    }
    parsed
}

/// Parse a worker count from text; `0` means auto.
fn parse_workers(raw: &str) -> Option<usize> {
    raw.trim().parse::<usize>().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config_with_workers(workers: usize) -> EngineConfig {
        EngineConfig {
            workers,
            ..EngineConfig::default()
        }
    }

    #[test]
    fn explicit_worker_count_is_used() {
        assert_eq!(config_with_workers(4).resolved_workers(), 4);
        assert_eq!(config_with_workers(1).resolved_workers(), 1);
    }

    #[test]
    fn zero_workers_means_available_parallelism_clamped() {
        let expected = std::thread::available_parallelism()
            .map(|parallelism| parallelism.get())
            .unwrap_or(1)
            .clamp(1, MAX_WORKERS);
        assert_eq!(config_with_workers(0).resolved_workers(), expected);
    }

    #[test]
    fn explicit_worker_count_is_clamped() {
        assert_eq!(
            config_with_workers(usize::MAX).resolved_workers(),
            MAX_WORKERS
        );
    }

    #[test]
    fn worker_count_parsing_rejects_garbage() {
        assert_eq!(parse_workers("8"), Some(8));
        assert_eq!(parse_workers(" 22 "), Some(22));
        assert_eq!(parse_workers("0"), Some(0));
        assert_eq!(parse_workers("auto"), None);
        assert_eq!(parse_workers("-1"), None);
        assert_eq!(parse_workers(""), None);
    }

    #[test]
    fn from_options_projects_app_server_and_tls_sections() {
        let options: AppOptions = serde_json::from_value(serde_json::json!({
            "App": { "MaxConnections": 512 },
            "Server": {
                "DrainTimeoutSeconds": 7,
                "StreamDrainTimeoutSeconds": 99
            },
            "Tls": { "CertPath": "certs/site.pem", "KeyPath": "certs/site.key" }
        }))
        .expect("options parse");

        let config = EngineConfig::from_options(&options);
        assert_eq!(config.max_connections, 512);
        assert_eq!(config.drain_timeout, Duration::from_secs(7));
        assert_eq!(config.stream_drain_timeout, Duration::from_secs(99));
        assert_eq!(config.header_read_timeout, DEFAULT_HEADER_READ_TIMEOUT);
        assert_eq!(config.max_header_buf, DEFAULT_MAX_HEADER_BUF);
        let tls = config.tls.expect("both TLS paths configured");
        assert_eq!(tls.cert_path, "certs/site.pem");
        assert_eq!(tls.key_path, "certs/site.key");
        // No appsettings in the test environment; absent key keeps auto.
        if std::env::var(WORKERS_ENV).is_err() {
            assert_eq!(config.workers, 0);
        }
    }

    #[test]
    fn half_configured_tls_is_ignored() {
        let options: AppOptions = serde_json::from_value(serde_json::json!({
            "Tls": { "CertPath": "certs/site.pem" }
        }))
        .expect("options parse");
        assert!(EngineConfig::from_options(&options).tls.is_none());

        let options: AppOptions = serde_json::from_value(serde_json::json!({
            "Tls": { "KeyPath": "certs/site.key" }
        }))
        .expect("options parse");
        assert!(EngineConfig::from_options(&options).tls.is_none());
    }

    #[test]
    fn zero_max_connections_is_bumped_to_one() {
        let options: AppOptions = serde_json::from_value(serde_json::json!({
            "App": { "MaxConnections": 0 }
        }))
        .expect("options parse");
        assert_eq!(EngineConfig::from_options(&options).max_connections, 1);
    }
}
