//! Minimal metrics port: counters, histograms and gauges.
//!
//! The framework does not ship an exporter; applications register the meter
//! (Prometheus/OpenTelemetry/in-memory) and components such as the AI executor
//! emit through this trait. Implementations must be cheap and non-blocking on
//! the hot path.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

/// A counter that only increases.
pub trait Counter: Send + Sync {
    /// Add `value`.
    fn add(&self, value: u64);

    /// Add one.
    fn inc(&self) {
        self.add(1);
    }
}

/// A histogram of `f64` observations.
pub trait Histogram: Send + Sync {
    /// Record one observation.
    fn record(&self, value: f64);
}

/// A gauge that can go up and down.
pub trait Gauge: Send + Sync {
    /// Set the current value.
    fn set(&self, value: f64);

    /// Add `delta`.
    fn add(&self, delta: f64);
}

/// Metrics factory.
pub trait IMeter: Send + Sync {
    /// Counter for `name` + tags.
    fn counter(&self, name: &'static str, tags: &[(&str, &str)]) -> Arc<dyn Counter>;
    /// Histogram for `name` + tags.
    fn histogram(&self, name: &'static str, tags: &[(&str, &str)]) -> Arc<dyn Histogram>;
    /// Gauge for `name` + tags.
    fn gauge(&self, name: &'static str, tags: &[(&str, &str)]) -> Arc<dyn Gauge>;

    /// Snapshot of every recorded series, for the framework's `/metrics` bridge.
    ///
    /// The default returns nothing: a meter that exports directly (OpenTelemetry,
    /// Prometheus client) does not need to expose its series here, and
    /// `MemoryMeter` implements it so `/metrics` can render business metrics.
    /// Implementations should return a consistent, bounded snapshot and never
    /// block the scrape on network I/O.
    fn snapshot(&self) -> Vec<MetricSample> {
        Vec::new()
    }
}

/// Kind of a [`MetricSample`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum MetricKind {
    /// Monotonic counter.
    Counter,
    /// Point-in-time value.
    Gauge,
    /// Observation aggregate (`value` = sum, `count` = observations).
    Histogram,
}

impl MetricKind {
    /// Prometheus type label (`counter`/`gauge`/`summary`).
    pub fn prometheus_type(self) -> &'static str {
        match self {
            Self::Counter => "counter",
            Self::Gauge => "gauge",
            // A bucket-less histogram is exported as a summary (count + sum).
            Self::Histogram => "summary",
        }
    }
}

/// One recorded series, as seen by the `/metrics` exporter.
#[derive(Debug, Clone, PartialEq)]
pub struct MetricSample {
    /// Metric name (counters conventionally end in `_total`).
    pub name: String,
    /// Kind of the series.
    pub kind: MetricKind,
    /// Labels, sorted by name (unique names).
    pub tags: Vec<(String, String)>,
    /// Counter/gauge value, or the observation sum for a histogram.
    pub value: f64,
    /// Observation count for a histogram (`0` for counters/gauges).
    pub count: u64,
}

/// A meter that drops every observation.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoopMeter;

struct NoopMetric;

impl Counter for NoopMetric {
    fn add(&self, _value: u64) {}
}

impl Histogram for NoopMetric {
    fn record(&self, _value: f64) {}
}

impl Gauge for NoopMetric {
    fn set(&self, _value: f64) {}
    fn add(&self, _delta: f64) {}
}

impl IMeter for NoopMeter {
    fn counter(&self, _name: &'static str, _tags: &[(&str, &str)]) -> Arc<dyn Counter> {
        Arc::new(NoopMetric)
    }

    fn histogram(&self, _name: &'static str, _tags: &[(&str, &str)]) -> Arc<dyn Histogram> {
        Arc::new(NoopMetric)
    }

    fn gauge(&self, _name: &'static str, _tags: &[(&str, &str)]) -> Arc<dyn Gauge> {
        Arc::new(NoopMetric)
    }
}

/// One series identity: metric name + sorted, unique label pairs.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct MetricId {
    name: String,
    tags: Vec<(String, String)>,
}

impl MetricId {
    fn new(name: &str, tags: &[(&str, &str)]) -> Self {
        let mut tags: Vec<(String, String)> = tags
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        tags.sort();
        // Duplicate label names are invalid in Prometheus; keep the first value
        // after sorting so the exported text stays well-formed.
        tags.dedup_by(|left, right| left.0 == right.0);
        Self {
            name: name.to_string(),
            tags,
        }
    }
}

