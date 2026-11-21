//! Reload an options section from a JSON file without restarting the host.
//!
//! [`OptionsFileWatcher`] is a [`PeriodicBackgroundService`] that polls one
//! file, binds a section of it to `T`, and publishes each new snapshot through
//! an [`OptionsMonitor`]. It complements `HostBuilder::add_options`, which binds
//! the merged `appsettings.json` once at startup.
//!
//! Semantics:
//!
//! * the first tick runs after one interval; until then the startup snapshot
//!   from `appsettings.json` stays active;
//! * file content is hashed, so touching the file without changing bytes does
//!   not publish a replacement;
//! * a missing file is not an error — it is normal when an operator drops the
//!   file after the process started; the old snapshot stays active;
//! * malformed JSON / a failed bind is reported as a typed error (logged by the
//!   worker) and the old snapshot stays active: a bad edit never takes the
//!   service down. Fixing the file changes its hash and the next tick retries.
//!
//! ```ignore
//! Host::builder()
//!     .watch_options_file::<GatewayConfig>("Gateway", "gateway.json", Duration::from_secs(5))
//!     .build();
//! ```
//!
//! A handler or service then injects `Arc<dyn IOptionsMonitor<GatewayConfig>>`
//! and calls `current()` on the hot path.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use webx_core::config;
use webx_core::error::{Error, Result};
use webx_core::hosted::PeriodicBackgroundService;
use webx_core::options_monitor::OptionsMonitor;

/// Poll one JSON file and publish its bound section through `monitor`.
pub struct OptionsFileWatcher<T>
where
    T: Send + Sync + 'static,
{
    monitor: Arc<OptionsMonitor<T>>,
    section: String,
    path: PathBuf,
    interval: Duration,
    /// Hash of the last successfully applied (or rejected) content. A parse
    /// error is recorded too, so a broken file is reported once instead of on
    /// every tick; editing the file changes the hash and retries.
    last_hash: Mutex<Option<u64>>,
}

impl<T> Clone for OptionsFileWatcher<T>
where
    T: Send + Sync + 'static,
{
    fn clone(&self) -> Self {
        Self {
            monitor: Arc::clone(&self.monitor),
            section: self.section.clone(),
            path: self.path.clone(),
            interval: self.interval,
            last_hash: Mutex::new(*self.last_hash.lock().unwrap_or_else(|p| p.into_inner())),
        }
    }
}

impl<T> OptionsFileWatcher<T>
where
    T: for<'de> serde::Deserialize<'de> + Default + Send + Sync + 'static,
{
    /// Watch `path`, binding its `section` node to `T`.
    ///
    /// `section` uses the same spelling as `HostBuilder::add_options`; `"."`
    /// or an empty string binds the whole file.
    pub fn new(
        monitor: Arc<OptionsMonitor<T>>,
        section: impl Into<String>,
        path: impl Into<PathBuf>,
        interval: Duration,
    ) -> Self {
        Self {
            monitor,
            section: section.into(),
            path: path.into(),
            interval,
            last_hash: Mutex::new(None),
        }
    }

    /// The watched path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The watched configuration section.
    pub fn section(&self) -> &str {
        &self.section
    }

    /// The shared snapshot holder.
    pub fn monitor(&self) -> &Arc<OptionsMonitor<T>> {
        &self.monitor
    }

    /// Read and apply the file now.
    ///
    /// Returns `Ok(false)` when the file is absent or unchanged. Publication is
    /// skipped for unchanged content; parse/bind errors are returned so a caller
    /// can report a rejected reload.
    pub fn reload_now(&self) -> Result<bool> {
        let bytes = match std::fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                tracing::debug!(
                    path = %self.path.display(),
                    "options file is not present yet; keeping the current snapshot"
                );
                return Ok(false);
            }
            Err(error) => {
                return Err(Error::Internal(format!(
                    "cannot read options file {}: {error}",
                    self.path.display()
                )));
            }
        };

        let hash = content_hash(&bytes);
        {
            let mut last = self.last_hash.lock().unwrap_or_else(|p| p.into_inner());
            if *last == Some(hash) {
                return Ok(false);
            }
            // Record before parsing: a broken file is logged once per edit, not
            // once per interval.
            *last = Some(hash);
        }

        let json: serde_json::Value = serde_json::from_slice(&bytes).map_err(|error| {
            Error::Validation(format!(
                "invalid JSON in options file {}: {error}",
                self.path.display()
            ))
        })?;
        // Strict rebind: a bad snapshot is rejected and the previous snapshot
        // stays active. The lossy binder used to install `T::default()` here,
        // which silently reset a section after a typo.
        let bound: T = config::try_bind_config(&json, &self.section).map_err(|error| {
            Error::Validation(format!(
                "rejected reload of {}: {error}",
                self.path.display()
            ))
        })?;
        self.monitor.set(bound);
        tracing::info!(
            path = %self.path.display(),
            section = %self.section,
            "options reloaded"
        );
        Ok(true)
    }
}

