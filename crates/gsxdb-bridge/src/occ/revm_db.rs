//! MV-backed `DatabaseRef` adapter for running real revm inside OCC
//! speculative execution.
//!
//! The standalone [`crate::vm::RevmExecutor::execute`] reads + writes
//! a `&mut State` directly. That's the right shape for non-OCC
//! dispatch (e.g. the bundle executor's `production-evm-executor`
//! path) but doesn't fit OCC: the speculative loop must not mutate
//! canonical state — it accumulates writes in the per-tx MV store
//! and lets the consolidation step apply them. Concurrent revm
//! executions across rayon threads each need their own read view
//! that observes (a) the MV store at their own `TxnIdx`, (b) the
//! intra-bundle local accumulator built up by earlier steps in the
//! same bundle, and (c) the canonical pre-block snapshot for
//! cold-cache addresses.
//!
//! This module provides [`OccRevmDb`] — a `DatabaseRef` that
//! composes those three layers — and [`run_evm_step`], a
//! helper that runs a single [`EvmTx`] through revm against an
//! `OccRevmDb`, returns the read set + write set in OCC shape, and
//! reverts cleanly when revm rejects.
//!
//! Read-set tracking is captured during revm's read calls (via a
//! `RefCell<Vec<ReadEntry>>` since `DatabaseRef` takes `&self`),
//! mirroring the manual-arithmetic path's [`ReadEntry`] shape so
//! the OCC validator can detect invalidations identically under
//! both modes.
//!
//! Scope (matches the standalone `RevmExecutor` path): EVM value
//! transfers only. Contract code + storage land in a follow-up tied
//! to PR #28 (contracts inside OCC need the same MV-backed view for
//! `code_by_hash_ref` / `storage_ref` to read through bytes_state).

#![cfg(feature = "production-evm-executor")]

use core::convert::Infallible;
use std::cell::RefCell;
use std::collections::HashMap;

use gsx_revm::{monad_context_with_db, MonadBuilder};
use revm::{
    bytecode::Bytecode,
    context::TxEnv,
    database::WrapDatabaseRef,
    database_interface::DatabaseRef,
    primitives::{Address as RevmAddress, Bytes, B256, KECCAK_EMPTY, U256},
    state::AccountInfo,
    ExecuteEvm,
};

use gsxdb_state::{AccountNonce, Address, BalanceSlot, EvmTx, State};

use crate::occ::mv_store::{MvStore, TxnIdx};
use crate::occ::txn::ReadEntry;
use crate::RejectReason;

/// Default gas limit for an EVM value transfer (matches
/// [`crate::vm::revm_executor::TRANSFER_GAS_LIMIT`]).
const TRANSFER_GAS_LIMIT: u64 = 21_000;

/// MV-store-backed read view for revm, sized to a single OCC
/// `TxnIdx` and the per-bundle local accumulator owned by the
/// caller.
///
/// `local` is a snapshot reference: the caller holds a `HashMap`
/// that accumulates intra-bundle writes; this adapter consults it
/// before falling through to the MV store. The adapter does not
/// mutate `local` — write-back happens in [`run_evm_step`] after
/// revm returns successfully.
///
/// Reads tracked through this adapter land in [`Self::reads`]
/// (interior-mutable so we can write through `&self`, which
/// `DatabaseRef` requires). The caller drains them into the txn's
/// outer read-set after each successful revm step.
///
/// **Single-threaded use only.** The `RefCell` makes this `!Send`
/// and `!Sync`. OCC's parallelism is at the txn-idx level (each
/// `execute_one` / `execute_call` runs on one rayon worker), so an
/// `OccRevmDb` lives entirely on one thread for one step and never
/// needs to cross a rayon scope. If a future refactor parallelises
/// within a single call, this struct needs an `Arc<Mutex<...>>` on
/// the reads buffer instead of a `RefCell` (or a per-thread
/// adapter).
pub(crate) struct OccRevmDb<'a> {
    state: &'a State,
    mv: &'a MvStore,
    idx: TxnIdx,
    local: &'a HashMap<Address, BalanceSlot>,
    reads: RefCell<Vec<ReadEntry>>,
}

