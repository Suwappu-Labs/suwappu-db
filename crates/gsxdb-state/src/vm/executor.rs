//! Move VM execution framework — call-site trait + types.
//!
//! GSX-DB delegates Move bytecode execution to an abstract executor trait.
//! S9 expands this from a passthrough placeholder into a real call-site
//! that accepts bytecode + entry function + arguments + a module store.
//!
//! See `docs/spec/move-execution.md` for the full design rationale.
//!
//! ## Feature gates
//!
//! - `(none)` or `mock-move-executor` — uses [`MockMoveExecutor`]. Simulates
//!   the canonical `Coin<T>` `transfer` entry point so cross-VM parity
//!   tests can stay green while the trait shape changes (S9.2 → S9.4).
//! - `production-move-executor` — uses `AptosMoveExecutor` (lands in S9.5;
//!   wraps `move-vm-runtime` and bundles a canonical `Coin<T>` module).
//!
//! ## Invocation site
//!
//! The Move executor is invoked from `gsxdb-bridge::bundle::BundleExecutor`
//! when an `Intent::Call` targets a Move module (S9.4). Each bundle gets
//! one [`MoveSessionState`]; resource writes are buffered and applied to
//! the substrate at bundle commit through the lane-separation gate.

use crate::{AccountNonce, MoveAddress, MoveCoinValue};

/// Move identifier (function or module name). Bytes match Move's
/// identifier rules: ASCII, starts with letter or underscore, contains
/// only `[A-Za-z0-9_]`. Validated at construction.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Identifier(String);

/// Reasons [`Identifier::new`] rejects a string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdentifierError {
    /// Empty string.
    Empty,
    /// Contains a byte outside the Move identifier alphabet.
    InvalidByte(u8),
    /// First byte is not a letter or underscore.
    InvalidLeadingByte(u8),
}

impl Identifier {
    /// Validate and construct.
    ///
    /// # Errors
    ///
    /// Returns [`IdentifierError`] if `s` is empty, starts with a digit, or
    /// contains a byte outside `[A-Za-z0-9_]`.
    pub fn new(s: impl Into<String>) -> Result<Self, IdentifierError> {
        let s = s.into();
        let bytes = s.as_bytes();
        if bytes.is_empty() {
            return Err(IdentifierError::Empty);
        }
        let first = bytes[0];
        if !(first.is_ascii_alphabetic() || first == b'_') {
            return Err(IdentifierError::InvalidLeadingByte(first));
        }
        for &b in &bytes[1..] {
            if !(b.is_ascii_alphanumeric() || b == b'_') {
                return Err(IdentifierError::InvalidByte(b));
            }
        }
        Ok(Self(s))
    }

    /// Borrow the underlying string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Module identifier: address + module name.
///
/// Maps to Move's `<address>::<module>`. S9.5 maps directly onto
/// `move_core_types::language_storage::ModuleId`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ModuleId {
    /// Account hosting the module.
    pub address: MoveAddress,
    /// Module name within that account.
    pub name: Identifier,
}

/// Move type tag for type-parameter binding and resource keys.
///
/// Phase-1 covers the primitive types, vectors, and user structs that
/// the canonical `Coin<T>` module needs. S9.5 maps directly onto
/// `move_core_types::language_storage::TypeTag`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TypeTag {
    /// `bool`.
    Bool,
    /// `u8`.
    U8,
    /// `u16`.
    U16,
    /// `u32`.
    U32,
    /// `u64`.
    U64,
    /// `u128`.
    U128,
    /// `u256`.
    U256,
    /// `address`.
    Address,
    /// `signer`.
    Signer,
    /// `vector<T>`.
    Vector(Box<TypeTag>),
    /// User-defined struct.
    Struct(StructTag),
}

/// User-defined struct tag.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct StructTag {
    /// Defining module's account.
    pub address: MoveAddress,
    /// Defining module's name.
    pub module: Identifier,
    /// Struct name.
    pub name: Identifier,
    /// Type-parameter bindings.
    pub type_params: Vec<TypeTag>,
}

/// One Move VM entry-function invocation. Built by `BundleExecutor`
/// from an `Intent::Call` whose target is a Move module.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MoveCall {
    /// Caller address (signer). Cross-VM intents from EVM use the
    /// canonical EVM→Move projection (left-zero-pad to 32 bytes).
    pub caller: MoveAddress,
    /// Module being invoked.
    pub module: ModuleId,
    /// Entry function within the module.
    pub function: Identifier,
    /// Type-argument bindings for generic functions (e.g. the `T` in
    /// `Coin<T>::transfer<T>`).
    pub type_arguments: Vec<TypeTag>,
    /// BCS-encoded arguments. Verifier matches them against the
    /// function signature before invocation.
    pub arguments: Vec<Vec<u8>>,
}

