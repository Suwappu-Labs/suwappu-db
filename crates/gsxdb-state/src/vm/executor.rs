//! Move VM execution framework.
//!
//! GSX-DB delegates Move bytecode execution to an abstract executor trait.
//! This allows:
//!
//! - **Mock mode** (development/testing): Simple in-memory state machine
//!   without real bytecode execution. Default for Phase 1.
//! - **Production mode** (S9+): Real Aptos move-vm-runtime executing Move bytecode
//!   compiled to canonical Aptos bytecode format.
//!
//! ## Feature Gates
//!
//! - `(none)` or `mock-move-executor` — uses [`MockMoveExecutor`]
//! - `production-move-executor` — uses [`AptosMoveExecutor`] (requires aptos-core)
//!
//! The executor is invoked at transaction commit time by the bridge's
//! `MoveProjector` to validate that a transition respects Move invariants.

use crate::{AccountNonce, BalanceSlot, MoveAddress, MoveCoinValue};

/// Result of Move bytecode execution against an account's resources.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionOutcome {
    /// Execution succeeded, producing the new coin value and sequence number.
    Success {
        /// The account's coin balance after execution.
        coin_value: MoveCoinValue,
        /// The account's sequence number after execution.
        sequence: AccountNonce,
    },
    /// Execution failed (out of gas, abort, invalid bytecode, etc).
    Failure,
}

/// Trait for executing Move bytecode.
///
/// Implementations are responsible for:
/// - Loading the compiled Move module(s) for the account
/// - Executing the target function in the Move VM
/// - Extracting the resulting coin value and sequence number
/// - Handling errors gracefully
pub trait MoveExecutor: std::fmt::Debug {
    /// Execute Move bytecode for the account at `addr` with the given initial state.
    ///
    /// Returns the resulting coin value and nonce, or `ExecutionOutcome::Failure` if
    /// the execution failed or the account's resources could not be accessed.
    fn execute(
        &self,
        addr: &MoveAddress,
        initial: BalanceSlot,
    ) -> ExecutionOutcome;
}

/// Mock Move executor for development and testing.
///
/// Implements the simplest possible semantics: reads the input coin value
/// and sequence number directly, with no bytecode execution. This is sufficient
/// for Phase 1 (S1–S8) and for property tests that don't require bytecode semantics.
///
/// The mock executor is deterministic and never fails.
#[derive(Debug, Clone, Copy)]
pub struct MockMoveExecutor;

impl MoveExecutor for MockMoveExecutor {
    fn execute(
        &self,
        _addr: &MoveAddress,
        initial: BalanceSlot,
    ) -> ExecutionOutcome {
        ExecutionOutcome::Success {
            coin_value: initial.move_coin_value(),
            sequence: initial.nonce(),
        }
    }
}

/// Production Move executor using Aptos move-vm-runtime.
///
/// This executor is only available behind the `production-move-executor` feature.
/// It loads and executes real Aptos Move bytecode, validating all Move invariants.
#[cfg(feature = "production-move-executor")]
#[derive(Debug)]
pub struct AptosMoveExecutor {
    // TODO(S9): Wire in aptos-core dependency.
    // Will hold:
    // - Module cache loaded from the block's Move artifact archive
    // - Reference to the runtime (lazy-initialized on first use)
    _phantom: std::marker::PhantomData<()>,
}

#[cfg(feature = "production-move-executor")]
impl MoveExecutor for AptosMoveExecutor {
    fn execute(
        &self,
        _addr: &MoveAddress,
        _initial: BalanceSlot,
    ) -> ExecutionOutcome {
        // TODO(S9): Implement real Aptos move-vm-runtime execution.
        // This is a placeholder that always succeeds with the input state.
        // At S9 close, this will:
        // 1. Load the Move module for addr from the module store
        // 2. Construct a Move interpreter session
        // 3. Execute the canonical entry point (e.g., `AptosCoin::coin_value`)
        // 4. Extract the resulting coin value and sequence number
        // 5. Handle execution errors (abort, out of gas) and return Failure
        ExecutionOutcome::Failure
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mock_executor_preserves_state() {
        let executor = MockMoveExecutor;
        let addr = MoveAddress::from_hex("0x0000000000000000000000000000000000000000000000000000000000000001")
            .unwrap();
        let slot = BalanceSlot::with_nonce(100, AccountNonce::new(42));

        let outcome = executor.execute(&addr, slot);

        match outcome {
            ExecutionOutcome::Success { coin_value, sequence } => {
                assert_eq!(coin_value.to_u128(), 100);
                assert_eq!(sequence.value, 42);
            }
            ExecutionOutcome::Failure => panic!("mock executor should never fail"),
        }
    }

    #[test]
    fn mock_executor_handles_zero_nonce() {
        let executor = MockMoveExecutor;
        let addr = MoveAddress::from_hex("0x0000000000000000000000000000000000000000000000000000000000000002")
            .unwrap();
        let slot = BalanceSlot::new(999);

        let outcome = executor.execute(&addr, slot);

        match outcome {
            ExecutionOutcome::Success { coin_value, sequence } => {
                assert_eq!(coin_value.to_u128(), 999);
                assert_eq!(sequence.value, 0);
            }
            ExecutionOutcome::Failure => panic!("mock executor should never fail"),
        }
    }
}
