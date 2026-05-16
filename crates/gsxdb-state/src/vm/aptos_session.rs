//! Aptos Move VM session-layer adapters (S9.5e first cut).
//!
//! The Aptos `MoveVM::execute_loaded_function` takes a `Loader`, a
//! `MoveVmDataCache`, a `GasMeter`, a `TraversalContext`, and a
//! `NativeContextExtensions`. aptos-core ships reference impls for
//! most of these (`UnsyncModuleStorage`, `EagerLoader`,
//! `MoveVmDataCacheAdapter`, `TransactionDataCache`, `UnmeteredGasMeter`).
//!
//! What gsx-db has to supply itself: two **one-method** adapter traits
//! that bridge our `ModuleStore` and `MoveBalanceView` to the Aptos
//! types.
//!
//! - [`GsxdbModuleBytes`] adapts `&dyn ModuleStore` to
//!   [`move_vm_types::code::ModuleBytesStorage`].
//! - [`EmptyResourceResolver`] is a first-cut [`ResourceResolver`]
//!   that reports "no resource" for every query. S9.5f replaces it
//!   with a real reader that builds canonical `0x1::coin::CoinStore`
//!   bytes from a `MoveBalanceView`.
//!
//! Once both adapters compile against the real Aptos types, the next
//! session can wire them into `AptosMoveExecutor::execute` via the
//! Aptos-provided factories (`AsUnsyncModuleStorage`, `EagerLoader`,
//! `MoveVmDataCacheAdapter`).

use bytes::Bytes;
use move_binary_format::errors::{PartialVMResult, VMResult};
use move_core_types::{
    account_address::AccountAddress,
    identifier::IdentStr,
    language_storage::StructTag,
    metadata::Metadata,
    value::MoveTypeLayout,
};
use move_vm_runtime::{RuntimeEnvironment, WithRuntimeEnvironment};
use move_vm_types::{code::ModuleBytesStorage, resolver::ResourceResolver};

use crate::vm::executor::{Identifier, ModuleId, ModuleStore, MoveBalanceView};
use crate::MoveAddress;

/// Adapter: `&dyn ModuleStore` → `move_vm_types::code::ModuleBytesStorage`
/// (plus `move_vm_runtime::WithRuntimeEnvironment` — required by
/// `AsUnsyncModuleStorage`).
///
/// The Aptos `UnsyncModuleStorage` expects raw bytes keyed by
/// `(AccountAddress, IdentStr)`. Our `ModuleStore` uses gsx-db's
/// `(MoveAddress, Identifier)`. The conversion is byte-for-byte —
/// `MoveAddress(pub [u8; 32])` is wire-compatible with `AccountAddress`
/// (Aptos uses 32-byte addresses).
pub(crate) struct GsxdbModuleBytes<'a> {
    pub store: &'a dyn ModuleStore,
    pub env: RuntimeEnvironment,
}

impl<'a> ModuleBytesStorage for GsxdbModuleBytes<'a> {
    fn fetch_module_bytes(
        &self,
        address: &AccountAddress,
        module_name: &IdentStr,
    ) -> VMResult<Option<Bytes>> {
        // AccountAddress::into_bytes() yields the 32-byte representation.
        let our_addr = MoveAddress(address.into_bytes());

        // gsx-db's Identifier validates the same alphabet Move uses
        // ([A-Za-z_][A-Za-z0-9_]*). An IdentStr that passed Aptos's
        // identifier validation will always pass ours. If somehow it
        // doesn't, surface as "no module" rather than panicking.
        let our_name = match Identifier::new(module_name.as_str()) {
            Ok(n) => n,
            Err(_) => return Ok(None),
        };

        let id = ModuleId {
            address: our_addr,
            name: our_name,
        };

        Ok(self.store.get(&id).map(|cm| Bytes::from(cm.bytes)))
    }
}

impl<'a> WithRuntimeEnvironment for GsxdbModuleBytes<'a> {
    fn runtime_environment(&self) -> &RuntimeEnvironment {
        &self.env
    }
}

/// First-cut [`ResourceResolver`] that reports "no resource" for every
/// `(address, struct_tag)` query.
///
/// Kept for tests + bypass scenarios. Production path is
/// [`BalanceViewResolver`].
pub(crate) struct EmptyResourceResolver<'a> {
    pub _balance_view: &'a dyn MoveBalanceView,
}

impl<'a> ResourceResolver for EmptyResourceResolver<'a> {
    fn get_resource_bytes_with_metadata_and_layout(
        &self,
        _address: &AccountAddress,
        _struct_tag: &StructTag,
        _metadata: &[Metadata],
        _layout: Option<&MoveTypeLayout>,
    ) -> PartialVMResult<(Option<Bytes>, usize)> {
        Ok((None, 0))
    }
}