/// Compiled Move module bytes. Phase-1 stores these as opaque bytes;
/// S9.5 deserializes via `move-binary-format::CompiledModule`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompiledModule {
    /// Canonical Move bytecode (BCS-encoded).
    pub bytes: Vec<u8>,
}

/// Module storage backend. Owned by `BundleExecutor`; populated by
/// `Intent::DeployModule` and read by [`MoveExecutor::execute`].
///
/// S9.3 lands the in-memory + redb-backed implementations.
pub trait ModuleStore: Send + Sync + std::fmt::Debug {
    /// Look up a module by id. Returns an owned copy so the caller can
    /// pass the bytes into the verifier / runtime without holding the
    /// store lock across an interpreter call.
    fn get(&self, id: &ModuleId) -> Option<CompiledModule>;

    /// Deploy a module. Returns [`ModuleStoreError::AlreadyExists`] if
    /// `id` is already populated — upgrades require a separate path
    /// (deferred per `docs/spec/move-execution.md` open question).
    ///
    /// # Errors
    ///
    /// Storage-backend failures surface as [`ModuleStoreError::Backend`].
    fn put(
        &mut self,
        id: ModuleId,
        module: CompiledModule,
    ) -> Result<(), ModuleStoreError>;

    /// `true` iff `id` has a module.
    fn contains(&self, id: &ModuleId) -> bool;
}

/// In-memory [`ModuleStore`] — `BTreeMap<ModuleId, CompiledModule>`.
///
/// Phase-1 + tests. A redb-backed `RedbModuleStore` lands alongside
/// `AptosMoveExecutor` in S9.5 once the bytecode-bytes-on-disk format
/// is settled (it'll mirror `RedbBlockStore`'s versioned encoding).
#[derive(Debug, Clone, Default)]
pub struct InMemoryModuleStore {
    modules: std::collections::BTreeMap<ModuleId, CompiledModule>,
}

impl InMemoryModuleStore {
    /// New empty store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of deployed modules.
    #[must_use]
    pub fn len(&self) -> usize {
        self.modules.len()
    }

    /// `true` iff no modules deployed.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.modules.is_empty()
    }
}

impl ModuleStore for InMemoryModuleStore {
    fn get(&self, id: &ModuleId) -> Option<CompiledModule> {
        self.modules.get(id).cloned()
    }

    fn put(
        &mut self,
        id: ModuleId,
        module: CompiledModule,
    ) -> Result<(), ModuleStoreError> {
        if self.modules.contains_key(&id) {
            return Err(ModuleStoreError::AlreadyExists(id));
        }
        self.modules.insert(id, module);
        Ok(())
    }

    fn contains(&self, id: &ModuleId) -> bool {
        self.modules.contains_key(id)
    }
}

/// Reasons [`ModuleStore::put`] fails.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModuleStoreError {
    /// Re-deploy at an existing id.
    AlreadyExists(ModuleId),
    /// Underlying storage backend error.
    Backend(String),
}

/// Read-only view over the substrate's balance store, scoped to Move
/// resources. Move executor reads `Coin<T>` resources through this so
/// it doesn't see redb / RocksDB directly — preserves lane separation.
pub trait MoveBalanceView: Send + Sync + std::fmt::Debug {
    /// Current coin value at `addr`.
    fn coin_value(&self, addr: &MoveAddress) -> MoveCoinValue;

    /// Current sequence number (Move-side) at `addr`.
    fn nonce(&self, addr: &MoveAddress) -> AccountNonce;
}

/// Per-bundle Move session. Threaded into [`MoveExecutor::execute`]
/// for the duration of one `Intent::Call`. Reads route through
/// `balance_view`; writes accumulate in `buffered_writes` and are
/// applied to the substrate at bundle commit through the
/// lane-separation gate.
#[derive(Debug)]
pub struct MoveSessionState<'a> {
    /// Read-side view into the substrate.
    pub balance_view: &'a dyn MoveBalanceView,
    /// Resource writes produced by Move execution. Apply at bundle commit.
    pub buffered_writes: Vec<ResourceWrite>,
    /// Move-emitted events produced this bundle. Surfaced through the
    /// telemetry layer; not part of canonical state.
    pub events: Vec<MoveEvent>,
}

impl<'a> MoveSessionState<'a> {
    /// New empty session over `balance_view`.
    pub fn new(balance_view: &'a dyn MoveBalanceView) -> Self {
        Self {
            balance_view,
            buffered_writes: Vec::new(),
            events: Vec::new(),
        }
    }
}

/// A buffered resource write produced by Move execution. Applied to
/// the substrate at bundle commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceWrite {
    /// Address whose `Coin<T>` resource is being updated.
    pub addr: MoveAddress,
    /// New coin value.
    pub coin_value: MoveCoinValue,
    /// New nonce.
    pub nonce: AccountNonce,
}

