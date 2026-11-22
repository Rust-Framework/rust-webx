//! Request-level idempotency: replay, per-key execution, in-flight 409 and the
//! header contract.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use webx::*;

static EXECUTIONS: AtomicUsize = AtomicUsize::new(0);
static SLOW: AtomicBool = AtomicBool::new(false);
static ENTERED: AtomicBool = AtomicBool::new(false);
static RELEASE: tokio::sync::Notify = tokio::sync::Notify::const_new();
/// The handler shares process-wide statics, so requests-driven tests run one at
/// a time (cargo runs test functions in parallel by default).
static TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[derive(Debug, Default, WebxRequestMeta)]
struct CreateRequest {}

#[post("/api/counter")]
impl IRequest<CounterReply> for CreateRequest {}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
struct CounterReply {
    count: usize,
}

#[derive(Default)]
struct CreateHandler;

#[handler]
#[async_trait::async_trait]
impl IRequestHandler<CreateRequest, CounterReply> for CreateHandler {
    async fn handle(&mut self, _req: CreateRequest) -> ActionResult<CounterReply> {
        let count = EXECUTIONS.fetch_add(1, Ordering::SeqCst) + 1;
        if SLOW.load(Ordering::SeqCst) {
            ENTERED.store(true, Ordering::SeqCst);
            RELEASE.notified().await;
        }
        Ok(CounterReply { count })
    }
}

fn free_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("free port");
    listener.local_addr().expect("addr").port()
}

async fn spawn_host(port: u16) -> ServerHandle {
    let address = format!("127.0.0.1:{port}");
    let host = Host::builder()
        .mode(AppMode::Development)
        .no_spa()
        .use_idempotency(
            Arc::new(InMemoryIdempotencyStore::new()),
            Duration::from_secs(60),
        )
        .build();
    let handle = host.server_handle();
    tokio::spawn(async move {
        let _ = host.run_at(&address).await;
    });
    let base = format!("http://127.0.0.1:{port}");
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if reqwest::get(format!("{base}/health/live")).await.is_ok() {
            return handle;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("host did not become ready");
}

async fn post(client: &reqwest::Client, base: &str, key: Option<&str>) -> reqwest::Response {
    let mut request = client.post(format!("{base}/api/counter"));
    if let Some(key) = key {
        request = request.header(IDEMPOTENCY_KEY_HEADER, key);
    }
    request.send().await.expect("request")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_repeated_key_replays_the_stored_response() {
    let _serial = TEST_LOCK.lock().await;
    EXECUTIONS.store(0, Ordering::SeqCst);
    SLOW.store(false, Ordering::SeqCst);
    let port = free_port();
    let handle = spawn_host(port).await;
    let client = reqwest::Client::new();
    let base = format!("http://127.0.0.1:{port}");

    let first = post(&client, &base, Some("order-1")).await;
    assert_eq!(first.status(), 200);
    assert_eq!(
        first
            .headers()
            .get("idempotency-replayed")
            .map(|v| v.as_bytes()),
        None
    );
    let first_body: CounterReply = first.json().await.expect("json");
    assert_eq!(first_body.count, 1);

    let retry = post(&client, &base, Some("order-1")).await;
    assert_eq!(retry.status(), 200);
    assert_eq!(
        retry
            .headers()
            .get("idempotency-replayed")
            .map(|v| v.as_bytes()),
        Some(&b"true"[..])
    );
    assert_eq!(
        retry.headers().get("content-type").expect("ct"),
        "application/json"
    );
    let retry_body: CounterReply = retry.json().await.expect("json");
    assert_eq!(retry_body, first_body);

    // A different key runs the handler again.
    let other = post(&client, &base, Some("order-2")).await;
    assert_eq!(other.status(), 200);
    assert_eq!(other.json::<CounterReply>().await.expect("json").count, 2);

    // No key means no idempotency.
    let plain = post(&client, &base, None).await;
    assert_eq!(plain.status(), 200);
    assert_eq!(plain.json::<CounterReply>().await.expect("json").count, 3);

    // Only state-changing methods are affected.
    let get = reqwest::Client::new()
        .get(format!("{base}/health/live"))
        .header(IDEMPOTENCY_KEY_HEADER, "order-1")
        .send()
        .await
        .expect("get");
    assert_eq!(get.status(), 200);

    handle.shutdown();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_concurrent_duplicate_gets_409_and_then_replays() {
    let _serial = TEST_LOCK.lock().await;
    EXECUTIONS.store(0, Ordering::SeqCst);
    SLOW.store(true, Ordering::SeqCst);
    ENTERED.store(false, Ordering::SeqCst);
    let port = free_port();
    let handle = spawn_host(port).await;
    let base = format!("http://127.0.0.1:{port}");

    let first = {
        let base = base.clone();
        tokio::spawn(async move {
            reqwest::Client::new()
                .post(format!("{base}/api/counter"))
                .header(IDEMPOTENCY_KEY_HEADER, "job-1")
                .send()
                .await
                .expect("first request")
        })
    };
    // Wait until the handler is running (the key is reserved).
    let deadline = Instant::now() + Duration::from_secs(5);
    while !ENTERED.load(Ordering::SeqCst) {
        assert!(Instant::now() < deadline, "the first request never started");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    let duplicate = reqwest::Client::new()
        .post(format!("{base}/api/counter"))
        .header(IDEMPOTENCY_KEY_HEADER, "job-1")
        .send()
        .await
        .expect("duplicate");
    assert_eq!(duplicate.status(), 409);
    assert_eq!(duplicate.headers().get("retry-after").expect("header"), "1");

    RELEASE.notify_waiters();
    let completed = first.await.expect("join");
    assert_eq!(completed.status(), 200);
    let completed_body: CounterReply = completed.json().await.expect("json");
    assert_eq!(completed_body.count, 1);

    // After completion the retry replays instead of re-running.
    let retry = reqwest::Client::new()
        .post(format!("{base}/api/counter"))
        .header(IDEMPOTENCY_KEY_HEADER, "job-1")
        .send()
        .await
        .expect("retry");
    assert_eq!(
        retry.headers().get("idempotency-replayed").expect("header"),
        "true"
    );
    assert_eq!(retry.json::<CounterReply>().await.expect("json").count, 1);

    SLOW.store(false, Ordering::SeqCst);
    handle.shutdown();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_invalid_key_is_a_400_problem() {
    let port = free_port();
    let handle = spawn_host(port).await;

    let response = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{port}/api/counter"))
        .header(IDEMPOTENCY_KEY_HEADER, "has space")
        .send()
        .await
        .expect("request");
    assert_eq!(response.status(), 400);
    assert_eq!(
        response.headers().get("content-type").expect("ct"),
        "application/problem+json"
    );

    handle.shutdown();
}
