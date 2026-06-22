//! Block storage + recovery via replay.
//!
//! S8 ships:
//!
//! - A [`Block`] type committing to height, parent hash, the block's
//!   intents, and the post-execution state root from S6.
//! - A [`BlockStore`] trait + in-memory and redb-backed
//!   implementations.
//! - A [`replay`] function that takes a `BlockStore` and an empty
//!   `State`, walks blocks in height order, re-executes each through
//!   `BlockExecutor`, and verifies the produced state root matches
//!   the recorded one.
//!
//! Phase-1 ships single-parent linear blocks. The "DAG" framing in
//! the original sprint name leaves room for multi-parent blocks
//! (Narwhal-style) which would require a `parents: Vec<BlockHash>`
//! field — a single-field shape change with no impact on the
//! commitment scheme. Out of scope for phase-1.

pub mod block;
pub mod replay;
pub mod store;

pub use block::{Block, BlockHash};
pub use replay::{replay, RecoveryError};
pub use store::{BlockStore, InMemoryBlockStore, RedbBlockStore};