impl<'a> OccRevmDb<'a> {
    pub(crate) fn new(
        state: &'a State,
        mv: &'a MvStore,
        idx: TxnIdx,
        local: &'a HashMap<Address, BalanceSlot>,
    ) -> Self {
        Self {
            state,
            mv,
            idx,
            local,
            reads: RefCell::new(Vec::new()),
        }
    }

    /// Consume the adapter and return the captured reads. Each
    /// entry is an MV-store read (intra-bundle local hits are
    /// not tracked — they're already covered by the read entry
    /// from the step that wrote them).
    pub(crate) fn into_reads(self) -> Vec<ReadEntry> {
        self.reads.into_inner()
    }
}

impl DatabaseRef for OccRevmDb<'_> {
    type Error = Infallible;

    fn basic_ref(&self, address: RevmAddress) -> Result<Option<AccountInfo>, Self::Error> {
        let addr = Address(address.into_array());
        let slot = if let Some(&slot) = self.local.get(&addr) {
            // Intra-bundle write seen by a later step. No
            // outer-read-set entry: the read for this address
            // was already recorded by the step that wrote it.
            slot
        } else {
            let (slot, source) = self.mv.read(self.state, &addr, self.idx);
            self.reads.borrow_mut().push(ReadEntry { addr, source });
            slot
        };
        Ok(Some(AccountInfo {
            balance: U256::from(slot.canonical()),
            nonce: slot.nonce().value,
            code_hash: KECCAK_EMPTY,
            ..AccountInfo::default()
        }))
    }

    fn code_by_hash_ref(&self, _code_hash: B256) -> Result<Bytecode, Self::Error> {
        // Value-transfer increment matches the standalone RevmExecutor:
        // no contract code surfaces here. Contract execution inside
        // OCC needs MV-backed code/storage lookups too; that's a
        // follow-up alongside PR #28 (contracts in OCC).
        Ok(Bytecode::default())
    }

    fn storage_ref(&self, _address: RevmAddress, _index: U256) -> Result<U256, Self::Error> {
        Ok(U256::ZERO)
    }

    fn block_hash_ref(&self, _number: u64) -> Result<B256, Self::Error> {
        Ok(B256::ZERO)
    }
}

/// Outcome of a single revm-driven OCC step.
#[derive(Debug)]
pub(crate) struct EvmStepOutcome {
    /// Addresses read via the MV store, in observation order.
    pub reads: Vec<ReadEntry>,
}

