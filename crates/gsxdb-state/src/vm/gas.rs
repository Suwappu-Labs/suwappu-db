//! Bounded gas meter for the production Aptos Move executor.
//!
//! `move-vm-runtime` drives execution through a [`GasMeter`]; gsx-db's
//! S9 executor passed `UnmeteredGasMeter`, so a malicious or buggy module
//! could loop without bound. [`BoundedGasMeter`] charges a flat cost per
//! VM operation against a fixed budget and aborts with
//! [`StatusCode::OUT_OF_GAS`] once the budget is exhausted — the
//! DoS-protection floor a real VM needs.
//!
//! Costs are deliberately coarse (flat per-operation tiers, not a
//! calibrated Aptos gas schedule). For the canonical `0x1::coin` surface
//! this is sufficient: it bounds total work without pretending to price
//! each opcode. A calibrated schedule is a follow-on once the Move
//! surface grows beyond transfers.

use move_binary_format::{
    errors::{PartialVMError, PartialVMResult},
    file_format::CodeOffset,
};
use move_core_types::{
    account_address::AccountAddress,
    gas_algebra::{InternalGas, NumArgs, NumBytes, NumTypeNodes},
    identifier::IdentStr,
    language_storage::ModuleId,
    vm_status::StatusCode,
};
use move_vm_types::{
    gas::{DependencyGasMeter, DependencyKind, GasMeter, NativeGasMeter, SimpleInstruction},
    views::{TypeView, ValueView},
};

/// Default per-call internal-gas budget. ~10M flat-cost operations — far
/// more than any `0x1::coin` entry function needs, but a hard ceiling
/// that aborts an unbounded loop. Tune per the gas-schedule follow-on.
pub const DEFAULT_MOVE_GAS_BUDGET: u64 = 10_000_000;

// Flat per-operation cost tiers (internal gas units).
const COST_BASE: u64 = 1; // simple instrs, locals, refs, branches, comparisons
const COST_AGGREGATE: u64 = 2; // pack / unpack / vector construction
const COST_CALL: u64 = 8; // function calls
const COST_GLOBAL: u64 = 10; // borrow_global / exists / move_from / move_to
const COST_DEPENDENCY: u64 = 10; // module dependency load
const COST_NATIVE: u64 = 50; // native function dispatch
const COST_LOAD_RESOURCE: u64 = 100; // resource load from storage

/// A [`GasMeter`] that bounds total VM work against a fixed budget.
#[derive(Debug, Clone)]
pub struct BoundedGasMeter {
    remaining: u64,
}

impl BoundedGasMeter {
    /// Construct a meter with the given internal-gas budget.
    #[must_use]
    pub fn new(budget: u64) -> Self {
        Self { remaining: budget }
    }

    /// Internal-gas units left.
    #[must_use]
    pub fn remaining(&self) -> u64 {
        self.remaining
    }

    /// Deduct `amount`; abort with `OUT_OF_GAS` if the budget is exhausted.
    fn charge(&mut self, amount: u64) -> PartialVMResult<()> {
        match self.remaining.checked_sub(amount) {
            Some(rem) => {
                self.remaining = rem;
                Ok(())
            }
            None => {
                self.remaining = 0;
                Err(PartialVMError::new(StatusCode::OUT_OF_GAS))
            }
        }
    }
}

impl Default for BoundedGasMeter {
    fn default() -> Self {
        Self::new(DEFAULT_MOVE_GAS_BUDGET)
    }
}

impl DependencyGasMeter for BoundedGasMeter {
    fn charge_dependency(
        &mut self,
        _kind: DependencyKind,
        _addr: &AccountAddress,
        _name: &IdentStr,
        _size: NumBytes,
    ) -> PartialVMResult<()> {
        self.charge(COST_DEPENDENCY)
    }
}

impl NativeGasMeter for BoundedGasMeter {
    fn legacy_gas_budget_in_native_context(&self) -> InternalGas {
        InternalGas::from(self.remaining)
    }

    fn charge_native_execution(&mut self, _amount: InternalGas) -> PartialVMResult<()> {
        self.charge(COST_NATIVE)
    }

    fn use_heap_memory_in_native_context(&mut self, _amount: u64) -> PartialVMResult<()> {
        self.charge(COST_BASE)
    }
}

