# Move execution (S9 — IQ-3/4/5)

## Goal

Define the concrete execution model for Move bytecode in gsx-db. Replaces
the placeholder `MoveExecutor` trait (which is a passthrough returning the
input state) with a real bytecode-execution surface backed by Aptos
`move-vm-runtime`.

Phase-1 ran with `MockMoveExecutor` because the load-bearing invariants
(dual-projection, OCC determinism, cross-VM canonical equivalence) are
structural — they don't require bytecode semantics. S9 closes this gap by
wiring real Move bytecode through the bundle executor.

## Why the current trait surface is insufficient

`crates/gsxdb-state/src/vm/executor.rs` defines:

```rust
pub trait MoveExecutor {
    fn execute(&self, addr: &MoveAddress, initial: BalanceSlot)
        -> ExecutionOutcome;
}
```

Three problems:

1. **No bytecode argument.** Real Move execution needs the compiled module
   to invoke. The trait can't accept user-deployed modules.
2. **No entry function / arguments.** There's no way to specify which Move
   function to call or what arguments to pass.
3. **Not called from anywhere.** `grep -rn MoveExecutor crates/` returns
   only the file itself. Implementing the production backend produces zero
   runtime behavior change.

S9 replaces this with a real call-site trait, wires it into the bundle
executor, and adds an Aptos-backed implementation.

## Execution model

### When Move bytecode runs

Move bytecode executes **per-intent inside `BundleExecutor`** when the
intent is `Intent::Call { target: MoveContract(module_id, function), ... }`.

- One Move VM session per bundle (matches Aptos's per-block session
  model).
- Each `Intent::Call` invokes one entry function with the provided
  arguments.
- Session state (resources read/written) is collected and applied to
  the substrate at bundle commit time — preserving the lane-separation
  invariant.

### What runs

The Move VM executes:

1. **User-deployed modules.** Stored in a new `ModuleStore` indexed by
   `(account_address, module_name)`. Deployed via a new
   `Intent::DeployModule` variant.
2. **Standard library.** A pinned subset of `aptos-move-stdlib`:
   `vector`, `option`, `signer`, `error`, `string` — no I/O, no
   ChainID, no time-sources, no randomness. Pinned by content hash;
   any change to the stdlib bundle is a wire-incompatible upgrade.
3. **The canonical `Coin` module.** A gsx-db-vendored `Coin<T>` module
   compatible with Aptos's `aptos_framework::coin` ABI — minimum
   surface to maintain dual-projection: `balance(addr)`, `transfer`,
   `mint`, `burn`. The `MoveCoinValue` projection reads from this
   module's resource.

### Address shape (IQ-4)

Resolved: **`Address` enum with canonical projection.**

```rust
pub enum Address {
    Evm(EvmAddress),     // 20 bytes
    Move(MoveAddress),   // 32 bytes
}
```

- EVM-side calls use 20-byte addresses unchanged.
- Move-side calls use 32-byte Aptos-compatible addresses.
- Cross-VM bridge: `EvmAddress -> MoveAddress` is the 20-byte address
  left-zero-padded to 32 bytes. The reverse is rejected at the bridge
  boundary if the upper 12 bytes are non-zero.
- This means every EVM address has exactly one canonical Move
  representation, but not every Move address has an EVM
  representation. The dual-projection invariant
  `EVM balanceOf(addr) == Move Coin.value(canonical_move(addr))` holds
  on the EVM-addressable subset.

### Nonce semantics (IQ-5)

Resolved: **per-account `AccountNonce { evm: u64, move_seq: u64 }`.**

- EVM-side transactions increment `evm` (standard Ethereum semantics).
- Move-side transactions increment `move_seq` (Aptos sequence-number
  semantics).
- An `Intent::Call` originating from an EVM caller increments `evm`
  only. An `Intent::Call` from a Move caller increments `move_seq`
  only. Cross-VM intents (a single intent that touches both sides)
  increment both atomically inside the bundle.
