//! Nonce semantics — EVM nonce ↔ Move sequence number mapping.
//!
//! EVM uses explicit nonces (transaction count). Move (Aptos) uses implicit sequence numbers
//! derived from account state. This module provides a unified abstraction for Proposition 1.

/// EVM transaction nonce (u64).
/// Incremented on every transaction initiated by the account.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct EvmNonce(pub u64);

impl EvmNonce {
    /// Increment the nonce for the next transaction.
    #[must_use]
    pub fn next(self) -> Self {
        EvmNonce(self.0.saturating_add(1))
    }

    /// Check if a nonce value is valid for executing the next transaction.
    #[must_use]
    pub fn is_valid_for_next_tx(self, incoming_nonce: u64) -> bool {
        incoming_nonce == self.0
    }
}

/// Move/Aptos sequence number (u64).
/// Incremented on every transaction from the account (same semantics as EVM nonce).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct MoveSequenceNumber(pub u64);

impl MoveSequenceNumber {
    /// Increment the sequence number for the next transaction.
    #[must_use]
    pub fn next(self) -> Self {
        MoveSequenceNumber(self.0.saturating_add(1))
    }

    /// Check if a sequence number is valid for executing the next transaction.
    #[must_use]
    pub fn is_valid_for_next_tx(self, incoming_seq: u64) -> bool {
        incoming_seq == self.0
    }
}

/// Unified account nonce across both EVM and Move.
///
/// For Proposition 1 (dual-VM consistency), the EVM nonce and Move sequence number
/// must always be equal. This struct enforces that invariant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[derive(Default)]
pub struct AccountNonce {
    /// Current value (shared between both VMs).
    pub value: u64,
}

impl AccountNonce {
    /// Create a new nonce with the given value.
    #[must_use]
    pub fn new(value: u64) -> Self {
        AccountNonce { value }
    }

    /// Get the EVM nonce view.
    #[must_use]
    pub fn evm_nonce(&self) -> EvmNonce {
        EvmNonce(self.value)
    }

    /// Get the Move sequence number view.
    #[must_use]
    pub fn move_sequence(&self) -> MoveSequenceNumber {
        MoveSequenceNumber(self.value)
    }

    /// Increment for both VMs atomically.
    #[must_use]
    pub fn next(&self) -> Self {
        AccountNonce {
            value: self.value.saturating_add(1),
        }
    }

    /// Check if an incoming nonce is valid for the next transaction (works for both VMs).
    #[must_use]
    pub fn is_valid_next(&self, incoming: u64) -> bool {
        incoming == self.value
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evm_nonce_increment() {
        let nonce = EvmNonce(10);
        assert_eq!(nonce.next().0, 11);
        assert!(nonce.is_valid_for_next_tx(10));
        assert!(!nonce.is_valid_for_next_tx(11));
    }

    #[test]
    fn move_sequence_increment() {
        let seq = MoveSequenceNumber(5);
        assert_eq!(seq.next().0, 6);
        assert!(seq.is_valid_for_next_tx(5));
        assert!(!seq.is_valid_for_next_tx(6));
    }

    #[test]
    fn account_nonce_unified() {
        let nonce = AccountNonce::new(42);
        assert_eq!(nonce.evm_nonce().0, 42);
        assert_eq!(nonce.move_sequence().0, 42);

        let next = nonce.next();
        assert_eq!(next.evm_nonce().0, 43);
        assert_eq!(next.move_sequence().0, 43);
    }

    #[test]
    fn account_nonce_validation() {
        let nonce = AccountNonce::new(100);
        assert!(nonce.is_valid_next(100));
        assert!(!nonce.is_valid_next(99));
        assert!(!nonce.is_valid_next(101));
    }

    #[test]
    fn saturating_add_on_overflow() {
        let nonce = EvmNonce(u64::MAX);
        assert_eq!(nonce.next().0, u64::MAX); // Saturates
    }
}
