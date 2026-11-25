//! Request-level idempotency middleware: replayable response headers, fencing
//! tokens, the route allow-list and DI auto-discovery.
//!
//! The in-memory/EF store protocol tests live with their stores; this file
//! covers the middleware contract (what the framework captures, replays and
//! passes through the port) and how the host assembles the middleware.

mod test_utils;

use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use webx_core::error::{Error, Result};
use webx_core::idempotency::{
    IIdempotencyStore, IdempotencyBegin, IdempotencyLease, IdempotentResponse,
    IDEMPOTENCY_KEY_HEADER, IDEMPOTENCY_REPLAYED_HEADER,
};
use webx_core::middleware::{IMiddleware, NextStage};
use webx_core::mode::AppMode;
use webx_host::idempotency::{IdempotencyMiddleware, InMemoryIdempotencyStore};
use webx_host::server::Host;

use test_utils::TestHttpContext;

const TTL: Duration = Duration::from_secs(60);

fn peer() -> SocketAddr {
    SocketAddr::from(([127, 0, 0, 1], 40000))
}

fn context(method: &str, path: &str, key: &str) -> TestHttpContext {
    TestHttpContext::new(method, path)
        .with_header(IDEMPOTENCY_KEY_HEADER, key)
        .with_remote_addr(peer())
}

/// Everything the store saw, for assertions.
#[derive(Default)]
struct RecordingStore {
    begins: AtomicUsize,
    completes: AtomicUsize,
    releases: AtomicUsize,
}

impl RecordingStore {
    fn begins(&self) -> usize {
        self.begins.load(Ordering::SeqCst)
    }

    fn completes(&self) -> usize {
        self.completes.load(Ordering::SeqCst)
    }

