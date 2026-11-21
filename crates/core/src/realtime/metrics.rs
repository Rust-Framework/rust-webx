//! Process-wide real-time metrics (2.3-6).
//!
//! Sessions and connections push into this shared accumulator (cheap atomics on
//! the hot path); the host's `/metrics` endpoint renders a snapshot. The
//! per-connection [`ConnectionStats`](super::ConnectionStats) remains available
//! to handlers; these counters survive after a connection is gone.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use crate::metrics::IMeter;

/// Dispatch latency histogram bounds, in seconds (µs-scale WebSocket dispatch).
const DISPATCH_BUCKETS_SECS: [f64; 13] = [
    0.0001, 0.00025, 0.0005, 0.001, 0.0025, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0,
];

/// Shared real-time counters.
#[derive(Debug, Default)]
pub struct RealtimeMetrics {
    connections_opened: AtomicU64,
    connections_closed: AtomicU64,
    messages_in: AtomicU64,
    messages_out: AtomicU64,
    bytes_in: AtomicU64,
    bytes_out: AtomicU64,
    dropped_outbound: AtomicU64,
    rate_limited: AtomicU64,
    dispatch_count: AtomicU64,
    dispatch_sum_micros: AtomicU64,
    dispatch_buckets: [AtomicU64; DISPATCH_BUCKETS_SECS.len()],
    closes: Mutex<BTreeMap<u16, u64>>,
    /// Values already mirrored into the app's `IMeter`. `/metrics` scrapes can
    /// overlap or repeat, so the bridge exports **deltas**; otherwise a counter
    /// would be double-counted on every scrape.
    export_cursor: Mutex<[u64; 8]>,
}

/// Read-only snapshot of [`RealtimeMetrics`] for rendering/tests.
#[derive(Debug, Clone)]
pub struct RealtimeMetricsSnapshot {
    /// Sessions opened.
    pub connections_opened: u64,
    /// Sessions closed.
    pub connections_closed: u64,
    /// Inbound messages delivered to handlers.
    pub messages_in: u64,
    /// Outbound messages queued.
    pub messages_out: u64,
    /// Inbound payload bytes.
    pub bytes_in: u64,
    /// Outbound payload bytes.
    pub bytes_out: u64,
    /// Outbound frames dropped because the per-connection queue was full.
    pub dropped_outbound: u64,
    /// Inbound rate-limit violations.
    pub rate_limited: u64,
    /// Message dispatches measured (message-as-request endpoints).
    pub dispatch_count: u64,
    /// Total dispatch time, microseconds.
    pub dispatch_sum_micros: u64,
    /// Cumulative dispatch counts per histogram bucket (µs-scale).
    pub dispatch_buckets: Vec<(f64, u64)>,
    /// Close code -> count.
    pub closes: Vec<(u16, u64)>,
}

