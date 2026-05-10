//! Address shape mapping — handle EVM (20-byte) ↔ Move/Aptos (32-byte) conversions.
//!
//! GSX-DB's canonical state uses EVM-shaped 20-byte addresses. When interacting with
//! Move VMs (which use 32-byte addresses), this module provides bidirectional mapping.

use crate::Address;

/// Move/Aptos-shaped address (32 bytes).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MoveAddress(pub [u8; 32]);

impl MoveAddress {
    /// Project a Move address to EVM shape (20 bytes).
    ///
    /// **Strategy:** Take the last 20 bytes of the 32-byte Move address.
    /// This preserves the significant bits for most Aptos addresses while fitting EVM shape.
    ///
    /// For addresses with leading zeros (common in Aptos for object/collection addresses),
    /// this captures the meaningful suffix. The projection is deterministic but not bijective.
    pub fn to_evm(&self) -> Address {
        let mut addr = [0u8; 20];
        // Take the last 20 bytes: Aptos addresses are often zero-padded on the left,
        // so the meaningful bits are on the right.
        addr.copy_from_slice(&self.0[12..32]);
        Address(addr)
    }

    /// Lift an EVM address to Move shape (32 bytes).
    ///
    /// **Strategy:** Pad the 20-byte EVM address with 12 zero bytes on the left.
    /// This is the inverse of `to_evm()` for EVM-originated addresses.
    ///
    /// For native Aptos addresses (which have meaningful leading bytes), this is lossy;
    /// use only for addresses known to originate from EVM.
    pub fn from_evm(addr: &Address) -> Self {
        let mut move_addr = [0u8; 32];
        // Pad with zeros on the left, EVM address on the right
        move_addr[12..32].copy_from_slice(&addr.0);
        MoveAddress(move_addr)
    }

    /// Parse a hex string as a Move address (32 bytes, 0x-prefixed or raw).
    pub fn from_hex(s: &str) -> Result<Self, String> {
        let hex_str = if s.starts_with("0x") {
            &s[2..]
        } else {
            s
        };

        if hex_str.len() != 64 {
            return Err(format!(
                "Move address must be 32 bytes (64 hex chars), got {}",
                hex_str.len()
            ));
        }

        let bytes =
            hex::decode(hex_str).map_err(|e| format!("Invalid hex: {}", e))?;
        let mut addr = [0u8; 32];
        addr.copy_from_slice(&bytes);
        Ok(MoveAddress(addr))
    }

    /// Format as 0x-prefixed hex string.
    pub fn to_hex(&self) -> String {
        format!("0x{}", hex::encode(self.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evm_move_roundtrip_for_evm_addresses() {
        let evm = Address([1u8; 20]);
        let move_addr = MoveAddress::from_evm(&evm);
        let back = move_addr.to_evm();
        assert_eq!(back, evm);
    }

    #[test]
    fn move_to_evm_takes_last_20_bytes() {
        let mut move_addr = [0u8; 32];
        move_addr[0] = 255;  // Leading byte
        move_addr[12] = 42;  // At position 12
        move_addr[31] = 99;  // Last byte

        let move_addr = MoveAddress(move_addr);
        let evm = move_addr.to_evm();

        // The EVM address should be the last 20 bytes
        assert_eq!(evm.0[0], 42);    // Position 12 → position 0 in EVM
        assert_eq!(evm.0[19], 99);   // Position 31 → position 19 in EVM
    }

    #[test]
    fn hex_parsing() {
        let hex = "0x0000000000000000000000000000000000000000000000000000000000000001";
        let addr = MoveAddress::from_hex(hex).expect("parse");
        assert_eq!(addr.0[31], 1);
        assert_eq!(addr.to_hex(), hex.to_lowercase());
    }

    #[test]
    fn hex_parsing_invalid_length() {
        let hex = "0x01";
        let result = MoveAddress::from_hex(hex);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("64 hex chars"));
    }
}
