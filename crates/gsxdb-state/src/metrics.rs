//! Structured metrics and observability for GSX-DB.
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
    pub fn get(&self) -> f64 {
        *self.value.lock().unwrap_or_else(|e| e.into_inner())
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
    pub fn mean(&self) -> f64 {
        let samples = self
            .samples
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        if samples.is_empty() {
            0.0
        } else {
            samples.iter().sum::<f64>() / samples.len() as f64
        }
    }

    /// Get P99 percentile.
    pub fn p99(&self) -> f64 {
        let mut samples = self
            .samples
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        if samples.is_empty() {
            return 0.0;
        }
        samples.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let idx = (samples.len() as f64 * 0.99) as usize;
        samples.get(idx).copied().unwrap_or(0.0)
    }

    /// Get count of samples.
    pub fn count(&self) -> usize {
        self.samples.lock().unwrap_or_else(|e| e.into_inner()).len()
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
    pub fn get(&self) -> u64 {
        *self.value.lock().unwrap_or_else(|e| e.into_inner())
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
    pub fn start() -> Self {
        Self {
            start: Instant::now(),
        }
    }

    /// Get elapsed milliseconds.
    pub fn elapsed_ms(&self) -> f64 {
        self.start.elapsed().as_secs_f64() * 1000.0
    }
}

/// All metrics collected by GSX-DB.
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
    /// rate; a sustained ratio above ~0.3 is the Block-STM PPoPP
    /// paper's worst-case bound and indicates the workload is
    /// past the parallel break-even.
    pub occ_aborts_total: Counter,
    /// HARDENING rec 6 — chains-with-missing-anchor count from the
    /// most recent parity check. KelpDAO lost $292M when a single
    /// missing-verifier path drained funds; this metric exposes
    /// the same condition before the loss.
    pub anchor_parity_missing_chains: Gauge,
    /// HARDENING rec 6 — sum of divergent (chain, state_root) pairs
    /// observed in parity checks. Paired with the per-chain log
    /// for forensic replay.
    pub anchor_parity_divergent_total: Counter,
}

impl Metrics {
    /// Create a new metrics collector.
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

    /// Export metrics in Prometheus text format.
    pub fn to_prometheus_text(&self) -> String {
        let mut output = String::new();

        // Gauges
        output.push_str("# HELP gsxdb_block_height Current block height\n");
        output.push_str("# TYPE gsxdb_block_height gauge\n");
        output.push_str(&format!(
            "gsxdb_block_height {}\n",
            self.block_height.get() as u64
        ));

        output.push_str("# HELP gsxdb_snapshot_size_bytes Latest snapshot size in bytes\n");
        output.push_str("# TYPE gsxdb_snapshot_size_bytes gauge\n");
        output.push_str(&format!(
            "gsxdb_snapshot_size_bytes {}\n",
            self.snapshot_size_bytes.get() as u64
        ));

        output.push_str("# HELP gsxdb_tree_depth State tree depth\n");
        output.push_str("# TYPE gsxdb_tree_depth gauge\n");
        output.push_str(&format!(
            "gsxdb_tree_depth {}\n",
            self.tree_depth.get() as u64
        ));

        output.push_str("# HELP gsxdb_address_count Number of addresses in state\n");
        output.push_str("# TYPE gsxdb_address_count gauge\n");
        output.push_str(&format!(
            "gsxdb_address_count {}\n",
            self.address_count.get() as u64
        ));

        output.push_str("# HELP gsxdb_state_size_bytes Total state size in bytes\n");
        output.push_str("# TYPE gsxdb_state_size_bytes gauge\n");
        output.push_str(&format!(
            "gsxdb_state_size_bytes {}\n",
            self.state_size_bytes.get() as u64
        ));

        // Histograms (as summary metrics)
        output
            .push_str("# HELP gsxdb_block_duration_ms Block execution duration in milliseconds\n");
        output.push_str("# TYPE gsxdb_block_duration_ms histogram\n");
        output.push_str(&format!(
            "gsxdb_block_duration_ms_count {}\n",
            self.block_duration_ms.count()
        ));
        output.push_str(&format!(
            "gsxdb_block_duration_ms_sum {}\n",
            self.block_duration_ms
                .samples
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .iter()
                .sum::<f64>()
        ));

        output
            .push_str("# HELP gsxdb_anchor_latency_ms Anchor submission latency in milliseconds\n");
        output.push_str("# TYPE gsxdb_anchor_latency_ms histogram\n");
        output.push_str(&format!(
            "gsxdb_anchor_latency_ms_count {}\n",
            self.anchor_latency_ms.count()
        ));
        output.push_str(&format!(
            "gsxdb_anchor_latency_ms_sum {}\n",
            self.anchor_latency_ms
                .samples
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .iter()
                .sum::<f64>()
        ));

        output.push_str(
            "# HELP gsxdb_parity_check_duration_ms Parity check duration in milliseconds\n",
        );
        output.push_str("# TYPE gsxdb_parity_check_duration_ms histogram\n");
        output.push_str(&format!(
            "gsxdb_parity_check_duration_ms_count {}\n",
            self.parity_check_duration_ms.count()
        ));
        output.push_str(&format!(
            "gsxdb_parity_check_duration_ms_sum {}\n",
            self.parity_check_duration_ms
                .samples
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .iter()
                .sum::<f64>()
        ));

        // Counters
        output.push_str("# HELP gsxdb_blocks_committed Total blocks committed\n");
        output.push_str("# TYPE gsxdb_blocks_committed counter\n");
        output.push_str(&format!(
            "gsxdb_blocks_committed {}\n",
            self.blocks_committed.get()
        ));

        output.push_str("# HELP gsxdb_anchors_submitted Total anchors submitted\n");
        output.push_str("# TYPE gsxdb_anchors_submitted counter\n");
        output.push_str(&format!(
            "gsxdb_anchors_submitted {}\n",
            self.anchors_submitted.get()
        ));

        output.push_str("# HELP gsxdb_parity_failures Total parity check failures\n");
        output.push_str("# TYPE gsxdb_parity_failures counter\n");
        output.push_str(&format!(
            "gsxdb_parity_failures {}\n",
            self.parity_failures.get()
        ));

        // HARDENING rec 8 — additional metrics anchored to peer-chain
        // post-mortems. Each emits even when zero so absence is itself
        // an alert.
        output.push_str(
            "# HELP gsxdb_occ_collapse_to_sequential_total \
             Hot-slot conflict-storm collapses (HARDENING rec 2.2; Aptos AIP-47)\n",
        );
        output.push_str("# TYPE gsxdb_occ_collapse_to_sequential_total counter\n");
        output.push_str(&format!(
            "gsxdb_occ_collapse_to_sequential_total {}\n",
            self.occ_collapse_to_sequential_total.get()
        ));

        output.push_str(
            "# HELP gsxdb_occ_aborts_total \
             OCC re-executions; abort_rate = total / blocks_committed (Block-STM PPoPP)\n",
        );
        output.push_str("# TYPE gsxdb_occ_aborts_total counter\n");
        output.push_str(&format!(
            "gsxdb_occ_aborts_total {}\n",
            self.occ_aborts_total.get()
        ));

        output.push_str(
            "# HELP gsxdb_anchor_parity_missing_chains \
             Chains missing an anchor at last parity check (HARDENING rec 6; KelpDAO 292M)\n",
        );
        output.push_str("# TYPE gsxdb_anchor_parity_missing_chains gauge\n");
        output.push_str(&format!(
            "gsxdb_anchor_parity_missing_chains {}\n",
            self.anchor_parity_missing_chains.get() as u64
        ));

        output.push_str(
            "# HELP gsxdb_anchor_parity_divergent_total \
             Cumulative divergent (chain, state_root) pairs across parity checks\n",
        );
        output.push_str("# TYPE gsxdb_anchor_parity_divergent_total counter\n");
        output.push_str(&format!(
            "gsxdb_anchor_parity_divergent_total {}\n",
            self.anchor_parity_divergent_total.get()
        ));

        output
    }
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
            h.record(i as f64);
        }
        let p99 = h.p99();
        assert!(p99 >= 99.0 && p99 <= 100.0);
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
        assert!(text.contains("gsxdb_block_height 42"));
        assert!(text.contains("gsxdb_blocks_committed 5"));
        assert!(text.contains("gsxdb_block_duration_ms_count 1"));
    }
}
