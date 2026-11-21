//! HTTP request metrics exposed at `GET /metrics` (Prometheus text format).
//!
//! Two layers:
//!
//! * **Process-wide counters** from 2.1 — `http_requests_total` and the
//!   status-class totals, kept unchanged for existing dashboards.
//! * **Per-route RED signals** — request counts by status class and a
//!   fixed-bucket latency histogram, labelled with the matched route pattern
//!   (`unmatched` for 404/405). The label set is bounded so a hostile client
//!   cannot grow the map by requesting random paths.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use webx_core::error::Result;
use webx_core::http::IHttpContext;
use webx_core::middleware::IMiddleware;
use webx_core::routing::IEndpoint;

/// Upper bound on distinct route labels kept in memory.
const MAX_ROUTE_LABELS: usize = 512;

/// Histogram upper bounds in microseconds; values above the last one land in
/// the `+Inf` bucket.
const LATENCY_BUCKETS_US: [u64; 10] = [
    50, 100, 250, 500, 1_000, 2_500, 5_000, 10_000, 30_000, 60_000,
];

/// Per-route counters and latency histogram.
#[derive(Debug)]
struct RouteMetrics {
    requests: AtomicU64,
    status_2xx: AtomicU64,
    status_4xx: AtomicU64,
    status_5xx: AtomicU64,
    /// Requests that carried a measurable duration.
    latency_count: AtomicU64,
    latency_sum_us: AtomicU64,
    /// Non-cumulative per-bucket counts, rendered cumulatively.
    buckets: [AtomicU64; LATENCY_BUCKETS_US.len()],
    /// Values above the last explicit bucket (`+Inf`).
    overflow: AtomicU64,
}

impl RouteMetrics {
    fn new() -> Self {
        Self {
            requests: AtomicU64::new(0),
            status_2xx: AtomicU64::new(0),
            status_4xx: AtomicU64::new(0),
            status_5xx: AtomicU64::new(0),
            latency_count: AtomicU64::new(0),
            latency_sum_us: AtomicU64::new(0),
            buckets: std::array::from_fn(|_| AtomicU64::new(0)),
            overflow: AtomicU64::new(0),
        }
    }

