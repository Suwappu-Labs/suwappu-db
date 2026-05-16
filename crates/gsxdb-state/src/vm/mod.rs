//! VM-shape transaction types and read-side projectors.
//!
//! The dual-VM design's promise is that an EVM-shaped transaction and a
//! Move-shaped transaction expressing the same logical operation produce
//! identical canonical state. This module holds the typed entry points
//! (`tx`), the read paths (`projector`), and the Move executor call-site
//! (`executor`). Execution paths live in `gsxdb-bridge::vm` because they
//! mutate state through the `BridgeToken` capability gate.
//!
//! See `docs/spec/move-execution.md` for the S9 execution model.

#[cfg(feature = "production-move-executor")]
pub mod aptos_session;
pub mod executor;
pub mod projector;
pub mod tx;

pub use executor::{
    AbortLocation, CompiledModule, Identifier, IdentifierError, InMemoryModuleStore,
    MockMoveExecutor, ModuleId, ModuleStore, ModuleStoreError, MoveBalanceView, MoveCall,
    MoveEvent, MoveExecutionError, MoveExecutor, MoveOutcome, MoveSessionState, ResourceWrite,
    StructTag, TypeTag, ABORT_INSUFFICIENT_BALANCE, CANONICAL_COIN_ADDRESS,
};
#[cfg(feature = "production-move-executor")]
pub use executor::AptosMoveExecutor;
#[cfg(feature = "production-move-executor")]
pub use aptos_session::{canonical_coin_bytecode, canonical_coin_module_id};
pub use projector::{EvmProjector, EvmView, MoveProjector, MoveView};
pub use tx::{CanonicalTransfer, EvmTx, MoveTx};
