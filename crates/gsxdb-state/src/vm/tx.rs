//! VM-shape transaction types.
//!
//! Both [`EvmTx`] and [`MoveTx`] canonicalise to a single VM-agnostic
//! shape. The bridge consumes the canonical shape; the VM-typed wrappers
//! exist so transaction encoders can produce them in the shape native to
//! their VM (EVM ABI in one case, Move BCS in the other) without the
//! bridge needing to know which VM the transaction came from.
//!
//! Phase-1 only models the transfer primitive. Future slices (S5
//! cross-VM intent queue) extend this with contract calls / Move entry
//! function calls.

use crate::Address;

/// EVM-shaped transfer transaction.
///
/// Field names match the EVM mental model: `from` is the signing
/// account, `to` is the recipient, `value` is the wei-equivalent amount,
/// `nonce` is the caller-supplied transaction sequence the EVM validates
/// against the sender's current account nonce. Callers in EVM-flavoured
/// code paths build these directly; the canonical-intent shape consumed
/// by the bridge is opaque to them.
///
/// The mock executor (`MockEvm`) and pre-real-revm bundle paths ignore
/// `nonce` — it's only consulted by `RevmExecutor` when the
/// `production-evm-executor` feature is enabled. Real revm rejects the
/// transaction with `NonceTooLow` / `NonceTooHigh` if `nonce` does not
/// equal the sender's current account nonce, which is what makes
/// envelope-supplied nonces the replay-defence boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EvmTx {
    /// Sending account (EVM `msg.sender` for an externally-owned account).
    pub from: Address,
    /// Recipient account.
    pub to: Address,
    /// Transfer amount in canonical units (1:1 with `BalanceSlot`'s u128).
    pub value: u128,
    /// Caller-supplied transaction nonce. Must equal the sender's
    /// current account nonce for the EVM to accept the transaction;
    /// otherwise the EVM rejects with `NonceTooLow` / `NonceTooHigh`.
    /// Mock paths ignore this field.
    pub nonce: u64,
}

/// Move-shaped transfer transaction.
///
/// Field names match the Sui / Aptos mental model: `signer` is the
/// transaction author, `recipient` is the destination, `amount` is the
/// `Coin::value`-equivalent amount.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MoveTx {
    /// Signing account (Move transaction sender).
    pub signer: Address,
    /// Recipient account.
    pub recipient: Address,
    /// Transfer amount in canonical units (1:1 with `BalanceSlot`'s u128).
    pub amount: u128,
}

/// Canonicalisation target. Both VM transaction shapes flatten to this
/// representation before reaching the bridge.
///
/// Kept as a private struct in this module so callers think in
/// VM-shaped types; the bridge's `Intent` type is the public wire shape
/// and we reconstruct it via [`EvmTx::to_canonical`] /
/// [`MoveTx::to_canonical`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CanonicalTransfer {
    /// Source address.
    pub from: Address,
    /// Destination address.
    pub to: Address,
    /// Transfer amount.
    pub amount: u128,
}

impl EvmTx {
    /// Project to the VM-agnostic canonical form.
    #[must_use]
    pub fn to_canonical(self) -> CanonicalTransfer {
        CanonicalTransfer {
            from: self.from,
            to: self.to,
            amount: self.value,
        }
    }
}

impl MoveTx {
    /// Project to the VM-agnostic canonical form.
    #[must_use]
    pub fn to_canonical(self) -> CanonicalTransfer {
        CanonicalTransfer {
            from: self.signer,
            to: self.recipient,
            amount: self.amount,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(byte: u8) -> Address {
        Address([byte; 20])
    }

    #[test]
    fn evm_and_move_canonicalise_to_same_transfer() {
        let evm = EvmTx {
            from: addr(1),
            to: addr(2),
            value: 100,
            nonce: 0,
        };
        let mv = MoveTx {
            signer: addr(1),
            recipient: addr(2),
            amount: 100,
        };

        assert_eq!(evm.to_canonical(), mv.to_canonical());
    }

    #[test]
    fn distinct_amounts_canonicalise_distinctly() {
        let a = EvmTx {
            from: addr(1),
            to: addr(2),
            value: 100,
            nonce: 0,
        }
        .to_canonical();
        let b = EvmTx {
            from: addr(1),
            to: addr(2),
            value: 101,
            nonce: 0,
        }
        .to_canonical();

        assert_ne!(a, b);
    }

    #[test]
    fn nonce_is_excluded_from_canonical_form() {
        // The canonical transfer is the wire-level intent; the envelope
        // nonce is consumed at validation time by the EVM and does not
        // surface to the bridge. Two tx envelopes that differ only in
        // nonce produce the same canonical transfer.
        let a = EvmTx {
            from: addr(1),
            to: addr(2),
            value: 100,
            nonce: 0,
        }
        .to_canonical();
        let b = EvmTx {
            from: addr(1),
            to: addr(2),
            value: 100,
            nonce: 42,
        }
        .to_canonical();

        assert_eq!(a, b);
    }

    #[test]
    fn fields_round_trip_through_canonical_form() {
        let evm = EvmTx {
            from: addr(1),
            to: addr(2),
            value: 999,
            nonce: 7,
        };
        let c = evm.to_canonical();
        assert_eq!(c.from, evm.from);
        assert_eq!(c.to, evm.to);
        assert_eq!(c.amount, evm.value);

        let mv = MoveTx {
            signer: addr(1),
            recipient: addr(2),
            amount: 999,
        };
        let c = mv.to_canonical();
        assert_eq!(c.from, mv.signer);
        assert_eq!(c.to, mv.recipient);
        assert_eq!(c.amount, mv.amount);
    }
}