impl GasMeter for BoundedGasMeter {
    fn balance_internal(&self) -> InternalGas {
        InternalGas::from(self.remaining)
    }

    fn charge_simple_instr(&mut self, _instr: SimpleInstruction) -> PartialVMResult<()> {
        self.charge(COST_BASE)
    }

    fn charge_br_false(&mut self, _target_offset: Option<CodeOffset>) -> PartialVMResult<()> {
        self.charge(COST_BASE)
    }

    fn charge_br_true(&mut self, _target_offset: Option<CodeOffset>) -> PartialVMResult<()> {
        self.charge(COST_BASE)
    }

    fn charge_branch(&mut self, _target_offset: CodeOffset) -> PartialVMResult<()> {
        self.charge(COST_BASE)
    }

    fn charge_pop(&mut self, _popped_val: impl ValueView) -> PartialVMResult<()> {
        self.charge(COST_BASE)
    }

    fn charge_call(
        &mut self,
        _module_id: &ModuleId,
        _func_name: &str,
        _args: impl IntoIterator<Item = impl ValueView>,
        _num_locals: NumArgs,
    ) -> PartialVMResult<()> {
        self.charge(COST_CALL)
    }

    fn charge_call_generic(
        &mut self,
        _module_id: &ModuleId,
        _func_name: &str,
        _ty_args: impl ExactSizeIterator<Item = impl TypeView>,
        _args: impl ExactSizeIterator<Item = impl ValueView>,
        _num_locals: NumArgs,
    ) -> PartialVMResult<()> {
        self.charge(COST_CALL)
    }

    fn charge_ld_const(&mut self, _size: NumBytes) -> PartialVMResult<()> {
        self.charge(COST_BASE)
    }

    fn charge_ld_const_after_deserialization(
        &mut self,
        _val: impl ValueView,
    ) -> PartialVMResult<()> {
        self.charge(COST_BASE)
    }

    fn charge_copy_loc(&mut self, _val: impl ValueView) -> PartialVMResult<()> {
        self.charge(COST_BASE)
    }

    fn charge_move_loc(&mut self, _val: impl ValueView) -> PartialVMResult<()> {
        self.charge(COST_BASE)
    }

    fn charge_store_loc(&mut self, _val: impl ValueView) -> PartialVMResult<()> {
        self.charge(COST_BASE)
    }

    fn charge_pack(
        &mut self,
        _is_generic: bool,
        _args: impl ExactSizeIterator<Item = impl ValueView>,
    ) -> PartialVMResult<()> {
        self.charge(COST_AGGREGATE)
    }

    fn charge_unpack(
        &mut self,
        _is_generic: bool,
        _args: impl ExactSizeIterator<Item = impl ValueView>,
    ) -> PartialVMResult<()> {
        self.charge(COST_AGGREGATE)
    }

    fn charge_pack_closure(
        &mut self,
        _is_generic: bool,
        _args: impl ExactSizeIterator<Item = impl ValueView>,
    ) -> PartialVMResult<()> {
        self.charge(COST_AGGREGATE)
    }

    fn charge_read_ref(&mut self, _val: impl ValueView) -> PartialVMResult<()> {
        self.charge(COST_BASE)
    }

    fn charge_write_ref(
        &mut self,
        _new_val: impl ValueView,
        _old_val: impl ValueView,
    ) -> PartialVMResult<()> {
        self.charge(COST_BASE)
    }

    fn charge_eq(&mut self, _lhs: impl ValueView, _rhs: impl ValueView) -> PartialVMResult<()> {
        self.charge(COST_BASE)
    }

    fn charge_neq(&mut self, _lhs: impl ValueView, _rhs: impl ValueView) -> PartialVMResult<()> {
        self.charge(COST_BASE)
    }

    fn charge_borrow_global(
        &mut self,
        _is_mut: bool,
        _is_generic: bool,
        _ty: impl TypeView,
        _is_success: bool,
    ) -> PartialVMResult<()> {
        self.charge(COST_GLOBAL)
    }

    fn charge_exists(
        &mut self,
        _is_generic: bool,
        _ty: impl TypeView,
        _exists: bool,
    ) -> PartialVMResult<()> {
        self.charge(COST_GLOBAL)
    }

