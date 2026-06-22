//! Structured metrics and observability for Suwappu-DB.
//!
//! Exports metrics via OpenTelemetry + Prometheus for production observability.
//! Tracks block processing, anchor latency, state size, and parity checks.

use std::sync::{Arc, Mutex};
use std::time::Instant;

/// Metric type for gauge values (current state).
#[derive(Debug, Clone)]
pub struct Gauge {
    value: Arc<Mutex<f64>>,
}

impl Gauge {
    /// Create a new gauge.
    #[must_use]
    pub fn new() -> Self {
        Self {
            value: Arc::new(Mutex::new(0.0)),
        }
    }

    /// Set gauge to value.
    pub fn set(&self, value: f64) {
        if let Ok(mut v) = self.value.lock() {
            *v = value;
        }
    }

    /// Get current value.
    #[must_use]
    pub fn get(&self) -> f64 {
        *self.value.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Increment by delta.
    pub fn add(&self, delta: f64) {
        if let Ok(mut v) = self.value.lock() {
            *v += delta;
        }
    }
}

impl Default for Gauge {
    fn default() -> Self {
        Self::new()
    }
}

/// Metric type for histogram (distribution of values).
#[derive(Debug, Clone)]
pub struct Histogram {
    samples: Arc<Mutex<Vec<f64>>>,
}

impl Histogram {
    /// Create a new histogram.
    #[must_use]
    pub fn new() -> Self {
        Self {
            samples: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Record a sample.
    pub fn record(&self, value: f64) {
        if let Ok(mut samples) = self.samples.lock() {
            samples.push(value);
            // Keep only last 1000 samples to avoid unbounded growth
            if samples.len() > 1000 {
                samples.remove(0);
            }
        }
    }

    /// Get mean of samples.
    #[must_use]
    pub fn mean(&self) -> f64 {
        let samples = self
            .samples
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        if samples.is_empty() {
            0.0
        } else {
            samples.iter().sum::<f64>() / samples.len() as f64
        }
    }

    /// Get P99 percentile.
    #[must_use]
    pub fn p99(&self) -> f64 {
        let mut samples = self
            .samples
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        if samples.is_empty() {
            return 0.0;
        }
        samples.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let idx = (samples.len() as f64 * 0.99) as usize;
        samples.get(idx).copied().unwrap_or(0.0)
    }

    /// Get count of samples.
    #[must_use]
    pub fn count(&self) -> usize {
        self.samples.lock().unwrap_or_else(std::sync::PoisonError::into_inner).len()
    }

    /// **S12.3** — Sum of all recorded samples. Required for the
    /// Prometheus summary `_sum` line.
    #[must_use]
    pub fn sum(&self) -> f64 {
        self.samples
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .sum()
    }

    /// **S12.3** — Arbitrary quantile (`0.0..=1.0`). Used to emit
    /// `_summary{quantile="0.5"}`, `quantile="0.95"`, `quantile="0.99"`.
    /// Naïve linear interpolation; the histogram bound (last 1000
    /// samples) keeps this cheap.
    #[must_use]
    pub fn quantile(&self, q: f64) -> f64 {
        let q = q.clamp(0.0, 1.0);
        let mut samples = self
            .samples
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        if samples.is_empty() {
            return 0.0;
        }
        samples.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let idx = ((samples.len() - 1) as f64 * q).round() as usize;
        samples.get(idx).copied().unwrap_or(0.0)
    }
}

impl Default for Histogram {
    fn default() -> Self {
        Self::new()
    }
}

/// Counter for monotonically increasing values.
#[derive(Debug, Clone)]
pub struct Counter {
    value: Arc<Mutex<u64>>,
}

impl Counter {
    /// Create a new counter.
    #[must_use]
    pub fn new() -> Self {
        Self {
            value: Arc::new(Mutex::new(0)),
        }
    }

    /// Increment counter by 1.
    pub fn inc(&self) {
        if let Ok(mut v) = self.value.lock() {
            *v += 1;
        }
    }

    /// Increment by delta.
    pub fn add(&self, delta: u64) {
        if let Ok(mut v) = self.value.lock() {
            *v += delta;
        }
    }

    /// Get current value.
    #[must_use]
    pub fn get(&self) -> u64 {
        *self.value.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl Default for Counter {
    fn default() -> Self {
        Self::new()
    }
}

/// Scoped timer for measuring durations.
pub struct Timer {
    start: Instant,
}

impl Timer {
    /// Start a new timer.
    #[must_use]
    pub fn start() -> Self {
        Self {
            start: Instant::now(),
        }
    }

    /// Get elapsed milliseconds.
    #[must_use]
    pub fn elapsed_ms(&self) -> f64 {
        self.start.elapsed().as_secs_f64() * 1000.0
    }
}

/// All metrics collected by Suwappu-DB.
pub struct Metrics {
    /// Current block height.
    pub block_height: Gauge,
    /// Block processing time in milliseconds.
    pub block_duration_ms: Histogram,
    /// Anchor submission latency in milliseconds.
    pub anchor_latency_ms: Histogram,
    /// Latest snapshot size in bytes.
    pub snapshot_size_bytes: Gauge,
    /// State tree depth (max height in trie).
    pub tree_depth: Gauge,
    /// Parity check duration in milliseconds.
    pub parity_check_duration_ms: Histogram,
    /// Number of addresses in state.
    pub address_count: Gauge,
    /// Total state size in bytes (approx).
    pub state_size_bytes: Gauge,
    /// Blocks successfully committed.
    pub blocks_committed: Counter,
    /// Anchors successfully submitted.
    pub anchors_submitted: Counter,
    /// Parity check failures.
    pub parity_failures: Counter,

    // HARDENING rec 8 — four metrics that named other-chain
    // post-mortems cite as load-bearing. Each emits even when zero
    // so absence of the metric is itself an alert.

    /// HARDENING rec 2.2 — number of blocks where the OCC scheduler
    /// detected a hot-slot conflict storm and collapsed remaining
    /// txns to sequential execution. First sustained increase here
    /// is the canonical Block-STM contention signal; per Aptos
    /// AIP-47, the right response is an Aggregator-style write, not
    /// a DAG-liveness investigation.
    pub occ_collapse_to_sequential_total: Counter,
    /// HARDENING rec 2.2 — total OCC aborts observed across all
    /// blocks. Paired with `blocks_committed` to compute the abort
    /// rate; a sustained ratio above ~0.3 is the Block-STM `PPoPP`
    /// paper's worst-case bound and indicates the workload is
    /// past the parallel break-even.
    pub occ_aborts_total: Counter,
    /// HARDENING rec 6 — chains-with-missing-anchor count from the
    /// most recent parity check. `KelpDAO` lost $292M when a single
    /// missing-verifier path drained funds; this metric exposes
    /// the same condition before the loss.
    pub anchor_parity_missing_chains: Gauge,
    /// HARDENING rec 6 — sum of divergent (chain, `state_root`) pairs
    /// observed in parity checks. Paired with the per-chain log
    /// for forensic replay.
    pub anchor_parity_divergent_total: Counter,
}

impl Metrics {
    /// Create a new metrics collector.
    #[must_use]
    pub fn new() -> Self {
        Self {
            block_height: Gauge::new(),
            block_duration_ms: Histogram::new(),
            anchor_latency_ms: Histogram::new(),
            snapshot_size_bytes: Gauge::new(),
            tree_depth: Gauge::new(),
            parity_check_duration_ms: Histogram::new(),
            address_count: Gauge::new(),
            state_size_bytes: Gauge::new(),
            blocks_committed: Counter::new(),
            anchors_submitted: Counter::new(),
            parity_failures: Counter::new(),
            occ_collapse_to_sequential_total: Counter::new(),
            occ_aborts_total: Counter::new(),
            anchor_parity_missing_chains: Gauge::new(),
            anchor_parity_divergent_total: Counter::new(),
        }
    }

    /// **S12.3** — Export metrics in Prometheus text-exposition format.
    ///
    /// Conforms to <https://prometheus.io/docs/instrumenting/exposition_formats/>:
    /// every metric has a paired `# HELP` and `# TYPE` line; histograms
    /// emit as `summary` with `quantile="0.5"`, `0.95`, `0.99`, plus
    /// the required `_sum` and `_count` series. The previous emission
    /// labelled the histograms as `histogram` but only carried
    /// count+sum, which the Prometheus parser flags — `summary` is the
    /// correct type for our sample-array-based aggregation.
    #[must_use]
    pub fn to_prometheus_text(&self) -> String {
        let mut out = String::new();

        // ---- Gauges ----
        emit_gauge(&mut out, "suwappudb_block_height", "Current block height",
            self.block_height.get());
        emit_gauge(&mut out, "suwappudb_snapshot_size_bytes", "Latest snapshot size in bytes",
            self.snapshot_size_bytes.get());
        emit_gauge(&mut out, "suwappudb_tree_depth", "State tree depth",
            self.tree_depth.get());
        emit_gauge(&mut out, "suwappudb_address_count", "Number of addresses in state",
            self.address_count.get());
        emit_gauge(&mut out, "suwappudb_state_size_bytes", "Total state size in bytes",
            self.state_size_bytes.get());
        emit_gauge(
            &mut out,
            "suwappudb_anchor_parity_missing_chains",
            "Chains missing an anchor at last parity check (HARDENING rec 6; KelpDAO 292M)",
            self.anchor_parity_missing_chains.get(),
        );

        // ---- Summaries (sample-based; sub for full histogram) ----
        emit_summary(
            &mut out,
            "suwappudb_block_duration_ms",
            "Block execution duration in milliseconds",
            &self.block_duration_ms,
        );
        emit_summary(
            &mut out,
            "suwappudb_anchor_latency_ms",
            "Anchor submission latency in milliseconds",
            &self.anchor_latency_ms,
        );
        emit_summary(
            &mut out,
            "suwappudb_parity_check_duration_ms",
            "Parity check duration in milliseconds",
            &self.parity_check_duration_ms,
        );

        // ---- Counters ----
        emit_counter(&mut out, "suwappudb_blocks_committed", "Total blocks committed",
            self.blocks_committed.get());
        emit_counter(&mut out, "suwappudb_anchors_submitted", "Total anchors submitted",
            self.anchors_submitted.get());
        emit_counter(&mut out, "suwappudb_parity_failures", "Total parity check failures",
            self.parity_failures.get());
        emit_counter(
            &mut out,
            "suwappudb_occ_collapse_to_sequential_total",
            "Hot-slot conflict-storm collapses (HARDENING rec 2.2; Aptos AIP-47)",
            self.occ_collapse_to_sequential_total.get(),
        );
        emit_counter(
            &mut out,
            "suwappudb_occ_aborts_total",
            "OCC re-executions; abort_rate = total / blocks_committed (Block-STM PPoPP)",
            self.occ_aborts_total.get(),
        );
        emit_counter(
            &mut out,
            "suwappudb_anchor_parity_divergent_total",
            "Cumulative divergent (chain, state_root) pairs across parity checks",
            self.anchor_parity_divergent_total.get(),
        );

        out
    }
}

fn emit_gauge(out: &mut String, name: &str, help: &str, value: f64) {
    out.push_str(&format!("# HELP {name} {help}\n"));
    out.push_str(&format!("# TYPE {name} gauge\n"));
    out.push_str(&format!("{name} {value}\n"));
}

fn emit_counter(out: &mut String, name: &str, help: &str, value: u64) {
    out.push_str(&format!("# HELP {name} {help}\n"));
    out.push_str(&format!("# TYPE {name} counter\n"));
    out.push_str(&format!("{name} {value}\n"));
}

fn emit_summary(out: &mut String, name: &str, help: &str, h: &Histogram) {
    out.push_str(&format!("# HELP {name} {help}\n"));
    out.push_str(&format!("# TYPE {name} summary\n"));
    // Quantiles: 0.5 / 0.95 / 0.99. Always emit, even when count == 0,
    // so scraper alerts on missing series fire on a stuck producer.
    out.push_str(&format!(
        "{name}{{quantile=\"0.5\"}} {}\n",
        h.quantile(0.5)
    ));
    out.push_str(&format!(
        "{name}{{quantile=\"0.95\"}} {}\n",
        h.quantile(0.95)
    ));
    out.push_str(&format!(
        "{name}{{quantile=\"0.99\"}} {}\n",
        h.quantile(0.99)
    ));
    out.push_str(&format!("{name}_sum {}\n", h.sum()));
    out.push_str(&format!("{name}_count {}\n", h.count()));
}

impl Default for Metrics {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gauge_set_and_get() {
        let g = Gauge::new();
        g.set(42.0);
        assert_eq!(g.get(), 42.0);
    }

    #[test]
    fn gauge_add() {
        let g = Gauge::new();
        g.set(10.0);
        g.add(5.0);
        assert_eq!(g.get(), 15.0);
    }

    #[test]
    fn histogram_record_and_mean() {
        let h = Histogram::new();
        h.record(10.0);
        h.record(20.0);
        h.record(30.0);
        assert_eq!(h.mean(), 20.0);
        assert_eq!(h.count(), 3);
    }

    #[test]
    fn histogram_p99() {
        let h = Histogram::new();
        for i in 1..=100 {
            h.record(f64::from(i));
        }
        let p99 = h.p99();
        assert!((99.0..=100.0).contains(&p99));
    }

    #[test]
    fn counter_inc() {
        let c = Counter::new();
        c.inc();
        c.inc();
        assert_eq!(c.get(), 2);
    }

    #[test]
    fn counter_add() {
        let c = Counter::new();
        c.add(10);
        assert_eq!(c.get(), 10);
    }

    #[test]
    fn timer_elapsed() {
        let timer = Timer::start();
        std::thread::sleep(std::time::Duration::from_millis(10));
        let elapsed = timer.elapsed_ms();
        assert!(elapsed >= 9.0);
    }

    #[test]
    fn metrics_to_prometheus_text() {
        let m = Metrics::new();
        m.block_height.set(42.0);
        m.blocks_committed.add(5);
        m.block_duration_ms.record(100.0);

        let text = m.to_prometheus_text();
        assert!(text.contains("suwappudb_block_height 42"));
        assert!(text.contains("suwappudb_blocks_committed 5"));
        assert!(text.contains("suwappudb_block_duration_ms_count 1"));
    }

    /// S12.3: every metric series has paired `# HELP` and `# TYPE`
    /// lines (Prometheus exposition format requires it).
    #[test]
    fn prometheus_emits_help_and_type_for_every_metric() {
        let m = Metrics::new();
        let text = m.to_prometheus_text();

        // Every metric name appears once in `# HELP <name> ...` and
        // once in `# TYPE <name> ...`. Pair count must match.
        let help_count = text.lines().filter(|l| l.starts_with("# HELP ")).count();
        let type_count = text.lines().filter(|l| l.starts_with("# TYPE ")).count();
        assert_eq!(help_count, type_count, "HELP/TYPE line counts diverged");

        // Each declared metric should appear at least once as data.
        for name in [
            "suwappudb_block_height",
            "suwappudb_snapshot_size_bytes",
            "suwappudb_tree_depth",
            "suwappudb_address_count",
            "suwappudb_state_size_bytes",
            "suwappudb_anchor_parity_missing_chains",
            "suwappudb_block_duration_ms",
            "suwappudb_anchor_latency_ms",
            "suwappudb_parity_check_duration_ms",
            "suwappudb_blocks_committed",
            "suwappudb_anchors_submitted",
            "suwappudb_parity_failures",
            "suwappudb_occ_collapse_to_sequential_total",
            "suwappudb_occ_aborts_total",
            "suwappudb_anchor_parity_divergent_total",
        ] {
            assert!(
                text.contains(&format!("# TYPE {name} ")),
                "missing # TYPE line for {name}",
            );
        }
    }

    /// S12.3: summary metrics expose three quantile series + sum +
    /// count, even on an empty histogram.
    #[test]
    fn prometheus_summary_emits_quantiles_sum_count() {
        let m = Metrics::new();
        m.block_duration_ms.record(10.0);
        m.block_duration_ms.record(20.0);
        m.block_duration_ms.record(30.0);

        let text = m.to_prometheus_text();
        assert!(text.contains("suwappudb_block_duration_ms{quantile=\"0.5\"}"));
        assert!(text.contains("suwappudb_block_duration_ms{quantile=\"0.95\"}"));
        assert!(text.contains("suwappudb_block_duration_ms{quantile=\"0.99\"}"));
        assert!(text.contains("suwappudb_block_duration_ms_sum 60"));
        assert!(text.contains("suwappudb_block_duration_ms_count 3"));
    }

    /// S12.3: summary type is what Prometheus expects from sample-
    /// array-based aggregation (we don't compute bucket counts).
    #[test]
    fn prometheus_histogram_metrics_use_summary_type() {
        let m = Metrics::new();
        let text = m.to_prometheus_text();
        // None of our histograms should label as `histogram` — that
        // would require `_bucket{le="..."}` lines we don't emit.
        for name in [
            "suwappudb_block_duration_ms",
            "suwappudb_anchor_latency_ms",
            "suwappudb_parity_check_duration_ms",
        ] {
            assert!(
                text.contains(&format!("# TYPE {name} summary")),
                "{name} should be `summary` not `histogram`"
            );
            assert!(
                !text.contains(&format!("# TYPE {name} histogram")),
                "{name} mistakenly labeled `histogram`"
            );
        }
    }

    /// S12.3: quantile uses rounded-nearest indexing; emit increasing.
    #[test]
    fn histogram_quantile_monotonic_for_sorted_input() {
        let h = Histogram::new();
        for i in 1..=100 {
            h.record(f64::from(i));
        }
        let p50 = h.quantile(0.5);
        let p95 = h.quantile(0.95);
        let p99 = h.quantile(0.99);
        assert!(p50 <= p95, "p50={p50} > p95={p95}");
        assert!(p95 <= p99, "p95={p95} > p99={p99}");
        assert!(p99 <= 100.0);
        assert_eq!(h.sum(), (1..=100).map(f64::from).sum::<f64>());
    }
}
