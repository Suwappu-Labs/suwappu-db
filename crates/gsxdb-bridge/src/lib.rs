//! The bridge between `gsxdb-lane` (untrusted data ingest) and `gsxdb-state`
//! (authoritative state).
//!
//! Lane code submits [`Intent`]s. The bridge validates them — signature
//! checks, OCC conflict checks, balance checks — and on success produces a
//! [`gsxdb_state::StateChange`] that is applied through a `BridgeToken` only
//! this crate can mint.
//!
//! Phase-1 implementation is intentionally thin. S3 (CE-MVCC + OCC) and S4
//! (cross-VM intent queue) extend [`Bridge::submit`] with real validation.

#![deny(missing_docs)]
// HARDENING rec 2.3 — Wormhole 2022 ($326M) traced to a deprecated
// `load_instruction_at` accepting a forged sysvar account. The
// gsxdb-bridge crate is the only capability-gated mutation surface;
// deny deprecated functions at the crate level so a future
// "legacy verify" or "unchecked variant" can't slip past review.
// Halborn: https://www.halborn.com/blog/post/explained-the-wormhole-hack-february-2022
#![deny(deprecated)]

pub mod anchor;
pub mod bundle;
pub mod occ;
pub mod recovery;
pub mod sync;
pub mod telemetry;
pub mod vm;

pub use anchor::{
    Anchor, AnchorDispatcher, AnchorHash, AnchorLog, AppendError, ChainId, ParityResult,
    GENESIS_PARENT,
};
pub use bundle::{
    Bundle, BundleExecutor, BundleGenerator, BundleOutcome, BundleResult, BundleStep, CallCtx,
    ContractRegistry,
};
pub use occ::{BlockExecutor, BlockReport, TxOutcome};
pub use recovery::{
    replay, Block, BlockHash, BlockStore, InMemoryBlockStore, RecoveryError, RedbBlockStore,
};
pub use sync::{L2StateSyncer, L2SyncConfig};
pub use telemetry::{
    record_block_metrics, record_parity_metrics, record_state_metrics, AnchorTimer, BlockTimer,
    ParityTimer,
};
pub use vm::{EvmError, MockEvm, MockMove, MoveError};

use gsxdb_state::{Address, Balance, BridgeToken, State, StateChange};

/// An untrusted intent submitted from the lane.
///
/// Not `Copy` — `Call` carries a `Vec<u8>` for calldata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Intent {
    /// Transfer `amount` from `from` to `to`. Source must hold ≥ `amount`.
    Transfer {
        /// Source address.
        from: Address,
        /// Destination address.
        to: Address,
        /// Amount in wei-equivalent units.
        amount: u128,
    },
    /// Invoke the contract at `target` with `calldata` and `value`. Dispatched
    /// through the [`ContractRegistry`] at block-execution time. If `target`
    /// isn't a registered contract, the call falls back to a plain transfer
    /// of `value` from `caller` to `target`.
    ///
    /// Bridge-level [`Bridge::submit`] does NOT handle `Call` — only the
    /// block executor does, because dispatch needs registry access. Calling
    /// `Bridge::submit` with a `Call` returns
    /// [`RejectReason::CallRequiresRegistry`].
    Call {
        /// Originating account (EOA for top-level, parent contract for sub-calls).
        caller: Address,
        /// Contract address being called. May or may not be a registered contract.
        target: Address,
        /// Native value passed with the call.
        value: u128,
        /// Opaque payload. Phase-1 mock contracts agree on shape; real
        /// revm parses ABI.
        calldata: Vec<u8>,
    },
    /// Deploy a Move module to the substrate's [`ModuleStore`]. S9.3.
    ///
    /// The module is keyed by `(account, name)`; re-deploys are rejected
    /// (`ModuleStoreError::AlreadyExists`). Upgrades are a separate
    /// surface — deferred per `docs/spec/move-execution.md` open
    /// question on Aptos `compatible` upgrade policy.
    ///
    /// `bytes` is opaque BCS-encoded Move bytecode. Verifier runs at
    /// deploy time in S9.5 (currently passthrough; the verifier needs
    /// `move-bytecode-verifier`). The lane carries any byte sequence;
    /// the bundle executor + verifier reject malformed bytecode.
    DeployModule {
        /// Originating Move account (32-byte). Must match the
        /// `address` field of the deployed module's `ModuleId`.
        account: gsxdb_state::MoveAddress,
        /// Module name. Validated as a Move [`Identifier`] before
        /// reaching the lane.
        name: gsxdb_state::Identifier,
        /// Opaque BCS-encoded bytecode.
        bytes: Vec<u8>,
    },
}

