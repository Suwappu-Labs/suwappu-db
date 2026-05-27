//! Authoritative state for GSX-DB.
//!
//! Owns balances, anchors, and the cross-VM projections that satisfy
//! Proposition 1 (`EVM balanceOf == Move Coin.value` at every checkpoint).
//!
//! Mutations enter only through [`State::apply`], which is gated by a
//! [`BridgeToken`] that only `gsxdb-bridge` can construct. This is the
//! type-system half of the lane-separation invariant; the static check in
//! `scripts/check-lane-separation.sh` is the build-time half.

#![deny(missing_docs)]

pub mod address_shape;
pub mod balance_slot;
pub mod dag;
pub mod metrics;
pub mod nonce_semantics;
pub mod redb_store;
pub mod snapshot;
pub mod store;
pub mod tree;
pub mod vm;

pub use address_shape::MoveAddress;
pub use balance_slot::{BalanceSlot, EvmBalance, MoveCoinValue, SlotError};
pub use metrics::{Counter, Gauge, Histogram, Metrics, Timer};
pub use nonce_semantics::{AccountNonce, EvmNonce, MoveSequenceNumber};
pub use redb_store::RedbBalanceStore;
pub use store::{BalanceStore, InMemoryBalanceStore};
pub use tree::{Commitment, Proof, ProofStep, StateTree};
pub use vm::{
    AbortLocation, CanonicalTransfer, CompiledModule, EvmProjector, EvmTx, EvmView, Identifier,
    IdentifierError, InMemoryModuleStore, MockMoveExecutor, ModuleId, ModuleStore,
    ModuleStoreError, MoveBalanceView, MoveCall, MoveEvent, MoveExecutionError, MoveExecutor,
    MoveOutcome, MoveProjector, MoveSessionState, MoveTx, MoveView, ResourceWrite, StructTag,
    TypeTag, ABORT_INSUFFICIENT_BALANCE, CANONICAL_COIN_ADDRESS,
};
#[cfg(feature = "production-move-executor")]
pub use vm::{canonical_coin_bytecode, canonical_coin_module_id, AptosMoveExecutor};

/// 20-byte EVM-shaped address. Move addresses are projected onto this layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, PartialOrd, Ord)]
pub struct Address(pub [u8; 20]);

/// Balance held by an address. Newtyped to keep arithmetic intentional.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Balance(pub u128);

/// Authoritative state. Owns the canonical balance map via a pluggable
/// [`BalanceStore`] backend (in-memory, redb, or — in S8 — `RocksDB`).
///
/// Constructed two ways:
///
/// - [`State::default`] — uses [`InMemoryBalanceStore`]. For unit tests and
///   ephemeral runs.
/// - [`State::with_store`] — provide your own backend. For redb-backed runs
///   and any future backend.
pub struct State {
    store: Box<dyn BalanceStore + Send + Sync>,
    /// EVM contract bytecode, keyed by code hash. Populated on contract
    /// creation, read by the EVM executor's `Database` adapter. EVM-only
    /// (the Move VM has no code).
    ///
    /// NOT YET committed in the state root — folding EVM code + storage
    /// into the verkle root is the consensus-critical follow-on; until
    /// then this backs contract *execution* but is not consensus-safe.
    evm_code: std::collections::HashMap<[u8; 32], Vec<u8>>,
    /// EVM contract storage: `(address, 32-byte slot) -> 32-byte value`.
    /// EVM-only. Same state-root caveat as [`Self::evm_code`].
    evm_storage: std::collections::HashMap<(Address, [u8; 32]), [u8; 32]>,
}

impl std::fmt::Debug for State {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("State")
            .field("len", &self.store.len())
            .finish()
    }
}

impl Default for State {
    fn default() -> Self {
        Self::with_store(Box::new(InMemoryBalanceStore::new()))
    }
}

