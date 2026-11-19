//! Hot-reloadable options: an immutable snapshot plus change notifications.
//!
//! `add_options::<T>` binds a configuration section once at startup. A gateway
//! needs to change routing/pricing/key settings without restarting, so this
//! module adds the missing half: [`OptionsMonitor<T>`] holds the current
//! snapshot in an [`arc_swap::ArcSwap`] and publishes each replacement through a
//! `tokio::sync::watch` channel.
//!
//! * [`IOptionsMonitor::current`] is the hot-path read: a lock-free `Arc` clone.
//! * [`IOptionsMonitor::subscribe`] gives consumers a receiver that observes
//!   every replacement (a lagging consumer still reads the latest value).
//! * Values are immutable. Replacements are whole snapshots, never in-place
//!   mutation, so a handler either sees the old or the new configuration.
//!
//! The host wires this to `appsettings.json` with
//! `HostBuilder::add_options_monitor` (startup snapshot) and
//! `HostBuilder::watch_options_file` (poll a JSON file and reload it in place).

use arc_swap::ArcSwap;
use std::fmt;
use std::sync::Arc;
use tokio::sync::watch;

/// Read-only handle to the current `T` snapshot.
///
/// Implemented by [`OptionsMonitor`]; applications depend on this trait so a
/// test can inject a fixed monitor. `current()` must be cheap and must not
/// perform I/O.
pub trait IOptionsMonitor<T>: Send + Sync
where
    T: Send + Sync + 'static,
{
    /// The current snapshot.
    fn current(&self) -> Arc<T>;

    /// A receiver that sees every subsequent replacement.
    ///
    /// The receiver starts with the current snapshot; `changed()` resolves after
    /// the next [`OptionsMonitor::set`]. Dropping it unsubscribes.
    fn subscribe(&self) -> watch::Receiver<Arc<T>>;
}

/// Snapshot holder with change notification.
///
/// `T` is expected to be a plain configuration struct. Cloning the monitor is
/// not supported: share it through `Arc` (the host registers `Arc<OptionsMonitor<T>>`).
pub struct OptionsMonitor<T>
where
    T: Send + Sync + 'static,
{
    current: ArcSwap<T>,
    updates: watch::Sender<Arc<T>>,
}

impl<T> OptionsMonitor<T>
where
    T: Send + Sync + 'static,
{
    /// A monitor whose initial snapshot is `initial`.
    pub fn new(initial: T) -> Self {
        let initial = Arc::new(initial);
        let (updates, _receiver) = watch::channel(Arc::clone(&initial));
        Self {
            current: ArcSwap::from(initial),
            updates,
        }
    }

    /// Replace the snapshot and notify subscribers.
    ///
    /// Returns the snapshot that was current before the call, which lets a
    /// caller log or roll back a rejected reload.
    pub fn set(&self, value: T) -> Arc<T> {
        self.replace(Arc::new(value))
    }

    /// Replace the snapshot with an already shared value.
    pub fn replace(&self, value: Arc<T>) -> Arc<T> {
        let previous = self.current.swap(Arc::clone(&value));
        // `send_replace` updates even when nobody is subscribed, so a consumer
        // that subscribes after the reload still starts from the new value.
        self.updates.send_replace(value);
        previous
    }

    /// The current snapshot (same value as [`IOptionsMonitor::current`]).
    pub fn current(&self) -> Arc<T> {
        self.current.load_full()
    }

    /// Subscribe to subsequent replacements.
    pub fn subscribe(&self) -> watch::Receiver<Arc<T>> {
        self.updates.subscribe()
    }
}

impl<T> IOptionsMonitor<T> for OptionsMonitor<T>
where
    T: Send + Sync + 'static,
{
    fn current(&self) -> Arc<T> {
        self.current.load_full()
    }

    fn subscribe(&self) -> watch::Receiver<Arc<T>> {
        self.updates.subscribe()
    }
}

impl<T> fmt::Debug for OptionsMonitor<T>
where
    T: Send + Sync + 'static,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OptionsMonitor").finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct Settings {
        value: u32,
    }

    #[tokio::test]
    async fn current_returns_the_initial_snapshot() {
        let monitor = OptionsMonitor::new(Settings { value: 1 });
        assert_eq!(monitor.current().value, 1);
    }

    #[tokio::test]
    async fn set_replaces_the_snapshot_and_notifies_subscribers() {
        let monitor = OptionsMonitor::new(Settings { value: 1 });
        let mut receiver = monitor.subscribe();
        assert_eq!(receiver.borrow().value, 1);

        let previous = monitor.set(Settings { value: 2 });
        assert_eq!(previous.value, 1, "set returns the replaced snapshot");
        assert_eq!(monitor.current().value, 2);

        receiver.changed().await.expect("an update is published");
        assert_eq!(receiver.borrow().value, 2);
    }

    #[tokio::test]
    async fn a_late_subscriber_starts_from_the_latest_snapshot() {
        let monitor = OptionsMonitor::new(Settings { value: 1 });
        monitor.set(Settings { value: 9 });

        let receiver = monitor.subscribe();
        assert_eq!(receiver.borrow().value, 9);
        assert!(!receiver.has_changed().expect("sender alive"));
    }

    #[tokio::test]
    async fn replacing_without_subscribers_still_updates_the_value() {
        let monitor = OptionsMonitor::new(Settings { value: 1 });
        monitor.set(Settings { value: 3 });
        assert_eq!(monitor.current().value, 3);

        let receiver = monitor.subscribe();
        assert_eq!(receiver.borrow().value, 3);
        assert!(!receiver.has_changed().expect("sender alive"));
    }

    #[tokio::test]
    async fn the_trait_object_exposes_the_same_snapshot() {
        let monitor = Arc::new(OptionsMonitor::new(Settings { value: 4 }));
        let port: Arc<dyn IOptionsMonitor<Settings>> = monitor.clone();
        assert_eq!(port.current().value, 4);

        monitor.set(Settings { value: 5 });
        assert_eq!(port.current().value, 5);
    }
}