/// Reasons an intent can be rejected during validation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RejectReason {
    /// Source balance is below the requested transfer amount.
    InsufficientBalance,
    /// Transfer amount overflowed `u128` arithmetic. Phase-2 will use a
    /// 256-bit type internally.
    AmountOverflow,
    /// `Bridge::submit` was called with `Intent::Call`. Calls require a
    /// `ContractRegistry`, which only the block executor holds. Lift the
    /// call into a block and use [`crate::BlockExecutor`].
    CallRequiresRegistry,
    /// `Bridge::submit` was called with `Intent::DeployModule`. Module
    /// deploys require a `ModuleStore`, which only the bundle executor
    /// holds. Lift the deploy into a block and use the bundle executor.
    DeployModuleRequiresModuleStore,
    /// A bundle step was a `MoveCall` or `DeployModule` but the bundle
    /// executor was invoked through `execute` (without a Move runtime).
    /// Use `BundleExecutor::execute_with_move_runtime` for bundles
    /// containing Move-VM-bound steps.
    MoveRuntimeRequired,
    /// A `MoveCall` step's executor returned an error (typed via
    /// `MoveExecutionError`). The textual form is in the variant.
    MoveCallFailed(String),
    /// A `DeployModule` step's `ModuleStore::put` rejected the deploy
    /// (already-exists or backend failure). The textual form is in
    /// the variant.
    ModuleDeployFailed(String),
    /// A post-execution EVM balance exceeded `u128::MAX` and cannot be
    /// represented in the canonical balance map. The transaction reverts
    /// rather than saturating — saturating would debit a sender without
    /// fully crediting the recipient, breaking balance conservation.
    BalanceOverflow,
}

/// Wraps a mutable [`State`] reference and offers the only validated path to
/// mutate it. Hold one for the duration of a transaction.
pub struct Bridge<'s> {
    state: &'s mut State,
    token: BridgeToken,
}

impl<'s> Bridge<'s> {
    /// Open a bridge over the given state. Cheap; intended to be created
    /// per-transaction.
    pub fn new(state: &'s mut State) -> Self {
        Self {
            state,
            token: BridgeToken::__for_bridge_only(),
        }
    }

    /// Read-through to state. Lane code uses this for balance lookups so it
    /// never needs to hold a `&State` directly.
    #[must_use]
    pub fn balance_of(&self, addr: &Address) -> Balance {
        self.state.balance_of(addr)
    }

    /// Read-through to the `bytes_state` registry. Protocol code uses this
    /// to read a registry blob (e.g. the authority / validator stake
    /// registry) without holding a `&State` directly. The write side is
    /// [`Bridge::write_bytes`]; this is the matching read, mirroring
    /// [`Bridge::balance_of`] for balances.
    #[must_use]
    pub fn read_bytes(&self, addr: &Address) -> Option<&[u8]> {
        self.state.read_bytes(addr)
    }

