//! Contract registry — address-keyed [`BundleGenerator`]s.
//!
//! A "contract" in phase-1 is a Rust closure: given a [`CallCtx`]
//! describing the call (caller, target, value, calldata), it returns
//! a [`Bundle`] of state-mutating steps. The registry maps addresses
//! to these closures; the block executor (next slice) dispatches
//! `Intent::Call { caller, target, calldata }` to whichever generator
//! is registered at `target`.
//!
//! Per IQ-3, this is mock substrate. Real revm and real Move drop in
//! when those land — they would replace `BundleGenerator` impls with
//! ones that interpret real bytecode and return real state diffs as
//! bundles.
//!
//! # Recursion
//!
//! Phase-1 disallows recursion. The block executor passes the current
//! call depth in [`CallCtx`]; generators can read it but the block
//! executor refuses to dispatch a `Call` whose depth would exceed 1.
//! Real-VM integration revisits.

use crate::bundle::types::Bundle;
use gsxdb_state::{Address, State};
use std::collections::HashMap;
use std::sync::Arc;

/// Context passed to a [`BundleGenerator`] when it's invoked.
pub struct CallCtx<'a> {
    /// The address that invoked the contract (the EOA or upstream
    /// contract). For top-level calls, this is the user's address.
    pub caller: Address,
    /// The contract address being called (the registry key).
    pub target: Address,
    /// Native value passed with the call. Phase-1 only models the
    /// EVM-style "send value with the call" pattern at this layer;
    /// generators that don't care can ignore this.
    pub value: u128,
    /// Opaque call payload. Real revm parses ABI; mock contracts
    /// agree on whatever shape they want.
    pub calldata: &'a [u8],
    /// Read-only access to canonical state at call time. Generators
    /// inspect balances here to produce a bundle; they cannot mutate
    /// state directly. Mutations flow through the returned bundle.
    pub state: &'a State,
    /// Call depth (0 for the top-level intent, 1 for a sub-call from
    /// a contract). Phase-1 cap is 1; the block executor enforces.
    pub depth: u8,
}

/// A contract's generator function. Pure with respect to state — reads
/// from `ctx.state`, returns a bundle, no side effects.
pub trait BundleGenerator: Send + Sync {
    /// Build the bundle of state mutations this contract performs in
    /// response to the call described by `ctx`.
    fn generate(&self, ctx: &CallCtx) -> Bundle;
}

/// Convenience: any matching closure is a [`BundleGenerator`].
impl<F> BundleGenerator for F
where
    F: Fn(&CallCtx) -> Bundle + Send + Sync,
{
    fn generate(&self, ctx: &CallCtx) -> Bundle {
        (self)(ctx)
    }
}

/// Address-keyed map of [`BundleGenerator`]s.
///
/// Cheap to clone — internally `Arc`-shared generators.
#[derive(Clone, Default)]
pub struct ContractRegistry {
    by_address: HashMap<Address, Arc<dyn BundleGenerator>>,
}

impl std::fmt::Debug for ContractRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ContractRegistry")
            .field("contracts", &self.by_address.len())
            .finish()
    }
}

impl ContractRegistry {
    /// Empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a generator at `addr`. Replaces any existing entry.
    pub fn register(&mut self, addr: Address, gen: Arc<dyn BundleGenerator>) {
        self.by_address.insert(addr, gen);
    }

    /// Look up a generator. `None` means "no contract at that address"
    /// (the block executor treats this as a regular EOA — no dispatch).
    #[must_use]
    pub fn get(&self, addr: &Address) -> Option<Arc<dyn BundleGenerator>> {
        self.by_address.get(addr).cloned()
    }

    /// Number of registered contracts.
    #[must_use]
    pub fn len(&self) -> usize {
        self.by_address.len()
    }

    /// `true` iff no contracts are registered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.by_address.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bundle::types::BundleStep;
    use gsxdb_state::EvmTx;

    fn addr(byte: u8) -> Address {
        Address([byte; 20])
    }

    #[test]
    fn empty_registry_lookups_return_none() {
        let r = ContractRegistry::new();
        assert!(r.is_empty());
        assert!(r.get(&addr(1)).is_none());
    }

    #[test]
    fn register_and_lookup() {
        let mut r = ContractRegistry::new();
        let gen: Arc<dyn BundleGenerator> = Arc::new(|_: &CallCtx| Bundle::new());
        r.register(addr(7), gen);

        assert_eq!(r.len(), 1);
        assert!(r.get(&addr(7)).is_some());
        assert!(r.get(&addr(8)).is_none());
    }

    #[test]
    fn closure_generates_bundle_from_ctx() {
        // A "forwarder" mock contract: any call with value > 0
        // emits a single Evm step crediting addr(99).
        let recipient = addr(99);
        let gen = move |ctx: &CallCtx| {
            if ctx.value > 0 {
                Bundle::single(BundleStep::Evm(EvmTx {
                    from: ctx.target,
                    to: recipient,
                    value: ctx.value,
                    nonce: 0,
                }))
            } else {
                Bundle::new()
            }
        };

        let state = State::default();
        let ctx = CallCtx {
            caller: addr(1),
            target: addr(7),
            value: 50,
            calldata: &[],
            state: &state,
            depth: 0,
        };
        let bundle = gen.generate(&ctx);
        assert_eq!(bundle.len(), 1);

        let ctx_zero = CallCtx { value: 0, ..ctx };
        let bundle = gen.generate(&ctx_zero);
        assert_eq!(bundle.len(), 0);
    }

    #[test]
    fn registry_replaces_existing_entry_on_re_register() {
        let mut r = ContractRegistry::new();
        let gen_a: Arc<dyn BundleGenerator> = Arc::new(|_: &CallCtx| Bundle::new());
        let gen_b: Arc<dyn BundleGenerator> = Arc::new(|_: &CallCtx| {
            Bundle::single(BundleStep::Evm(EvmTx {
                from: addr(1),
                to: addr(2),
                value: 1,
                nonce: 0,
            }))
        });

        r.register(addr(7), gen_a);
        r.register(addr(7), gen_b);

        assert_eq!(r.len(), 1);
        let g = r.get(&addr(7)).unwrap();
        let state = State::default();
        let bundle = g.generate(&CallCtx {
            caller: addr(0),
            target: addr(7),
            value: 0,
            calldata: &[],
            state: &state,
            depth: 0,
        });
        // gen_b returns a 1-step bundle; gen_a would have returned empty.
        assert_eq!(bundle.len(), 1);
    }
}
