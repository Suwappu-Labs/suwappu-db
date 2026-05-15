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
use move_vm_types::{code::ModuleBytesStorage, resolver::ResourceResolver};

use crate::vm::executor::{Identifier, ModuleId, ModuleStore, MoveBalanceView};
use crate::MoveAddress;

/// Adapter: `&dyn ModuleStore` → `move_vm_types::code::ModuleBytesStorage`.
///
/// The Aptos `UnsyncModuleStorage` expects raw bytes keyed by
/// `(AccountAddress, IdentStr)`. Our `ModuleStore` uses gsx-db's
/// `(MoveAddress, Identifier)`. The conversion is byte-for-byte
/// — `MoveAddress(pub [u8; 32])` is wire-compatible with
/// `AccountAddress` (Aptos uses 32-byte addresses).
pub(crate) struct GsxdbModuleBytes<'a> {
    pub store: &'a dyn ModuleStore,
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

/// First-cut [`ResourceResolver`] that reports "no resource" for every
/// `(address, struct_tag)` query.
///
/// The Aptos `MoveVmDataCacheAdapter` calls
/// `get_resource_bytes_with_metadata_and_layout` whenever the running
/// Move bytecode touches a resource not already in its cache. Returning
/// `Ok((None, 0))` makes every such read look like a "resource does not
/// exist" — Move bytecode that calls `move_from` / `borrow_global` on
/// such a resource will abort cleanly.
///
/// S9.5f replaces this with a [`MoveBalanceView`]-backed resolver that
/// recognizes the canonical `0x1::coin::CoinStore<T>` struct tag and
/// returns its BCS-encoded form. Once that ships, real `transfer` calls
/// flow through the VM.
pub(crate) struct EmptyResourceResolver<'a> {
    // Unused in this first cut but reserved so the resolver's API
    // doesn't change between S9.5e and S9.5f.
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
        // First cut: no resources. Real resolver lands in S9.5f.
        Ok((None, 0))
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
        let adapter = GsxdbModuleBytes { store: &store };

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

        let adapter = GsxdbModuleBytes { store: &store };
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
}