/// Move-emitted event. Phase-1 captures type + opaque payload; S9.5
/// can swap for `move-binary-format` event types.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MoveEvent {
    /// Event type (typically a struct tag).
    pub type_tag: TypeTag,
    /// BCS-encoded event payload.
    pub data: Vec<u8>,
}

/// Result of one successful [`MoveExecutor::execute`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MoveOutcome {
    /// BCS-encoded return values from the entry function.
    pub return_values: Vec<Vec<u8>>,
    /// Events emitted during the call.
    pub events: Vec<MoveEvent>,
    /// Resource writes accumulated during the call.
    pub resource_writes: Vec<ResourceWrite>,
}

/// Reasons [`MoveExecutor::execute`] fails.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MoveExecutionError {
    /// Module referenced by [`MoveCall::module`] is not deployed.
    ModuleNotFound(ModuleId),
    /// `move-bytecode-verifier` rejected the module. Carries the
    /// verifier-reported reason for diagnostics.
    BytecodeVerificationFailed(String),
    /// Move abort during interpretation. `code` is the user-defined
    /// abort code; `location` is the call site.
    Abort {
        /// User-defined abort code (Move convention: `error::*` helpers).
        code: u64,
        /// Where the abort happened.
        location: AbortLocation,
    },
    /// Execution ran past the gas budget (currently `u64::MAX`; S12).
    OutOfGas,
    /// Arguments didn't match the entry function's signature.
    InvalidArguments(String),
    /// Module referenced a dependency that isn't deployed.
    LinkerError(String),
}

/// Location of a Move abort. Maps to
/// `move_binary_format::errors::AbortLocation`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AbortLocation {
    /// Module that raised the abort (`None` for `<script>` calls).
    pub module: Option<ModuleId>,
    /// Index into the module's function table.
    pub function_index: u16,
    /// Bytecode offset within that function.
    pub instruction_index: u16,
}

/// Move bytecode executor.
///
/// Phase-1 ships [`MockMoveExecutor`], which simulates the canonical
/// `Coin<T>` `transfer` entry point so cross-VM parity tests can stay
/// green while the trait shape changes (S9.2 → S9.4). S9.5 swaps in
/// `AptosMoveExecutor` backed by `move-vm-runtime`.
pub trait MoveExecutor: Send + Sync + std::fmt::Debug {
    /// Execute one entry function in the Move VM.
    ///
    /// # Errors
    ///
    /// See [`MoveExecutionError`].
    fn execute(
        &self,
        call: &MoveCall,
        modules: &dyn ModuleStore,
        state: &mut MoveSessionState<'_>,
    ) -> Result<MoveOutcome, MoveExecutionError>;
}

/// Mock Move executor for development + testing.
///
/// Simulates a canonical `Coin<T>` `transfer(to: address, amount: u64)`
/// entry point by reading the caller's and recipient's balances from
/// the session's `balance_view`, producing matched [`ResourceWrite`]s,
/// and returning empty `return_values`. Every other entry function
/// returns an empty [`MoveOutcome`] (no writes, no events) — sufficient
/// for the S9.2–S9.4 wiring to compile + cross-VM parity tests to pass
/// without a real Move backend.
///
/// This is deterministic. The bytecode is never parsed; the entry
/// function's name + argument layout are matched directly.
#[derive(Debug, Clone, Copy, Default)]
pub struct MockMoveExecutor;

/// Address of the canonical gsx-db `Coin<T>` module (`0x1::coin`).
/// Matches Aptos's `0x1::coin` location.
pub const CANONICAL_COIN_ADDRESS: MoveAddress = MoveAddress([
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    1,
]);

