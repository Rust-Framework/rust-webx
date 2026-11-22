//! Business metrics bridge: `IMeter` series are rendered by `GET /metrics`.
//!
//! A metrics-enabled host registers a bounded `MemoryMeter` by default, so a
//! handler that resolves `Arc<dyn IMeter>` shows up in `/metrics` without extra
//! wiring; an application `use_meter` registration takes precedence.

#![allow(non_snake_case)]

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use webx::*;

#[derive(Debug, Default, WebxRequestMeta)]
struct TickRequest {}

#[get("/api/tick")]
impl IRequest<TickReply> for TickRequest {}

#[derive(Debug, Serialize, Deserialize)]
struct TickReply {
    ok: bool,
}

#[derive(Inject)]
struct TickHandler {
    #[inject]
    meter: Arc<dyn IMeter>,
}

#[handler(inject)]
#[async_trait::async_trait]
impl IRequestHandler<TickRequest, TickReply> for TickHandler {
    async fn handle(&mut self, _req: TickRequest) -> ActionResult<TickReply> {
        self.meter
            .counter("app_ticks_total", &[("route", "tick")])
            .inc();
        self.meter.histogram("app_tick_seconds", &[]).record(0.25);
        Ok(TickReply { ok: true })
    }
}

fn free_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("free port");
    listener.local_addr().expect("addr").port()
}

fn metrics_enabled_builder() -> HostBuilder {
    Host::builder()
        .mode(AppMode::Development)
        .no_spa()
        .configure(|app| {
            app.useOptions(|options| {
                options.metrics.enabled = true;
            });
        })
}

async fn spawn_host(port: u16) -> ServerHandle {
    let address = format!("127.0.0.1:{port}");
    let host = metrics_enabled_builder().build();
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
async fn business_metrics_are_exported_at_metrics() {
    let port = free_port();
    let handle = spawn_host(port).await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();

    let reply = client
        .get(format!("{base}/api/tick"))
        .send()
        .await
        .expect("tick");
    assert_eq!(reply.status(), 200);

    let metrics = client
        .get(format!("{base}/metrics"))
        .send()
        .await
        .expect("metrics");
    assert_eq!(metrics.status(), 200);
    let text = metrics.text().await.expect("body");
    assert!(
        text.contains("# TYPE app_ticks_total counter"),
        "counter type missing: {text}"
    );
    assert!(
        text.contains("app_ticks_total{route=\"tick\"} 1"),
        "counter series missing: {text}"
    );
    assert!(
        text.contains("# TYPE app_tick_seconds summary"),
        "histogram type missing: {text}"
    );
    assert!(
        text.contains("app_tick_seconds_count 1"),
        "histogram count missing: {text}"
    );
    assert!(
        text.contains("app_tick_seconds_sum 0.25"),
        "histogram sum missing: {text}"
    );

    handle.shutdown();
}

#[tokio::test]
async fn metrics_enabled_registers_a_default_meter() {
    let host = metrics_enabled_builder().build();
    let meter = host
        .provider()
        .get::<dyn IMeter>()
        .expect("a default IMeter is registered when metrics are enabled");
    assert!(
        meter.snapshot().is_empty(),
        "the default meter starts with no series"
    );
}
