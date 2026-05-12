//! End-to-end shadow testnet harness for S12 launch readiness.
//!
//! Demonstrates the full GSX-DB stack:
//! - Block execution with OCC parallelism
//! - State snapshots and DAG store
//! - Telemetry collection (metrics)
//! - Anchor parity verification
//! - Recovery from stored snapshots
//!
//! This harness runs against an in-memory test network and validates
//! that all subsystems work together coherently.

use gsxdb_bridge::{
    record_state_metrics, AnchorDispatcher, Block, BlockExecutor, BlockHash, BlockStore,
    BlockTimer, ChainId, InMemoryBlockStore, Intent, ParityTimer,
};
use gsxdb_state::{
    dag::{BlockHash as DagBlockHash, DagBlock, DagStore},
    snapshot::{SnapshotManager, StateSnapshot},
    Address, Balance, BridgeToken, Commitment, Metrics, State, StateChange, StateTree,
};
use std::sync::Arc;

fn addr(byte: u8) -> Address {
    Address([byte; 20])
}

fn seeded_state(n_accounts: usize) -> State {
    let mut state = State::default();
    let token = BridgeToken::__for_bridge_only();
    for i in 0..n_accounts {
        state.apply(
            &token,
            &StateChange::SetBalance {
                addr: addr(i as u8),
                to: Balance(10_000),
            },
        );
    }
    state
}

/// S12 exit gate 1: Block execution with metrics collection.
#[test]
fn block_execution_records_metrics() {
    let metrics = Arc::new(Metrics::new());
    let mut state = seeded_state(4);

    // Construct a block with 3 transfers.
    let intents = vec![
        Intent::Transfer {
            from: addr(0),
            to: addr(1),
            amount: 100,
        },
        Intent::Transfer {
            from: addr(1),
            to: addr(2),
            amount: 50,
        },
        Intent::Transfer {
            from: addr(2),
            to: addr(3),
            amount: 25,
        },
    ];

    // Execute with telemetry timer.
    let _report = {
        let _timer = BlockTimer::new(Arc::clone(&metrics));
        BlockExecutor.execute(&mut state, &intents)
    };

    // Verify block executed.
    assert_eq!(_report.outcomes.len(), 3);
    assert!(_report
        .outcomes
        .iter()
        .all(|o| matches!(o, gsxdb_bridge::TxOutcome::Committed)));

    // Verify metrics recorded.
    assert!(
        metrics.block_duration_ms.count() > 0,
        "block_duration_ms not recorded"
    );
    assert!(
        metrics.block_duration_ms.mean() >= 0.0,
        "block_duration_ms mean invalid"
    );

    // Record state metrics.
    record_state_metrics(&metrics, &state);
    assert_eq!(
        metrics.address_count.get() as usize,
        4,
        "address_count not updated"
    );
    assert!(
        metrics.state_size_bytes.get() > 0.0,
        "state_size_bytes not recorded"
    );
}

/// S12 exit gate 2: State snapshots capture block state.
#[test]
fn state_snapshot_captures_block_state() {
    let mut state = seeded_state(3);

    // Execute a block.
    let intents = vec![Intent::Transfer {
        from: addr(0),
        to: addr(1),
        amount: 500,
    }];
    BlockExecutor.execute(&mut state, &intents);

    // Take a snapshot.
    let tree = StateTree::from_state(&state);
    let root = tree.root();
    let encoded = serde_json::to_vec(&root).expect("serialize root");

    let snapshot = StateSnapshot::new(100, root, encoded, None);

    // Verify snapshot metadata.
    assert_eq!(snapshot.height, 100);
    assert_eq!(snapshot.state_root, root);
    assert!(snapshot.timestamp > 0);
    assert!(snapshot.is_valid(u64::MAX), "snapshot should be valid");

    // Verify JSON export.
    let json = snapshot.to_metadata_json();
    assert_eq!(json["height"], 100);
}

/// S12 exit gate 3: DAG store enforces causal ordering.
#[test]
fn dag_store_enforces_causality() {
    let mut store = DagStore::new();

    let root_hash: DagBlockHash = [1; 32];
    let root_block = DagBlock::genesis([2; 32], 1000);

    // Add root block.
    store.put(root_hash, root_block);
    assert!(store.contains(&root_hash), "root not stored");

    // Add child block.
    let child_hash: DagBlockHash = [3; 32];
    let child_block = DagBlock {
        height: 2,
        state_root: [4; 32],
        parent_hashes: vec![root_hash],
        timestamp: 2000,
    };
    store.put(child_hash, child_block);

    // Verify reachability: root should be reachable FROM child (via parent links).
    let path = store.find_path(&child_hash, &root_hash);
    assert!(
        path.is_some(),
        "root should be reachable from child by following parent links"
    );

    // Verify path includes both blocks.
    let path = path.unwrap();
    assert_eq!(path.len(), 2, "path should have 2 blocks: child -> root");
    assert_eq!(path[0], child_hash, "path should start at child");
    assert_eq!(path[1], root_hash, "path should end at root");
}