    /// Validate an intent and, if it passes, apply it to state atomically.
    ///
    /// On `Err`, no state mutation occurs.
    ///
    /// # Errors
    ///
    /// Returns [`RejectReason::InsufficientBalance`] when the source balance
    /// is below the requested transfer amount, and
    /// [`RejectReason::AmountOverflow`] when the destination balance would
    /// overflow `u128`.
    #[allow(clippy::needless_pass_by_value)] // by-value communicates "consumed intent"
    pub fn submit(&mut self, intent: Intent) -> Result<(), RejectReason> {
        match intent {
            Intent::Call { .. } => Err(RejectReason::CallRequiresRegistry),
            Intent::DeployModule { .. } => {
                Err(RejectReason::DeployModuleRequiresModuleStore)
            }
            Intent::Transfer { from, to, amount } => {
                let from_balance = self.state.balance_of(&from).0;

                if from_balance < amount {
                    return Err(RejectReason::InsufficientBalance);
                }

                // Self-transfer is a structural no-op. Without this
                // guard the two `apply` calls below race: the second
                // (credit) overwrites the first (debit) and leaves the
                // address at `balance + amount`. We still validate the
                // balance above so the error surface stays consistent
                // with non-self transfers.
                if from == to {
                    return Ok(());
                }

                let to_balance = self.state.balance_of(&to).0;
                let new_to = to_balance
                    .checked_add(amount)
                    .ok_or(RejectReason::AmountOverflow)?;
                let new_from = from_balance - amount;

                self.state.apply(
                    &self.token,
                    &StateChange::SetBalance {
                        addr: from,
                        to: Balance(new_from),
                    },
                );
                self.state.apply(
                    &self.token,
                    &StateChange::SetBalance {
                        addr: to,
                        to: Balance(new_to),
                    },
                );
                Ok(())
            }
        }
    }

    /// Set an account's balance and nonce wholesale.
    ///
    /// The real EVM executor's write-back path: revm returns absolute
    /// post-execution account state, and a value transfer advances the
    /// sender's nonce — which `submit`'s `SetBalance` cannot persist (it
    /// zeroes the nonce).
    ///
    /// `pub(crate)`: this is an UNVALIDATED raw mutation (it sets an
    /// absolute balance/nonce, bypassing `submit`'s balance checks). Only
    /// the in-crate `revm_executor` may call it, after revm has produced a
    /// validated post-state. Exposing it publicly would let any `Bridge`
    /// holder mint/burn or rewrite nonces, defeating lane separation.
    ///
    /// Gated on `production-evm-executor`: its only caller is the in-crate
    /// `revm_executor` (itself behind that feature), so without it the
    /// method would be dead code and trip `-D warnings`.
    #[cfg(feature = "production-evm-executor")]
    pub(crate) fn set_account(&mut self, addr: Address, balance: Balance, nonce: u64) {
        self.state.apply(
            &self.token,
            &StateChange::SetAccount {
                addr,
                balance,
                nonce,
            },
        );
    }

    /// Store EVM contract bytecode under its code hash (contract-creation
    /// write-back). Mechanism only; same authorization model as
    /// [`Bridge::set_account`]. `pub(crate)` + feature-gated for the same
    /// reason: only the in-crate `revm_executor` writes contract state back,
    /// and these are unvalidated raw mutations.
    #[cfg(feature = "production-evm-executor")]
    pub(crate) fn set_code(&mut self, code_hash: [u8; 32], code: Vec<u8>) {
        self.state
            .apply(&self.token, &StateChange::SetCode { code_hash, code });
    }

    /// Point an account at the contract code it runs (contract-creation
    /// write-back). Pairs with [`Bridge::set_code`].
    #[cfg(feature = "production-evm-executor")]
    pub(crate) fn set_account_code(&mut self, addr: Address, code_hash: [u8; 32]) {
        self.state
            .apply(&self.token, &StateChange::SetAccountCode { addr, code_hash });
    }

    /// Set an EVM contract storage slot (execution write-back).
    #[cfg(feature = "production-evm-executor")]
    pub(crate) fn set_storage(&mut self, addr: Address, slot: [u8; 32], value: [u8; 32]) {
        self.state.apply(
            &self.token,
            &StateChange::SetStorage { addr, slot, value },
        );
    }

    /// Write a reserved-address bytes-state record (L2 verifying key, DA
    /// anchor, governance registry) — the surface gsx-dag's
    /// `GsxDbSubstrate` needs to un-stub its L2 / DA / governance intent
    /// arms. Mechanism only; same authorization model as
    /// [`Bridge::set_account`].
    pub fn write_bytes(&mut self, addr: Address, bytes: Vec<u8>) {
        self.state
            .apply(&self.token, &StateChange::SetBytes { addr, bytes });
    }

