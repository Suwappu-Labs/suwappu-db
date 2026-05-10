//! VM-shape transaction types and read-side projectors.
//!
//! The dual-VM design's promise is that an EVM-shaped transaction and a
//! Move-shaped transaction expressing the same logical operation produce
//! identical canonical state. This module holds the typed entry points
//! (`tx`) and the read paths (`projector`) that operationalise that
//! promise. The execution paths live in `gsxdb-bridge::vm` because they
//! mutate state through the `BridgeToken` capability gate.

pub mod executor;
pub mod projector;
pub mod tx;

pub use executor::{ExecutionOutcome, MockMoveExecutor, MoveExecutor};
#[cfg(feature = "production-move-executor")]
pub use executor::AptosMoveExecutor;
pub use projector::{EvmProjector, EvmView, MoveProjector, MoveView};
pub use tx::{CanonicalTransfer, EvmTx, MoveTx};