    fn record(&self, status: u16, elapsed: Option<Duration>) {
        self.requests.fetch_add(1, Ordering::Relaxed);
        match status {
            200..=299 => {
                self.status_2xx.fetch_add(1, Ordering::Relaxed);
            }
            400..=499 => {
                self.status_4xx.fetch_add(1, Ordering::Relaxed);
            }
            500..=599 => {
                self.status_5xx.fetch_add(1, Ordering::Relaxed);
            }
            _ => {}
        }
        if let Some(elapsed) = elapsed {
            let micros = elapsed.as_micros().min(u128::from(u64::MAX)) as u64;
            self.latency_count.fetch_add(1, Ordering::Relaxed);
            self.latency_sum_us.fetch_add(micros, Ordering::Relaxed);
            match LATENCY_BUCKETS_US.iter().position(|limit| micros <= *limit) {
                Some(index) => {
                    self.buckets[index].fetch_add(1, Ordering::Relaxed);
                }
                None => {
                    self.overflow.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
    }
}

/// Shared HTTP metrics.
#[derive(Default)]
pub struct HttpMetrics {
    total: AtomicU64,
    status_2xx: AtomicU64,
    status_4xx: AtomicU64,
    status_5xx: AtomicU64,
    /// Requests currently inside the pipeline (including the `/metrics` scrape).
    inflight: AtomicU64,
    /// Responses with status 504 (the engine's inbound-deadline signal).
    timeouts: AtomicU64,
    routes: RwLock<HashMap<Arc<str>, RouteMetrics>>,
    /// Requests whose route label was dropped by [`MAX_ROUTE_LABELS`].
    route_label_overflow: AtomicU64,
}

impl HttpMetrics {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Record a status without a route label (2.1-compatible API).
    pub fn record(&self, status: u16) {
        self.record_status(status);
    }

    fn record_status(&self, status: u16) {
        self.total.fetch_add(1, Ordering::Relaxed);
        match status {
            200..=299 => {
                self.status_2xx.fetch_add(1, Ordering::Relaxed);
            }
            400..=499 => {
                self.status_4xx.fetch_add(1, Ordering::Relaxed);
            }
            500..=599 => {
                self.status_5xx.fetch_add(1, Ordering::Relaxed);
            }
            _ => {}
        }
        if status == 504 {
            self.timeouts.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Record one completed request for `route`.
    ///
    /// `elapsed` is `None` for contexts that do not expose a start timestamp
    /// (test fakes); the request is counted but not added to the histogram.
    pub fn record_request(&self, route: &str, status: u16, elapsed: Option<Duration>) {
        self.record_status(status);
        {
            let read = self
                .routes
                .read()
                .unwrap_or_else(|poison| poison.into_inner());
            if let Some(metrics) = read.get(route) {
                metrics.record(status, elapsed);
                return;
            }
        }
        let mut write = self
            .routes
            .write()
            .unwrap_or_else(|poison| poison.into_inner());
        if write.len() >= MAX_ROUTE_LABELS {
            self.route_label_overflow.fetch_add(1, Ordering::Relaxed);
            return;
        }
        write
            .entry(Arc::from(route))
            .or_insert_with(RouteMetrics::new)
            .record(status, elapsed);
    }

    /// Mark one request as in flight.
    pub fn begin_request(&self) {
        self.inflight.fetch_add(1, Ordering::Relaxed);
    }

    /// Mark one request as finished.
    pub fn end_request(&self) {
        self.inflight.fetch_sub(1, Ordering::Relaxed);
    }

    /// Current number of in-flight requests.
    pub fn inflight(&self) -> u64 {
        self.inflight.load(Ordering::Relaxed)
    }

    pub fn render_prometheus(&self) -> String {
        let total = self.total.load(Ordering::Relaxed);
        let s2 = self.status_2xx.load(Ordering::Relaxed);
        let s4 = self.status_4xx.load(Ordering::Relaxed);
        let s5 = self.status_5xx.load(Ordering::Relaxed);
        let inflight = self.inflight.load(Ordering::Relaxed);
        let timeouts = self.timeouts.load(Ordering::Relaxed);
        let overflow = self.route_label_overflow.load(Ordering::Relaxed);

        let mut out = String::with_capacity(4096);
        out.push_str("# HELP http_requests_total Total HTTP requests processed\n");
        out.push_str("# TYPE http_requests_total counter\n");
        out.push_str(&format!("http_requests_total {total}\n"));
        out.push_str("# HELP http_responses_2xx_total 2xx responses\n");
        out.push_str("# TYPE http_responses_2xx_total counter\n");
        out.push_str(&format!("http_responses_2xx_total {s2}\n"));
        out.push_str("# HELP http_responses_4xx_total 4xx responses\n");
        out.push_str("# TYPE http_responses_4xx_total counter\n");
        out.push_str(&format!("http_responses_4xx_total {s4}\n"));
        out.push_str("# HELP http_responses_5xx_total 5xx responses\n");
        out.push_str("# TYPE http_responses_5xx_total counter\n");
        out.push_str(&format!("http_responses_5xx_total {s5}\n"));
        out.push_str("# HELP http_requests_inflight Requests currently inside the pipeline\n");
        out.push_str("# TYPE http_requests_inflight gauge\n");
        out.push_str(&format!("http_requests_inflight {inflight}\n"));
        out.push_str(
            "# HELP http_request_timeouts_total 504 responses (inbound deadline or gateway timeout)\n",
        );
        out.push_str("# TYPE http_request_timeouts_total counter\n");
        out.push_str(&format!("http_request_timeouts_total {timeouts}\n"));
        if overflow > 0 {
            out.push_str(
                "# HELP http_route_label_overflow_total Requests dropped from route labels\n",
            );
            out.push_str("# TYPE http_route_label_overflow_total counter\n");
            out.push_str(&format!("http_route_label_overflow_total {overflow}\n"));
        }

        let read = self
            .routes
            .read()
            .unwrap_or_else(|poison| poison.into_inner());
        if read.is_empty() {
            return out;
        }
        let mut routes: Vec<(&Arc<str>, &RouteMetrics)> = read.iter().collect();
        routes.sort_by(|left, right| left.0.cmp(right.0));

        out.push_str("# HELP http_route_requests_total Requests by matched route pattern\n");
        out.push_str("# TYPE http_route_requests_total counter\n");
        for (route, metrics) in &routes {
            let label = escape_label(route);
            for (class, counter) in [
                ("2xx", &metrics.status_2xx),
                ("4xx", &metrics.status_4xx),
                ("5xx", &metrics.status_5xx),
            ] {
                out.push_str(&format!(
                    "http_route_requests_total{{route=\"{label}\",status=\"{class}\"}} {}\n",
                    counter.load(Ordering::Relaxed)
                ));
            }
        }

        out.push_str("# HELP http_request_duration_seconds Request latency by matched route\n");
        out.push_str("# TYPE http_request_duration_seconds histogram\n");
        for (route, metrics) in &routes {
            let label = escape_label(route);
            let mut cumulative = 0u64;
            for (index, limit) in LATENCY_BUCKETS_US.iter().enumerate() {
                cumulative += metrics.buckets[index].load(Ordering::Relaxed);
                out.push_str(&format!(
                    "http_request_duration_seconds_bucket{{route=\"{label}\",le=\"{}\"}} {cumulative}\n",
                    format_seconds(*limit)
                ));
            }
            cumulative += metrics.overflow.load(Ordering::Relaxed);
            out.push_str(&format!(
                "http_request_duration_seconds_bucket{{route=\"{label}\",le=\"+Inf\"}} {cumulative}\n"
            ));
            let sum_us = metrics.latency_sum_us.load(Ordering::Relaxed) as f64 / 1_000_000.0;
            out.push_str(&format!(
                "http_request_duration_seconds_sum{{route=\"{label}\"}} {sum_us}\n"
            ));
            out.push_str(&format!(
                "http_request_duration_seconds_count{{route=\"{label}\"}} {}\n",
                metrics.latency_count.load(Ordering::Relaxed)
            ));
        }
        out
    }
}

fn format_seconds(micros: u64) -> String {
    let seconds = micros as f64 / 1_000_000.0;
    let mut text = format!("{seconds}");
    if !text.contains('.') {
        text.push_str(".0");
    }
    text
}

fn escape_label(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '\\' => escaped.push_str("\\\\"),
            '"' => escaped.push_str("\\\""),
            '\n' => escaped.push_str("\\n"),
            other => escaped.push(other),
        }
    }
    escaped
}

/// Records request counts, route labels and latency into [`HttpMetrics`].
pub struct MetricsMiddleware {
    metrics: Arc<HttpMetrics>,
}

impl MetricsMiddleware {
    pub fn new(metrics: Arc<HttpMetrics>) -> Self {
        Self { metrics }
    }
}

#[async_trait::async_trait]
impl IMiddleware for MetricsMiddleware {
    async fn invoke(&self, _ctx: &mut dyn IHttpContext) -> Result<std::ops::ControlFlow<()>> {
        self.metrics.begin_request();
        Ok(std::ops::ControlFlow::Continue(()))
    }

    async fn after(&self, ctx: &mut dyn IHttpContext) -> Result<()> {
        let status = ctx.response().status();
        let route = ctx.request().route_pattern().unwrap_or("unmatched");
        self.metrics.record_request(route, status, ctx.elapsed());
        self.metrics.end_request();
        Ok(())
    }
}

/// Serves Prometheus text metrics from a shared [`HttpMetrics`] instance.
pub struct MetricsEndpoint {
    metrics: Arc<HttpMetrics>,
    admission: Option<Arc<webx_engine::shared::AdmissionStats>>,
    /// Optional app meter: the real-time snapshot is mirrored into it on scrape
    /// so `IMeter`-based exporters (OpenTelemetry) see the same counters.
    meter: Option<Arc<dyn webx_core::metrics::IMeter>>,
    realtime: Option<(
        Arc<webx_core::realtime::RealtimeMetrics>,
        Arc<dyn webx_core::realtime::IConnectionRegistry>,
    )>,
    events: Option<Arc<webx_core::mediator::EventMetrics>>,
}

impl MetricsEndpoint {
    pub fn new(metrics: Arc<HttpMetrics>) -> Self {
        Self {
            metrics,
            admission: None,
            meter: None,
            realtime: None,
            events: None,
        }
    }

    /// Also render the engine's connection-admission counters.
    pub fn with_admission(mut self, admission: Arc<webx_engine::shared::AdmissionStats>) -> Self {
        self.admission = Some(admission);
        self
    }

    /// Mirror the real-time snapshot into the app's
    /// [`IMeter`](webx_core::metrics::IMeter) on scrape.
    pub fn with_meter(mut self, meter: Arc<dyn webx_core::metrics::IMeter>) -> Self {
        self.meter = Some(meter);
        self
    }

    /// Also render the real-time (WebSocket) counters and the registry gauge.
    pub fn with_realtime(
        mut self,
        metrics: Arc<webx_core::realtime::RealtimeMetrics>,
        registry: Arc<dyn webx_core::realtime::IConnectionRegistry>,
    ) -> Self {
        self.realtime = Some((metrics, registry));
        self
    }

    /// Also render the event publish/dispatch counters.
    pub fn with_events(mut self, metrics: Arc<webx_core::mediator::EventMetrics>) -> Self {
        self.events = Some(metrics);
        self
    }
}

// The endpoint is a read-only snapshot renderer: the registry and counters are
// internally synchronized, so a panic mid-scrape cannot leave an observable
// broken invariant. Keep the 2.1 auto-trait surface (removing an auto trait is
// a semver-major change).
impl std::panic::UnwindSafe for MetricsEndpoint {}
impl std::panic::RefUnwindSafe for MetricsEndpoint {}

#[async_trait::async_trait]
impl IEndpoint for MetricsEndpoint {
    async fn handle(&self, ctx: &mut dyn IHttpContext) -> Result<()> {
        let mut body = self.metrics.render_prometheus();
        if let Some(admission) = &self.admission {
            body.push_str(&render_admission(admission));
        }
        if let Some((realtime, registry)) = &self.realtime {
            body.push_str(&render_realtime(realtime, registry.as_ref()));
            if let Some(meter) = &self.meter {
                realtime.export_to(meter.as_ref());
            }
        }
        if let Some(events) = &self.events {
            body.push_str(&render_events(events));
        }
        ctx.response_mut().set_status(200);
        ctx.response_mut()
            .set_header("content-type", "text/plain; version=0.0.4; charset=utf-8");
        ctx.response_mut().write_bytes(body.into_bytes()).await
    }
}

/// Prometheus text for the event transport.
fn render_events(metrics: &webx_core::mediator::EventMetrics) -> String {
    use std::fmt::Write as _;

    let snapshot = metrics.snapshot();
    let mut out = String::with_capacity(512);
    let _ = writeln!(
        out,
        "# HELP events_published_total Events accepted by the transport\n\
         # TYPE events_published_total counter\n\
         events_published_total {}",
        snapshot.published
    );
    let _ = writeln!(
        out,
        "# HELP events_publish_failed_total Events rejected by encoding or the transport\n\
         # TYPE events_publish_failed_total counter\n\
         events_publish_failed_total {}",
        snapshot.publish_failed
    );
    let _ = writeln!(
        out,
        "# HELP events_dispatch_failed_total Dispatches with at least one failed handler\n\
         # TYPE events_dispatch_failed_total counter\n\
         events_dispatch_failed_total {}",
        snapshot.dispatch_failed
    );
    let _ = writeln!(
        out,
        "# HELP events_dropped_no_handler_total Events dropped because no handler was registered\n\
         # TYPE events_dropped_no_handler_total counter\n\
         events_dropped_no_handler_total {}",
        snapshot.dropped_no_handler
    );
    let _ = writeln!(
        out,
        "# HELP events_dead_lettered_total Events written to a dead-letter destination\n\
         # TYPE events_dead_lettered_total counter\n\
         events_dead_lettered_total {}",
        snapshot.dead_lettered
    );
    let _ = writeln!(
        out,
        "# HELP events_consumer_reconnects_total Broker consumer reconnect attempts\n\
         # TYPE events_consumer_reconnects_total counter\n\
         events_consumer_reconnects_total {}",
        snapshot.consumer_reconnects
    );
    out
}

/// Prometheus text for the real-time (WebSocket) subsystem.
fn render_realtime(
    metrics: &webx_core::realtime::RealtimeMetrics,
    registry: &dyn webx_core::realtime::IConnectionRegistry,
) -> String {
    use std::fmt::Write as _;

    let snapshot = metrics.snapshot();
    let mut out = String::with_capacity(2048);
    let _ = writeln!(
        out,
        "# HELP realtime_connections_active Live real-time connections tracked by the registry\n\
         # TYPE realtime_connections_active gauge\n\
         realtime_connections_active {}",
        registry.len()
    );
    let _ = writeln!(
        out,
        "# HELP realtime_connections_opened_total Real-time sessions opened\n\
         # TYPE realtime_connections_opened_total counter\n\
         realtime_connections_opened_total {}",
        snapshot.connections_opened
    );
    let _ = writeln!(
        out,
        "# HELP realtime_connections_closed_total Real-time sessions closed\n\
         # TYPE realtime_connections_closed_total counter\n\
         realtime_connections_closed_total {}",
        snapshot.connections_closed
    );
    let _ = writeln!(
        out,
        "# HELP realtime_messages_in_total Inbound messages delivered to handlers\n\
         # TYPE realtime_messages_in_total counter\n\
         realtime_messages_in_total {}",
        snapshot.messages_in
    );
    let _ = writeln!(
        out,
        "# HELP realtime_messages_out_total Outbound messages queued\n\
         # TYPE realtime_messages_out_total counter\n\
         realtime_messages_out_total {}",
        snapshot.messages_out
    );
    let _ = writeln!(
        out,
        "# HELP realtime_message_bytes_in_total Inbound payload bytes\n\
         # TYPE realtime_message_bytes_in_total counter\n\
         realtime_message_bytes_in_total {}",
        snapshot.bytes_in
    );
    let _ = writeln!(
        out,
        "# HELP realtime_message_bytes_out_total Outbound payload bytes\n\
         # TYPE realtime_message_bytes_out_total counter\n\
         realtime_message_bytes_out_total {}",
        snapshot.bytes_out
    );
    let _ = writeln!(
        out,
        "# HELP realtime_messages_dropped_total Outbound messages dropped by backpressure\n\
         # TYPE realtime_messages_dropped_total counter\n\
         realtime_messages_dropped_total {}",
        snapshot.dropped_outbound
    );
    let _ = writeln!(
        out,
        "# HELP realtime_rate_limited_total Inbound rate-limit violations\n\
         # TYPE realtime_rate_limited_total counter\n\
         realtime_rate_limited_total {}",
        snapshot.rate_limited
    );
    if !snapshot.closes.is_empty() {
        let _ = writeln!(
            out,
            "# HELP realtime_connection_closes_total Sessions closed by code\n\
             # TYPE realtime_connection_closes_total counter"
        );
        for (code, count) in &snapshot.closes {
            let _ = writeln!(
                out,
                "realtime_connection_closes_total{{code=\"{code}\"}} {count}"
            );
        }
    }
    if snapshot.dispatch_count > 0 {
        let _ = writeln!(
            out,
            "# HELP realtime_message_dispatch_seconds Message dispatch latency\n\
             # TYPE realtime_message_dispatch_seconds histogram"
        );
        for (bound, count) in &snapshot.dispatch_buckets {
            let _ = writeln!(
                out,
                "realtime_message_dispatch_seconds_bucket{{le=\"{bound}\"}} {count}"
            );
        }
        let _ = writeln!(
            out,
            "realtime_message_dispatch_seconds_bucket{{le=\"+Inf\"}} {}",
            snapshot.dispatch_count
        );
        let _ = writeln!(
            out,
            "realtime_message_dispatch_seconds_sum {}",
            snapshot.dispatch_sum_micros as f64 / 1_000_000.0
        );
        let _ = writeln!(
            out,
            "realtime_message_dispatch_seconds_count {}",
            snapshot.dispatch_count
        );
    }
    out
}

fn render_admission(stats: &webx_engine::shared::AdmissionStats) -> String {
    format!(
        "# HELP engine_connections_admitted_total Connections admitted by the global governor\n\
         # TYPE engine_connections_admitted_total counter\n\
         engine_connections_admitted_total {}\n\
         # HELP engine_connections_rejected_total Connections that waited past the admission deadline and got 503\n\
         # TYPE engine_connections_rejected_total counter\n\
         engine_connections_rejected_total {}\n\
         # HELP engine_admission_waiting Accepted sockets currently waiting for an admission slot\n\
         # TYPE engine_admission_waiting gauge\n\
         engine_admission_waiting {}\n\
         # HELP engine_connections_active Connections currently open (holding an admission permit)\n\
         # TYPE engine_connections_active gauge\n\
         engine_connections_active {}\n\
         # HELP engine_connections_capacity Configured admission capacity (App.MaxConnections)\n\
         # TYPE engine_connections_capacity gauge\n\
         engine_connections_capacity {}\n",
        stats.admitted(),
        stats.rejected(),
        stats.waiting(),
        stats.active(),
        stats.capacity()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_metrics_render_prometheus_counters() {
        let metrics = webx_core::mediator::EventMetrics::new();
        metrics.record_published();
        metrics.record_publish_failed();
        metrics.record_dispatch_failed();
        metrics.record_dropped_no_handler();
        metrics.record_dead_lettered();
        metrics.record_consumer_reconnect();
        let text = render_events(&metrics);
        assert!(text.contains("events_published_total 1"));
        assert!(text.contains("events_publish_failed_total 1"));
        assert!(text.contains("events_dispatch_failed_total 1"));
        assert!(text.contains("events_dropped_no_handler_total 1"));
        assert!(text.contains("events_dead_lettered_total 1"));
        assert!(text.contains("events_consumer_reconnects_total 1"));
    }

    #[test]
    fn route_metrics_render_labels_and_cumulative_buckets() {
        let metrics = HttpMetrics::default();
        metrics.begin_request();
        metrics.record_request("/hello", 200, Some(Duration::from_micros(120)));
        metrics.end_request();
        metrics.record_request("/hello", 500, Some(Duration::from_millis(2)));
        metrics.record_request("unmatched", 404, None);

        let text = metrics.render_prometheus();
        assert!(text.contains("http_route_requests_total{route=\"/hello\",status=\"2xx\"} 1"));
        assert!(text.contains("http_route_requests_total{route=\"/hello\",status=\"5xx\"} 1"));
        assert!(text.contains("http_route_requests_total{route=\"unmatched\",status=\"4xx\"} 1"));
        // 120µs falls into le=0.00025, 2ms into le=0.005; buckets are cumulative.
        assert!(text
            .contains("http_request_duration_seconds_bucket{route=\"/hello\",le=\"0.00025\"} 1"));
        assert!(
            text.contains("http_request_duration_seconds_bucket{route=\"/hello\",le=\"0.005\"} 2")
        );
        assert!(
            text.contains("http_request_duration_seconds_bucket{route=\"/hello\",le=\"+Inf\"} 2")
        );
        assert!(text.contains("http_request_duration_seconds_count{route=\"/hello\"} 2"));
        // The 404 had no elapsed time: counted, not histogrammed.
        assert!(text.contains("http_request_duration_seconds_count{route=\"unmatched\"} 0"));
        assert!(text.contains("http_requests_inflight 0"));
        assert!(text.contains("http_request_timeouts_total 0"));
    }

    #[test]
    fn deadline_timeouts_are_counted() {
        let metrics = HttpMetrics::default();
        metrics.record_request("/slow", 504, Some(Duration::from_secs(30)));
        let text = metrics.render_prometheus();
        assert!(text.contains("http_request_timeouts_total 1"));
        assert!(
            text.contains("http_request_duration_seconds_bucket{route=\"/slow\",le=\"+Inf\"} 1")
        );
    }

    #[test]
    fn route_labels_are_bounded() {
        let metrics = HttpMetrics::default();
        for index in 0..(MAX_ROUTE_LABELS + 10) {
            metrics.record_request(&format!("/route-{index}"), 200, None);
        }
        let text = metrics.render_prometheus();
        assert!(text.contains(&format!("http_route_label_overflow_total {}", 10)));
        let labelled = text.matches("http_route_requests_total{").count();
        assert_eq!(labelled, MAX_ROUTE_LABELS * 3);
    }

    #[test]
    fn label_values_are_escaped() {
        let metrics = HttpMetrics::default();
        metrics.record_request("/a\"b\\c", 200, None);
        let text = metrics.render_prometheus();
        assert!(text.contains("route=\"/a\\\"b\\\\c\""));
    }
}
