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

/// Process-local meter for tests and small deployments.
#[derive(Debug, Default, Clone)]
pub struct MemoryMeter {
    inner: Arc<Mutex<MeterData>>,
}

#[derive(Debug, Default)]
struct MeterData {
    counters: BTreeMap<String, u64>,
    histograms: BTreeMap<String, Vec<f64>>,
    gauges: BTreeMap<String, f64>,
}

impl MemoryMeter {
    /// Empty meter.
    pub fn new() -> Self {
        Self::default()
    }

    /// Current counter value (0 when absent).
    pub fn counter_value(&self, name: &str, tags: &[(&str, &str)]) -> u64 {
        self.lock()
            .counters
            .get(&metric_key(name, tags))
            .copied()
            .unwrap_or(0)
    }

    /// Recorded histogram observations.
    pub fn histogram_values(&self, name: &str, tags: &[(&str, &str)]) -> Vec<f64> {
        self.lock()
            .histograms
            .get(&metric_key(name, tags))
            .cloned()
            .unwrap_or_default()
    }

    /// Current gauge value (0 when absent).
    pub fn gauge_value(&self, name: &str, tags: &[(&str, &str)]) -> f64 {
        self.lock()
            .gauges
            .get(&metric_key(name, tags))
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
    key: String,
}

impl Counter for MemoryCounter {
    fn add(&self, value: u64) {
        *self
            .meter
            .lock()
            .counters
            .entry(self.key.clone())
            .or_insert(0) += value;
    }
}

struct MemoryHistogram {
    meter: MemoryMeter,
    key: String,
}

impl Histogram for MemoryHistogram {
    fn record(&self, value: f64) {
        self.meter
            .lock()
            .histograms
            .entry(self.key.clone())
            .or_default()
            .push(value);
    }
}

struct MemoryGauge {
    meter: MemoryMeter,
    key: String,
}

impl Gauge for MemoryGauge {
    fn set(&self, value: f64) {
        self.meter.lock().gauges.insert(self.key.clone(), value);
    }

    fn add(&self, delta: f64) {
        *self
            .meter
            .lock()
            .gauges
            .entry(self.key.clone())
            .or_insert(0.0) += delta;
    }
}

impl IMeter for MemoryMeter {
    fn counter(&self, name: &'static str, tags: &[(&str, &str)]) -> Arc<dyn Counter> {
        Arc::new(MemoryCounter {
            meter: self.clone(),
            key: metric_key(name, tags),
        })
    }

    fn histogram(&self, name: &'static str, tags: &[(&str, &str)]) -> Arc<dyn Histogram> {
        Arc::new(MemoryHistogram {
            meter: self.clone(),
            key: metric_key(name, tags),
        })
    }

    fn gauge(&self, name: &'static str, tags: &[(&str, &str)]) -> Arc<dyn Gauge> {
        Arc::new(MemoryGauge {
            meter: self.clone(),
            key: metric_key(name, tags),
        })
    }
}

fn metric_key(name: &str, tags: &[(&str, &str)]) -> String {
    let mut tags = tags.to_vec();
    tags.sort_by(|left, right| left.0.cmp(right.0));
    let mut key = String::with_capacity(name.len() + tags.len() * 16);
    key.push_str(name);
    key.push('{');
    for (index, (tag, value)) in tags.iter().enumerate() {
        if index > 0 {
            key.push(',');
        }
        key.push_str(tag);
        key.push('=');
        key.push_str(value);
    }
    key.push('}');
    key
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
}