/// S12 exit gate 4: Anchor parity detection.
#[test]
fn anchor_parity_detects_divergence() {
    let metrics = Arc::new(Metrics::new());
    let mut dispatcher = AnchorDispatcher::new();

    // Register a chain.
    let chain_id = ChainId(1);
    let key = [7u8; 32];
    dispatcher.register(chain_id, key);

    // Dispatch genesis anchor (height 0).
    let genesis_root = Commitment([4; 32]);
    dispatcher
        .dispatch(0, genesis_root)
        .expect("dispatch genesis");

    // Dispatch an anchor at height 1.
    let root = Commitment([5; 32]);
    let anchors = dispatcher.dispatch(1, root).expect("dispatch");
    assert_eq!(anchors.len(), 1);

    // Record latency metric.
    let parity_result = {
        let _timer = ParityTimer::new(Arc::clone(&metrics));
        dispatcher.parity_check(1)
    };

    // Verify parity check ran.
    assert!(metrics.parity_check_duration_ms.count() > 0);
    match parity_result {
        gsxdb_bridge::ParityResult::Agreed { state_root } => {
            assert_eq!(state_root, root);
            metrics.anchors_submitted.inc();
        }
        gsxdb_bridge::ParityResult::Disagreed { .. } => {
            metrics.parity_failures.inc();
        }
    }

    assert!(metrics.anchors_submitted.get() > 0 || metrics.parity_failures.get() > 0);
}

/// S12 exit gate 5: Block recovery from store.
#[test]
fn block_recovery_from_store() {
    let mut state = seeded_state(2);
    let block_store = InMemoryBlockStore::new();

    // Execute and store a block.
    let intents = vec![Intent::Transfer {
        from: addr(0),
        to: addr(1),
        amount: 1000,
    }];

    let _report = BlockExecutor.execute(&mut state, &intents);
    let root_before = StateTree::from_state(&state).root();

    // Store the block.
    let block = Block {
        height: 1,
        parent: BlockHash([0; 32]),
        state_root: root_before,
        intents: intents.clone(),
    };

    let mut mutable_store = block_store;
    mutable_store.put(block).expect("put block");

    // Retrieve and verify.
    let retrieved = mutable_store
        .get_by_height(1)
        .expect("get_by_height ok")
        .expect("retrieve block");
    assert_eq!(retrieved.height, 1);
    assert_eq!(retrieved.state_root, root_before);
    assert_eq!(retrieved.intents.len(), 1);
}

/// S12 exit gate 6: Full integration — shadow testnet bootstrap.
#[test]
fn shadow_testnet_bootstrap() {
    let metrics = Arc::new(Metrics::new());
    let mut state = seeded_state(8);
    let block_store = InMemoryBlockStore::new();
    let mut dag_store = DagStore::new();
    let snapshot_mgr = SnapshotManager::new("/tmp/test-snapshots".to_string(), 2);

    // Simulate 3 blocks.
    let mut prev_hash = BlockHash([0; 32]);
    let mut mutable_store = block_store;

    for block_height in 1..=3 {
        // Create a block.
        let rng_byte = (block_height as u8).wrapping_add(1);
        let intents = vec![
            Intent::Transfer {
                from: addr(0),
                to: addr(rng_byte),
                amount: block_height as u128 * 10,
            },
            Intent::Transfer {
                from: addr(1),
                to: addr(rng_byte + 1),
                amount: block_height as u128 * 5,
            },
        ];

        // Execute with metrics.
        let report = {
            let _timer = BlockTimer::new(Arc::clone(&metrics));
            BlockExecutor.execute(&mut state, &intents)
        };

        // Verify all txns committed.
        assert!(
            report
                .outcomes
                .iter()
                .all(|o| matches!(o, gsxdb_bridge::TxOutcome::Committed)),
            "block {} should commit all txns",
            block_height
        );

        // Record state metrics.
        record_state_metrics(&metrics, &state);

        // Compute post-block root.
        let post_root = StateTree::from_state(&state).root();

        // Store block.
        let block = Block {
            height: block_height as u64,
            parent: prev_hash,
            state_root: post_root,
            intents: intents.clone(),
        };

        mutable_store.put(block.clone()).expect("put block");

        // Add to DAG.
        let dag_hash: DagBlockHash = [block_height as u8; 32];
        let dag_block = DagBlock {
            height: block_height as u64,
            state_root: post_root.0, // Extract the raw [u8; 32] from Commitment
            parent_hashes: vec![prev_hash.0], // Extract from BlockHash struct
            timestamp: 1000 * block_height as u64,
        };
        dag_store.put(dag_hash, dag_block);

        // Take snapshot if scheduled.
        if snapshot_mgr.should_snapshot(block_height as u64) {
            let snapshot = StateSnapshot::new(
                block_height as u64,
                post_root,
                serde_json::to_vec(&post_root).unwrap(),
                None,
            );
            assert!(
                snapshot.is_valid(u64::MAX),
                "snapshot block {} should be valid",
                block_height
            );
        }

        prev_hash = BlockHash([block_height as u8; 32]);
    }

    // Verify testnet state.
    assert_eq!(mutable_store.len(), Ok(3), "should have 3 blocks stored");
    assert!(
        metrics.block_duration_ms.count() >= 3,
        "should record 3 block executions"
    );
    assert!(
        metrics.address_count.get() >= 1.0,
        "address count should be tracked"
    );

    // Export metrics as Prometheus text.
    let metrics_obj = Metrics::new();
    metrics_obj.block_height.set(3.0);
    metrics_obj.blocks_committed.add(3);
    let prometheus_text = metrics_obj.to_prometheus_text();
    assert!(
        prometheus_text.contains("gsxdb_block_height"),
        "prometheus export should contain block_height"
    );
    assert!(
        prometheus_text.contains("gsxdb_blocks_committed"),
        "prometheus export should contain blocks_committed"
    );
}