impl MoveExecutor for MockMoveExecutor {
    fn execute(
        &self,
        call: &MoveCall,
        _modules: &dyn ModuleStore,
        state: &mut MoveSessionState<'_>,
    ) -> Result<MoveOutcome, MoveExecutionError> {
        // Only the canonical 0x1::coin::transfer entry is simulated;
        // every other call returns an empty outcome (no state change).
        let is_canonical_coin = call.module.address == CANONICAL_COIN_ADDRESS
            && call.module.name.as_str() == "coin";
        let is_transfer = call.function.as_str() == "transfer";
        if !(is_canonical_coin && is_transfer) {
            return Ok(MoveOutcome {
                return_values: Vec::new(),
                events: Vec::new(),
                resource_writes: Vec::new(),
            });
        }

        // transfer(to: address, amount: u64)
        if call.arguments.len() != 2 {
            return Err(MoveExecutionError::InvalidArguments(format!(
                "0x1::coin::transfer expects 2 arguments, got {}",
                call.arguments.len()
            )));
        }
        let to_bytes: &[u8; 32] = call.arguments[0].as_slice().try_into().map_err(|_| {
            MoveExecutionError::InvalidArguments(
                "0x1::coin::transfer: arg 0 must be 32-byte address".into(),
            )
        })?;
        let to_addr = MoveAddress(*to_bytes);
        let amount_bytes: &[u8; 8] = call.arguments[1].as_slice().try_into().map_err(|_| {
            MoveExecutionError::InvalidArguments(
                "0x1::coin::transfer: arg 1 must be u64 (8 bytes LE)".into(),
            )
        })?;
        let amount = u64::from_le_bytes(*amount_bytes);

        let caller_balance = state.balance_view.coin_value(&call.caller).to_u128();
        if u128::from(amount) > caller_balance {
            return Err(MoveExecutionError::Abort {
                code: ABORT_INSUFFICIENT_BALANCE,
                location: AbortLocation {
                    module: Some(call.module.clone()),
                    function_index: 0,
                    instruction_index: 0,
                },
            });
        }

        let caller_new = caller_balance - u128::from(amount);
        let recipient_new = state.balance_view.coin_value(&to_addr).to_u128() + u128::from(amount);
        let caller_nonce = state.balance_view.nonce(&call.caller).next();
        let recipient_nonce = state.balance_view.nonce(&to_addr);

        state.buffered_writes.push(ResourceWrite {
            addr: call.caller,
            coin_value: MoveCoinValue::from_u128(caller_new),
            nonce: caller_nonce,
        });
        state.buffered_writes.push(ResourceWrite {
            addr: to_addr,
            coin_value: MoveCoinValue::from_u128(recipient_new),
            nonce: recipient_nonce,
        });

        Ok(MoveOutcome {
            return_values: Vec::new(),
            events: Vec::new(),
            resource_writes: state.buffered_writes.clone(),
        })
    }
}

/// Standard Move abort code for "insufficient balance" — chosen to match
/// Aptos's `aptos_framework::coin::EINSUFFICIENT_BALANCE` so the abort
/// is wire-comparable when the real backend lands in S9.5.
pub const ABORT_INSUFFICIENT_BALANCE: u64 = 0x10006;

/// Production Move executor backed by Aptos `move-vm-runtime`.
///
/// **Scaffold only as of S9.5a.** The real `move-vm-runtime` integration
/// is deferred to S9.5b — see the dep-choice notes below. With
/// `production-move-executor` enabled the executor compiles + the trait
/// is satisfied; execution returns `MoveExecutionError::UnsupportedScheme`
/// equivalents until 5b lands the actual interpreter calls.
///
/// ## Why S9.5 splits into three sub-PRs
///
/// Aptos's Move VM crates (`move-binary-format`, `move-bytecode-verifier`,
/// `move-vm-runtime`, `move-vm-types`, `move-core-types`, `aptos-move-stdlib`)
/// are **not on crates.io** in usable form (only `move-core-types = "0.0.3"`,
/// 2021-vintage). Pulling from `aptos-labs/aptos-core` git brings in 100+
/// transitive crates, ~500 MB of target dir, multiple new licenses for
/// cargo-deny, and a 10–30 min first build. That work doesn't fit into one
/// session honestly, so S9.5 lands in three PRs:
///
/// - **S9.5a (this PR)** — scaffold + dep-choice docs. No `aptos-core`
///   pull. Executor compiles + returns errors that say "not yet wired."
/// - **S9.5b** — git deps + real `move-vm-runtime` integration. Resolves
///   the cargo-deny advisory + license fallout in one go.
/// - **S9.5c** — compile + bundle the canonical gsx-db `Coin<T>` Move
///   module (mirrors `aptos_framework::coin` ABI minus the bits we
///   don't use). Land production-move-executor ON by default.
///
/// ## Dep choices (S9.5b's job)
///
/// Target version: latest `aptos-release-v1.x` stable. Pin by tag, not
/// branch (per the gsx-dag lane-auditor convention — `version = "*"` +
/// git tag is the supported shape). Minimum subset:
///
/// - `move-core-types` — `ModuleId`, `Identifier`, `TypeTag`
/// - `move-binary-format` — `CompiledModule::deserialize`
/// - `move-bytecode-verifier` — runs at deploy + load time
/// - `move-vm-runtime` — `MoveVM`, `Session`
/// - `move-vm-types` — `Value`, types the runtime needs
/// - `aptos-move-stdlib` — pinned subset of natives (`vector`,
///   `option`, `signer`, `error`, `string` — NOT `chain_id`,
///   `timestamp`, or anything I/O-dependent)
///
/// Explicitly NOT pulled (we have our own):
///
/// - `aptos-vm` — the full Aptos block executor
/// - `aptos-state-view` — state access (we route through `MoveBalanceView`)
/// - `aptos-gas-schedule` — gas metering (S12 scope)
/// - `aptos-consensus`, `aptos-storage`, `aptos-network` — full node deps
#[cfg(feature = "production-move-executor")]
#[derive(Debug, Clone, Copy, Default)]
pub struct AptosMoveExecutor;