/// Execute one [`EvmTx`] through revm against the MV-backed view,
/// merging revm's diff into the caller-owned `local` accumulator on
/// success.
///
/// Behaviour mirrors [`crate::vm::revm_executor::RevmExecutor::execute`]:
///
/// - Envelope nonce is validated against the read-view nonce; mismatch
///   surfaces as [`RejectReason::InvalidNonce`]. revm itself also
///   rejects on the same condition (defence in depth — we read the
///   nonce from the MV-backed view, not directly from `state`, so the
///   pre-check honours OCC's serialisable view).
/// - revm runs with `gas_price = 0`, `gas_limit = TRANSFER_GAS_LIMIT`.
/// - On revert (insufficient balance, etc.) the caller-owned `local`
///   is untouched.
/// - Post-execution balances must fit in `u128`; otherwise the step
///   reverts with [`RejectReason::BalanceOverflow`] — matches the
///   standalone executor's behaviour so OCC and bundle paths agree on
///   the overflow shape.
///
/// Returns:
/// - `Ok(EvmStepOutcome { reads })` — the step committed locally;
///   `local` has been updated with the revm diff; `reads` lists the
///   MV reads observed during the step (caller appends to its
///   outer `read_set`).
/// - `Err(reject)` — the step did not commit; `local` is unchanged;
///   the caller should clear its idx in the MV store and propagate
///   the reject.
pub(crate) fn run_evm_step(
    tx: &EvmTx,
    state: &State,
    mv: &MvStore,
    idx: TxnIdx,
    local: &mut HashMap<Address, BalanceSlot>,
) -> Result<EvmStepOutcome, RejectReason> {
    // Pre-validate the envelope nonce against the read view (MV +
    // local + snapshot). Matches the standalone RevmExecutor pre-check
    // and surfaces a typed `InvalidNonce` instead of a generic revm
    // error. Note the lookup uses the same fall-through order as the
    // adapter, so the OCC read tracking captures the same versions
    // revm itself would observe.
    let sender_slot = match local.get(&tx.from) {
        Some(&slot) => slot,
        None => mv.read(state, &tx.from, idx).0,
    };
    if tx.nonce != sender_slot.nonce().value {
        return Err(RejectReason::InvalidNonce);
    }

    let db = OccRevmDb::new(state, mv, idx, local);

    // Run revm. The closure-style block isolates the immutable borrow
    // of `db` so we can move it into `into_reads` after revm returns.
    //
    // Set a per-txn-idx block beneficiary so that two disjoint EVM
    // calls in the same block don't accidentally share a read /
    // touched-account record on the default `Address::ZERO`
    // beneficiary. Without this, txn N reading the beneficiary as a
    // snapshot would be invalidated by any earlier txn that wrote to
    // `Address::ZERO` (e.g. when one of its participants happens to
    // equal `Address::ZERO` — a real risk because `Address([0; 20])`
    // is a perfectly legal user address in this chain). At
    // `gas_price = 0` the beneficiary balance never changes, so the
    // per-txn beneficiary is a pure-read sentinel: no other txn ever
    // writes to it, so the validator sees no conflict. The sentinel
    // shape is "`gsx-occ-bnf\0` || idx_be(8)" so it's distinguishable
    // from user addresses in traces.
    let beneficiary = {
        let mut bytes = [0u8; 20];
        bytes[..12].copy_from_slice(b"gsx-occ-bnf\0");
        bytes[12..].copy_from_slice(&(idx as u64).to_be_bytes());
        RevmAddress::from(bytes)
    };
    let (result, diff) = {
        let mut ctx = monad_context_with_db(WrapDatabaseRef(&db));
        ctx.block.beneficiary = beneficiary;
        let mut evm = ctx.build_monad();
        let txenv = TxEnv::builder()
            .caller(RevmAddress::from(tx.from.0))
            .to(RevmAddress::from(tx.to.0))
            .value(U256::from(tx.value))
            .gas_limit(TRANSFER_GAS_LIMIT)
            .gas_price(0)
            .nonce(tx.nonce)
            .data(Bytes::new())
            .build_fill();
        let out = evm
            .transact(txenv)
            .map_err(|_| RejectReason::InsufficientBalance)?;
        (out.result, out.state)
    };

    if !result.is_success() {
        return Err(RejectReason::InsufficientBalance);
    }

    // Pre-convert every touched balance and filter the diff down to
    // *real* changes before mutating `local`. Two reasons:
    //
    // 1. Atomicity. A u128 overflow on any touched address reverts
    //    the whole step without leaving a partial diff in the
    //    accumulator. Matches the standalone executor's gate.
    // 2. OCC parallelism. revm marks the block beneficiary as
    //    `touched` during gas accounting even at `gas_price = 0`,
    //    where the actual balance + nonce don't change. Staging
    //    that as an MV write would make two disjoint EVM calls in
    //    the same block false-conflict on the beneficiary address
    //    and serialize (the second's snapshot read becomes stale
    //    against the first's beneficiary write), tanking
    //    parallelism on EVM workloads. Drop writes whose post-state
    //    equals the pre-state we'd read through `OccRevmDb`.
    let mut writes: Vec<(Address, BalanceSlot)> = Vec::new();
    for (revm_addr, account) in diff {
        if !account.is_touched() {
            continue;
        }
        let balance = u128::try_from(account.info.balance)
            .map_err(|_| RejectReason::BalanceOverflow)?;
        let post_nonce = account.info.nonce;
        let addr = Address(revm_addr.into_array());

        // Resolve the pre-state via the same fall-through order as
        // the read adapter. This is cheap (HashMap lookup + at most
        // one MV/snapshot read; we don't record into the read set
        // again because the read was already recorded by
        // `basic_ref` during revm execution).
        let pre_slot = match local.get(&addr) {
            Some(&slot) => slot,
            None => mv.read(state, &addr, idx).0,
        };
        if pre_slot.canonical() == balance && pre_slot.nonce().value == post_nonce {
            // No-op touch (e.g. beneficiary at zero gas price).
            // Drop it so the OCC validator doesn't see a phantom
            // write that serialises disjoint EVM calls.
            continue;
        }

        writes.push((
            addr,
            BalanceSlot::with_nonce(balance, AccountNonce::new(post_nonce)),
        ));
    }

    // Capture reads now (before consuming `db`).
    let reads = db.into_reads();

    // Commit the (filtered) diff to the caller-owned local
    // accumulator. The outer execute_call publishes these to the MV
    // store at the end of the bundle.
    for (addr, slot) in writes {
        local.insert(addr, slot);
    }

    Ok(EvmStepOutcome { reads })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::occ::mv_store::ReadSource;
    use gsxdb_state::{Balance, BridgeToken, StateChange};

    fn addr(byte: u8) -> Address {
        Address([byte; 20])
    }

    fn seeded(addr_in: Address, amount: u128) -> State {
        let mut state = State::default();
        let token = BridgeToken::__for_bridge_only();
        state.apply(
            &token,
            &StateChange::SetBalance {
                addr: addr_in,
                to: Balance(amount),
            },
        );
        state
    }

    /// Successful step against an empty MV store records the sender
    /// + recipient reads (both observed from the canonical snapshot)
    /// and stages the post-state into `local`. revm may also probe
    /// the block beneficiary at gas-accounting time — that's a real
    /// read and lands in the OCC read set too; the test asserts the
    /// load-bearing transfer participants are present and snapshot-
    /// sourced rather than pinning an exact count.
    #[test]
    fn run_evm_step_reads_snapshot_and_stages_local() {
        let alice = addr(1);
        let bob = addr(2);
        let state = seeded(alice, 100);
        let mv = MvStore::new();
        let mut local: HashMap<Address, BalanceSlot> = HashMap::new();

        let outcome = run_evm_step(
            &EvmTx {
                from: alice,
                to: bob,
                value: 30,
                nonce: 0,
            },
            &state,
            &mv,
            5,
            &mut local,
        )
        .expect("step commits");

        // Every recorded read came from the canonical snapshot
        // (no prior MV writes in the test setup).
        for r in &outcome.reads {
            assert_eq!(r.source, ReadSource::Snapshot);
        }
        // Sender + recipient are recorded.
        let read_addrs: std::collections::BTreeSet<Address> =
            outcome.reads.iter().map(|r| r.addr).collect();
        assert!(read_addrs.contains(&alice), "alice missing from reads");
        assert!(read_addrs.contains(&bob), "bob missing from reads");

        // Local accumulator now holds the post-state.
        let post_alice = local.get(&alice).copied().expect("alice in local");
        let post_bob = local.get(&bob).copied().expect("bob in local");
        assert_eq!(post_alice.canonical(), 70);
        assert_eq!(post_bob.canonical(), 30);
        // The sender's nonce advanced via revm.
        assert_eq!(post_alice.nonce().value, 1);
    }

    /// A step whose envelope nonce doesn't match the read view
    /// (state nonce) rejects with InvalidNonce — the OCC adapter
    /// must enforce the same replay-defence boundary as the
    /// standalone executor.
    #[test]
    fn run_evm_step_rejects_stale_envelope_nonce() {
        let alice = addr(1);
        let bob = addr(2);
        let state = seeded(alice, 100);
        let mv = MvStore::new();
        let mut local: HashMap<Address, BalanceSlot> = HashMap::new();

        let err = run_evm_step(
            &EvmTx {
                from: alice,
                to: bob,
                value: 10,
                nonce: 1, // state nonce is 0
            },
            &state,
            &mv,
            0,
            &mut local,
        )
        .expect_err("nonce 1 with state nonce 0 must reject");
        assert_eq!(err, RejectReason::InvalidNonce);
        assert!(local.is_empty(), "rejected step leaves local untouched");
    }

    /// Two sequential steps from the same sender against an
    /// initially-empty local accumulator see the bumped nonce on the
    /// second step (the sender + recipient land in `local` after the
    /// first step, so subsequent reads of those addresses are local-
    /// hits that don't record into the read set). revm may still
    /// touch ancillary addresses (e.g. the block beneficiary at
    /// gas-accounting time); the test asserts neither alice nor bob
    /// re-appears in the second step's reads, rather than pinning a
    /// zero-count that revm's internal probing breaks.
    #[test]
    fn run_evm_step_threads_local_accumulator_across_calls() {
        let alice = addr(1);
        let bob = addr(2);
        let state = seeded(alice, 100);
        let mv = MvStore::new();
        let mut local: HashMap<Address, BalanceSlot> = HashMap::new();

        let _first = run_evm_step(
            &EvmTx {
                from: alice,
                to: bob,
                value: 30,
                nonce: 0,
            },
            &state,
            &mv,
            0,
            &mut local,
        )
        .unwrap();

        let second = run_evm_step(
            &EvmTx {
                from: alice,
                to: bob,
                value: 20,
                nonce: 1, // alice's local nonce is now 1
            },
            &state,
            &mv,
            0,
            &mut local,
        )
        .unwrap();

        // Both addresses are already in local; neither should appear
        // in the second step's MV-read set.
        let second_addrs: std::collections::BTreeSet<Address> =
            second.reads.iter().map(|r| r.addr).collect();
        assert!(
            !second_addrs.contains(&alice),
            "alice should be a local hit on the second step, not an MV read"
        );
        assert!(
            !second_addrs.contains(&bob),
            "bob should be a local hit on the second step, not an MV read"
        );

        assert_eq!(local[&alice].canonical(), 50);
        assert_eq!(local[&bob].canonical(), 50);
        assert_eq!(local[&alice].nonce().value, 2);
    }

    /// A transfer whose recipient would exceed u128::MAX reverts
    /// without staging any local write — matches the standalone
    /// executor's atomic-revert shape.
    #[test]
    fn run_evm_step_reverts_on_balance_overflow_without_partial_writes() {
        let alice = addr(1);
        let bob = addr(2);
        let mut state = seeded(alice, 10);
        let token = BridgeToken::__for_bridge_only();
        state.apply(
            &token,
            &StateChange::SetBalance {
                addr: bob,
                to: Balance(u128::MAX - 5),
            },
        );
        let mv = MvStore::new();
        let mut local: HashMap<Address, BalanceSlot> = HashMap::new();

        let err = run_evm_step(
            &EvmTx {
                from: alice,
                to: bob,
                value: 10,
                nonce: 0,
            },
            &state,
            &mv,
            0,
            &mut local,
        )
        .expect_err("u128-overflow recipient must revert");
        assert_eq!(err, RejectReason::BalanceOverflow);
        assert!(
            local.is_empty(),
            "overflow revert must not leak a partial write into local"
        );
    }

    /// A read against an address with an earlier-idx MV write
    /// records `ReadSource::Version(...)`, matching the manual-
    /// arithmetic path's read-tracking shape so the OCC validator
    /// treats both code paths identically.
    #[test]
    fn run_evm_step_records_version_reads_from_mv_store() {
        let alice = addr(1);
        let bob = addr(2);
        let state = seeded(alice, 1_000);
        let mv = MvStore::new();
        // Earlier-idx writer (idx 1) credits bob.
        mv.write(
            bob,
            BalanceSlot::with_nonce(500, AccountNonce::new(0)),
            1,
        );

        let mut local: HashMap<Address, BalanceSlot> = HashMap::new();
        let outcome = run_evm_step(
            &EvmTx {
                from: alice,
                to: bob,
                value: 100,
                nonce: 0,
            },
            &state,
            &mv,
            5, // reader idx > 1, so the version is visible
            &mut local,
        )
        .unwrap();

        // bob's read should resolve to the MV version at idx 1;
        // alice's falls through to the canonical snapshot.
        let bob_read = outcome
            .reads
            .iter()
            .find(|r| r.addr == bob)
            .expect("bob in reads");
        assert_eq!(bob_read.source, ReadSource::Version(1));

        let alice_read = outcome
            .reads
            .iter()
            .find(|r| r.addr == alice)
            .expect("alice in reads");
        assert_eq!(alice_read.source, ReadSource::Snapshot);

        // Post-state: bob credited from his MV-visible 500.
        assert_eq!(local[&bob].canonical(), 600);
        assert_eq!(local[&alice].canonical(), 900);
    }
}
