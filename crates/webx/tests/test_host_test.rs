//! 2.4-5: `TestHost` dispatches typed requests in-process (zero ports), with a
//! controllable clock, per-dispatch DI scopes and deterministic chaos.
//!
//! These tests never bind a socket and run on the default current-thread
//! `#[tokio::test]` runtime, so they are deterministic and fast.

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use webx::*;

#[derive(Debug, Serialize, Deserialize)]
struct EchoView {
    value: String,
    user_id: Option<String>,
    now_unix: u64,
    scoped_id: u64,
}

#[derive(Default, WebxRequestMeta)]
struct EchoRequest {
    value: String,
}

impl IRequest<EchoView> for EchoRequest {}

/// Scoped probe: a fresh instance per dispatch scope.
struct ScopedProbe(u64);

#[derive(Inject)]
struct EchoHandler {
    #[inject]
    clock: Arc<dyn IClock>,
    #[inject]
    scoped: Arc<ScopedProbe>,
    #[inject]
    calls: Arc<AtomicUsize>,
}

#[handler(inject)]
#[async_trait::async_trait]
impl IRequestHandler<EchoRequest, EchoView> for EchoHandler {
    async fn handle(&mut self, req: EchoRequest) -> ActionResult<EchoView> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(EchoView {
            value: req.value,
            user_id: RequestContext::identity().map(|user| user.id.clone()),
            now_unix: self.clock.now_unix(),
            scoped_id: self.scoped.0,
        })
    }
}

/// Build a test host with the request's handler wired through the mediator.
fn test_host(clock: Arc<TestClock>) -> (TestHost, Arc<AtomicUsize>) {
    let calls = Arc::new(AtomicUsize::new(0));
    let handler_calls = Arc::clone(&calls);
    let scoped_counter = Arc::new(AtomicU64::new(0));
    let scoped = Arc::clone(&scoped_counter);
    let host = TestHost::from_builder_with_clock(
        Host::builder()
            .mode(AppMode::Development)
            .no_spa()
            .register(move |svc| {
                svc.add_mediator()
                    .singleton({
                        let calls = Arc::clone(&handler_calls);
                        move |_| calls.clone()
                    })
                    .scoped(move |_| Arc::new(ScopedProbe(scoped.fetch_add(1, Ordering::SeqCst))))
            }),
        clock,
    );
    (host, calls)
}

#[tokio::test]
async fn dispatch_runs_the_handler_without_binding_a_port() {
    let clock = Arc::new(TestClock::from_unix(1_700_000_000));
    let (host, calls) = test_host(Arc::clone(&clock));
    let reply = host
        .dispatch(EchoRequest {
            value: "hi".to_string(),
        })
        .await
        .expect("dispatch");
    assert_eq!(reply.value, "hi");
    assert_eq!(reply.user_id, None, "anonymous dispatch");
    assert_eq!(reply.now_unix, 1_700_000_000);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    host.shutdown();
}

#[tokio::test]
async fn dispatch_as_sets_the_request_identity() {
    let clock = Arc::new(TestClock::from_unix(1_700_000_000));
    let (host, _calls) = test_host(clock);
    let identity = IdentityUser {
        id: "user-42".to_string(),
        name: "Alice".to_string(),
        ..Default::default()
    };
    let reply = host
        .dispatch_as(
            identity,
            EchoRequest {
                value: "whoami".to_string(),
            },
        )
        .await
        .expect("dispatch");
    assert_eq!(reply.user_id.as_deref(), Some("user-42"));
    host.shutdown();
}

#[tokio::test]
async fn test_clock_is_controllable_from_the_test() {
    let clock = Arc::new(TestClock::from_unix(1_700_000_000));
    let (host, _calls) = test_host(Arc::clone(&clock));
    let first = host
        .dispatch(EchoRequest {
            value: "t1".to_string(),
        })
        .await
        .expect("dispatch");
    assert_eq!(first.now_unix, 1_700_000_000);
    clock.advance(std::time::Duration::from_secs(90));
    let second = host
        .dispatch(EchoRequest {
            value: "t2".to_string(),
        })
        .await
        .expect("dispatch");
    assert_eq!(second.now_unix, 1_700_000_090, "no wall-clock drift");
    host.shutdown();
}

#[tokio::test]
async fn scoped_services_are_fresh_per_dispatch() {
    let clock = Arc::new(TestClock::from_unix(1_700_000_000));
    let (host, _calls) = test_host(clock);
    let first = host
        .dispatch(EchoRequest {
            value: "a".to_string(),
        })
        .await
        .expect("dispatch");
    let second = host
        .dispatch(EchoRequest {
            value: "b".to_string(),
        })
        .await
        .expect("dispatch");
    assert_eq!(first.scoped_id, 0);
    assert_eq!(second.scoped_id, 1, "each dispatch owns a fresh scope");
    host.shutdown();
}

#[tokio::test]
async fn chaos_injects_failures_and_latency_deterministically() {
    let clock = Arc::new(TestClock::from_unix(1_700_000_000));
    let (host, calls) = test_host(clock);
    host.chaos().fail_next(1);
    let failure = host
        .dispatch(EchoRequest {
            value: "fault".to_string(),
        })
        .await;
    assert!(
        matches!(&failure, Err(error) if error.status_code() == 503),
        "injected fault must be a typed 503: {failure:?}"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0, "handler never ran");

    host.chaos()
        .delay_next(std::time::Duration::from_millis(20));
    let started = std::time::Instant::now();
    host.dispatch(EchoRequest {
        value: "delayed".to_string(),
    })
    .await
    .expect("dispatch after chaos");
    assert!(
        started.elapsed() >= std::time::Duration::from_millis(15),
        "injected latency must be observable"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(host.chaos().dispatched(), 2);
    host.shutdown();
}