- Replay protection: an intent with `caller_nonce != current_nonce`
  for the caller's side is rejected at bundle-admit time. No
  out-of-order nonces; gaps are a hard error.

### Gas

Out of scope for S9. The Move VM is invoked with `gas_budget = u64::MAX`
inside `BundleExecutor`, and any abort that hits the gas limit is treated
as a runtime error (returned as `MoveExecutionError::OutOfGas`). Real gas
metering lands in S12 alongside telemetry — gas accounting in the bundle
executor is a separate sprint-scope item.

## New trait surface

```rust
pub struct MoveCall {
    pub caller: MoveAddress,
    pub module: ModuleId,         // (account_address, module_name)
    pub function: Identifier,     // e.g. "transfer"
    pub type_arguments: Vec<TypeTag>,
    pub arguments: Vec<Vec<u8>>,  // BCS-encoded
}

pub trait ModuleStore: Send + Sync {
    fn get(&self, id: &ModuleId) -> Option<&CompiledModule>;
    fn put(&mut self, id: ModuleId, module: CompiledModule)
        -> Result<(), ModuleStoreError>;
    fn contains(&self, id: &ModuleId) -> bool;
}

pub trait MoveExecutor: Send + Sync + std::fmt::Debug {
    fn execute(
        &self,
        call: &MoveCall,
        modules: &dyn ModuleStore,
        state: &mut MoveSessionState,
    ) -> Result<MoveOutcome, MoveExecutionError>;
}

pub struct MoveSessionState<'a> {
    // Resources read/written during the call.
    // Reads check the substrate's BalanceStore for Coin<T> resources;
    // writes are buffered and applied at bundle commit.
    pub balance_store: &'a dyn BalanceStore,
    pub buffered_writes: Vec<ResourceWrite>,
}

pub struct MoveOutcome {
    pub return_values: Vec<Vec<u8>>,    // BCS-encoded
    pub events: Vec<MoveEvent>,
    pub resource_writes: Vec<ResourceWrite>,
}

pub enum MoveExecutionError {
    ModuleNotFound(ModuleId),
    BytecodeVerificationFailed(String),
    Abort { code: u64, location: AbortLocation },
    OutOfGas,
    InvalidArguments(String),
    LinkerError(String),
}
```

### What the trait doesn't model (intentional)

- **Network access.** No syscalls reach the network. Modules that import
  non-deterministic stdlib functions fail verification.
- **Persistent off-VM state.** Resource writes are buffered in
  `MoveSessionState` and applied through the substrate's `BalanceStore`
  at bundle commit. The Move VM doesn't see redb / RocksDB directly.
- **Cross-bundle reads.** Each bundle gets a fresh session; reads see
  the bundle's pre-image state. Inter-bundle dependencies go through
  the OCC scheduler (S4).

## Implementation phases (S9 sub-passes)

S9 ships as 5 PRs to bound review scope:

| Sub-pass | Scope | Estimate |
|---|---|---|
| S9.1 | Design doc (this file) + IQ-3 update | 1 session |
| S9.2 | New trait + types in `gsxdb-state::vm`; `MockMoveExecutor` re-impl against new surface; tests at new shape | 1 session |
| S9.3 | `ModuleStore` (in-memory + redb-backed); `Intent::DeployModule`; tests | 1 session |
| S9.4 | Wire `MoveExecutor::execute()` into `BundleExecutor` for `Intent::Call`; cross-VM parity test still passes with mock | 1 session |
| S9.5 | `AptosMoveExecutor` impl using `move-vm-runtime`; canonical `Coin<T>` module bundled; `production-move-executor` feature ON in default build | 2 sessions |
| S9.6 | 10k cross-VM parity gate with real Aptos backend; close-out doc updates | 1 session |

The trait + module store + bundle wiring (S9.2 → S9.4) ships against the
mock so cross-VM parity stays green while the trait shape changes. Aptos
integration lands as the final swap.

