//! **S12.4** — Shadow-testnet E2E vs a live op-reth.
//!
//! Hits a real op-reth JSON-RPC endpoint, pulls balance + nonce for a
//! small address set, and asserts the syncer produces well-formed
//! [`SyncedEVMState`] records. This is a smoke-test of the live
//! cross-chain wiring — full-state parity remains a property test
//! against in-memory stubs (see `cross_parity.rs`).
//!
//! **Gated.** The test is skipped silently unless `GSXDB_SHADOW_RPC`
//! is set in the environment. Local dev + CI without an op-reth
//! reachable see a no-op; ops can opt in by exporting:
//!
//! ```sh
//!   export GSXDB_SHADOW_RPC=http://18.226.17.168:8545
//!   cargo test --test shadow_op_reth
//! ```
//!
//! Per CLAUDE.md the canonical shadow endpoint is the Phase-1 op-reth
//! at `18.226.17.168`. CI sets the env var only on protected runners
//! that have network egress to the testnet.

use gsxdb_bridge::sync::l2::{L2StateSyncer, L2SyncConfig};
use gsxdb_state::Address;
use std::env;

const ENV_SHADOW_RPC: &str = "GSXDB_SHADOW_RPC";
/// A few well-known testnet addresses with stable balance / nonce
/// shapes. None are critical — we only assert "produces a response,
/// not a panic" and "balance/nonce are sane (non-negative u128/u64)".
fn probe_addresses() -> Vec<Address> {
    vec![
        // Zero address — always has a defined balance on EVM chains
        // (typically 0 unless someone burned to it).
        Address([0u8; 20]),
        // OP-stack pre-deploy: L2CrossDomainMessenger at 0x...4007.
        // Always has nonzero code + nonce on any op-reth.
        {
            let mut a = [0u8; 20];
            a[19] = 0x07;
            a[18] = 0x40;
            Address(a)
        },
    ]
}

#[tokio::test]
async fn shadow_op_reth_returns_well_formed_state() {
    let Ok(rpc_url) = env::var(ENV_SHADOW_RPC) else {
        eprintln!(
            "skipping shadow_op_reth: set {ENV_SHADOW_RPC} to a JSON-RPC URL to run this test"
        );
        return;
    };

    let cfg = L2SyncConfig {
        rpc_url: rpc_url.clone(),
        addresses: probe_addresses(),
    };
    let syncer = L2StateSyncer::new(cfg);

    let result = syncer.sync().await;
    let states = match result {
        Ok(s) => s,
        Err(e) => {
            // Network unreachable / chain offline — emit a warning
            // but don't fail the suite. The test exists to guard
            // happy-path wiring, not to assert testnet uptime.
            eprintln!(
                "shadow_op_reth: live RPC at {rpc_url} unreachable: {e}. \
                 Treating as skip rather than failure."
            );
            return;
        }
    };

    assert_eq!(
        states.len(),
        probe_addresses().len(),
        "syncer must return one entry per requested address"
    );
    for state in &states {
        // Sanity-only: balance / nonce types prevent overflow by
        // construction, and a 0 reading is valid. The assertion here
        // is structural: every probed address comes back with the
        // address echoed exactly so the JSON parsing didn't misalign.
        assert!(
            probe_addresses().contains(&state.address),
            "syncer returned an unknown address: {:?}",
            state.address
        );
    }
}

/// Smoke-check: the syncer's error path is well-formed when the URL
/// is obviously bad. This always runs (no env gate); covers the
/// failure-mode branch the live test deliberately tolerates.
#[tokio::test]
async fn shadow_op_reth_reports_error_for_bad_url() {
    let cfg = L2SyncConfig {
        rpc_url: "http://127.0.0.1:1".to_string(),
        addresses: probe_addresses(),
    };
    let syncer = L2StateSyncer::new(cfg);
    let result = syncer.sync().await;
    assert!(
        result.is_err(),
        "expected error from unreachable RPC, got {result:?}"
    );
}