impl RealtimeMetrics {
    /// Mirror the **new** counters into the framework metrics port ([`IMeter`]).
    ///
    /// The Prometheus endpoint renders the hot-path collector directly (no
    /// per-event metric lookups on the connection path). Applications that
    /// export through `IMeter` (OpenTelemetry, a custom registry) call this on a
    /// scrape/tick, so real-time health shows up next to HTTP, media and
    /// business metrics without a second metrics model.
    ///
    /// Only the delta since the previous export is added: `/metrics` scrapes may
    /// repeat or overlap, and `IMeter` counters are monotonic. The target meter
    /// is expected to stay stable for the life of the process (replacing it does
    /// not replay history; the cursor continues).
    ///
    /// The dispatch-latency histogram and the close-code breakdown stay
    /// Prometheus-only: `IMeter` has no cumulative-bucket representation and its
    /// tag values are borrowed `&str` (a dynamic close code cannot be a tag).
    pub fn export_to(&self, meter: &dyn IMeter) {
        let values = [
            self.connections_opened.load(Ordering::Relaxed),
            self.connections_closed.load(Ordering::Relaxed),
            self.messages_in.load(Ordering::Relaxed),
            self.messages_out.load(Ordering::Relaxed),
            self.bytes_in.load(Ordering::Relaxed),
            self.bytes_out.load(Ordering::Relaxed),
            self.dropped_outbound.load(Ordering::Relaxed),
            self.rate_limited.load(Ordering::Relaxed),
        ];
        let deltas = {
            let mut cursor = self
                .export_cursor
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let mut deltas = [0u64; 8];
            for (index, value) in values.iter().enumerate() {
                deltas[index] = value.saturating_sub(cursor[index]);
                cursor[index] = *value;
            }
            deltas
        };
        const NAMES: [&str; 8] = [
            "realtime_connections_opened_total",
            "realtime_connections_closed_total",
            "realtime_messages_in_total",
            "realtime_messages_out_total",
            "realtime_message_bytes_in_total",
            "realtime_message_bytes_out_total",
            "realtime_messages_dropped_total",
            "realtime_rate_limited_total",
        ];
        let tags: &[(&str, &str)] = &[];
        for (name, delta) in NAMES.iter().zip(deltas) {
            if delta > 0 {
                meter.counter(name, tags).add(delta);
            }
        }
    }

    /// Empty accumulator.
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a session start.
    pub fn record_connection_opened(&self) {
        self.connections_opened.fetch_add(1, Ordering::Relaxed);
    }

    /// Record a session end.
    pub fn record_connection_closed(&self) {
        self.connections_closed.fetch_add(1, Ordering::Relaxed);
    }

    /// Record one inbound message.
    pub fn record_message_in(&self, bytes: u64) {
        self.messages_in.fetch_add(1, Ordering::Relaxed);
        self.bytes_in.fetch_add(bytes, Ordering::Relaxed);
    }

    /// Record one outbound message.
    pub fn record_message_out(&self, bytes: u64) {
        self.messages_out.fetch_add(1, Ordering::Relaxed);
        self.bytes_out.fetch_add(bytes, Ordering::Relaxed);
    }

    /// Record one outbound frame dropped under backpressure.
    pub fn record_dropped_outbound(&self) {
        self.dropped_outbound.fetch_add(1, Ordering::Relaxed);
    }

    /// Record one inbound rate-limit violation.
    pub fn record_rate_limited(&self) {
        self.rate_limited.fetch_add(1, Ordering::Relaxed);
    }

    /// Record the close code of a finished session.
    pub fn record_close(&self, code: u16) {
        let mut closes = self
            .closes
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *closes.entry(code).or_insert(0) += 1;
    }

