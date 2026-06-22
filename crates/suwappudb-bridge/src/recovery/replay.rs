//! Recovery via deterministic replay.
//!
//! Walks blocks in height order from `from`, re-executes each through
//! the [`crate::BlockExecutor`], and verifies the resulting state-tree
//! root matches the recorded `Block::state_root`. Any mismatch surfaces
//! as [`RecoveryError::StateRootMismatch`].
//!
//! # Determinism dependency
//!
//! Replay correctness depends on `BlockExecutor` being deterministic
//! given the same input intents and starting state. CE-MVCC OCC's
//! determinism (S4) is the key invariant; the property tests in
//! `cross_parity` and `recovery` exercise it across both live and
//! replayed paths.

use super::block::{BlockHash, GENESIS_PARENT};
use super::store::{BlockStore, BlockStoreError};
use crate::{BlockExecutor, ContractRegistry};
use suwappudb_state::{Commitment, State, StateTree};

/// Reasons replay can fail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecoveryError {
    /// Storage layer failed while reading replay data.
    Storage(BlockStoreError),
    /// Block at `height` has a parent hash that doesn't match the
    /// previous block's hash.
    ParentHashMismatch {
        /// Height of the block whose parent is wrong.
        height: u64,
        /// What the block claimed.
        got: BlockHash,
        /// What the previous block hashes to.
        expected: BlockHash,
    },
    /// Re-executing block at `height` produced a different state root
    /// than the recorded one.
    StateRootMismatch {
        /// Height of the divergent block.
        height: u64,
        /// Recorded state root.
        recorded: Commitment,
        /// State root computed from re-execution.
        computed: Commitment,
    },
    /// Block heights aren't contiguous starting from `from`.
    HeightGap {
        /// Expected next height.
        expected: u64,
        /// Got height.
        got: u64,
    },
}

