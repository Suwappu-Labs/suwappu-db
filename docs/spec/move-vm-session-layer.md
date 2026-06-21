# Move VM session layer (S9.5d design)

## Context

S9.5c wired `move-binary-format::CompiledModule::deserialize` and
`move-bytecode-verifier::verify_module` into `AptosMoveExecutor`. With
the feature on, the executor performs **real** bytecode-format
validation but cannot yet invoke the interpreter — `MoveVM::execute_loaded_function`
requires a session layer that aptos-vm provides and that we deliberately
don't pull in.

This doc inventories what suwappu-db needs to build to bridge our
`MoveBalanceView` to Aptos's stateless VM. It's the design pass for
S9.5d before any code lands.

## The API mismatch

The Aptos `MoveVM` at tag `aptos-node-v1.44.9-hotfix` is:

```rust
pub struct MoveVM;
impl MoveVM {
    pub fn execute_loaded_function(
        function: LoadedFunction,
        serialized_args: Vec<impl Borrow<[u8]>>,
        data_cache: &mut impl MoveVmDataCache,
        gas_meter: &mut impl GasMeter,
        traversal_context: &mut TraversalContext,
        extensions: &mut NativeContextExtensions,
        loader: &impl Loader,
    ) -> VMResult<SerializedReturnValues>;
}
```

> "Move VM is completely stateless. It is used to execute a single loaded function with its type arguments fully instantiated."

The "load a function from a module ID + entry name + type args" step,
the "track resource reads/writes from globally-stored Move resources"
step, the "charge gas per opcode" step, and the "satisfy native
function calls" step are all the caller's responsibility.

`aptos-vm` provides all of this with a `Session` type — wrapping a
`StateView`, configured gas, the Aptos native function table, etc.
Pulling `aptos-vm` brings the full Aptos block executor + state-view +
storage stack, which we deliberately don't want.

So suwappu-db builds its own minimal session layer.

## Trait surface to implement

Six trait families, totaling ~60 methods across the Aptos
`move-vm-runtime` crate:

### 1. `Loader` (composite)

`Loader` extends:

- `ClosureLoader` (auto-impl for `InstantiatedFunctionLoader`)
- `FunctionDefinitionLoader` (1 fn — `load_function`)
- `ModuleMetadataLoader` (2 fns — `load_module_metadata`, `unmetered_load_module_metadata`)
- `NativeModuleLoader` (2 fns)
- `StructDefinitionLoader` (4 fns — `load_struct_definition`, `load_struct_layout`, etc.)
- `InstantiatedFunctionLoader` + `InstantiatedFunctionLoaderHelper` (4–6 fns)

Plus `Loader::unmetered_module_storage() -> &dyn ModuleStorage`.

Total Loader: ~15 methods.

### 2. `ModuleStorage` (+ `WithRuntimeEnvironment` + `LayoutCache`)

The module-bytes-and-metadata cache the loader sits on top of.
26 methods covering:

- Bytes lookup (`fetch_existing_module_bytes`, `fetch_module_bytes`)
- `Module` lookup (verified + loaded form)
- `RuntimeEnvironment` access (per-chain VM config)
- Layout-cache management

The suwappu-db `ModuleStore` (in `suwappudb-state::vm::executor`) holds the
opaque bytecode; the `ModuleStorage` impl wraps it with the
`CompiledModule` + `Module` + layout cache.

### 3. `MoveVmDataCache` (+ `NativeContextMoveVmDataCache`)

Where resource reads + writes flow during a session. 9 methods:

- `borrow_resource(addr, type_tag) -> &Value` — reads from `MoveBalanceView`
- `move_to(addr, type_tag, value)` — buffers a write
- `move_from(addr, type_tag) -> Value` — removes a resource
- `exists(addr, type_tag) -> bool`
- ...

At session commit, the buffered writes get translated into our
`ResourceWrite { addr, coin_value, nonce }` and routed to substrate
through the bridge — that's already wired in
`suwappudb-bridge::bundle::executor::apply_resource_writes` (S9.4).

The new work: extracting `coin_value` + `nonce` from Move's
opaque `Value` representation.

### 4. `GasMeter`

S12 lands real gas metering. For S9.5d we provide an
`UnmeteredGasMeter` that never aborts and reports 0 gas used.
The trait has 3–4 methods (charge per opcode, charge for native, etc).

### 5. `TraversalContext`

Tracks already-loaded modules during a single call to avoid
re-loading. Aptos provides `TraversalContext::new()`; we can
just instantiate one per session.

### 6. `NativeContextExtensions`

Container for per-session state used by native functions. For our
minimum stdlib subset (`vector`, `option`, `signer`, `error`,
`string`), the default empty `NativeContextExtensions::new()` is
likely sufficient. To confirm during impl.

## `LoadedFunction` construction

The MoveVM call expects a `LoadedFunction` — a struct holding a
reference to the verified function definition with its type args
already instantiated.