#[cfg(feature = "production-move-executor")]
impl MoveExecutor for AptosMoveExecutor {
    fn execute(
        &self,
        call: &MoveCall,
        modules: &dyn ModuleStore,
        state: &mut MoveSessionState<'_>,
    ) -> Result<MoveOutcome, MoveExecutionError> {
        use crate::vm::aptos_session::{decode_coin_store, BalanceViewResolver, GsxdbModuleBytes};
        use move_binary_format::file_format::CompiledModule;
        use move_core_types::{
            account_address::AccountAddress,
            identifier::Identifier as AptosIdentifier,
            language_storage::ModuleId as AptosModuleId,
            vm_status::StatusCode,
        };
        use move_vm_runtime::{
            data_cache::{MoveVmDataCacheAdapter, TransactionDataCache},
            module_traversal::{TraversalContext, TraversalStorage},
            move_vm::MoveVM,
            native_extensions::NativeContextExtensions,
            AsUnsyncModuleStorage, EagerLoader, InstantiatedFunctionLoader,
            LegacyLoaderConfig, RuntimeEnvironment,
        };
        use crate::vm::gas::BoundedGasMeter;

        // 1. Module-not-found check.
        let cm = modules
            .get(&call.module)
            .ok_or_else(|| MoveExecutionError::ModuleNotFound(call.module.clone()))?;

        // 2. Deserialize Move bytecode (move-binary-format).
        let compiled = CompiledModule::deserialize(&cm.bytes).map_err(|e| {
            MoveExecutionError::BytecodeVerificationFailed(format!("deserialize: {e:?}"))
        })?;

        // 3. Run the bytecode verifier (move-bytecode-verifier).
        move_bytecode_verifier::verifier::verify_module(&compiled).map_err(|e| {
            MoveExecutionError::BytecodeVerificationFailed(format!("verify: {e:?}"))
        })?;

        // 4. Build the session (S9.5e adapters + S9.5f BalanceViewResolver
        //    + Aptos-provided UnsyncModuleStorage / EagerLoader /
        //    TransactionDataCache / MoveVmDataCacheAdapter / GasMeter /
        //    TraversalContext / NativeContextExtensions).
        let bytes_storage = GsxdbModuleBytes {
            store: modules,
            env: RuntimeEnvironment::new(std::iter::empty()),
        };
        let module_storage = bytes_storage.as_unsync_module_storage();
        let loader = EagerLoader::new(&module_storage);
        let resolver = BalanceViewResolver {
            balance_view: state.balance_view,
        };
        let mut data_cache = TransactionDataCache::empty();
        // Bounded gas: the interpreter charges per-op against this budget
        // and aborts with OUT_OF_GAS if exhausted. Loading stays unmetered
        // (LegacyLoaderConfig::unmetered below); execution is metered.
        let mut gas = BoundedGasMeter::default();
        let traversal_storage = TraversalStorage::new();
        let mut traversal = TraversalContext::new(&traversal_storage);
        let mut extensions = NativeContextExtensions::default();

        // 5. Load the entry function. Convert gsx-db's (MoveAddress, Identifier)
        //    + function name to Aptos's (AccountAddress, ModuleId, Identifier).
        let aptos_module_id = AptosModuleId::new(
            AccountAddress::new(call.module.address.0),
            AptosIdentifier::new(call.module.name.as_str().to_string()).map_err(|e| {
                MoveExecutionError::LinkerError(format!("module-id ident: {e:?}"))
            })?,
        );
        let aptos_function_name = AptosIdentifier::new(call.function.as_str().to_string())
            .map_err(|e| {
                MoveExecutionError::LinkerError(format!("function-name ident: {e:?}"))
            })?;

        let loaded = loader
            .load_instantiated_function(
                &LegacyLoaderConfig::unmetered(),
                &mut gas,
                &mut traversal,
                &aptos_module_id,
                &aptos_function_name,
                &[], // type args — S9.5g converts gsx TypeTag list
            )
            .map_err(|e| MoveExecutionError::LinkerError(format!("load_function: {e:?}")))?;

        // 6. Execute. The Move VM's signature uses serialized BCS arg
        //    bytes — call.arguments is already Vec<Vec<u8>>. The adapter
        //    holds &mut data_cache for the duration of this call; it
        //    drops at the end of the inner block so the data_cache is
        //    free for `into_effects` afterwards.
        {
            let mut adapter =
                MoveVmDataCacheAdapter::new(&mut data_cache, &resolver, &loader);
            MoveVM::execute_loaded_function(
                loaded,
                call.arguments.clone(),
                &mut adapter,
                &mut gas,
                &mut traversal,
                &mut extensions,
                &loader,
            )
            .map_err(|e| {
                // Distinguish gas exhaustion from a contract abort: now that
                // BoundedGasMeter can fail with OUT_OF_GAS, collapsing every
                // VM error into `Abort { code: 0 }` would make Out-Of-Gas
                // indistinguishable from a code-0 abort for callers/telemetry.
                if e.major_status() == StatusCode::OUT_OF_GAS {
                    MoveExecutionError::OutOfGas
                } else {
                    MoveExecutionError::Abort {
                        code: 0,
                        location: AbortLocation {
                            module: Some(call.module.clone()),
                            function_index: 0,
                            instruction_index: 0,
                        },
                    }
                }
            })?;
        }

        // 7. Extract `ResourceWrite`s from the data_cache.
        //
        // `into_effects` walks every modified resource and returns a
        // `Changes<Bytes>` keyed by `(account, struct_tag)`. We filter
        // for the canonical `0x1::coin::CoinStore` resource (our only
        // recognised state-bearing type today), BCS-decode the 16-byte
        // payload, and translate to `ResourceWrite`. Non-CoinStore
        // resource writes are silently dropped — S9.5g+ extend the
        // schema, but until then anything else is out of scope for the
        // dual-projection invariant.
        let changes = data_cache
            .into_effects(&module_storage)
            .map_err(|e| MoveExecutionError::LinkerError(format!("into_effects: {e:?}")))?;
        let mut resource_writes = Vec::new();
        for (aptos_addr, account_changes) in changes.accounts() {
            let our_addr = MoveAddress(aptos_addr.into_bytes());
            for (struct_tag, op) in account_changes.resources() {
                let is_coin_store = struct_tag.address == AccountAddress::new([
                    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                    0, 0, 0, 0, 0, 1,
                ]) && struct_tag.module.as_str() == "coin"
                    && struct_tag.name.as_str() == "CoinStore";
                if !is_coin_store {
                    continue;
                }
                let bytes = match op.as_ref().ok() {
                    Some(b) => b,
                    None => continue, // Delete op — handled by callers via separate signal
                };
                if let Some((value, sequence)) = decode_coin_store(bytes) {
                    resource_writes.push(ResourceWrite {
                        addr: our_addr,
                        coin_value: MoveCoinValue::from_u128(u128::from(value)),
                        nonce: AccountNonce::new(sequence),
                    });
                }
            }
        }

        Ok(MoveOutcome {
            return_values: Vec::new(),
            events: Vec::new(),
            resource_writes,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::BalanceSlot;
    use std::collections::HashMap;

    /// In-memory `MoveBalanceView` for tests.
    #[derive(Debug, Default)]
    struct InMemoryView {
        balances: HashMap<MoveAddress, BalanceSlot>,
    }

    impl InMemoryView {
        fn set(&mut self, addr: MoveAddress, slot: BalanceSlot) {
            self.balances.insert(addr, slot);
        }
    }

    impl MoveBalanceView for InMemoryView {
        fn coin_value(&self, addr: &MoveAddress) -> MoveCoinValue {
            self.balances
                .get(addr)
                .copied()
                .unwrap_or_else(|| BalanceSlot::new(0))
                .move_coin_value()
        }
        fn nonce(&self, addr: &MoveAddress) -> AccountNonce {
            self.balances
                .get(addr)
                .copied()
                .unwrap_or_else(|| BalanceSlot::new(0))
                .nonce()
        }
    }

    // Empty store stand-in for executor tests — Mock never reads from
    // `modules`. We use the real `InMemoryModuleStore::new()` here so
    // the tests double as smoke tests for the impl.
    fn empty_store() -> InMemoryModuleStore {
        InMemoryModuleStore::new()
    }

    fn move_addr(byte: u8) -> MoveAddress {
        let mut bytes = [0u8; 32];
        bytes[31] = byte;
        MoveAddress(bytes)
    }

    fn coin_module() -> ModuleId {
        ModuleId {
            address: CANONICAL_COIN_ADDRESS,
            name: Identifier::new("coin").unwrap(),
        }
    }

    fn transfer_call(from: MoveAddress, to: MoveAddress, amount: u64) -> MoveCall {
        MoveCall {
            caller: from,
            module: coin_module(),
            function: Identifier::new("transfer").unwrap(),
            type_arguments: Vec::new(),
            arguments: vec![to.0.to_vec(), amount.to_le_bytes().to_vec()],
        }
    }

    #[test]
    fn identifier_rejects_empty() {
        assert_eq!(
            Identifier::new(""),
            Err(IdentifierError::Empty)
        );
    }

    #[test]
    fn identifier_rejects_leading_digit() {
        assert!(matches!(
            Identifier::new("1foo"),
            Err(IdentifierError::InvalidLeadingByte(b'1'))
        ));
    }

    #[test]
    fn identifier_accepts_underscore_leading() {
        assert!(Identifier::new("_coin").is_ok());
    }

    #[test]
    fn identifier_rejects_special_chars() {
        assert!(matches!(
            Identifier::new("co-in"),
            Err(IdentifierError::InvalidByte(b'-'))
        ));
    }

    #[test]
    fn module_store_round_trips_get_put_contains() {
        let mut store = InMemoryModuleStore::new();
        let id = coin_module();
        let module = CompiledModule {
            bytes: vec![0xCA, 0xFE, 0xBA, 0xBE, 1, 2, 3],
        };
        assert!(!store.contains(&id));
        assert!(store.get(&id).is_none());
        assert!(store.is_empty());
        assert_eq!(store.len(), 0);

        store.put(id.clone(), module.clone()).expect("first put");
        assert!(store.contains(&id));
        assert_eq!(store.get(&id), Some(module.clone()));
        assert!(!store.is_empty());
        assert_eq!(store.len(), 1);

        // Re-deploy rejected.
        let err = store.put(id.clone(), module).expect_err("re-deploy");
        assert!(matches!(err, ModuleStoreError::AlreadyExists(_)));
        assert_eq!(store.len(), 1);
    }

    #[test]
    fn module_store_distinguishes_by_address_and_name() {
        let mut store = InMemoryModuleStore::new();
        let id_a = ModuleId {
            address: move_addr(1),
            name: Identifier::new("a").unwrap(),
        };
        let id_b = ModuleId {
            address: move_addr(1),
            name: Identifier::new("b").unwrap(),
        };
        let id_c = ModuleId {
            address: move_addr(2),
            name: Identifier::new("a").unwrap(),
        };
        store
            .put(id_a.clone(), CompiledModule { bytes: vec![1] })
            .unwrap();
        store
            .put(id_b.clone(), CompiledModule { bytes: vec![2] })
            .unwrap();
        store
            .put(id_c.clone(), CompiledModule { bytes: vec![3] })
            .unwrap();
        assert_eq!(store.len(), 3);
        assert_eq!(store.get(&id_a).unwrap().bytes, vec![1]);
        assert_eq!(store.get(&id_b).unwrap().bytes, vec![2]);
        assert_eq!(store.get(&id_c).unwrap().bytes, vec![3]);
    }

    #[test]
    fn mock_transfer_succeeds_with_matching_writes() {
        let alice = move_addr(1);
        let bob = move_addr(2);
        let mut view = InMemoryView::default();
        view.set(alice, BalanceSlot::new(1000));
        view.set(bob, BalanceSlot::new(500));

        let mut state = MoveSessionState::new(&view);
        let outcome = MockMoveExecutor
            .execute(&transfer_call(alice, bob, 250), &empty_store(), &mut state)
            .expect("transfer should succeed");

        assert_eq!(outcome.resource_writes.len(), 2);

        let alice_write = &outcome.resource_writes[0];
        assert_eq!(alice_write.addr, alice);
        assert_eq!(alice_write.coin_value.to_u128(), 750);
        assert_eq!(alice_write.nonce.value, 1);

        let bob_write = &outcome.resource_writes[1];
        assert_eq!(bob_write.addr, bob);
        assert_eq!(bob_write.coin_value.to_u128(), 750);
        assert_eq!(bob_write.nonce.value, 0);
    }

    #[test]
    fn mock_transfer_aborts_on_insufficient_balance() {
        let alice = move_addr(1);
        let bob = move_addr(2);
        let mut view = InMemoryView::default();
        view.set(alice, BalanceSlot::new(100));

        let mut state = MoveSessionState::new(&view);
        let err = MockMoveExecutor
            .execute(&transfer_call(alice, bob, 500), &empty_store(), &mut state)
            .expect_err("should abort");

        assert!(matches!(
            err,
            MoveExecutionError::Abort {
                code: ABORT_INSUFFICIENT_BALANCE,
                ..
            }
        ));
        assert!(state.buffered_writes.is_empty());
    }

    #[test]
    fn mock_returns_empty_for_unknown_module() {
        let alice = move_addr(1);
        let view = InMemoryView::default();
        let mut state = MoveSessionState::new(&view);
        let call = MoveCall {
            caller: alice,
            module: ModuleId {
                address: move_addr(99),
                name: Identifier::new("not_coin").unwrap(),
            },
            function: Identifier::new("transfer").unwrap(),
            type_arguments: Vec::new(),
            arguments: vec![alice.0.to_vec(), 10u64.to_le_bytes().to_vec()],
        };

        let outcome = MockMoveExecutor
            .execute(&call, &empty_store(), &mut state)
            .expect("unknown module should return empty outcome");
        assert!(outcome.resource_writes.is_empty());
        assert!(outcome.return_values.is_empty());
        assert!(outcome.events.is_empty());
    }

    #[test]
    fn mock_rejects_wrong_arg_count() {
        let alice = move_addr(1);
        let view = InMemoryView::default();
        let mut state = MoveSessionState::new(&view);
        let call = MoveCall {
            caller: alice,
            module: coin_module(),
            function: Identifier::new("transfer").unwrap(),
            type_arguments: Vec::new(),
            arguments: vec![alice.0.to_vec()], // missing amount
        };

        let err = MockMoveExecutor
            .execute(&call, &empty_store(), &mut state)
            .expect_err("wrong arg count should reject");
        assert!(matches!(err, MoveExecutionError::InvalidArguments(_)));
    }

    #[cfg(feature = "production-move-executor")]
    #[test]
    fn aptos_executor_rejects_unknown_module() {
        let alice = move_addr(1);
        let view = InMemoryView::default();
        let mut state = MoveSessionState::new(&view);
        let executor = AptosMoveExecutor;
        let err = executor
            .execute(&transfer_call(alice, move_addr(2), 1), &empty_store(), &mut state)
            .expect_err("unknown module rejects");
        assert!(matches!(err, MoveExecutionError::ModuleNotFound(_)));
    }

    #[cfg(feature = "production-move-executor")]
    #[test]
    fn aptos_executor_canonical_coin_bytecode_reaches_interpreter() {
        // S9.5g end-to-end smoke test: deploy the compiled canonical
        // Coin module bytecode, invoke balance() on a fresh address,
        // and confirm AptosMoveExecutor::execute drives the full
        // pipeline through to MoveVM::execute_loaded_function.
        //
        // We use the view function balance(addr) — it reads CoinStore
        // via borrow_global. With no resource at the address +
        // BalanceViewResolver returning the zero-valued CoinStore,
        // borrow_global succeeds and balance() returns 0. Test passes
        // iff the executor reports Ok(MoveOutcome).
        use crate::vm::aptos_session::{canonical_coin_bytecode, canonical_coin_module_id};

        let alice = move_addr(1);
        let view = InMemoryView::default();
        let mut state = MoveSessionState::new(&view);

        let mut modules = InMemoryModuleStore::new();
        modules
            .put(
                canonical_coin_module_id(),
                CompiledModule {
                    bytes: canonical_coin_bytecode().to_vec(),
                },
            )
            .unwrap();

        let executor = AptosMoveExecutor;
        let call = MoveCall {
            caller: alice,
            module: canonical_coin_module_id(),
            function: Identifier::new("balance").unwrap(),
            type_arguments: Vec::new(),
            arguments: vec![alice.0.to_vec()],
        };

        let outcome = executor
            .execute(&call, &modules, &mut state)
            .expect("balance() reaches the interpreter and returns Ok");

        // balance() is a #[view] function: no resource writes, no
        // events. The return value comes back as SerializedReturnValues
        // which we currently don't surface (S9.5g+ will).
        assert!(outcome.resource_writes.is_empty());
        assert!(outcome.events.is_empty());
    }

    #[cfg(feature = "production-move-executor")]
    #[test]
    fn aptos_executor_rejects_malformed_bytecode() {
        // S9.5c: bytecode-format verification is wired. Random bytes
        // fail CompiledModule::deserialize and surface as
        // BytecodeVerificationFailed before the executor ever attempts
        // execution.
        let alice = move_addr(1);
        let view = InMemoryView::default();
        let mut state = MoveSessionState::new(&view);
        let mut modules = InMemoryModuleStore::new();
        modules
            .put(
                coin_module(),
                CompiledModule {
                    bytes: vec![0xCA, 0xFE, 0xBA, 0xBE, 0xDE, 0xAD, 0xBE, 0xEF],
                },
            )
            .unwrap();
        let executor = AptosMoveExecutor;
        let err = executor
            .execute(&transfer_call(alice, move_addr(2), 1), &modules, &mut state)
            .expect_err("malformed bytecode rejected");
        match err {
            MoveExecutionError::BytecodeVerificationFailed(msg) => {
                assert!(msg.contains("deserialize"), "msg was {msg}");
            }
            other => panic!("expected BytecodeVerificationFailed, got {other:?}"),
        }
    }

    #[test]
    fn mock_rejects_malformed_address_arg() {
        let alice = move_addr(1);
        let view = InMemoryView::default();
        let mut state = MoveSessionState::new(&view);
        let call = MoveCall {
            caller: alice,
            module: coin_module(),
            function: Identifier::new("transfer").unwrap(),
            type_arguments: Vec::new(),
            arguments: vec![vec![1, 2, 3], 10u64.to_le_bytes().to_vec()],
        };

        let err = MockMoveExecutor
            .execute(&call, &empty_store(), &mut state)
            .expect_err("malformed address should reject");
        assert!(matches!(err, MoveExecutionError::InvalidArguments(_)));
    }
}