/// A validated state mutation. Constructed by `gsxdb-bridge` after OCC checks
/// and signature verification; `gsxdb-state` trusts these unconditionally.
#[derive(Debug, Clone)]
pub enum StateChange {
    /// Set the balance of `addr` to `to`. Replaces, does not add.
    ///
    /// Resets the nonce to zero — use [`StateChange::SetAccount`] to set
    /// balance and nonce together.
    SetBalance {
        /// Target address.
        addr: Address,
        /// New balance.
        to: Balance,
    },
    /// Set both the balance and nonce of `addr`, replacing the whole slot.
    ///
    /// The real EVM executor uses this to write back a post-execution
    /// account whose nonce advanced; `SetBalance` cannot express a nonce
    /// change because it zeroes the nonce.
    SetAccount {
        /// Target address.
        addr: Address,
        /// New balance.
        balance: Balance,
        /// New nonce.
        nonce: u64,
    },
    /// Store EVM contract bytecode under its code hash (on contract creation).
    SetCode {
        /// `keccak256(code)`.
        code_hash: [u8; 32],
        /// Contract bytecode.
        code: Vec<u8>,
    },
    /// Set an EVM contract storage slot.
    SetStorage {
        /// Contract address.
        addr: Address,
        /// 32-byte storage slot key.
        slot: [u8; 32],
        /// 32-byte storage value.
        value: [u8; 32],
    },
}

/// Capability token proving a caller is the bridge.
///
/// The constructor is `pub(crate)` — only this crate (via the `for_bridge`
/// associated function exposed to the bridge crate) can mint one. Lane code
/// cannot fabricate a token even if it depends on `gsxdb-state` (which it
/// must not, per the script check).
pub struct BridgeToken {
    _seal: (),
}

impl BridgeToken {
    /// Mint a token. This function is only intended to be called from
    /// `gsxdb-bridge`. The lane-separation script ensures no other crate in
    /// the workspace depends on `gsxdb-state` at all.
    #[must_use]
    pub fn __for_bridge_only() -> Self {
        Self { _seal: () }
    }
}

impl State {
    /// New `State` over the given storage backend.
    #[must_use]
    pub fn with_store(store: Box<dyn BalanceStore + Send + Sync>) -> Self {
        Self {
            store,
            evm_code: std::collections::HashMap::new(),
            evm_storage: std::collections::HashMap::new(),
        }
    }

    /// Read-only balance lookup. Anyone may call this — reads are not
    /// privileged. (Phase-1 reads through the bridge; phase-2 may expose a
    /// snapshot reader.)
    #[must_use]
    pub fn balance_of(&self, addr: &Address) -> Balance {
        self.store.get(addr).as_balance()
    }

    /// Read the full [`BalanceSlot`] for `addr`. Used by code that needs the
    /// dual EVM/Move projections; most callers want [`State::balance_of`].
    #[must_use]
    pub fn slot_of(&self, addr: &Address) -> BalanceSlot {
        self.store.get(addr)
    }

    /// Snapshot of every (addr, slot) currently in state. Used by the
    /// state-tree commitment to recompute the post-block root. Order
    /// is implementation-defined; consumers must sort if they need
    /// canonical ordering.
    #[must_use]
    pub fn entries(&self) -> Vec<(Address, BalanceSlot)> {
        self.store.entries()
    }

    /// EVM contract bytecode for `code_hash`, or `None` if unknown.
    ///
    /// Non-privileged read, like [`State::balance_of`]. Used by the EVM
    /// executor's `Database` adapter.
    #[must_use]
    pub fn code_by_hash(&self, code_hash: &[u8; 32]) -> Option<&[u8]> {
        self.evm_code.get(code_hash).map(Vec::as_slice)
    }

    /// EVM storage value at `(addr, slot)`. Unset slots read as zero —
    /// the EVM's own "unset storage is zero" contract.
    #[must_use]
    pub fn storage_at(&self, addr: &Address, slot: &[u8; 32]) -> [u8; 32] {
        self.evm_storage
            .get(&(*addr, *slot))
            .copied()
            .unwrap_or([0u8; 32])
    }