The `FunctionDefinitionLoader::load_function(...)` is meant to
produce this. Implementation: dereference our `ModuleStore`,
deserialize + verify (we already do this in S9.5c), then resolve
the entry function by name + type-arg list.

## Implementation strategy

Single new module `crates/suwappudb-state/src/vm/aptos_session.rs`:

```rust
#[cfg(feature = "production-move-executor")]
pub(crate) struct AptosSession<'a> {
    modules: &'a dyn ModuleStore,
    balance_view: &'a dyn MoveBalanceView,
    runtime_env: Arc<RuntimeEnvironment>,
    layout_cache: LayoutCache,
    buffered_writes: Vec<ResourceWrite>,
    // ... gas meter, traversal ctx, etc.
}

#[cfg(feature = "production-move-executor")]
impl Loader for AptosSession<'_> { /* 15 methods */ }

#[cfg(feature = "production-move-executor")]
impl ModuleStorage for AptosSession<'_> { /* 26 methods */ }

#[cfg(feature = "production-move-executor")]
impl MoveVmDataCache for AptosSession<'_> { /* 9 methods */ }

#[cfg(feature = "production-move-executor")]
impl GasMeter for UnmeteredGasMeter { /* 4 methods, all no-op */ }
```

Then `AptosMoveExecutor::execute` wires:

1. Build the `AptosSession` from `modules` + `state.balance_view`.
2. Resolve `LoadedFunction` from `call.module` + `call.function` + `call.type_arguments`.
3. Call `MoveVM::execute_loaded_function(loaded_fn, call.arguments, session, ...)`.
4. Drain `session.buffered_writes` into `MoveOutcome::resource_writes`.

## Estimated work

| Sub-step | Methods | Sessions |
|---|---|---|
| `ModuleStorage` + `WithRuntimeEnvironment` + `LayoutCache` | 26 | 1–2 |
| `Loader` composite (6 sub-traits) | 15 | 1 |
| `MoveVmDataCache` + value-extraction | 9 | 1 |
| `UnmeteredGasMeter` | 4 | <0.5 |
| Integration + tests | — | 0.5 |
| **Total** | **~60** | **~3–4** |

Each session likely doesn't reach a green build. The work is fighting
the Aptos type system with our own `MoveBalanceView` abstraction in
the middle.

## Why this is harder than it looks

The Aptos VM traits assume the caller has access to:

- A `RuntimeEnvironment` that owns native function tables, VM config,
  metadata
- A `LayoutCache` that builds + caches Move struct layouts from
  module bytes
- A `Module` (verified + linked form of `CompiledModule`) — built
  by `move-vm-runtime`'s loader pipeline

These are normally constructed by `aptos-vm` during block executor
setup. Building them ourselves requires understanding Aptos internals
that aren't well-documented outside the aptos-vm + aptos-vm-types
source.

Mitigation: lean heavily on aptos-vm-types as a reference (we can
read the source even if we don't pull the crate) and crib the
construction patterns.

## Open questions for S9.5d

- **`RuntimeEnvironment` construction.** Aptos-VM builds this from
  `OnChainConfig` blobs. We don't have on-chain config. The
  smallest valid default needs identifying.
- **Native function table.** What's the minimum set we need? Aptos's
  default registers ~30 natives across stdlib + framework. We want
  just the stdlib subset (`vector`, `option`, `signer`, `error`,
  `string`). Building a minimal table by hand.
- **Move value ↔ suwappu-db value conversion.** The Move VM produces
  `move_vm_types::values::Value`, an opaque tagged-union. We need to
  extract `u128` (for `coin_value`) and `u64` (for `nonce`) from
  resources. Likely needs `Value::cast_u128()` / `MoveStructLayout`
  walk.
- **Address arg encoding.** Move expects 32-byte `AccountAddress`
  args BCS-encoded. We have `MoveAddress(pub [u8; 32])`. The mapping
  is direct but needs verification against the BCS spec.

## Out of scope for S9.5d

- Module **upgrade** semantics (compat checks) — separate path.
- Real **gas metering** — `UnmeteredGasMeter` is fine; S12 lands real.
- **Parallelism** inside a single bundle's Move execution — defer.
- **Cross-chain Move resources** (LTP transfers) — defer to
  suwappu-lattice-protocol integration.

## Cross-references

- [docs/spec/move-execution.md](move-execution.md) — the S9.1 design
  doc for the executor surface suwappu-db exposes.
- [IQ-3](../iq/IQ-3-move-vm-choice.md) — why Aptos.
- aptos-core source under `~/.cargo/git/checkouts/aptos-core-*/77535b5/`:
  - `third_party/move/move-vm/runtime/src/move_vm.rs` — top-level VM
  - `third_party/move/move-vm/runtime/src/storage/loader/traits.rs` — Loader
  - `third_party/move/move-vm/runtime/src/storage/module_storage.rs` — ModuleStorage
  - `third_party/move/move-vm/runtime/src/data_cache.rs` — MoveVmDataCache
  - `aptos-move/aptos-vm-types/` — reference impls (don't pull, just read)
