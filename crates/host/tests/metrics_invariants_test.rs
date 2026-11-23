//! Process-wide admission snapshots (`ServerStats`) and the internal
//! invariant self-check rendered by `GET /metrics`.
//!
//! The invariants are "engine-owned" (only the accept loop and the global
//! governor move these counters), so the abnormal case is injected through the
//! public engine surface: `ConnectionPermit` opens an active slot and
//! `set_capacity` provides the reference the governor should enforce.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Semaphore;
use webx_core::mode::AppMode;
use webx_engine::shared::{AdmissionStats, ConnectionPermit};
use webx_engine::ServerStats;
use webx_host::metrics::{HttpMetrics, MetricsEndpoint};
use webx_host::server::Host;

fn free_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("free port");
    listener.local_addr().expect("addr").port()
}

/// Take one admission slot so `active` reflects an open connection.
fn hold_permit(stats: &Arc<AdmissionStats>, governor: &Arc<Semaphore>) -> ConnectionPermit {
    ConnectionPermit::new(
        Arc::clone(governor)
            .try_acquire_owned()
            .expect("admission permit"),
        Arc::clone(stats),
    )
}

#[test]
fn snapshot_reads_one_sample_of_every_admission_counter() {
    let stats = Arc::new(AdmissionStats::default());
    assert_eq!(stats.snapshot(), ServerStats::default());

    stats.set_capacity(2);
    let governor = Arc::new(Semaphore::new(2));
    let first = hold_permit(&stats, &governor);
    let second = hold_permit(&stats, &governor);

    let sample = stats.snapshot();
    assert_eq!(sample.active, 2);
    assert_eq!(sample.capacity, 2);
    // admitted/rejected/active are engine-owned; this test only proves the
    // snapshot carries them through (the engine records them in the accept loop).
    assert_eq!(sample.admitted, 0);
    assert_eq!(sample.rejected, 0);
    assert_eq!(sample.waiting, 0);
    assert!(!sample.shutting_down);
    assert_eq!(sample.invariant_violations(), 0);

    drop(first);
    assert_eq!(stats.snapshot().active, 1, "the permit drives the gauge");
    drop(second);
    assert_eq!(stats.snapshot().active, 0);
}

#[test]
fn over_capacity_connection_is_reported_by_the_invariant_gauge() {
    let stats = Arc::new(AdmissionStats::default());
    stats.set_capacity(1);
    let governor = Arc::new(Semaphore::new(2));
    let first = hold_permit(&stats, &governor);
    let second = hold_permit(&stats, &governor);

    let sample = stats.snapshot();
    assert_eq!(sample.active, 2);
    assert_eq!(
        sample.invariant_violations(),
        1,
        "active above capacity is one violated invariant"
    );

    let endpoint = MetricsEndpoint::new(HttpMetrics::new()).with_admission(Arc::clone(&stats));
    let text = endpoint.render();
    assert!(
        text.contains("engine_admission_invariant_violations 1"),
        "the self-check must be visible on /metrics: {text}"
    );
    assert!(
        text.contains("engine_admission_invariant_violations_total 1"),
        "the cumulative observation counter starts at the first scrape: {text}"
    );
    assert!(
        text.contains("engine_admission_active_ratio 2"),
        "active / capacity is rendered against the configured cap: {text}"
    );
    // A repeated scrape re-renders the gauge, accumulates the bounded counter and
    // does not flood the log (its once-semantics are unit-tested in
    // `webx_host::metrics`).
    let repeated = endpoint.render();
    assert!(repeated.contains("engine_admission_invariant_violations 1"));
    assert!(repeated.contains("engine_admission_invariant_violations_total 2"));

    drop(first);
    drop(second);
}

#[test]
#[allow(clippy::field_reassign_with_default)] // `#[non_exhaustive]` blocks struct update syntax here
fn a_wait_left_over_after_shutdown_is_an_invariant_violation() {
    let mut sample = ServerStats::default();
    sample.waiting = 3;
    sample.shutting_down = true;
    assert_eq!(sample.invariant_violations(), 1);

    let mut recovered = ServerStats::default();
    recovered.shutting_down = true;
    assert_eq!(recovered.invariant_violations(), 0);

    // Capacity 0 means "not configured": the capacity invariant cannot be
    // evaluated, so the gauge must not report a false positive.
    let mut unknown_capacity = ServerStats::default();
    unknown_capacity.active = 5;
    assert_eq!(unknown_capacity.invariant_violations(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn healthy_metrics_scrape_reports_no_invariant_violations() {
    let port = free_port();
    let addr = format!("127.0.0.1:{port}");
    let host = Host::builder()
        .mode(AppMode::Development)
        .no_spa()
        .configure(|app| {
            app.useOptions(|options| {
                options.metrics.enabled = true;
                options.app.max_connections = 8;
            })
        })
        .build();
    let spawn_addr = addr.clone();
    tokio::spawn(async move {
        let _ = host.run_at(&spawn_addr).await;
    });
    // The host exposes no readiness future; follow the existing admission tests
    // and give the listener a fixed settle delay instead of polling.
    tokio::time::sleep(Duration::from_millis(300)).await;

    let body = reqwest::get(format!("http://{addr}/metrics"))
        .await
        .expect("metrics request")
        .text()
        .await
        .expect("metrics body");

    assert!(
        body.contains("engine_admission_invariant_violations 0"),
        "a healthy server must not report violations: {body}"
    );
    assert!(
        body.contains("engine_admission_invariant_violations_total 0"),
        "the observation counter stays 0 on a healthy scrape: {body}"
    );
    assert!(
        body.contains("engine_admission_shutting_down 0"),
        "the shutdown gauge is 0 while serving: {body}"
    );
    let ratio: f64 = body
        .lines()
        .find_map(|line| line.strip_prefix("engine_admission_active_ratio "))
        .expect("active ratio is rendered when a capacity is configured")
        .parse()
        .expect("ratio is a float");
    assert!(
        (0.0..=1.0).contains(&ratio),
        "the scrape connection itself is at most one slot of capacity 8: {ratio}"
    );
}