/// Process-local meter for tests, small deployments and `/metrics` rendering.
///
/// Series are bounded by `max_series` (default: unlimited). A new series beyond
/// the cap is dropped and counted by [`Self::dropped_series`], so a label
/// explosion cannot exhaust memory. Counter names should follow the Prometheus
/// convention and end in `_total`.
#[derive(Debug, Clone)]
pub struct MemoryMeter {
    inner: Arc<Mutex<MeterData>>,
    max_series: usize,
}

#[derive(Debug, Default)]
struct MeterData {
    counters: BTreeMap<MetricId, u64>,
    histograms: BTreeMap<MetricId, Vec<f64>>,
    gauges: BTreeMap<MetricId, f64>,
    dropped: u64,
}

impl MeterData {
    fn series_count(&self) -> usize {
        self.counters.len() + self.histograms.len() + self.gauges.len()
    }
}

impl Default for MemoryMeter {
    fn default() -> Self {
        Self::new()
    }
}

impl MemoryMeter {
    /// Empty meter with no cardinality bound.
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(MeterData::default())),
            max_series: usize::MAX,
        }
    }

    /// Cap the number of distinct series (labels included); extra series are dropped.
    pub fn with_max_series(mut self, max_series: usize) -> Self {
        self.max_series = max_series.max(1);
        self
    }

    /// Number of live series.
    pub fn series_count(&self) -> usize {
        self.lock().series_count()
    }

    /// Series dropped because the cardinality cap was reached.
    pub fn dropped_series(&self) -> u64 {
        self.lock().dropped
    }

    /// Current counter value (0 when absent).
    pub fn counter_value(&self, name: &str, tags: &[(&str, &str)]) -> u64 {
        self.lock()
            .counters
            .get(&MetricId::new(name, tags))
            .copied()
            .unwrap_or(0)
    }

    /// Recorded histogram observations.
    pub fn histogram_values(&self, name: &str, tags: &[(&str, &str)]) -> Vec<f64> {
        self.lock()
            .histograms
            .get(&MetricId::new(name, tags))
            .cloned()
            .unwrap_or_default()
    }

    /// Current gauge value (0 when absent).
    pub fn gauge_value(&self, name: &str, tags: &[(&str, &str)]) -> f64 {
        self.lock()
            .gauges
            .get(&MetricId::new(name, tags))
            .copied()
            .unwrap_or(0.0)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, MeterData> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

struct MemoryCounter {
    meter: MemoryMeter,
    id: MetricId,
}

impl Counter for MemoryCounter {
    fn add(&self, value: u64) {
        let mut data = self.meter.lock();
        if let Some(current) = data.counters.get_mut(&self.id) {
            *current += value;
            return;
        }
        if data.series_count() < self.meter.max_series {
            data.counters.insert(self.id.clone(), value);
        } else {
            data.dropped += 1;
        }
    }
}

struct MemoryHistogram {
    meter: MemoryMeter,
    id: MetricId,
}

impl Histogram for MemoryHistogram {
    fn record(&self, value: f64) {
        let mut data = self.meter.lock();
        if let Some(values) = data.histograms.get_mut(&self.id) {
            values.push(value);
            return;
        }
        if data.series_count() < self.meter.max_series {
            data.histograms.insert(self.id.clone(), vec![value]);
        } else {
            data.dropped += 1;
        }
    }
}

struct MemoryGauge {
    meter: MemoryMeter,
    id: MetricId,
}

impl Gauge for MemoryGauge {
    fn set(&self, value: f64) {
        let mut data = self.meter.lock();
        if let Some(current) = data.gauges.get_mut(&self.id) {
            *current = value;
            return;
        }
        if data.series_count() < self.meter.max_series {
            data.gauges.insert(self.id.clone(), value);
        } else {
            data.dropped += 1;
        }
    }

    fn add(&self, delta: f64) {
        let mut data = self.meter.lock();
        if let Some(current) = data.gauges.get_mut(&self.id) {
            *current += delta;
            return;
        }
        if data.series_count() < self.meter.max_series {
            data.gauges.insert(self.id.clone(), delta);
        } else {
            data.dropped += 1;
        }
    }
}

impl IMeter for MemoryMeter {
    fn counter(&self, name: &'static str, tags: &[(&str, &str)]) -> Arc<dyn Counter> {
        Arc::new(MemoryCounter {
            meter: self.clone(),
            id: MetricId::new(name, tags),
        })
    }

    fn histogram(&self, name: &'static str, tags: &[(&str, &str)]) -> Arc<dyn Histogram> {
        Arc::new(MemoryHistogram {
            meter: self.clone(),
            id: MetricId::new(name, tags),
        })
    }

    fn gauge(&self, name: &'static str, tags: &[(&str, &str)]) -> Arc<dyn Gauge> {
        Arc::new(MemoryGauge {
            meter: self.clone(),
            id: MetricId::new(name, tags),
        })
    }

    fn snapshot(&self) -> Vec<MetricSample> {
        let data = self.lock();
        let mut samples = Vec::with_capacity(data.series_count());
        for (id, value) in &data.counters {
            samples.push(MetricSample {
                name: id.name.clone(),
                kind: MetricKind::Counter,
                tags: id.tags.clone(),
                value: *value as f64,
                count: 0,
            });
        }
        for (id, value) in &data.gauges {
            samples.push(MetricSample {
                name: id.name.clone(),
                kind: MetricKind::Gauge,
                tags: id.tags.clone(),
                value: *value,
                count: 0,
            });
        }
        for (id, values) in &data.histograms {
            samples.push(MetricSample {
                name: id.name.clone(),
                kind: MetricKind::Histogram,
                tags: id.tags.clone(),
                value: values.iter().sum(),
                count: values.len() as u64,
            });
        }
        samples
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_meter_records_all_three_kinds() {
        let meter = MemoryMeter::new();
        let tags = [("model", "fast"), ("mode", "stream")];
        meter.counter("requests", &tags).add(2);
        meter.counter("requests", &tags).inc();
        meter.histogram("latency_ms", &tags).record(12.0);
        meter.gauge("active", &tags).set(3.0);
        meter.gauge("active", &tags).add(-1.0);

        assert_eq!(meter.counter_value("requests", &tags), 3);
        assert_eq!(meter.histogram_values("latency_ms", &tags), vec![12.0]);
        assert_eq!(meter.gauge_value("active", &tags), 2.0);
        assert_eq!(meter.counter_value("requests", &[("model", "other")]), 0);
    }

    #[test]
    fn tags_are_order_independent() {
        let meter = MemoryMeter::new();
        meter.counter("requests", &[("a", "1"), ("b", "2")]).inc();
        assert_eq!(
            meter.counter_value("requests", &[("b", "2"), ("a", "1")]),
            1
        );
    }

    #[test]
    fn snapshot_exposes_every_series_with_kind_and_labels() {
        let meter = MemoryMeter::new();
        meter.counter("jobs_total", &[("queue", "a")]).add(3);
        meter.gauge("workers", &[]).set(2.0);
        meter
            .histogram("job_seconds", &[("queue", "a")])
            .record(0.5);
        meter
            .histogram("job_seconds", &[("queue", "a")])
            .record(1.5);

        let samples = meter.snapshot();
        assert_eq!(samples.len(), 3);

        let counter = samples
            .iter()
            .find(|sample| sample.name == "jobs_total")
            .expect("counter series");
        assert_eq!(counter.kind, MetricKind::Counter);
        assert_eq!(counter.value, 3.0);
        assert_eq!(counter.tags, vec![("queue".to_string(), "a".to_string())]);
        assert_eq!(counter.kind.prometheus_type(), "counter");

        let histogram = samples
            .iter()
            .find(|sample| sample.name == "job_seconds")
            .expect("histogram series");
        assert_eq!(histogram.kind, MetricKind::Histogram);
        assert_eq!(histogram.value, 2.0);
        assert_eq!(histogram.count, 2);
        assert_eq!(histogram.kind.prometheus_type(), "summary");
    }

    #[test]
    fn cardinality_cap_drops_only_new_series() {
        let meter = MemoryMeter::new().with_max_series(2);
        meter.counter("a", &[]).inc();
        meter.counter("b", &[]).inc();
        meter.counter("c", &[]).inc();
        assert_eq!(meter.series_count(), 2);
        assert_eq!(meter.dropped_series(), 1);

        // Existing series keep updating after the cap is reached.
        meter.counter("a", &[]).add(4);
        assert_eq!(meter.counter_value("a", &[]), 5);
        assert_eq!(meter.snapshot().len(), 2);
    }
}