#[async_trait::async_trait]
impl<T> PeriodicBackgroundService for OptionsFileWatcher<T>
where
    T: for<'de> serde::Deserialize<'de> + Default + Send + Sync + 'static,
{
    fn interval(&self) -> Duration {
        self.interval
    }

    async fn tick(&self) -> Result<()> {
        // A rejected reload is reported by the worker and the old snapshot stays
        // active (PeriodicBackgroundService keeps ticking by default).
        self.reload_now().map(|_| ())
    }
}

fn content_hash(bytes: &[u8]) -> u64 {
    let mut hasher = DefaultHasher::new();
    bytes.hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[derive(Debug, Clone, Default, PartialEq, Eq, serde::Deserialize)]
    struct Gateway {
        #[serde(default)]
        rpm: u64,
        #[serde(default)]
        model: String,
    }

    fn setup() -> (TempDir, PathBuf, Arc<OptionsMonitor<Gateway>>) {
        let dir = TempDir::new().expect("temp dir");
        let path = dir.path().join("gateway.json");
        let monitor = Arc::new(OptionsMonitor::new(Gateway::default()));
        (dir, path, monitor)
    }

    #[tokio::test]
    async fn applies_the_section_and_publishes_the_snapshot() {
        let (_dir, path, monitor) = setup();
        std::fs::write(
            &path,
            r#"{"Gateway":{"rpm":500,"model":"fast"},"Other":{"rpm":1}}"#,
        )
        .expect("write");
        let watcher = OptionsFileWatcher::new(
            Arc::clone(&monitor),
            "Gateway",
            &path,
            Duration::from_millis(50),
        );

        assert!(watcher.reload_now().expect("reload"));
        assert_eq!(monitor.current().rpm, 500);
        assert_eq!(monitor.current().model, "fast");
    }

    #[tokio::test]
    async fn unchanged_content_is_not_republished() {
        let (_dir, path, monitor) = setup();
        std::fs::write(&path, r#"{"Gateway":{"rpm":10}}"#).expect("write");
        let watcher = OptionsFileWatcher::new(
            Arc::clone(&monitor),
            "Gateway",
            &path,
            Duration::from_millis(50),
        );
        let mut receiver = monitor.subscribe();

        assert!(watcher.reload_now().expect("first reload"));
        receiver.changed().await.expect("first update");
        assert!(!watcher.reload_now().expect("unchanged reload"));
        assert!(!receiver.has_changed().expect("sender alive"));
    }

    #[tokio::test]
    async fn a_missing_file_keeps_the_current_snapshot() {
        let (_dir, path, monitor) = setup();
        let watcher = OptionsFileWatcher::new(
            Arc::clone(&monitor),
            "Gateway",
            &path,
            Duration::from_millis(50),
        );
        assert!(!watcher.reload_now().expect("missing is not an error"));
        assert_eq!(*monitor.current(), Gateway::default());
    }

    #[tokio::test]
    async fn invalid_json_is_rejected_and_the_old_snapshot_stays() {
        let (_dir, path, monitor) = setup();
        std::fs::write(&path, r#"{"Gateway":{"rpm":25}}"#).expect("write");
        let watcher = OptionsFileWatcher::new(
            Arc::clone(&monitor),
            "Gateway",
            &path,
            Duration::from_millis(50),
        );
        assert!(watcher.reload_now().expect("valid"));

        std::fs::write(&path, "{ not json").expect("write");
        assert!(matches!(
            watcher.reload_now(),
            Err(Error::Validation(message)) if message.contains("invalid JSON")
        ));
        assert_eq!(
            monitor.current().rpm,
            25,
            "a bad edit must not drop the running configuration"
        );

        // Same broken content: not reported again until the bytes change.
        assert!(!watcher.reload_now().expect("hash unchanged"));
        std::fs::write(&path, r#"{"Gateway":{"rpm":26}}"#).expect("write");
        assert!(watcher.reload_now().expect("fixed"));
        assert_eq!(monitor.current().rpm, 26);
    }

    #[tokio::test]
    async fn periodic_tick_applies_the_file() {
        let (_dir, path, monitor) = setup();
        std::fs::write(&path, r#"{"Gateway":{"rpm":7}}"#).expect("write");
        let watcher = OptionsFileWatcher::new(
            Arc::clone(&monitor),
            "Gateway",
            &path,
            Duration::from_millis(10),
        );
        watcher.tick().await.expect("tick");
        assert_eq!(monitor.current().rpm, 7);
    }
}
