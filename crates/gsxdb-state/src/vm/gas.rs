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
const COST_LOAD_RESOURCE: u64 = 100; // resource load from storage (base)

// Size-proportional surcharges (per KiB, rounded up) layered on top of the
// flat base costs above. A flat fee lets a transaction load or allocate large
// blobs for a constant price — a resource-size DoS gap (ambarish + Codex
// review on #25). These bound VM work by data size.
const COST_PER_KIB_HEAP: u64 = 1; // native-context heap memory
const COST_PER_KIB_RESOURCE: u64 = 5; // resource load from storage
const COST_PER_KIB_CONST: u64 = 1; // LdConst payload

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

    /// Charge proportional to `bytes` (rounded up to whole KiB) at
    /// `per_kib_cost`, never below `floor`. Closes the size-based DoS gaps a
    /// flat per-op fee leaves open (heap / resource / const loads).
    fn charge_bytes(&mut self, bytes: u64, per_kib_cost: u64, floor: u64) -> PartialVMResult<()> {
        let kib = bytes.saturating_add(1023) / 1024;
        let cost = kib.saturating_mul(per_kib_cost).max(floor);
        self.charge(cost)
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

    fn charge_native_execution(&mut self, amount: InternalGas) -> PartialVMResult<()> {
        // Charge the VM-reported gas, not a flat fee — a native whose cost
        // scales with input or runtime work must be metered proportionally,
        // or the bounded budget (the DoS guard this meter exists for) is
        // defeated by an expensive native that only pays a constant.
        self.charge(u64::from(amount))
    }

    fn use_heap_memory_in_native_context(&mut self, amount: u64) -> PartialVMResult<()> {
        // Proportional to heap requested: a native that accounts for large
        // memory must pay for it (was a flat COST_BASE — DoS gap, #25 review).
        self.charge_bytes(amount, COST_PER_KIB_HEAP, COST_BASE)
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

    fn charge_ld_const(&mut self, size: NumBytes) -> PartialVMResult<()> {
        // Loading a large constant costs more than a tiny one (#25 review).
        self.charge_bytes(u64::from(size), COST_PER_KIB_CONST, COST_BASE)
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
        args: impl ExactSizeIterator<Item = impl ValueView>,
    ) -> PartialVMResult<()> {
        // Proportional to element count: packing a large vector is more work
        // than a small one (was flat COST_AGGREGATE — #25 review).
        let cost = COST_AGGREGATE.saturating_add((args.len() as u64).saturating_mul(COST_BASE));
        self.charge(cost)
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
        bytes_loaded: NumBytes,
    ) -> PartialVMResult<()> {
        // Base load cost + per-KiB surcharge: a large resource load is more
        // expensive than a small one (was flat COST_LOAD_RESOURCE — #25 review).
        let kib = u64::from(bytes_loaded).saturating_add(1023) / 1024;
        self.charge(COST_LOAD_RESOURCE.saturating_add(kib.saturating_mul(COST_PER_KIB_RESOURCE)))
    }

    fn charge_native_function(
        &mut self,
        amount: InternalGas,
        _ret_vals: Option<impl ExactSizeIterator<Item = impl ValueView>>,
    ) -> PartialVMResult<()> {
        // Post-execution: charge the native's real reported cost, floored at
        // `COST_NATIVE` so even a zero-cost native still pays a per-call
        // dispatch minimum (bounds native-call count).
        self.charge(u64::from(amount).max(COST_NATIVE))
    }

    fn charge_native_function_before_execution(
        &mut self,
        _ty_args: impl ExactSizeIterator<Item = impl TypeView>,
        _args: impl ExactSizeIterator<Item = impl ValueView>,
    ) -> PartialVMResult<()> {
        // No pre-charge: `charge_native_function` charges the native's real
        // cost (with a `COST_NATIVE` floor) after it returns. Charging here
        // too would double-bill every native call and trigger premature
        // OUT_OF_GAS for transactions that should fit the budget.
        Ok(())
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

    /// **P1 fix:** native execution charges the VM-reported gas amount,
    /// proportional to the native's work — not a flat `COST_NATIVE`. An
    /// expensive native must draw the budget down by its real cost or the
    /// DoS bound the meter exists for is meaningless. (The function /
    /// before-execution hooks take `ValueView`/`TypeView` iterators this
    /// module avoids; they're exercised end-to-end by the parity gate.)
    #[test]
    fn native_execution_charges_reported_amount() {
        let mut gas = BoundedGasMeter::new(10_000);
        gas.charge_native_execution(InternalGas::from(500)).unwrap();
        assert_eq!(gas.remaining(), 9_500); // the reported 500, not COST_NATIVE
        gas.charge_native_execution(InternalGas::from(2_000)).unwrap();
        assert_eq!(gas.remaining(), 7_500);
    }

    /// A native execution that exceeds the remaining budget aborts with
    /// OUT_OF_GAS — the bound the budget enforces.
    #[test]
    fn native_execution_exhausts_budget() {
        let mut gas = BoundedGasMeter::new(100);
        let err = gas
            .charge_native_execution(InternalGas::from(1_000))
            .unwrap_err();
        assert_eq!(err.major_status(), StatusCode::OUT_OF_GAS);
    }
}