## Aptos dependency surface

Minimum subset of `aptos-core` needed (S9.5):

- `move-binary-format` — bytecode deserialization
- `move-bytecode-verifier` — bytecode verifier
- `move-vm-runtime` — interpreter
- `move-vm-types` — types used by the runtime
- `move-core-types` — `ModuleId`, `Identifier`, `TypeTag`, etc.
- `aptos-move-stdlib` — pinned standard library subset

Explicitly NOT pulled:

- `aptos-vm` (the full Aptos block executor — we have our own)
- `aptos-state-view` (state access — we route through `BalanceStore`)
- `aptos-gas-schedule` (gas metering — out of scope for S9)
- `aptos-consensus`, `aptos-storage`, `aptos-network` (full node deps)

Tracking a specific Aptos release tag is preferred over pinning to a
git rev. Initial target: `aptos-release-v1.X` (latest stable at S9.5
time). Track changes via the Aptos changelog; bump as part of routine
maintenance.

### Build cost

aptos-core's VM crates add ~500 MB to `target/`. Mitigation:

- CI uses `Swatinem/rust-cache` (already configured for the workspace).
- The `production-move-executor` feature is the gate; fast iteration in
  development can use the mock by disabling the feature
  (`--no-default-features --features mock-move-executor`).

## Exit gate

```
PROPTEST_CASES=10000 cargo test --test cross_vm_parity \
    interleaved_evm_move_preserves_invariant --features production-move-executor
```

10,000 random intent sequences over the dual-projection surface, executed
with the real Aptos Move VM, must satisfy:

`EVM balanceOf(addr) == Move Coin.value(canonical_move(addr))`

for every EVM-addressable account.

## Failure model

| Failure | Surface | Recovery |
|---|---|---|
| Bytecode verifier rejects module | `Intent::DeployModule` returns `BytecodeVerificationFailed` | Caller deploys corrected module |
| Module-not-found at call site | `Intent::Call` returns `ModuleNotFound` | Caller deploys module first |
| Move abort during execution | Intent rejected; bundle continues with other intents | Standard abort handling |
| Out of gas (post-S12) | Intent rejected; bundle continues | Caller submits with higher budget |
| Linker error (missing dependency) | `Intent::DeployModule` rejected | Caller deploys dependency first |
| Non-deterministic stdlib import | Bytecode verifier rejection at deploy | Caller removes the import |

## Open questions deferred from S9

- **Move parallelism.** Aptos's Block-STM runs Move in parallel within a
  block; gsx-db's OCC already does this for Intent-level operations.
  Investigate whether Move-level parallelism inside a bundle is worth
  the complexity. Defer to post-S9 perf work.
- **Module upgrades.** Aptos has a strict module-upgrade policy
  (`compatible` upgrades only). gsx-db inherits this but doesn't yet
  surface it. Defer to post-launch governance.
- **Cross-chain Move resources.** LTP attestations might carry Move
  resource transfers. Defer to LTP integration (gsx-lattice-protocol).

## Cross-references

- [IQ-3 — Move VM choice](../iq/IQ-3-move-vm-choice.md) — the dialect
  decision (Aptos).
- [IQ-4 (folded)](../iq/IQ-3-move-vm-choice.md#part-2--s9-revision-aptos-selected) — address shape (resolved via `Address` enum).
- [IQ-5 (folded)](../iq/IQ-3-move-vm-choice.md#part-2--s9-revision-aptos-selected) — nonce semantics (per-account `AccountNonce`).
- [dual-vm-projectors.md](dual-vm-projectors.md) — the substrate-side
  projection invariant this spec preserves.
- [ce-mvcc-occ.md](ce-mvcc-occ.md) — the OCC scheduler that drives
  per-intent invocation.
- [cross-vm-intent-queue.md](cross-vm-intent-queue.md) — the intent
  format that `Intent::Call` extends.