    /// Record the dispatch latency of one real-time request.
    pub fn record_dispatch_latency(&self, elapsed: std::time::Duration) {
        let micros = u64::try_from(elapsed.as_micros()).unwrap_or(u64::MAX);
        self.dispatch_count.fetch_add(1, Ordering::Relaxed);
        self.dispatch_sum_micros
            .fetch_add(micros, Ordering::Relaxed);
        let secs = elapsed.as_secs_f64();
        for (index, bound) in DISPATCH_BUCKETS_SECS.iter().enumerate() {
            if secs <= *bound {
                self.dispatch_buckets[index].fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    /// Snapshot every counter for rendering.
    pub fn snapshot(&self) -> RealtimeMetricsSnapshot {
        let buckets = DISPATCH_BUCKETS_SECS
            .iter()
            .enumerate()
            .map(|(index, bound)| (*bound, self.dispatch_buckets[index].load(Ordering::Relaxed)))
            .collect();
        let closes = self
            .closes
            .lock()
            .map(|closes| closes.iter().map(|(code, count)| (*code, *count)).collect())
            .unwrap_or_default();
        RealtimeMetricsSnapshot {
            connections_opened: self.connections_opened.load(Ordering::Relaxed),
            connections_closed: self.connections_closed.load(Ordering::Relaxed),
            messages_in: self.messages_in.load(Ordering::Relaxed),
            messages_out: self.messages_out.load(Ordering::Relaxed),
            bytes_in: self.bytes_in.load(Ordering::Relaxed),
            bytes_out: self.bytes_out.load(Ordering::Relaxed),
            dropped_outbound: self.dropped_outbound.load(Ordering::Relaxed),
            rate_limited: self.rate_limited.load(Ordering::Relaxed),
            dispatch_count: self.dispatch_count.load(Ordering::Relaxed),
            dispatch_sum_micros: self.dispatch_sum_micros.load(Ordering::Relaxed),
            dispatch_buckets: buckets,
            closes,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn counters_and_histogram_accumulate() {
        let metrics = RealtimeMetrics::new();
        metrics.record_connection_opened();
        metrics.record_message_in(10);
        metrics.record_message_out(4);
        metrics.record_message_out(6);
        metrics.record_dropped_outbound();
        metrics.record_rate_limited();
        metrics.record_close(1000);
        metrics.record_close(1008);
        metrics.record_close(1008);
        metrics.record_dispatch_latency(Duration::from_micros(300));

        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.connections_opened, 1);
        assert_eq!(snapshot.messages_in, 1);
        assert_eq!(snapshot.messages_out, 2);
        assert_eq!(snapshot.bytes_in, 10);
        assert_eq!(snapshot.bytes_out, 10);
        assert_eq!(snapshot.dropped_outbound, 1);
        assert_eq!(snapshot.rate_limited, 1);
        assert_eq!(snapshot.dispatch_count, 1);
        assert_eq!(snapshot.dispatch_sum_micros, 300);
        assert_eq!(snapshot.closes, vec![(1000, 1), (1008, 2)]);
        // 300µs falls into the 0.0005s bucket and every larger one.
        let at = |bound: f64| {
            snapshot
                .dispatch_buckets
                .iter()
                .find(|(b, _)| *b == bound)
                .map(|(_, count)| *count)
                .expect("bucket")
        };
        assert_eq!(at(0.0005), 1);
        assert_eq!(at(0.00025), 0);
        assert_eq!(at(1.0), 1);
    }

    #[test]
    fn snapshot_exports_deltas_to_the_framework_meter() {
        use crate::metrics::{IMeter, MemoryMeter};

        let metrics = RealtimeMetrics::new();
        metrics.record_connection_opened();
        metrics.record_message_in(10);
        metrics.record_message_out(20);
        metrics.record_dropped_outbound();
        metrics.record_rate_limited();
        metrics.record_connection_closed();

        let meter = MemoryMeter::new();
        let tags: &[(&str, &str)] = &[];
        metrics.export_to(&meter);
        assert_eq!(
            meter.counter_value("realtime_connections_opened_total", tags),
            1
        );
        assert_eq!(
            meter.counter_value("realtime_connections_closed_total", tags),
            1
        );
        assert_eq!(meter.counter_value("realtime_messages_in_total", tags), 1);
        assert_eq!(meter.counter_value("realtime_messages_out_total", tags), 1);
        assert_eq!(
            meter.counter_value("realtime_message_bytes_in_total", tags),
            10
        );
        assert_eq!(
            meter.counter_value("realtime_message_bytes_out_total", tags),
            20
        );
        assert_eq!(
            meter.counter_value("realtime_messages_dropped_total", tags),
            1
        );
        assert_eq!(meter.counter_value("realtime_rate_limited_total", tags), 1);

        // A repeated scrape must not double-count: only new increments are added.
        metrics.export_to(&meter);
        assert_eq!(meter.counter_value("realtime_messages_in_total", tags), 1);

        metrics.record_message_in(5);
        metrics.export_to(&meter);
        assert_eq!(meter.counter_value("realtime_messages_in_total", tags), 2);
        assert_eq!(
            meter.counter_value("realtime_message_bytes_in_total", tags),
            15
        );

        let _: &dyn IMeter = &meter;
    }
}