/// S9.5f: address of the canonical gsx-db Coin module. Mirrors the
/// `CANONICAL_COIN_ADDRESS` constant exposed by `MockMoveExecutor` so
/// real + mock executors agree on which `(account, module)` pair holds
/// the canonical balance resource.
const GSXDB_COIN_ADDRESS: AccountAddress = AccountAddress::new([
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    1,
]);
const GSXDB_COIN_MODULE_NAME: &str = "coin";
const GSXDB_COIN_STRUCT_NAME: &str = "CoinStore";

/// S9.5g: compiled bytecode for the canonical gsx-db Coin module.
///
/// Source: `move/canonical-coin/sources/coin.move` (gsx-db repo root).
/// Compiled by `aptos move compile` against `aptos-node-v1.44.9-hotfix`
/// MoveStdlib. The build artifact (`build/.../coin.mv`) is checked in
/// alongside this crate at `move-bytecode/canonical_coin.mv` so
/// production-move-executor builds don't need an aptos-cli install.
///
/// Re-generating after any change to the Move source:
///
/// ```sh
/// cd move/canonical-coin && aptos move compile --skip-fetch-latest-git-deps
/// cp build/gsxdb_canonical_coin/bytecode_modules/coin.mv \
///    crates/gsxdb-state/move-bytecode/canonical_coin.mv
/// ```
///
/// The bytecode is consumed by callers that want to auto-deploy the
/// module into a fresh `ModuleStore` at bundle-executor startup —
/// without it, an `Intent::Call` targeting `0x1::coin` would hit
/// `MoveExecutionError::ModuleNotFound` before reaching the
/// interpreter.
pub fn canonical_coin_bytecode() -> &'static [u8] {
    include_bytes!("../../move-bytecode/canonical_coin.mv")
}

/// gsx-db `ModuleId` for the canonical coin module. Use with
/// `ModuleStore::put` at bundle-executor startup.
pub fn canonical_coin_module_id() -> ModuleId {
    ModuleId {
        address: MoveAddress(GSXDB_COIN_ADDRESS.into_bytes()),
        name: Identifier::new(GSXDB_COIN_MODULE_NAME)
            .expect("canonical coin module name is a valid identifier"),
    }
}

/// BCS layout of the canonical gsx-db `CoinStore` Move resource:
///
/// ```move
/// module 0x1::coin {
///     struct CoinStore has key {
///         value: u64,      // u64 LE
///         sequence: u64,   // u64 LE
///     }
/// }
/// ```
///
/// 16 bytes total. The S9.5g Move source compiles to this layout.
/// Deliberately simpler than Aptos's `aptos_framework::coin::CoinStore`
/// (which adds `frozen: bool` + two `EventHandle`s) — gsx-db doesn't
/// need on-chain Move event handles, the events surface via
/// `MoveOutcome::events` separately.
pub(crate) const COIN_STORE_BCS_LEN: usize = 16;

#[inline]
fn encode_coin_store(value: u64, sequence: u64) -> [u8; COIN_STORE_BCS_LEN] {
    let mut out = [0u8; COIN_STORE_BCS_LEN];
    out[0..8].copy_from_slice(&value.to_le_bytes());
    out[8..16].copy_from_slice(&sequence.to_le_bytes());
    out
}

#[inline]
pub(crate) fn decode_coin_store(bytes: &[u8]) -> Option<(u64, u64)> {
    if bytes.len() != COIN_STORE_BCS_LEN {
        return None;
    }
    let value = u64::from_le_bytes(bytes[0..8].try_into().ok()?);
    let sequence = u64::from_le_bytes(bytes[8..16].try_into().ok()?);
    Some((value, sequence))
}

/// Returns `true` iff the struct tag identifies the canonical gsx-db
/// `CoinStore` resource.
fn is_canonical_coin_store(tag: &StructTag) -> bool {
    tag.address == GSXDB_COIN_ADDRESS
        && tag.module.as_str() == GSXDB_COIN_MODULE_NAME
        && tag.name.as_str() == GSXDB_COIN_STRUCT_NAME
}

/// Production [`ResourceResolver`] for gsx-db. Recognises the canonical
/// `0x1::coin::CoinStore` struct tag and builds its BCS-encoded form
/// from a [`MoveBalanceView`]. All other struct tags return "no
/// resource".
///
/// This is what `AptosMoveExecutor::execute` constructs per-bundle from
/// the [`MoveSessionState::balance_view`].
pub(crate) struct BalanceViewResolver<'a> {
    pub balance_view: &'a dyn MoveBalanceView,
}