    fn releases(&self) -> usize {
        self.releases.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl IIdempotencyStore for RecordingStore {
    async fn begin(&self, _key: &str, _ttl: Duration) -> Result<IdempotencyBegin> {
        self.begins.fetch_add(1, Ordering::SeqCst);
        Ok(IdempotencyBegin::Started)
    }

    async fn complete(
        &self,
        _key: &str,
        _response: IdempotentResponse,
        _ttl: Duration,
    ) -> Result<()> {
        self.completes.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    async fn release(&self, _key: &str) -> Result<()> {
        self.releases.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

/// A store that issues one fencing token per attempt and records which token the
/// middleware passes back on completion/release, plus the durations it was given.
#[derive(Default)]
struct LeaseRecordingStore {
    next_token: AtomicUsize,
    completed: Mutex<Vec<Option<String>>>,
    released: Mutex<Vec<Option<String>>>,
    begins: Mutex<Vec<(Duration, Duration)>>,
    token_less_calls: AtomicUsize,
}

#[async_trait]
impl IIdempotencyStore for LeaseRecordingStore {
    async fn begin(&self, _key: &str, _ttl: Duration) -> Result<IdempotencyBegin> {
        self.token_less_calls.fetch_add(1, Ordering::SeqCst);
        Ok(IdempotencyBegin::Started)
    }

    async fn complete(
        &self,
        _key: &str,
        _response: IdempotentResponse,
        _ttl: Duration,
    ) -> Result<()> {
        self.token_less_calls.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    async fn release(&self, _key: &str) -> Result<()> {
        self.token_less_calls.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    async fn begin_lease(
        &self,
        _key: &str,
        retention: Duration,
        lease: Duration,
    ) -> Result<(IdempotencyBegin, Option<IdempotencyLease>)> {
        self.begins.lock().expect("lock").push((retention, lease));
        let sequence = self.next_token.fetch_add(1, Ordering::SeqCst) + 1;
        let token = format!("attempt-{sequence}");
        Ok((
            IdempotencyBegin::Started,
            Some(IdempotencyLease::new(token)),
        ))
    }

    async fn complete_lease(
        &self,
        _key: &str,
        lease: Option<&IdempotencyLease>,
        _response: IdempotentResponse,
        _ttl: Duration,
    ) -> Result<()> {
        self.completed
            .lock()
            .expect("lock")
            .push(lease.map(|lease| lease.token.clone()));
        Ok(())
    }

    async fn release_lease(&self, _key: &str, lease: Option<&IdempotencyLease>) -> Result<()> {
        self.released
            .lock()
            .expect("lock")
            .push(lease.map(|lease| lease.token.clone()));
        Ok(())
    }
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .expect("bind")
        .local_addr()
        .expect("addr")
        .port()
}

async fn wait_until_live(base: &str) {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        if let Ok(response) = reqwest::get(format!("{base}/health/live")).await {
            if response.status().as_u16() == 200 {
                return;
            }
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the host did not become ready"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

fn header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.as_str())
}

/// The replayable response headers are captured on the first response and
/// replayed, in whitelist order, together with the body.
#[tokio::test]
async fn a_replay_carries_the_whitelisted_response_headers_in_order() {
    let store: Arc<dyn IIdempotencyStore> = Arc::new(InMemoryIdempotencyStore::new());
    let middleware = IdempotencyMiddleware::new(store, TTL);

    let mut first = context("POST", "/api/orders", "headers-1");
    let next = NextStage::new(|ctx| {
        Box::pin(async move {
            let response = ctx.response_mut();
            response.set_status(201);
            response.set_header("content-type", "application/json");
            response.set_header("location", "/api/orders/1");
            response.set_header("etag", "\"v1\"");
            response.set_header("last-modified", "Wed, 21 Oct 2015 07:28:00 GMT");
            response.set_header("cache-control", "private, max-age=60");
            response.set_header("x-not-replayable", "secret-value");
            response.write_bytes(br#"{"id":1}"#.to_vec()).await
        })
    });
    middleware
        .invoke_next(&mut first, next)
        .await
        .expect("first request");
    let (status, first_headers, _) = first.into_response_parts();
    assert_eq!(status, 201);
    assert_eq!(header(&first_headers, "location"), Some("/api/orders/1"));

    // A retry with the same key replays the response: the handler (and the
    // continuation) must not run again.
    let mut retry = context("POST", "/api/orders", "headers-1");
    let next = NextStage::new(|ctx| {
        Box::pin(async move {
            ctx.response_mut().set_status(500);
            Ok(())
        })
    });
    middleware
        .invoke_next(&mut retry, next)
        .await
        .expect("replay");
    let (status, headers, body) = retry.into_response_parts();
    assert_eq!(status, 201, "the replayed status wins");
    assert_eq!(body.as_deref(), Some(&br#"{"id":1}"#[..]));
    assert_eq!(header(&headers, IDEMPOTENCY_REPLAYED_HEADER), Some("true"));
    assert_eq!(header(&headers, "content-type"), Some("application/json"));
    assert_eq!(header(&headers, "x-not-replayable"), None);
    // Order is the whitelist order, independent of the original set order.
    let replay_order: Vec<String> = headers
        .iter()
        .map(|(name, _)| name.to_ascii_lowercase())
        .filter(|name| {
            ["location", "etag", "last-modified", "cache-control"].contains(&name.as_str())
        })
        .collect();
    assert_eq!(
        replay_order,
        vec!["location", "etag", "last-modified", "cache-control"]
    );
    assert_eq!(header(&headers, "location"), Some("/api/orders/1"));
    assert_eq!(header(&headers, "etag"), Some("\"v1\""));
    assert_eq!(
        header(&headers, "last-modified"),
        Some("Wed, 21 Oct 2015 07:28:00 GMT")
    );
    assert_eq!(
        header(&headers, "cache-control"),
        Some("private, max-age=60")
    );
}

/// The middleware calls the lease variants and passes the token back on both
/// completion and release.
#[tokio::test]
async fn the_middleware_passes_the_lease_token_back_to_the_store() {
    let store = Arc::new(LeaseRecordingStore::default());
    let middleware =
        IdempotencyMiddleware::new(Arc::clone(&store) as Arc<dyn IIdempotencyStore>, TTL);

    let mut ctx = context("POST", "/api/orders", "lease-1");
    let next = NextStage::new(|ctx| {
        Box::pin(async move {
            ctx.response_mut().set_status(201);
            ctx.response_mut().write_bytes(b"ok".to_vec()).await
        })
    });
    middleware
        .invoke_next(&mut ctx, next)
        .await
        .expect("request");

    assert_eq!(
        store.completed.lock().expect("lock").clone(),
        vec![Some("attempt-1".to_string())],
        "complete_lease must receive the token issued by begin_lease"
    );
    assert_eq!(
        store.token_less_calls.load(Ordering::SeqCst),
        0,
        "the middleware must use the lease variants"
    );

    // A non-2xx response releases the reservation with the same token.
    let mut failed = context("POST", "/api/orders", "lease-2");
    let next = NextStage::new(|ctx| {
        Box::pin(async move {
            ctx.response_mut().set_status(409);
            Ok(())
        })
    });
    middleware
        .invoke_next(&mut failed, next)
        .await
        .expect("release path");
    assert_eq!(
        store.released.lock().expect("lock").clone(),
        vec![Some("attempt-2".to_string())],
        "release_lease must receive the token issued by begin_lease"
    );
}

/// The middleware hands the store the replay retention plus an explicit lease,
/// defaulting to 60s and capped at the retention.
#[tokio::test]
async fn the_middleware_passes_an_explicit_lease_capped_at_the_retention() {
    let store = Arc::new(LeaseRecordingStore::default());
    let middleware = IdempotencyMiddleware::new(
        Arc::clone(&store) as Arc<dyn IIdempotencyStore>,
        Duration::from_secs(120),
    );
    let mut ctx = context("POST", "/api/orders", "lease-duration-1");
    middleware
        .invoke_next(&mut ctx, ok_next())
        .await
        .expect("request");
    assert_eq!(
        store.begins.lock().expect("lock").clone(),
        vec![(Duration::from_secs(120), Duration::from_secs(60))],
        "begin_lease receives (retention, lease): the TTL and the default 60s lease"
    );

    // A configured lease longer than the retention is capped at the retention, so
    // the reservation never outlives the response it protects.
    let store = Arc::new(LeaseRecordingStore::default());
    let middleware = IdempotencyMiddleware::new(
        Arc::clone(&store) as Arc<dyn IIdempotencyStore>,
        Duration::from_secs(30),
    )
    .with_lease(Duration::from_secs(600));
    let mut ctx = context("POST", "/api/orders", "lease-duration-2");
    middleware
        .invoke_next(&mut ctx, ok_next())
        .await
        .expect("request");
    assert_eq!(
        store.begins.lock().expect("lock").clone(),
        vec![(Duration::from_secs(30), Duration::from_secs(30))],
        "lease = min(LeaseSeconds, TtlSeconds)"
    );
}

/// A handler error also releases (never completes) the reservation.
#[tokio::test]
async fn a_handler_error_releases_the_lease_with_its_token() {
    let store = Arc::new(LeaseRecordingStore::default());
    let middleware =
        IdempotencyMiddleware::new(Arc::clone(&store) as Arc<dyn IIdempotencyStore>, TTL);

    let mut ctx = context("POST", "/api/orders", "lease-error");
    let next = NextStage::new(|_ctx| {
        Box::pin(async move { Err(Error::internal("handler failed".to_string())) })
    });
    middleware
        .invoke_next(&mut ctx, next)
        .await
        .expect_err("the handler error propagates");

    assert_eq!(
        store.released.lock().expect("lock").clone(),
        vec![Some("attempt-1".to_string())]
    );
    assert!(store.completed.lock().expect("lock").is_empty());
}

/// Requests outside `Paths` (or inside `SkipPaths`) never touch the store.
#[tokio::test]
async fn the_route_allow_list_skips_the_store_round_trip() {
    let store = Arc::new(RecordingStore::default());
    let middleware =
        IdempotencyMiddleware::new(Arc::clone(&store) as Arc<dyn IIdempotencyStore>, TTL)
            .with_paths(
                vec!["/api/orders".to_string()],
                vec!["/api/orders/internal".to_string()],
            );

    let mut allowed = context("POST", "/api/orders/123", "path-1");
    middleware
        .invoke_next(&mut allowed, ok_next())
        .await
        .expect("allowed path");
    assert_eq!(store.begins(), 1, "an allowed path reserves the key");
    assert_eq!(store.completes(), 1);

    for path in [
        "/api/orders-archive", // path-segment boundary, not a string prefix
        "/api/users",
        "/api/orders/internal", // SkipPaths wins over Paths
    ] {
        let mut ctx = context("POST", path, "path-2");
        middleware
            .invoke_next(&mut ctx, ok_next())
            .await
            .expect("skipped path");
        assert_eq!(
            store.begins(),
            1,
            "{path} must pass through without a store round trip"
        );
        let (status, _, _) = ctx.into_response_parts();
        assert_eq!(status, 200);
    }
}

/// An empty `Paths` list keeps the pre-2.8 behavior: every path is idempotent.
#[tokio::test]
async fn an_empty_allow_list_covers_every_path() {
    let store = Arc::new(RecordingStore::default());
    let middleware =
        IdempotencyMiddleware::new(Arc::clone(&store) as Arc<dyn IIdempotencyStore>, TTL);
    let mut ctx = context("POST", "/any/path", "all-1");
    middleware
        .invoke_next(&mut ctx, ok_next())
        .await
        .expect("request");
    assert_eq!(store.begins(), 1);
}

fn ok_next() -> NextStage {
    NextStage::new(|ctx| {
        Box::pin(async move {
            ctx.response_mut().set_status(200);
            ctx.response_mut().write_bytes(b"ok".to_vec()).await
        })
    })
}

/// `Idempotency.Enabled = true` picks up an `IIdempotencyStore` registered in DI.
#[tokio::test]
async fn enabled_config_uses_the_store_registered_in_di() {
    let store = Arc::new(RecordingStore::default());
    let registered = Arc::clone(&store);
    let port = free_port();
    let host = Host::builder()
        .mode(AppMode::Development)
        .no_spa()
        .configure(|builder| {
            builder.options(|options| {
                options.idempotency.enabled = true;
            })
        })
        .register(move |services| services.instance::<dyn IIdempotencyStore>(registered))
        .build();
    let handle = host.server_handle();
    tokio::spawn(async move {
        let _ = host.run_at(&format!("127.0.0.1:{port}")).await;
    });
    let base = format!("http://127.0.0.1:{port}");
    wait_until_live(&base).await;

    let response = reqwest::Client::new()
        .post(format!("{base}/health"))
        .header(IDEMPOTENCY_KEY_HEADER, "di-1")
        .send()
        .await
        .expect("request");
    assert!(
        response.status().is_client_error() || response.status().is_server_error(),
        "POST /health is not a route; the middleware still ran, got {}",
        response.status()
    );
    assert_eq!(
        store.begins(),
        1,
        "the DI-registered store must be used when Idempotency.Enabled is set"
    );
    assert_eq!(
        store.releases(),
        1,
        "the non-2xx probe response releases the reservation through the same store"
    );

    handle.shutdown();
}

/// An explicit `use_idempotency` wins over a store registered in DI.
#[tokio::test]
async fn explicit_use_idempotency_wins_over_the_di_store() {
    let di_store = Arc::new(RecordingStore::default());
    let explicit_store = Arc::new(RecordingStore::default());
    let registered = Arc::clone(&di_store);
    let explicit = Arc::clone(&explicit_store) as Arc<dyn IIdempotencyStore>;
    let port = free_port();
    let host = Host::builder()
        .mode(AppMode::Development)
        .no_spa()
        .configure(|builder| {
            builder.options(|options| {
                options.idempotency.enabled = true;
            })
        })
        .register(move |services| services.instance::<dyn IIdempotencyStore>(registered))
        .use_idempotency(explicit, TTL)
        .build();
    let handle = host.server_handle();
    tokio::spawn(async move {
        let _ = host.run_at(&format!("127.0.0.1:{port}")).await;
    });
    let base = format!("http://127.0.0.1:{port}");
    wait_until_live(&base).await;

    reqwest::Client::new()
        .post(format!("{base}/health"))
        .header(IDEMPOTENCY_KEY_HEADER, "priority-1")
        .send()
        .await
        .expect("request");
    assert_eq!(explicit_store.begins(), 1, "the explicit store must win");
    assert_eq!(
        explicit_store.releases(),
        1,
        "the probe response releases through the explicit store"
    );
    assert_eq!(di_store.begins(), 0, "the DI store must not be used");
    assert_eq!(di_store.releases(), 0, "the DI store must not be used");

    handle.shutdown();
}

/// M8: the Production warning for `Enabled=true` + `AllowAnonymous=true` + empty
/// `Paths` must not change behavior — the host still starts and the middleware
/// still reserves/releases through the store. (The warning itself is a tracing
/// event; this pins the documented "warn, do not fail" half of the contract.)
#[tokio::test]
async fn production_enabled_idempotency_warns_but_still_works() {
    let store = Arc::new(RecordingStore::default());
    let registered = Arc::clone(&store);
    let port = free_port();
    let host = Host::builder()
        .mode(AppMode::Production)
        .no_spa()
        .configure(|builder| {
            builder.options(|options| {
                options.idempotency.enabled = true;
                // The risky scope that triggers the startup warning.
                options.idempotency.allow_anonymous = true;
                options.idempotency.paths.clear();
            })
        })
        .register(move |services| services.instance::<dyn IIdempotencyStore>(registered))
        .build();
    let handle = host.server_handle();
    tokio::spawn(async move {
        let _ = host.run_at(&format!("127.0.0.1:{port}")).await;
    });
    let base = format!("http://127.0.0.1:{port}");
    wait_until_live(&base).await;

    reqwest::Client::new()
        .post(format!("{base}/health"))
        .header(IDEMPOTENCY_KEY_HEADER, "production-1")
        .send()
        .await
        .expect("request");
    assert_eq!(store.begins(), 1, "the middleware is still installed");
    assert_eq!(
        store.releases(),
        1,
        "the non-2xx probe response releases the reservation"
    );

    handle.shutdown();
}