/// Replay every block in `store` starting at `from` (inclusive)
/// against `state`, using `registry` for any [`crate::Intent::Call`]
/// dispatch. Returns the same `state` mutated to the post-replay
/// position.
///
/// `state` should typically be empty (recovery from cold start) or
/// represent the state at height `from - 1` (incremental recovery).
///
/// # Errors
///
/// Returns the first encountered error; no further blocks are
/// processed.
pub fn replay(
    store: &dyn BlockStore,
    state: &mut State,
    registry: &ContractRegistry,
    from: u64,
) -> Result<(), RecoveryError> {
    let blocks = store.iter_from(from).map_err(RecoveryError::Storage)?;
    let mut prev_hash = if from == 0 {
        GENESIS_PARENT
    } else {
        // Caller is doing incremental recovery from a non-zero start;
        // the parent of `from` must come from outside this call. Use
        // GENESIS_PARENT as a placeholder — caller is responsible for
        // ensuring `state` matches the start condition.
        store
            .get_by_height(from - 1)
            .map_err(RecoveryError::Storage)?
            .map_or(GENESIS_PARENT, |b| b.hash())
    };

    for (expected_height, block) in (from..).zip(blocks) {
        if block.height != expected_height {
            return Err(RecoveryError::HeightGap {
                expected: expected_height,
                got: block.height,
            });
        }
        if block.parent != prev_hash {
            return Err(RecoveryError::ParentHashMismatch {
                height: block.height,
                got: block.parent,
                expected: prev_hash,
            });
        }

        // Re-execute the block.
        let report = BlockExecutor.execute_with_registry(state, &block.intents, registry);
        // The block executor already computed its own state root
        // (S6 integration). Cross-check with the recorded one.
        if report.state_root != block.state_root {
            return Err(RecoveryError::StateRootMismatch {
                height: block.height,
                recorded: block.state_root,
                computed: report.state_root,
            });
        }
        // Defence in depth: also verify the post-replay state's tree
        // root agrees. Catches the (impossible by construction) case
        // where BlockReport.state_root drifts from State::tree.
        let live_root = StateTree::from_state(state).root();
        if live_root != block.state_root {
            return Err(RecoveryError::StateRootMismatch {
                height: block.height,
                recorded: block.state_root,
                computed: live_root,
            });
        }

        prev_hash = block.hash();
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recovery::block::Block;
    use crate::recovery::store::{BlockStore, BlockStoreError, InMemoryBlockStore};
    use crate::Intent;
    use suwappudb_state::{Address, Balance, BridgeToken, StateChange};

    fn addr(byte: u8) -> Address {
        Address([byte; 20])
    }

    fn seeded_state() -> State {
        let mut state = State::default();
        let token = BridgeToken::__for_bridge_only();
        for n in 0..8u8 {
            state.apply(
                &token,
                &StateChange::SetBalance {
                    addr: Address([n; 20]),
                    to: Balance(1_000),
                },
            );
        }
        state
    }

    /// Run a block live, capture (intents, `state_root`), record it, then
    /// re-execute via replay against a fresh seeded state — they must
    /// converge.
    #[test]
    fn replay_reproduces_live_state_single_block() {
        let mut live = seeded_state();
        let intents = vec![Intent::Transfer {
            from: addr(0),
            to: addr(1),
            amount: 100,
        }];
        let report = BlockExecutor.execute(&mut live, &intents);

        let mut store = InMemoryBlockStore::new();
        store
            .put(Block {
                height: 0,
                parent: GENESIS_PARENT,
                state_root: report.state_root,
                intents,
            })
            .unwrap();

        let mut replayed = seeded_state();
        replay(&store, &mut replayed, &ContractRegistry::new(), 0).unwrap();

        for n in 0..8u8 {
            assert_eq!(
                live.balance_of(&Address([n; 20])),
                replayed.balance_of(&Address([n; 20]))
            );
        }
    }

    #[test]
    fn replay_reproduces_live_state_multi_block() {
        let mut live = seeded_state();
        let mut store = InMemoryBlockStore::new();
        let mut prev = GENESIS_PARENT;

        for height in 0..5u64 {
            let intents = vec![Intent::Transfer {
                from: addr(0),
                to: addr(1),
                amount: 10,
            }];
            let report = BlockExecutor.execute(&mut live, &intents);
            let block = Block {
                height,
                parent: prev,
                state_root: report.state_root,
                intents,
            };
            prev = block.hash();
            store.put(block).unwrap();
        }

        let mut replayed = seeded_state();
        replay(&store, &mut replayed, &ContractRegistry::new(), 0).unwrap();

        for n in 0..8u8 {
            assert_eq!(
                live.balance_of(&Address([n; 20])),
                replayed.balance_of(&Address([n; 20])),
                "addr {n} divergence"
            );
        }
    }

    #[test]
    fn replay_detects_tampered_state_root() {
        let mut live = seeded_state();
        let intents = vec![Intent::Transfer {
            from: addr(0),
            to: addr(1),
            amount: 100,
        }];
        let report = BlockExecutor.execute(&mut live, &intents);

        let mut store = InMemoryBlockStore::new();
        // Tamper: record a deliberately wrong state root.
        store
            .put(Block {
                height: 0,
                parent: GENESIS_PARENT,
                state_root: Commitment([0xff; 32]),
                intents,
            })
            .unwrap();
        // Use `report.state_root` to silence unused warning — the
        // tamper here is independent of the live one.
        let _ = report.state_root;

        let mut replayed = seeded_state();
        let err = replay(&store, &mut replayed, &ContractRegistry::new(), 0).unwrap_err();
        match err {
            RecoveryError::StateRootMismatch { height, .. } => assert_eq!(height, 0),
            other => panic!("expected StateRootMismatch, got {other:?}"),
        }
    }

    #[test]
    fn replay_detects_height_gap() {
        let mut live = seeded_state();
        let intents = vec![Intent::Transfer {
            from: addr(0),
            to: addr(1),
            amount: 10,
        }];
        let report = BlockExecutor.execute(&mut live, &intents);

        let mut store = InMemoryBlockStore::new();
        // Skip height 0; insert at height 5.
        store
            .put(Block {
                height: 5,
                parent: GENESIS_PARENT,
                state_root: report.state_root,
                intents,
            })
            .unwrap();

        let mut replayed = seeded_state();
        let err = replay(&store, &mut replayed, &ContractRegistry::new(), 0).unwrap_err();
        match err {
            RecoveryError::HeightGap { expected, got } => {
                assert_eq!(expected, 0);
                assert_eq!(got, 5);
            }
            other => panic!("expected HeightGap, got {other:?}"),
        }
    }

    #[test]
    fn replay_detects_broken_parent_chain() {
        // Build two blocks; mutate block 1's parent to a wrong hash.
        let mut live = seeded_state();
        let mut store = InMemoryBlockStore::new();
        let intents_a = vec![Intent::Transfer {
            from: addr(0),
            to: addr(1),
            amount: 10,
        }];
        let report_a = BlockExecutor.execute(&mut live, &intents_a);
        let block_a = Block {
            height: 0,
            parent: GENESIS_PARENT,
            state_root: report_a.state_root,
            intents: intents_a,
        };
        store.put(block_a.clone()).unwrap();

        let intents_b = vec![Intent::Transfer {
            from: addr(2),
            to: addr(3),
            amount: 10,
        }];
        let report_b = BlockExecutor.execute(&mut live, &intents_b);
        let block_b = Block {
            height: 1,
            parent: BlockHash([0xee; 32]), // broken
            state_root: report_b.state_root,
            intents: intents_b,
        };
        store.put(block_b).unwrap();

        let mut replayed = seeded_state();
        let err = replay(&store, &mut replayed, &ContractRegistry::new(), 0).unwrap_err();
        match err {
            RecoveryError::ParentHashMismatch { height, .. } => assert_eq!(height, 1),
            other => panic!("expected ParentHashMismatch, got {other:?}"),
        }
    }

    #[test]
    fn replay_idempotent_under_repeat() {
        let mut live = seeded_state();
        let intents = vec![Intent::Transfer {
            from: addr(0),
            to: addr(1),
            amount: 50,
        }];
        let report = BlockExecutor.execute(&mut live, &intents);

        let mut store = InMemoryBlockStore::new();
        store
            .put(Block {
                height: 0,
                parent: GENESIS_PARENT,
                state_root: report.state_root,
                intents,
            })
            .unwrap();

        let mut a = seeded_state();
        replay(&store, &mut a, &ContractRegistry::new(), 0).unwrap();
        let mut b = seeded_state();
        replay(&store, &mut b, &ContractRegistry::new(), 0).unwrap();

        for n in 0..8u8 {
            assert_eq!(
                a.balance_of(&Address([n; 20])),
                b.balance_of(&Address([n; 20]))
            );
        }
    }

    #[derive(Default)]
    struct FailingStore;

    impl BlockStore for FailingStore {
        fn put(&mut self, _block: Block) -> Result<(), BlockStoreError> {
            Err(BlockStoreError::Backend("injected put failure".into()))
        }
        fn get_by_hash(&self, _hash: &BlockHash) -> Result<Option<Block>, BlockStoreError> {
            Err(BlockStoreError::Backend(
                "injected get_by_hash failure".into(),
            ))
        }
        fn get_by_height(&self, _height: u64) -> Result<Option<Block>, BlockStoreError> {
            Err(BlockStoreError::Backend(
                "injected get_by_height failure".into(),
            ))
        }
        fn latest(&self) -> Result<Option<Block>, BlockStoreError> {
            Err(BlockStoreError::Backend("injected latest failure".into()))
        }
        fn len(&self) -> Result<usize, BlockStoreError> {
            Err(BlockStoreError::Backend("injected len failure".into()))
        }
        fn iter_from(&self, _from: u64) -> Result<Vec<Block>, BlockStoreError> {
            Err(BlockStoreError::Backend(
                "injected iter_from failure".into(),
            ))
        }
    }

    #[test]
    fn replay_surfaces_storage_error_from_iter() {
        let store = FailingStore;
        let mut state = seeded_state();
        let err = replay(&store, &mut state, &ContractRegistry::new(), 0).unwrap_err();
        match err {
            RecoveryError::Storage(BlockStoreError::Backend(msg)) => {
                assert!(msg.contains("iter_from"));
            }
            other => panic!("expected storage error, got {other:?}"),
        }
    }
}