    /// Protocol-owned credit: add `amount` to `addr`'s balance with no
    /// debiting counterparty.
    ///
    /// This is the mechanism behind **genesis seeding** and protocol
    /// payouts (inflation, ring rewards, insurance, treasury, slashing
    /// distribution) — cases that [`Bridge::submit`] (transfer-only)
    /// cannot express and that gsx-dag's in-memory substrate handles via a
    /// private `credit_unchecked` path.
    ///
    /// **Authorization is the caller's responsibility.** Like
    /// [`Bridge::submit`], this is pure mechanism: lane separation still
    /// holds (the mutation goes through the [`BridgeToken`] gate), but
    /// whether a credit is legitimate is decided upstream. In gsx-dag only
    /// consensus-authorized, daemon-emitted intents (`GenesisAllocation`,
    /// `MintInflation`, `DistributeRewards`, slashing distribution) reach
    /// this method; user transactions never do.
    ///
    /// # Errors
    ///
    /// [`RejectReason::AmountOverflow`] if the resulting balance would
    /// exceed `u128`. A zero `amount` is a no-op.
    pub fn credit(&mut self, addr: Address, amount: Balance) -> Result<(), RejectReason> {
        if amount.0 == 0 {
            return Ok(());
        }
        let new = self
            .state
            .balance_of(&addr)
            .0
            .checked_add(amount.0)
            .ok_or(RejectReason::AmountOverflow)?;
        self.state.apply(
            &self.token,
            &StateChange::SetBalance {
                addr,
                to: Balance(new),
            },
        );
        Ok(())
    }

