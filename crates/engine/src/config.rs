//! Engine configuration derived from the application's merged `AppOptions`.
//!
//! The engine owns no separate configuration file: [`EngineConfig::from_options`]
//! projects the sections the host already loads (`App`, `Server`, `Tls`) onto
//! the values the worker pool needs, plus the worker count from
//! `Server.Workers`.
//!
//! # Worker count
//!
//! `Server.Workers` is projected from the merged [`AppOptions`] (the same
//! `Server` section as every other lifecycle setting), so a builder modifier
//! and `APP__Server__Workers` both reach the engine. `0` means **auto**:
//! `available_parallelism()`, clamped to `1..=64`. An explicit value is clamped
//! the same way, so a typo cannot spawn thousands of threads.
//!
//! # Connection limit
//!
//! `App.MaxConnections` is a **global server admission cap**. `Server` creates
//! one `Semaphore` for the whole worker pool (all listeners share it); a
//! connection is admitted only after acquiring that permit. There is no
//! per-worker floor or per-listener multiplication. Per-IP throttling belongs
//! to the rate-limit middleware.
//!
//! # Pre-request deadline
//!
//! `Server.FirstRequestTimeoutSeconds` bounds the interval between accepting a
//! connection and the first request reaching the service. It closes silent
//! TCP/partial-h2-preface connections before they can pin an admission slot.
//! Zero falls back to the secure default rather than disabling the deadline.

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
    /// Deadline for an accepted connection to start its first request.
    ///
    /// This is the admission guard for zero-byte/partial-preface connections;
    /// it is separate from hyper's per-request header timeout, which starts only
    /// after the protocol has been selected.
    pub first_request_timeout: Duration,
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
            workers: options.server.workers,
            max_connections: options.app.max_connections.max(1),
            header_read_timeout: DEFAULT_HEADER_READ_TIMEOUT,
            max_header_buf: DEFAULT_MAX_HEADER_BUF,
            drain_timeout: Duration::from_secs(options.server.drain_timeout_seconds),
            stream_drain_timeout: Duration::from_secs(options.server.stream_drain_timeout_seconds),
            // Zero is a secure fallback, not "disable": an unbounded
            // pre-request connection is a denial-of-service vector.
            first_request_timeout: Duration::from_secs(
                options.server.first_request_timeout_seconds.max(1),
            ),
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
            // Development/test hosts are often spawned many-at-a-time; keep their
            // thread footprint bounded. Production uses the full core count.
            let limit = if self.mode == AppMode::Development {
                8
            } else {
                MAX_WORKERS
            };
            available.clamp(1, limit)
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
            first_request_timeout: Duration::from_secs(10),
            request_timeout: None,
            tls: None,
            mode: AppMode::Production,
        }
    }
}

/// Upper bound accepted by Tokio's `Semaphore`; used to turn an invalid
/// `MaxConnections` into a typed startup error instead of a panic.
pub const MAX_SEMAPHORE_PERMITS: usize = tokio::sync::Semaphore::MAX_PERMITS;

/// Validate configuration values that feed allocation/admission primitives.
pub fn validate_max_connections(max_connections: usize) -> webx_core::error::Result<()> {
    if max_connections == 0 || max_connections > MAX_SEMAPHORE_PERMITS {
        return Err(webx_core::error::Error::Http(format!(
            "App.MaxConnections={max_connections} is outside the supported range 1..={MAX_SEMAPHORE_PERMITS}"
        )));
    }
    Ok(())
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
        assert_eq!(config.workers, 0);
        assert_eq!(config.first_request_timeout, Duration::from_secs(10));
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
