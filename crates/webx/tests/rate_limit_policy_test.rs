//! Rate-limit policy: per-route/per-subject budgets and the explicit
//! `IRateLimiter` entry, with the single 429 contract (`Retry-After`).

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use webx::*;

#[derive(Debug, Default, WebxRequestMeta)]
struct BusyRequest {}

#[get("/api/busy")]
impl IRequest<RateLimitOk> for BusyRequest {}

#[derive(Debug, Default, WebxRequestMeta)]
struct FreeRequest {}

#[get("/api/free")]
impl IRequest<RateLimitOk> for FreeRequest {}

#[derive(Debug, Serialize, Deserialize)]
struct RateLimitOk {
    ok: bool,
}

#[derive(Default)]
struct OkHandler;

#[handler]
#[async_trait::async_trait]
impl IRequestHandler<BusyRequest, RateLimitOk> for OkHandler {
    async fn handle(&mut self, _req: BusyRequest) -> ActionResult<RateLimitOk> {
        Ok(RateLimitOk { ok: true })
    }
}

#[derive(Default)]
struct FreeHandler;

#[handler]
#[async_trait::async_trait]
impl IRequestHandler<FreeRequest, RateLimitOk> for FreeHandler {
    async fn handle(&mut self, _req: FreeRequest) -> ActionResult<RateLimitOk> {
        Ok(RateLimitOk { ok: true })
    }
}

fn free_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("free port");
    listener.local_addr().expect("addr").port()
}

async fn spawn_host(
    port: u16,
    policy: Option<RateLimitPolicy>,
    limiter: Option<Arc<dyn IRateLimiter>>,
) -> ServerHandle {
    let address = format!("127.0.0.1:{port}");
    let mut builder = Host::builder().mode(AppMode::Development).no_spa();
    if let Some(policy) = policy {
        builder = builder.use_rate_limit_policy(policy);
    }
    if let Some(limiter) = limiter {
        builder = builder.use_rate_limiter(limiter);
    }
    let host = builder.build();
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_policy_limits_per_api_key_and_leaves_unmatched_paths_alone() {
    let port = free_port();
    let policy = RateLimitPolicy::new().with_rule(RateLimitRule::new(
        RateLimitRoute::prefix("/api/busy"),
        RateLimitSubject::ApiKey,
        // No refill: burst 1 means each key gets exactly one request.
        Arc::new(InMemoryRateLimiter::new(0.0, 1, 100)),
    ));
    let handle = spawn_host(port, Some(policy), None).await;
    let client = reqwest::Client::new();
    let base = format!("http://127.0.0.1:{port}");

    // The unmatched path is not rate limited by the policy.
    assert_eq!(
        client
            .get(format!("{base}/api/free"))
            .send()
            .await
            .expect("free")
            .status(),
        200
    );

    let busy = |key: &str| {
        client
            .get(format!("{base}/api/busy"))
            .header("x-api-key", key)
            .send()
    };

    assert_eq!(busy("key-a").await.expect("first").status(), 200);
    let limited = busy("key-a").await.expect("second");
    assert_eq!(limited.status(), 429);
    assert!(limited.headers().get("retry-after").is_some());
    // Another credential has its own budget.
    assert_eq!(busy("key-b").await.expect("other key").status(), 200);

    handle.shutdown();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_explicit_limiter_drives_the_pre_auth_middleware() {
    struct DenyAll;

    #[async_trait::async_trait]
    impl IRateLimiter for DenyAll {
        async fn check(&self, _key: &str) -> RateLimitDecision {
            RateLimitDecision::deny(3)
        }
    }

    let port = free_port();
    let handle = spawn_host(port, None, Some(Arc::new(DenyAll))).await;

    let response = reqwest::get(format!("http://127.0.0.1:{port}/api/free"))
        .await
        .expect("response");
    assert_eq!(response.status(), 429);
    assert_eq!(response.headers().get("retry-after").expect("header"), "3");
    assert_eq!(
        response
            .headers()
            .get("content-type")
            .expect("content type"),
        "application/problem+json"
    );

    handle.shutdown();
}