    /// Protocol-owned debit: remove `amount` from `addr`'s balance with no
    /// crediting counterparty (slashing burn, escrow / bond drain).
    ///
    /// Same authorization model as [`Bridge::credit`] — mechanism only.
    ///
    /// # Errors
    ///
    /// [`RejectReason::InsufficientBalance`] if `addr` holds less than
    /// `amount`. A zero `amount` is a no-op.
    pub fn debit(&mut self, addr: Address, amount: Balance) -> Result<(), RejectReason> {
        if amount.0 == 0 {
            return Ok(());
        }
        let current = self.state.balance_of(&addr).0;
        if current < amount.0 {
            return Err(RejectReason::InsufficientBalance);
        }
        self.state.apply(
            &self.token,
            &StateChange::SetBalance {
                addr,
                to: Balance(current - amount.0),
            },
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gsxdb_state::BridgeToken;

    fn seeded_state(addr: Address, amount: u128) -> State {
        let mut state = State::default();
        let token = BridgeToken::__for_bridge_only();
        state.apply(
            &token,
            &StateChange::SetBalance {
                addr,
                to: Balance(amount),
            },
        );
        state
    }

    #[test]
    fn transfer_moves_funds() {
        let alice = Address([1; 20]);
        let bob = Address([2; 20]);
        let mut state = seeded_state(alice, 100);

        let mut bridge = Bridge::new(&mut state);
        bridge
            .submit(Intent::Transfer {
                from: alice,
                to: bob,
                amount: 30,
            })
            .unwrap();

        assert_eq!(bridge.balance_of(&alice), Balance(70));
        assert_eq!(bridge.balance_of(&bob), Balance(30));
    }

    #[test]
    fn self_transfer_is_a_no_op() {
        // Regression test for a bug found by S4's property test:
        // without the from==to guard, the credit-write of `to`
        // overwrites the debit-write of `from`, inflating the balance.
        let alice = Address([1; 20]);
        let mut state = seeded_state(alice, 100);

        let mut bridge = Bridge::new(&mut state);
        bridge
            .submit(Intent::Transfer {
                from: alice,
                to: alice,
                amount: 30,
            })
            .unwrap();

        assert_eq!(bridge.balance_of(&alice), Balance(100));
    }

    #[test]
    fn self_transfer_still_checks_balance() {
        let alice = Address([1; 20]);
        let mut state = seeded_state(alice, 5);

        let mut bridge = Bridge::new(&mut state);
        let result = bridge.submit(Intent::Transfer {
            from: alice,
            to: alice,
            amount: 30,
        });

        assert_eq!(result, Err(RejectReason::InsufficientBalance));
        assert_eq!(bridge.balance_of(&alice), Balance(5));
    }

    #[test]
    fn transfer_rejects_insufficient_balance() {
        let alice = Address([1; 20]);
        let bob = Address([2; 20]);
        let mut state = seeded_state(alice, 5);

        let mut bridge = Bridge::new(&mut state);
        let result = bridge.submit(Intent::Transfer {
            from: alice,
            to: bob,
            amount: 30,
        });

        assert_eq!(result, Err(RejectReason::InsufficientBalance));
        assert_eq!(bridge.balance_of(&alice), Balance(5));
        assert_eq!(bridge.balance_of(&bob), Balance(0));
    }

    #[test]
    fn deploy_module_routes_to_bundle_executor() {
        // S9.3: Bridge::submit cannot deploy modules — that needs a
        // ModuleStore, which only the bundle executor owns. Until
        // S9.4 wires the bundle executor, surface this as a typed
        // rejection so callers learn to use the bundle path.
        let alice = Address([1; 20]);
        let mut state = seeded_state(alice, 100);

        let mut bridge = Bridge::new(&mut state);
        let mut account_bytes = [0u8; 32];
        account_bytes[12..32].copy_from_slice(&alice.0);
        let result = bridge.submit(Intent::DeployModule {
            account: gsxdb_state::MoveAddress(account_bytes),
            name: gsxdb_state::Identifier::new("hello_module").unwrap(),
            bytes: vec![0xCA, 0xFE, 0xBA, 0xBE],
        });

        assert_eq!(result, Err(RejectReason::DeployModuleRequiresModuleStore));
        // No state mutation.
        assert_eq!(bridge.balance_of(&alice), Balance(100));
    }

    #[test]
    fn credit_seeds_a_fresh_address() {
        // Genesis-seeding path: credit from zero with no counterparty —
        // the case `submit` (transfer-only) cannot express.
        let alice = Address([1; 20]);
        let mut state = State::default();

        let mut bridge = Bridge::new(&mut state);
        bridge.credit(alice, Balance(1_000)).unwrap();
        assert_eq!(bridge.balance_of(&alice), Balance(1_000));

        // Credits accumulate.
        bridge.credit(alice, Balance(500)).unwrap();
        assert_eq!(bridge.balance_of(&alice), Balance(1_500));

        // Zero is a no-op.
        bridge.credit(alice, Balance(0)).unwrap();
        assert_eq!(bridge.balance_of(&alice), Balance(1_500));
    }

    #[test]
    fn credit_rejects_overflow() {
        let alice = Address([1; 20]);
        let mut state = seeded_state(alice, u128::MAX);

        let mut bridge = Bridge::new(&mut state);
        let result = bridge.credit(alice, Balance(1));
        assert_eq!(result, Err(RejectReason::AmountOverflow));
        // Balance unchanged on rejection.
        assert_eq!(bridge.balance_of(&alice), Balance(u128::MAX));
    }

    #[test]
    fn debit_reduces_balance_and_guards_underflow() {
        let alice = Address([1; 20]);
        let mut state = seeded_state(alice, 100);

        let mut bridge = Bridge::new(&mut state);
        bridge.debit(alice, Balance(30)).unwrap();
        assert_eq!(bridge.balance_of(&alice), Balance(70));

        // Debiting more than held rejects and leaves the balance intact.
        let result = bridge.debit(alice, Balance(1_000));
        assert_eq!(result, Err(RejectReason::InsufficientBalance));
        assert_eq!(bridge.balance_of(&alice), Balance(70));

        // Zero is a no-op.
        bridge.debit(alice, Balance(0)).unwrap();
        assert_eq!(bridge.balance_of(&alice), Balance(70));
    }
}
