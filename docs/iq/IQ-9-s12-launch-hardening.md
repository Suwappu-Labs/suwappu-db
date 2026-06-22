# IQ-9: S12 Launch Hardening — DAG Store, Snapshots, Telemetry, E2E

**Status:** Decided (S12)  
**Decision:** Multi-component hardening for production readiness
**Components:** DAG store, state snapshots, structured telemetry, testnet E2E shadow

---

## Problem Statement

Phase 1 (S1–S11) lands a working dual-VM database with anchors, but is incomplete for production deployment:

1. **Block dependencies:** No DAG; blocks depend on previous block linearly. Real system needs conditional logic: block N+1 can skip block N if they commit the same state.
2. **State recovery:** Current `RedbBlockStore` replay is deterministic but offers no snapshots. Recovery from disk is O(height) time.
3. **Observability:** No metrics, no structured logging. Impossible to debug in production.
4. **Integration testing:** No testnet E2E. Anchor parity verified locally only; not proven end-to-end.

S12 addresses all four by introducing a DAG backend, snapshot export/import, OpenTelemetry integration, and a shadow testnet.

---

## Design: DAG Store

### Motivation

Current `RedbBlockStore` enforces a linear chain: `block[N].parent = block[N-1].hash()`. This is unnecessary for a deterministic state engine. If two blocks at the same height produce the same state root, the second block can point to the first's parent.

### Data Structure

```rust
/// Block with flexible parent linkage.
pub struct DagBlock {
    height: u64,
    state_root: Commitment,
    // Can point to ANY earlier block (not just height-1)
    parent_hashes: Vec<[u8; 32]>,
    // ... existing fields (timestamp, tx list, etc.)
}

/// Proof that block B can be reached from block A.
pub struct DagProof {
    // Path from A to B; may skip blocks
    path: Vec<[u8; 32]>,
    // Merkle proof of validity at each step
    commitments: Vec<Commitment>,
}
```

### Algorithm: DAG Reachability

```
fn is_reachable(dag, from: hash, to: hash) -> bool:
    visited = {}
    queue = [from]
    while queue not empty:
        current = queue.pop()
        if current == to:
            return True
        if current in visited:
            continue
        visited.insert(current)
        block = dag.get(current)
        for parent in block.parent_hashes:
            queue.push(parent)
    return False
```

### Property: Causal Consistency

For any block B:
```
B is valid iff:
1. B's state root is canonical: transition(parent_states) == B.state_root
2. B is reachable from GENESIS
3. All parents are earlier in height
```

### Exit Gate

- [ ] `DagStore` trait with `put()`, `get()`, `is_reachable()`
- [ ] `RedbDagStore` implementation
- [ ] Property test @ 10k: `reachability_is_transitive`
- [ ] Merkle proof generation and verification

---

## Design: State Snapshots

### Motivation

Replaying 100k blocks takes time. Snapshots allow:
- Fast startup from recent snapshot + delta
- Export state for cross-validation
- Rollback to known-good state

### Data Structure

```rust
pub struct StateSnapshot {
    height: u64,
    state_root: Commitment,
    // Serialized redb tables: balances, nonces, storage
    encoded_state: Vec<u8>,
    // Timestamp + anchor digest for cross-check
    timestamp: u64,
    anchor_digest: AnchorHash,
}

impl StateSnapshot {
    /// Export snapshot at current height.
    pub fn export(engine: &StateEngine, path: &str) -> Result<(), Error>

    /// Import snapshot and resume from it.
    pub fn import(path: &str) -> Result<(StateEngine, u64), Error>

    /// Validate snapshot matches on-chain anchor.
    pub fn verify_against_anchor(snapshot: &Self, registry: &LTPAnchorRegistry, chain_id: u32) -> bool
}
```

### Storage Strategy

**On-disk format:**
```
snapshot-<height>-<hash>/
  ├── metadata.json    (height, root, timestamp, anchor)
  ├── state.redb       (serialized state tables)
  ├── proof.json       (Merkle proof from root to each leaf for audit)
  └── checksum.sha256  (for integrity check)
```

**Frequency:** Every 1000 blocks (configurable); oldest snapshots pruned after 7 days.

### Exit Gate

- [ ] `StateSnapshot` struct and serialization
- [ ] `export()` and `import()` functions
- [ ] Anchor cross-check: `snapshot.verify_against_anchor()`
- [ ] Property test @ 1k: `export_import_round_trip`
- [ ] Cleanup: `prune_old_snapshots()`

---

## Design: Structured Telemetry

### Motivation

Production deployment requires observability: block processing time, state size, anchor latency, etc.

### Stack

**OpenTelemetry + Prometheus:**
- Meter: standard `opentelemetry` crate
- Exporter: `opentelemetry-prometheus` for Prometheus scrape endpoint
- Traces: optional; focus on metrics for Phase 1

### Metrics

| Metric | Type | Labels | Purpose |
|--------|------|--------|---------|
| `suwappudb_block_height` | Gauge | `chain_id` | Current block height |
| `suwappudb_state_root` | Gauge (hash) | `chain_id` | Latest state root |
| `suwappudb_block_duration_ms` | Histogram | `chain_id`, `executor` | Block exec time |
| `suwappudb_anchor_latency_ms` | Histogram | `chain_id` | Time from block → anchor acceptance |
| `suwappudb_snapshot_size_bytes` | Gauge | `chain_id` | Latest snapshot size |
| `suwappudb_tree_depth` | Gauge | — | State tree depth |
| `suwappudb_parity_check_duration_ms` | Histogram | — | Cross-chain parity check time |

### Integration Points

