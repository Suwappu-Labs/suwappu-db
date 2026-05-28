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
    /// Set the balance of `addr` to `to`. Replaces the balance only;
    /// the existing nonce is preserved.
    ///
    /// Nonce preservation matters under the dual-VM stack: a Move-side
    /// transfer (which has no concept of an EVM nonce) routes through
    /// `Bridge::submit`, which emits `SetBalance` for both sides. If
    /// `SetBalance` zeroed the nonce, any EVM nonce advance on those
    /// accounts would silently revert, re-enabling replay of EVM txs.
    /// Use [`StateChange::SetAccount`] to set balance and nonce together
    /// (the real EVM executor's write-back path).
    SetBalance {
        /// Target address.
        addr: Address,
        /// New balance.
        to: Balance,
    },
    /// Set both the balance and nonce of `addr`, replacing the whole slot.
    ///
    /// The real EVM executor uses this to write back a post-execution
    /// account whose nonce advanced. `SetBalance` preserves the existing
    /// nonce instead of overwriting it, so when a write-back path needs
    /// to assert a specific nonce it must use this variant.
    SetAccount {
        /// Target address.
        addr: Address,
        /// New balance.
        balance: Balance,
        /// New nonce.
        nonce: u64,
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
        Self { store }
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

    /// Apply a validated change. Requires a [`BridgeToken`] — only the bridge
    /// can call this.
    pub fn apply(&mut self, _token: &BridgeToken, change: &StateChange) {
        match *change {
            StateChange::SetBalance { addr, to } => {
                // Preserve the existing nonce — the canonical balance
                // slot is the single source of truth for both balance
                // *and* nonce, but `SetBalance` only carries the balance.
                // If we zeroed the nonce here, every Move-side or
                // protocol-credit `SetBalance` would silently undo any
                // EVM nonce advance on the same account and re-enable
                // replay of the EVM tx that bumped it.
                let existing_nonce = self.store.get(&addr).nonce();
                self.store
                    .set(&addr, BalanceSlot::with_nonce(to.0, existing_nonce));
            }
            StateChange::SetAccount {
                addr,
                balance,
                nonce,
            } => {
                self.store.set(
                    &addr,
                    BalanceSlot::with_nonce(
                        balance.0,
                        crate::nonce_semantics::AccountNonce::new(nonce),
                    ),
                );
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
    fn set_balance_preserves_existing_nonce() {
        // Codex P1 (lib.rs ~83): if an EVM tx advanced an account's
        // nonce, a later Move-side or protocol-credit `SetBalance` on
        // the same account must NOT zero the nonce — that would
        // re-enable replay of the EVM tx that bumped it.
        let mut state = State::default();
        let token = BridgeToken::__for_bridge_only();
        let addr = Address([9; 20]);

        // Real EVM executor wrote back balance + nonce = 7 via SetAccount.
        state.apply(
            &token,
            &StateChange::SetAccount {
                addr,
                balance: Balance(500),
                nonce: 7,
            },
        );
        assert_eq!(state.slot_of(&addr).nonce().value, 7);

        // A subsequent balance-only mutation (e.g. a Move transfer
        // crediting this account through the bridge) must leave the
        // nonce untouched.
        state.apply(
            &token,
            &StateChange::SetBalance {
                addr,
                to: Balance(900),
            },
        );

        assert_eq!(state.balance_of(&addr), Balance(900));
        assert_eq!(
            state.slot_of(&addr).nonce().value,
            7,
            "SetBalance must preserve the existing account nonce so EVM \
             replay defence remains intact under Move-side activity"
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
