//! Bridge process-local presence transitions to a shared [`IOnlinePresence`] store.
//!
//! `HostBuilder::use_online_presence(store)` installs:
//!
//! * the store as a DI singleton (`Arc<dyn IOnlinePresence>`);
//! * a [`PresenceBridge`] observer that maps the registry's atomic first/last
//!   transitions to `user:<id>` / `tenant:<id>` scopes;
//! * a [`PresenceHeartbeat`] background service that refreshes the TTL of the
//!   members this node owns.
//!
//! The registry already computes first/last inside its lock, so the bridge does
//! not count connections. Members are per **(user, node)**: two devices of one
//! user on the same node are one presence member (the shared store's job is
//! online authority, not device inventory). A node crash leaves entries that
//! expire after `ttl` unless another node keeps them alive; the owning node's
//! heartbeat is what prevents that for a healthy node.

use std::collections::HashSet;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use async_trait::async_trait;

use webx_core::error::Result;
use webx_core::hosted::{delay, BackgroundService, CancellationToken};
use webx_core::realtime::{
    IOnlinePresence, IPresenceObserver, PresenceDirection, PresenceMember, PresenceScope,
    PresenceTransition,
};
use webx_core::time::IClock;

/// Default lifetime of a presence member: long enough that a missed heartbeat
/// round does not flap a healthy node, short enough that a crashed node's users
/// disappear within a minute.
pub(crate) const DEFAULT_PRESENCE_TTL: Duration = Duration::from_secs(60);

/// Maps presence transitions into the shared store and tracks the members this
/// node must keep alive.
pub(crate) struct PresenceBridge {
    store: Arc<dyn IOnlinePresence>,
    ttl: Duration,
    node_id: String,
    clock: Arc<dyn IClock>,
    tracked: Mutex<HashSet<(PresenceScope, PresenceMember)>>,
}

impl PresenceBridge {
    /// Build a bridge for this node.
    pub(crate) fn new(
        store: Arc<dyn IOnlinePresence>,
        ttl: Duration,
        node_id: impl Into<String>,
        clock: Arc<dyn IClock>,
    ) -> Self {
        Self {
            store,
            ttl: ttl.max(Duration::from_secs(1)),
            node_id: node_id.into(),
            clock,
            tracked: Mutex::new(HashSet::new()),
        }
    }