impl<'a> ResourceResolver for BalanceViewResolver<'a> {
    fn get_resource_bytes_with_metadata_and_layout(
        &self,
        address: &AccountAddress,
        struct_tag: &StructTag,
        _metadata: &[Metadata],
        _layout: Option<&MoveTypeLayout>,
    ) -> PartialVMResult<(Option<Bytes>, usize)> {
        if !is_canonical_coin_store(struct_tag) {
            return Ok((None, 0));
        }
        let our_addr = MoveAddress(address.into_bytes());
        let value = u64::try_from(self.balance_view.coin_value(&our_addr).to_u128())
            .unwrap_or(u64::MAX);
        let sequence = self.balance_view.nonce(&our_addr).value;
        let encoded = encode_coin_store(value, sequence);
        Ok((Some(Bytes::from(encoded.to_vec())), COIN_STORE_BCS_LEN))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vm::executor::{
        CompiledModule, InMemoryModuleStore, ModuleStoreError as _,
    };
    use crate::{AccountNonce, MoveCoinValue};

    /// Minimal `MoveBalanceView` for adapter tests.
    #[derive(Debug, Default)]
    struct ZeroView;

    impl MoveBalanceView for ZeroView {
        fn coin_value(&self, _addr: &MoveAddress) -> MoveCoinValue {
            MoveCoinValue::from_u128(0)
        }
        fn nonce(&self, _addr: &MoveAddress) -> AccountNonce {
            AccountNonce::new(0)
        }
    }

    #[test]
    fn module_bytes_adapter_returns_none_for_missing_module() {
        let store = InMemoryModuleStore::new();
        let adapter = GsxdbModuleBytes {
            store: &store,
            env: RuntimeEnvironment::new(std::iter::empty()),
        };

        let addr = AccountAddress::new([1u8; 32]);
        let name = move_core_types::identifier::Identifier::new("missing").unwrap();
        let result = adapter.fetch_module_bytes(&addr, &name).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn module_bytes_adapter_returns_bytes_for_deployed_module() {
        let mut store = InMemoryModuleStore::new();
        let id = ModuleId {
            address: MoveAddress([7u8; 32]),
            name: Identifier::new("payments").unwrap(),
        };
        store
            .put(
                id,
                CompiledModule {
                    bytes: vec![0xCA, 0xFE, 0xBA, 0xBE],
                },
            )
            .unwrap();

        let adapter = GsxdbModuleBytes {
            store: &store,
            env: RuntimeEnvironment::new(std::iter::empty()),
        };
        let addr = AccountAddress::new([7u8; 32]);
        let name = move_core_types::identifier::Identifier::new("payments").unwrap();
        let result = adapter.fetch_module_bytes(&addr, &name).unwrap();

        assert_eq!(
            result.as_ref().map(|b| b.as_ref()),
            Some([0xCA_u8, 0xFE, 0xBA, 0xBE].as_slice())
        );
    }

    #[test]
    fn empty_resource_resolver_reports_no_resource() {
        let view = ZeroView;
        let resolver = EmptyResourceResolver {
            _balance_view: &view,
        };

        let addr = AccountAddress::new([1u8; 32]);
        let tag = StructTag {
            address: AccountAddress::new([1u8; 32]),
            module: move_core_types::identifier::Identifier::new("coin").unwrap(),
            name: move_core_types::identifier::Identifier::new("CoinStore").unwrap(),
            type_args: Vec::new(),
        };

        let (bytes, size) = resolver
            .get_resource_bytes_with_metadata_and_layout(&addr, &tag, &[], None)
            .unwrap();
        assert!(bytes.is_none());
        assert_eq!(size, 0);
    }

    #[test]
    fn coin_store_encode_decode_round_trip() {
        for (value, seq) in [(0u64, 0u64), (123, 456), (u64::MAX, u64::MAX - 1)] {
            let encoded = encode_coin_store(value, seq);
            let (v2, s2) = decode_coin_store(&encoded).expect("decode");
            assert_eq!((v2, s2), (value, seq));
        }
        // 16 byte fixed length
        assert_eq!(encode_coin_store(0, 0).len(), COIN_STORE_BCS_LEN);
        // truncated decode rejects
        assert!(decode_coin_store(&[0u8; 15]).is_none());
        assert!(decode_coin_store(&[0u8; 17]).is_none());
    }

    /// `MoveBalanceView` that maps a few addresses to fixed values.
    #[derive(Debug)]
    struct FixedView<'a> {
        balances: &'a [(MoveAddress, u128, u64)],
    }

    impl<'a> MoveBalanceView for FixedView<'a> {
        fn coin_value(&self, addr: &MoveAddress) -> MoveCoinValue {
            for (a, v, _) in self.balances {
                if a == addr {
                    return MoveCoinValue::from_u128(*v);
                }
            }
            MoveCoinValue::from_u128(0)
        }
        fn nonce(&self, addr: &MoveAddress) -> AccountNonce {
            for (a, _, n) in self.balances {
                if a == addr {
                    return AccountNonce::new(*n);
                }
            }
            AccountNonce::new(0)
        }
    }

    fn coin_store_tag() -> StructTag {
        StructTag {
            address: GSXDB_COIN_ADDRESS,
            module: move_core_types::identifier::Identifier::new("coin").unwrap(),
            name: move_core_types::identifier::Identifier::new("CoinStore").unwrap(),
            type_args: Vec::new(),
        }
    }

    #[test]
    fn balance_view_resolver_returns_canonical_coin_store_bytes() {
        let alice = MoveAddress([1u8; 32]);
        let view = FixedView {
            balances: &[(alice, 1000u128, 7u64)],
        };
        let resolver = BalanceViewResolver {
            balance_view: &view,
        };

        let aptos_alice = AccountAddress::new([1u8; 32]);
        let (bytes, size) = resolver
            .get_resource_bytes_with_metadata_and_layout(
                &aptos_alice,
                &coin_store_tag(),
                &[],
                None,
            )
            .unwrap();

        assert_eq!(size, COIN_STORE_BCS_LEN);
        let bytes = bytes.expect("CoinStore resource");
        assert_eq!(bytes.len(), COIN_STORE_BCS_LEN);
        let (value, sequence) = decode_coin_store(&bytes).expect("decode");
        assert_eq!(value, 1000);
        assert_eq!(sequence, 7);
    }

    #[test]
    fn balance_view_resolver_returns_none_for_other_struct_tags() {
        let view = ZeroView;
        let resolver = BalanceViewResolver {
            balance_view: &view,
        };

        let addr = AccountAddress::new([1u8; 32]);
        let other_tag = StructTag {
            address: AccountAddress::new([0xAA; 32]),
            module: move_core_types::identifier::Identifier::new("other").unwrap(),
            name: move_core_types::identifier::Identifier::new("Type").unwrap(),
            type_args: Vec::new(),
        };
        let (bytes, size) = resolver
            .get_resource_bytes_with_metadata_and_layout(&addr, &other_tag, &[], None)
            .unwrap();
        assert!(bytes.is_none());
        assert_eq!(size, 0);
    }

    #[test]
    fn canonical_coin_bytecode_deserializes_and_verifies() {
        use move_binary_format::file_format::CompiledModule;

        let bytes = canonical_coin_bytecode();
        assert!(
            !bytes.is_empty(),
            "compiled bytecode artifact missing — re-run aptos move compile"
        );

        let compiled = CompiledModule::deserialize(bytes)
            .expect("canonical coin bytecode deserializes");

        // The compiled module should declare its self_id matching our
        // canonical (address, name) pair.
        let self_id = compiled.self_id();
        assert_eq!(*self_id.address(), GSXDB_COIN_ADDRESS);
        assert_eq!(self_id.name().as_str(), GSXDB_COIN_MODULE_NAME);

        move_bytecode_verifier::verifier::verify_module(&compiled)
            .expect("canonical coin bytecode passes the verifier");
    }

    #[test]
    fn canonical_coin_module_id_matches_resolver_constants() {
        let id = canonical_coin_module_id();
        assert_eq!(id.address.0, GSXDB_COIN_ADDRESS.into_bytes());
        assert_eq!(id.name.as_str(), GSXDB_COIN_MODULE_NAME);
    }

    #[test]
    fn balance_view_resolver_zero_for_unseeded_address() {
        let view = ZeroView;
        let resolver = BalanceViewResolver {
            balance_view: &view,
        };

        let addr = AccountAddress::new([9u8; 32]);
        let (bytes, _) = resolver
            .get_resource_bytes_with_metadata_and_layout(&addr, &coin_store_tag(), &[], None)
            .unwrap();
        let bytes = bytes.expect("CoinStore resource always returned for canonical tag");
        let (value, sequence) = decode_coin_store(&bytes).expect("decode");
        assert_eq!(value, 0);
        assert_eq!(sequence, 0);
    }
}