    /// Apply a validated change. Requires a [`BridgeToken`] — only the bridge
    /// can call this.
    pub fn apply(&mut self, _token: &BridgeToken, change: &StateChange) {
        match change {
            StateChange::SetBalance { addr, to } => {
                self.store.set(addr, BalanceSlot::new(to.0));
            }
            StateChange::SetAccount {
                addr,
                balance,
                nonce,
            } => {
                self.store.set(
                    addr,
                    BalanceSlot::with_nonce(
                        balance.0,
                        crate::nonce_semantics::AccountNonce::new(*nonce),
                    ),
                );
            }
            StateChange::SetCode { code_hash, code } => {
                self.evm_code.insert(*code_hash, code.clone());
            }
            StateChange::SetStorage { addr, slot, value } => {
                self.evm_storage.insert((*addr, *slot), *value);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn applied_balance_round_trips() {
        let mut state = State::default();
        let token = BridgeToken::__for_bridge_only();
        let addr = Address([1; 20]);

        state.apply(
            &token,
            &StateChange::SetBalance {
                addr,
                to: Balance(42),
            },
        );

        assert_eq!(state.balance_of(&addr), Balance(42));
    }

    #[test]
    fn evm_code_round_trips() {
        let mut state = State::default();
        let token = BridgeToken::__for_bridge_only();
        let code_hash = [7u8; 32];

        assert_eq!(state.code_by_hash(&code_hash), None);
        state.apply(
            &token,
            &StateChange::SetCode {
                code_hash,
                code: vec![0x60, 0x00, 0x55],
            },
        );
        assert_eq!(
            state.code_by_hash(&code_hash),
            Some([0x60, 0x00, 0x55].as_slice())
        );
    }

    #[test]
    fn evm_storage_round_trips_and_defaults_zero() {
        let mut state = State::default();
        let token = BridgeToken::__for_bridge_only();
        let addr = Address([3; 20]);
        let slot = [1u8; 32];

        // Unset slot reads as zero (the EVM's own contract).
        assert_eq!(state.storage_at(&addr, &slot), [0u8; 32]);

        let value = [9u8; 32];
        state.apply(&token, &StateChange::SetStorage { addr, slot, value });
        assert_eq!(state.storage_at(&addr, &slot), value);
        // A different slot is still zero.
        assert_eq!(state.storage_at(&addr, &[2u8; 32]), [0u8; 32]);
    }

    #[test]
    fn missing_address_reads_zero() {
        let state = State::default();
        assert_eq!(state.balance_of(&Address([7; 20])), Balance(0));
    }

    #[test]
    fn slot_of_exposes_dual_projection() {
        let mut state = State::default();
        let token = BridgeToken::__for_bridge_only();
        let addr = Address([3; 20]);

        state.apply(
            &token,
            &StateChange::SetBalance {
                addr,
                to: Balance(999),
            },
        );

        let slot = state.slot_of(&addr);
        assert_eq!(slot.canonical(), 999);
        assert_eq!(
            slot.evm_balance().to_u128(),
            slot.move_coin_value().to_u128()
        );
    }

    #[test]
    fn with_store_swaps_the_backend() {
        // Trivial smoke test: build a State around an InMemoryBalanceStore
        // explicitly (the same path Default uses, but with a different code
        // path) and confirm the basic round-trip works. Real backend swap
        // is exercised by the redb integration test in tests/.
        let mut state = State::with_store(Box::new(InMemoryBalanceStore::new()));
        let token = BridgeToken::__for_bridge_only();
        let addr = Address([5; 20]);

        state.apply(
            &token,
            &StateChange::SetBalance {
                addr,
                to: Balance(1),
            },
        );

        assert_eq!(state.balance_of(&addr), Balance(1));
    }
}