    /// How often the heartbeat refreshes owned members (one third of the TTL,
    /// at least one second).
    pub(crate) fn heartbeat_interval(&self) -> Duration {
        (self.ttl / 3).max(Duration::from_secs(1))
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashSet<(PresenceScope, PresenceMember)>> {
        self.tracked.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn member(&self, transition: &PresenceTransition) -> PresenceMember {
        PresenceMember::new(transition.user_id.clone(), self.node_id.clone())
            .with_node_id(self.node_id.clone())
            .with_joined_at_unix_secs(self.clock.now_unix())
    }

    fn scopes(transition: &PresenceTransition) -> Vec<PresenceScope> {
        let mut scopes = vec![PresenceScope::user(transition.user_id.clone())];
        if let Some(tenant_id) = &transition.tenant_id {
            if !tenant_id.trim().is_empty() {
                scopes.push(PresenceScope::tenant(tenant_id.clone()));
            }
        }
        scopes
    }

    /// Refresh every member this node owns; failed scopes are logged and retried
    /// on the next heartbeat instead of aborting the loop.
    ///
    /// Touches are issued in bounded-concurrent chunks: a node with thousands of
    /// members would otherwise spend one round-trip per member sequentially and
    /// risk missing the TTL deadline before the round completes.
    pub(crate) async fn heartbeat(&self) {
        const CONCURRENCY: usize = 32;
        let entries: Vec<(PresenceScope, PresenceMember)> = self.lock().iter().cloned().collect();
        for chunk in entries.chunks(CONCURRENCY) {
            let touches = chunk.iter().map(|(scope, member)| async move {
                if let Err(error) = self.store.touch(scope, member, self.ttl).await {
                    tracing::warn!(
                        scope = %scope,
                        error = %error,
                        "presence heartbeat failed; retrying on the next round"
                    );
                }
            });
            futures_util::future::join_all(touches).await;
        }
    }
}

#[async_trait]
impl IPresenceObserver for PresenceBridge {
    async fn on_presence_transition(&self, transition: PresenceTransition) {
        let member = self.member(&transition);
        match transition.direction {
            // First connection on this node: the user becomes online here.
            PresenceDirection::Connected if transition.is_first() => {
                for scope in Self::scopes(&transition) {
                    match self.store.join(&scope, member.clone(), self.ttl).await {
                        Ok(_) => {
                            self.lock().insert((scope, member.clone()));
                        }
                        Err(error) => tracing::warn!(
                            scope = %scope,
                            user = %transition.user_id,
                            error = %error,
                            "failed to publish online presence; the local session stays online"
                        ),
                    }
                }
            }
            // Last connection on this node: the user goes offline here.
            PresenceDirection::Disconnected if transition.is_last() => {
                for scope in Self::scopes(&transition) {
                    if let Err(error) = self.store.leave(&scope, &member).await {
                        tracing::warn!(
                            scope = %scope,
                            user = %transition.user_id,
                            error = %error,
                            "failed to remove online presence"
                        );
                    }
                    self.lock().remove(&(scope, member.clone()));
                }
            }
            // A connection change while the user stays online: the heartbeat
            // keeps the entry alive; nothing to do.
            _ => {}
        }
    }
}

/// Periodically refreshes the TTL of the presence members this node owns.
pub(crate) struct PresenceHeartbeat {
    bridge: Arc<PresenceBridge>,
    interval: Duration,
}

impl PresenceHeartbeat {
    /// Build a heartbeat over a bridge.
    pub(crate) fn new(bridge: Arc<PresenceBridge>) -> Self {
        let interval = bridge.heartbeat_interval();
        Self { bridge, interval }
    }
}

#[async_trait]
impl BackgroundService for PresenceHeartbeat {
    async fn execute(&self, ct: CancellationToken) -> Result<()> {
        loop {
            // `delay` returns `false` when the token is cancelled and `true`
            // when the interval elapsed: inverted breaks/shutdown here means no
            // periodic refresh, and a cancelled token would spin the loop at
            // 100% CPU instead of observing shutdown.
            if !delay(&ct, self.interval).await {
                break;
            }
            self.bridge.heartbeat().await;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use webx_core::realtime::InMemoryOnlinePresence;
    use webx_core::time::TestClock;

    fn transition(
        user_id: &str,
        tenant_id: Option<&str>,
        connections: usize,
        direction: PresenceDirection,
    ) -> PresenceTransition {
        PresenceTransition {
            user_id: user_id.to_string(),
            tenant_id: tenant_id.map(str::to_string),
            connections,
            direction,
        }
    }

    #[tokio::test]
    async fn first_and_last_transition_join_and_leave_both_scopes() {
        let clock = Arc::new(TestClock::from_unix(1_700_000_000));
        let store = Arc::new(InMemoryOnlinePresence::with_clock(
            Default::default(),
            clock.clone(),
        ));
        let bridge = PresenceBridge::new(
            store.clone(),
            Duration::from_secs(30),
            "node-a",
            clock.clone(),
        );
        bridge
            .on_presence_transition(transition(
                "u1",
                Some("t1"),
                1,
                PresenceDirection::Connected,
            ))
            .await;
        assert_eq!(
            store
                .count(&PresenceScope::user("u1"))
                .await
                .expect("user count"),
            1
        );
        assert_eq!(
            store
                .count(&PresenceScope::tenant("t1"))
                .await
                .expect("tenant count"),
            1
        );

        // An additional device on the same node is not a transition; nothing changes.
        bridge
            .on_presence_transition(transition(
                "u1",
                Some("t1"),
                2,
                PresenceDirection::Connected,
            ))
            .await;
        assert_eq!(
            store
                .count(&PresenceScope::user("u1"))
                .await
                .expect("user count"),
            1
        );

        bridge
            .on_presence_transition(transition(
                "u1",
                Some("t1"),
                0,
                PresenceDirection::Disconnected,
            ))
            .await;
        assert_eq!(
            store
                .count(&PresenceScope::user("u1"))
                .await
                .expect("user count"),
            0
        );
        assert_eq!(
            store
                .count(&PresenceScope::tenant("t1"))
                .await
                .expect("tenant count"),
            0
        );
    }

    #[tokio::test]
    async fn heartbeat_keeps_a_healthy_node_online_and_ttl_drops_a_dead_one() {
        let clock = Arc::new(TestClock::from_unix(1_700_000_000));
        let store = Arc::new(InMemoryOnlinePresence::with_clock(
            Default::default(),
            clock.clone(),
        ));
        let bridge = PresenceBridge::new(
            store.clone(),
            Duration::from_secs(30),
            "node-a",
            clock.clone(),
        );
        bridge
            .on_presence_transition(transition("u1", None, 1, PresenceDirection::Connected))
            .await;

        // Two heartbeat intervals keep it online past the original TTL.
        clock.advance(Duration::from_secs(10));
        bridge.heartbeat().await;
        clock.advance(Duration::from_secs(10));
        bridge.heartbeat().await;
        clock.advance(Duration::from_secs(10));
        assert_eq!(
            store
                .count(&PresenceScope::user("u1"))
                .await
                .expect("count"),
            1
        );

        // Without the heartbeat the member expires.
        clock.advance(Duration::from_secs(31));
        assert_eq!(
            store
                .count(&PresenceScope::user("u1"))
                .await
                .expect("count"),
            0
        );
    }

    #[tokio::test]
    async fn heartbeat_observes_cancellation_instead_of_spinning() {
        let clock = Arc::new(TestClock::from_unix(1_700_000_000));
        let store = Arc::new(InMemoryOnlinePresence::with_clock(
            Default::default(),
            clock.clone(),
        ));
        // A short TTL gives a short heartbeat interval, so the test does not
        // depend on wall-clock seconds.
        let bridge = PresenceBridge::new(store, Duration::from_secs(3), "node-a", clock);
        let heartbeat = PresenceHeartbeat::new(Arc::new(bridge));
        let token = CancellationToken::new();
        let worker_token = token.clone();
        let join = tokio::spawn(async move { heartbeat.execute(worker_token).await });

        tokio::time::sleep(Duration::from_millis(50)).await;
        token.cancel();
        tokio::time::timeout(Duration::from_secs(1), join)
            .await
            .expect("heartbeat must observe cancellation instead of spinning")
            .expect("heartbeat task panicked")
            .expect("heartbeat returned an error");
    }
}