    fn charge_move_from(
        &mut self,
        _is_generic: bool,
        _ty: impl TypeView,
        _val: Option<impl ValueView>,
    ) -> PartialVMResult<()> {
        self.charge(COST_GLOBAL)
    }

    fn charge_move_to(
        &mut self,
        _is_generic: bool,
        _ty: impl TypeView,
        _val: impl ValueView,
        _is_success: bool,
    ) -> PartialVMResult<()> {
        self.charge(COST_GLOBAL)
    }

    fn charge_vec_pack(
        &mut self,
        _args: impl ExactSizeIterator<Item = impl ValueView>,
    ) -> PartialVMResult<()> {
        self.charge(COST_AGGREGATE)
    }

    fn charge_vec_len(&mut self) -> PartialVMResult<()> {
        self.charge(COST_BASE)
    }

    fn charge_vec_borrow(&mut self, _is_mut: bool) -> PartialVMResult<()> {
        self.charge(COST_BASE)
    }

    fn charge_vec_push_back(&mut self, _val: impl ValueView) -> PartialVMResult<()> {
        self.charge(COST_BASE)
    }

    fn charge_vec_pop_back(&mut self, _val: Option<impl ValueView>) -> PartialVMResult<()> {
        self.charge(COST_BASE)
    }

    fn charge_vec_unpack(
        &mut self,
        _expect_num_elements: NumArgs,
        _elems: impl ExactSizeIterator<Item = impl ValueView>,
    ) -> PartialVMResult<()> {
        self.charge(COST_AGGREGATE)
    }

    fn charge_vec_swap(&mut self) -> PartialVMResult<()> {
        self.charge(COST_BASE)
    }

    fn charge_load_resource(
        &mut self,
        _addr: AccountAddress,
        _ty: impl TypeView,
        _val: Option<impl ValueView>,
        _bytes_loaded: NumBytes,
    ) -> PartialVMResult<()> {
        self.charge(COST_LOAD_RESOURCE)
    }

    fn charge_native_function(
        &mut self,
        _amount: InternalGas,
        _ret_vals: Option<impl ExactSizeIterator<Item = impl ValueView>>,
    ) -> PartialVMResult<()> {
        self.charge(COST_NATIVE)
    }

    fn charge_native_function_before_execution(
        &mut self,
        _ty_args: impl ExactSizeIterator<Item = impl TypeView>,
        _args: impl ExactSizeIterator<Item = impl ValueView>,
    ) -> PartialVMResult<()> {
        self.charge(COST_NATIVE)
    }

    fn charge_drop_frame(
        &mut self,
        _locals: impl Iterator<Item = impl ValueView>,
    ) -> PartialVMResult<()> {
        self.charge(COST_BASE)
    }

    fn charge_create_ty(&mut self, _num_nodes: NumTypeNodes) -> PartialVMResult<()> {
        self.charge(COST_BASE)
    }

    fn charge_abort_message(&mut self, _bytes: &[u8]) -> PartialVMResult<()> {
        self.charge(COST_BASE)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Exercises only the budget arithmetic + OUT_OF_GAS path, via the
    // `SimpleInstruction` charge (a plain enum) and the private `charge`
    // helper — deliberately avoids `ValueView`/`TypeView` so the test
    // has no dependency on those trait surfaces.
    #[test]
    fn charges_deduct_until_exhausted() {
        let mut gas = BoundedGasMeter::new(COST_BASE * 2);
        assert!(gas.charge_simple_instr(SimpleInstruction::Add).is_ok());
        assert_eq!(gas.remaining(), COST_BASE);
        assert!(gas.charge_simple_instr(SimpleInstruction::Add).is_ok());
        assert_eq!(gas.remaining(), 0);
        // Budget exhausted → next charge underflows to OUT_OF_GAS.
        let err = gas.charge_simple_instr(SimpleInstruction::Add).unwrap_err();
        assert_eq!(err.major_status(), StatusCode::OUT_OF_GAS);
        assert_eq!(gas.remaining(), 0);
    }

    #[test]
    fn default_budget_is_nonzero() {
        assert_eq!(
            BoundedGasMeter::default().remaining(),
            DEFAULT_MOVE_GAS_BUDGET
        );
        assert!(DEFAULT_MOVE_GAS_BUDGET > 0);
    }
}
