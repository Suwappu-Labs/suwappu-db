//! Telemetry integration — instruments block execution, anchor submission,
//! and parity checks with observable metrics.
//!
//! Records metrics to a shared [`suwappudb_state::Metrics`] instance for
//! Prometheus export and observability dashboards.

use suwappudb_state::Metrics;
use std::sync::Arc;
use std::time::Instant;

/// Scoped timer for recording block processing duration.
///
/// Records elapsed time to metrics on drop.
pub struct BlockTimer {
    start: Instant,
    metrics: Arc<Metrics>,
}

impl BlockTimer {
    /// Start timing a block. Records `block_duration_ms` on drop.
    #[must_use]
    pub fn new(metrics: Arc<Metrics>) -> Self {
        Self {
            start: Instant::now(),
            metrics,
        }
    }
}

impl Drop for BlockTimer {
    fn drop(&mut self) {
        let elapsed_ms = self.start.elapsed().as_secs_f64() * 1000.0;
        self.metrics.block_duration_ms.record(elapsed_ms);
    }
}

/// Scoped timer for recording anchor submission latency.
///
/// Records elapsed time to metrics on drop.
pub struct AnchorTimer {
    start: Instant,
    metrics: Arc<Metrics>,
}

impl AnchorTimer {
    /// Start timing an anchor submission. Records `anchor_latency_ms` on drop.
    #[must_use]
    pub fn new(metrics: Arc<Metrics>) -> Self {
        Self {
            start: Instant::now(),
            metrics,
        }
    }
}

impl Drop for AnchorTimer {
    fn drop(&mut self) {
        let elapsed_ms = self.start.elapsed().as_secs_f64() * 1000.0;
        self.metrics.anchor_latency_ms.record(elapsed_ms);
    }
}

/// Scoped timer for recording parity check duration.
///
/// Records elapsed time to metrics on drop.
pub struct ParityTimer {
    start: Instant,
    metrics: Arc<Metrics>,
}

impl ParityTimer {
    /// Start timing a parity check. Records `parity_check_duration_ms` on drop.
    #[must_use]
    pub fn new(metrics: Arc<Metrics>) -> Self {
        Self {
            start: Instant::now(),
            metrics,
        }
    }
}

impl Drop for ParityTimer {
    fn drop(&mut self) {
        let elapsed_ms = self.start.elapsed().as_secs_f64() * 1000.0;
        self.metrics.parity_check_duration_ms.record(elapsed_ms);
    }
}

/// Record state snapshot metrics.
///
/// Called after computing state tree to record current tree metrics.
pub fn record_state_metrics(metrics: &Metrics, state: &suwappudb_state::State) {
    let entries = state.entries();
    metrics.address_count.set(entries.len() as f64);

    // Approximate state size: all entry data
    let mut total_bytes = 0usize;
    for (_addr, _slot) in &entries {
        // Address (20) + slot (32) + overhead
        total_bytes += 52;
    }
    metrics.state_size_bytes.set(total_bytes as f64);

    // Tree depth is always 20 (one byte per level in 20-byte address)
    metrics.tree_depth.set(20.0);
}

/// Record an executed block's OCC telemetry into [`Metrics`].
///
/// HARDENING rec 8 — emits the four counters that peer-chain
/// post-mortems flagged as load-bearing: `occ_aborts_total`,
/// `occ_collapse_to_sequential_total`, plus the standard
/// `blocks_committed` counter.
pub fn record_block_metrics(metrics: &Metrics, report: &crate::BlockReport) {
    metrics.blocks_committed.inc();
    metrics.occ_aborts_total.add(report.aborts as u64);
    if report.collapsed_to_sequential.is_some() {
        metrics.occ_collapse_to_sequential_total.inc();
    }
}

/// Record a parity check result.
///
/// HARDENING rec 8 — emits `anchor_parity_missing_chains` (gauge,
/// current) and `anchor_parity_divergent_total` (counter, cumulative).
/// Source: KelpDAO/LayerZero DVN compromise post-mortem.
pub fn record_parity_metrics(metrics: &Metrics, result: &crate::ParityResult) {
    match result {
        crate::ParityResult::Agreed { .. } => {
            metrics.anchor_parity_missing_chains.set(0.0);
        }
        crate::ParityResult::Disagreed {
            divergent,
            missing,
        } => {
            metrics.anchor_parity_missing_chains.set(missing.len() as f64);
            metrics
                .anchor_parity_divergent_total
                .add(divergent.len() as u64);
            metrics.parity_failures.inc();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_timer_records_elapsed() {
        let metrics = Arc::new(Metrics::new());
        {
            let _timer = BlockTimer::new(Arc::clone(&metrics));
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        // Timer dropped, metrics recorded
        assert!(metrics.block_duration_ms.count() > 0);
        assert!(metrics.block_duration_ms.mean() >= 4.0);
    }

    #[test]
    fn anchor_timer_records_elapsed() {
        let metrics = Arc::new(Metrics::new());
        {
            let _timer = AnchorTimer::new(Arc::clone(&metrics));
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(metrics.anchor_latency_ms.count() > 0);
        assert!(metrics.anchor_latency_ms.mean() >= 4.0);
    }

    #[test]
    fn parity_timer_records_elapsed() {
        let metrics = Arc::new(Metrics::new());
        {
            let _timer = ParityTimer::new(Arc::clone(&metrics));
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(metrics.parity_check_duration_ms.count() > 0);
        assert!(metrics.parity_check_duration_ms.mean() >= 4.0);
    }

    #[test]
    fn record_state_metrics_updates_counters() {
        let metrics = Metrics::new();
        let state = suwappudb_state::State::default();
        record_state_metrics(&metrics, &state);

        // Empty state should have 0 addresses
        assert_eq!(metrics.address_count.get() as u64, 0);
        assert_eq!(metrics.state_size_bytes.get() as u64, 0);
    }
}
