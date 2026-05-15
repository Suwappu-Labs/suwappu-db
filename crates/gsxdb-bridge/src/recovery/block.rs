//! Block data type + canonical hash.
//!
//! A block records the inputs to one `BlockExecutor` invocation plus
//! the resulting state root. Replay verifies that re-executing the
//! same intents reaches the same state root.
//!
//! # Encoding
//!
//! Canonical encoding for hashing:
//!
//! ```text
//! "GSXDB-BLOCK/HASH"
//! | height: u64 BE
//! | parent: 32 bytes
//! | state_root: 32 bytes
//! | intent_count: u32 BE
//! | for each intent: encode_intent(intent)
//! ```
//!
//! `encode_intent` is canonical, len-prefixed, type-tagged. Two
//! semantically-equal intents with different in-memory representations
//! must produce the same encoding.

use crate::Intent;
use blake3::Hasher;
use gsxdb_state::Commitment;

/// 32-byte block hash. BLAKE3 of the canonical encoding.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct BlockHash(pub [u8; 32]);

impl std::fmt::Debug for BlockHash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "BlockHash(0x")?;
        for b in &self.0[..4] {
            write!(f, "{b:02x}")?;
        }
        write!(f, "..)")
    }
}

/// Genesis parent — used for height-0 blocks.
pub const GENESIS_PARENT: BlockHash = BlockHash([0; 32]);

/// One block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Block {
    /// Logical block height. Genesis = 0; subsequent = parent.height + 1.
    pub height: u64,
    /// Hash of the parent block. [`GENESIS_PARENT`] for height 0.
    pub parent: BlockHash,
    /// State-tree root commitment after this block's intents were
    /// applied. Replay re-executes the intents and verifies the
    /// resulting tree root equals this field.
    pub state_root: Commitment,
    /// Intents applied in this block, in `BlockExecutor` order.
    pub intents: Vec<Intent>,
}

impl Block {
    /// Compute this block's hash.
    #[must_use]
    pub fn hash(&self) -> BlockHash {
        let mut h = Hasher::new();
        h.update(b"GSXDB-BLOCK/HASH");
        h.update(&self.height.to_be_bytes());
        h.update(&self.parent.0);
        h.update(&self.state_root.0);
        let count = u32::try_from(self.intents.len()).unwrap_or(u32::MAX);
        h.update(&count.to_be_bytes());
        for intent in &self.intents {
            encode_intent(&mut h, intent);
        }
        let mut out = [0u8; 32];
        out.copy_from_slice(h.finalize().as_bytes());
        BlockHash(out)
    }
}

fn encode_intent(h: &mut Hasher, intent: &Intent) {
    match intent {
        Intent::Transfer { from, to, amount } => {
            h.update(&[0u8]); // tag: Transfer
            h.update(&from.0);
            h.update(&to.0);
            h.update(&amount.to_be_bytes());
        }
        Intent::Call {
            caller,
            target,
            value,
            calldata,
        } => {
            h.update(&[1u8]); // tag: Call
            h.update(&caller.0);
            h.update(&target.0);
            h.update(&value.to_be_bytes());
            let len = u32::try_from(calldata.len()).unwrap_or(u32::MAX);
            h.update(&len.to_be_bytes());
            h.update(calldata);
        }
        Intent::DeployModule {
            account,
            name,
            bytes,
        } => {
            h.update(&[2u8]); // tag: DeployModule (S9.3)
            h.update(&account.0);
            let name_bytes = name.as_str().as_bytes();
            let name_len = u32::try_from(name_bytes.len()).unwrap_or(u32::MAX);
            h.update(&name_len.to_be_bytes());
            h.update(name_bytes);
            let bytes_len = u32::try_from(bytes.len()).unwrap_or(u32::MAX);
            h.update(&bytes_len.to_be_bytes());
            h.update(bytes);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gsxdb_state::Address;

    fn addr(byte: u8) -> Address {
        Address([byte; 20])
    }

    #[test]
    fn hash_is_deterministic() {
        let b = Block {
            height: 0,
            parent: GENESIS_PARENT,
            state_root: Commitment([1; 32]),
            intents: vec![Intent::Transfer {
                from: addr(1),
                to: addr(2),
                amount: 100,
            }],
        };
        assert_eq!(b.hash(), b.hash());
    }

    #[test]
    fn hash_differs_for_different_height() {
        let mut a = Block {
            height: 0,
            parent: GENESIS_PARENT,
            state_root: Commitment([1; 32]),
            intents: vec![],
        };
        let b = a.clone();
        a.height = 1;
        assert_ne!(a.hash(), b.hash());
    }

    #[test]
    fn hash_differs_for_different_parent() {
        let mut a = Block {
            height: 1,
            parent: GENESIS_PARENT,
            state_root: Commitment([1; 32]),
            intents: vec![],
        };
        let b = a.clone();
        a.parent = BlockHash([1; 32]);
        assert_ne!(a.hash(), b.hash());
    }

    #[test]
    fn hash_differs_for_different_state_root() {
        let mut a = Block {
            height: 0,
            parent: GENESIS_PARENT,
            state_root: Commitment([1; 32]),
            intents: vec![],
        };
        let b = a.clone();
        a.state_root = Commitment([2; 32]);
        assert_ne!(a.hash(), b.hash());
    }

    #[test]
    fn hash_differs_for_different_intents() {
        let a = Block {
            height: 0,
            parent: GENESIS_PARENT,
            state_root: Commitment([1; 32]),
            intents: vec![Intent::Transfer {
                from: addr(1),
                to: addr(2),
                amount: 100,
            }],
        };
        let b = Block {
            height: 0,
            parent: GENESIS_PARENT,
            state_root: Commitment([1; 32]),
            intents: vec![Intent::Transfer {
                from: addr(1),
                to: addr(2),
                amount: 101,
            }],
        };
        assert_ne!(a.hash(), b.hash());
    }

    #[test]
    fn transfer_and_call_intent_hash_differently() {
        let a = Block {
            height: 0,
            parent: GENESIS_PARENT,
            state_root: Commitment([1; 32]),
            intents: vec![Intent::Transfer {
                from: addr(1),
                to: addr(2),
                amount: 100,
            }],
        };
        let b = Block {
            height: 0,
            parent: GENESIS_PARENT,
            state_root: Commitment([1; 32]),
            intents: vec![Intent::Call {
                caller: addr(1),
                target: addr(2),
                value: 100,
                calldata: Vec::new(),
            }],
        };
        assert_ne!(a.hash(), b.hash());
    }
}