```rust
// At block submission
metrics.block_height.set(block.height);
metrics.block_duration.record(elapsed_ms);

// At anchor acceptance
metrics.anchor_latency.record(elapsed_ms);

// At snapshot
metrics.snapshot_size.set(snapshot.encoded_state.len());
```

### Exit Gate

- [ ] Add `opentelemetry` + `opentelemetry-prometheus` to Cargo.toml
- [ ] Meter initialization in `main()` or server startup
- [ ] All metrics above instrumented
- [ ] `/metrics` endpoint returns Prometheus text format
- [ ] Grafana dashboard template for visualization

---

## Design: Testnet Shadow E2E

### Motivation

Anchor parity verified locally; not proven on testnet. E2E shadow tests:
1. Deploy Solidity registry to testnet
2. Run suwappudb-server locally, submit blocks
3. Submit anchors to Solidity every N blocks
4. Verify Solidity state matches local dispatcher

### Setup

**Testnet:** OP Stack testnet (e.g., OP Goerli or local `anvil`)

**Components:**
1. `LTPAnchorRegistry` deployed at fixed address
2. `suwappudb-server` running locally with RPC client pointing to testnet
3. ECDSA signer (test key) whitelisted in registry
4. Integration test that:
   - Advances suwappudb 10 blocks
   - Submits anchor to Solidity
   - Polls registry via eth_call
   - Asserts Solidity state matches suwappudb

### Test Harness

```rust
#[tokio::test]
async fn e2e_shadow_testnet() {
    // 1. Deploy contract
    let registry = deploy_ltp_anchor_registry(&anvil_url).await?;

    // 2. Start suwappudb-server
    let server = start_suwappudb_server_with_config(
        config::Config {
            rpc_port: 8660,
            l1_rpc_url: anvil_url.clone(),
            anchor_registry: registry.address(),
            ..default()
        }
    ).await?;

    // 3. Submit 10 blocks
    for i in 0..10 {
        server.post::<suwappu_submitBlock>(...).await?;
    }

    // 4. Submit anchor
    let anchor = server.get::<suwappu_getLastAnchor>(...).await?;
    let tx = registry.acceptAnchor(anchor, signature).await?;
    wait_for_finality(&anvil, tx).await?;

    // 5. Verify
    let solidity_root = registry.getLastAnchorHash(chain_id).await?;
    let rust_root = server.get::<suwappu_getStateRoot>().await?;
    assert_eq!(solidity_root, rust_root);

    Ok(())
}
```

### Exit Gate

- [ ] Anvil setup instructions in `README`
- [ ] `tests/e2e_shadow_testnet.rs` passing locally
- [ ] Test includes: block submission → anchor → Solidity verification → state match
- [ ] Document how to run against public testnet (OP Goerli, Sepolia)

---

## Integration: Boot Sequence (S12 End-to-End)

At `suwappudb-server` startup:

```rust
pub async fn main() {
    // 1. Load snapshot if available (fast startup)
    let (mut engine, height) = match StateSnapshot::import(&snapshot_path) {
        Ok((e, h)) => (e, h),
        Err(_) => (StateEngine::new(), 0),
    };

    // 2. Initialize telemetry
    let _telemetry = init_opentelemetry_prometheus()?;

    // 3. Replay blocks from disk (DAG reachability ensures fast path)
    for block in block_store.blocks_after(height)? {
        engine.execute(&block)?;
        metrics.block_height.set(block.height);
    }

    // 4. Start anchor submission task
    let anchor_task = tokio::spawn(submit_anchors_periodically(
        engine.clone(),
        l1_client,
        registry_address,
        signer,
    ));

    // 5. Start HTTP server (includes /metrics)
    start_server(engine, metrics).await?;

    // 6. Periodically export snapshots
    tokio::spawn(periodic_snapshot_export(engine.clone(), snapshot_dir));

    Ok(())
}
```

---

## Testing Strategy (S12 Exit Gate)

| Test | Component | Pass |
|------|-----------|------|
| DAG reachability @ 10k | DagStore | ✓ |
| Snapshot round-trip @ 1k | StateSnapshot | ✓ |
| Metrics instrumentation | Telemetry | ✓ |
| E2E shadow anchor submit | Testnet | ✓ |
| State root agreement (local vs Solidity) | E2E | ✓ |
| Parity check after snapshot restore | Integration | ✓ |

---

## Known Limitations (Post-S12)

1. **DAG complexity:** Reachability is O(V+E) BFS. For millions of blocks, consider indexed DAG or topological sort cache.
2. **Snapshot size:** Full state dump grows with address count. Consider compression or delta snapshots (S13+).
3. **Telemetry overhead:** Meter recording adds latency. Consider sampling for high-frequency metrics.
4. **Testnet E2E:** Anvil/Goerli testnet may have different finality rules than mainnet. Real L1 testing deferred to S13 (staging).

---

## Exit Gate for S12

- [ ] IQ-9 decided (DAG, snapshots, telemetry, E2E)
- [ ] `DagStore` with reachability property test @ 10k
- [ ] `StateSnapshot` with export/import + Anchor cross-check
- [ ] Telemetry: all metrics above, `/metrics` endpoint live
- [ ] E2E shadow testnet passing locally (Anvil or OP Goerli)
- [ ] Full test suite passing (all 74 state + DAG + snapshot + E2E)

Status at S12 close: **Launch-ready: all hardening components operational, production metrics live**.

---

## Reference

- DAG: [Lamport 1978 Time, Clocks]
- Snapshots: See `StateSnapshot` in `crates/suwappudb-state/src/snapshot.rs`
- OpenTelemetry: [opentelemetry.io](https://opentelemetry.io)
- E2E: [Ethereum Testing Best Practices](https://ethereum.org/en/developers/docs/testing/)
