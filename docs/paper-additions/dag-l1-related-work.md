# Insertion: DAG L1 paper, new related-work paragraph

**Where:** §2 (Related Work), after the *Compliance-native
primitives* paragraph, before §3 (System Model).

---

## State substrates for dual-VM chains

Multi-VM chains differ in how they reconcile state across VMs.
Aptos hosts a Move-only substrate keyed by typed resources [Aptos
Move, 2022]; Sui exposes objects to Move with a parallel-execution
runtime over the same primitive [Sui Move, 2024]; Solana presents a
single account model to several runtimes through Sealevel [Solana,
2020]. Each design fixes the substrate at one VM's primitives and
projects others through it.

We take the inverse approach. The substrate of §7.4 (Suwappu-DB) is
neither EVM-shaped nor Move-shaped; it is a canonical
`(address, asset) → BalanceSlot` map with explicit projections per
VM. The dual-projection invariant (Proposition 4) is verified
structurally — by the Rust type system on the single canonical
field — rather than by reconciliation between two state machines.
The capability-gated mutation path through `BridgeToken` makes the
write surface a singleton in the type system, eliminating the
class of bugs where a non-bridge code path mutates state outside
the validation pipeline.

The closest prior work in spirit is Anchor [Anchor, 2021] which
introduces capability-token mutation gates on Solana programs at
the framework level. The Suwappu-DB capability gate is enforced at the
state-crate boundary (the smallest unit of code that can construct
the token), not at the framework level, which makes the invariant
auditable without reading any program logic.

### References to add

```bibtex
@misc{AptosMove2022,
  author = {Aptos Labs},
  title  = {Aptos Move: A safe, sandboxed asset-oriented language
            for smart contracts},
  year   = {2022}
}
@misc{SuiMove2024,
  author = {Mysten Labs},
  title  = {Move on Sui: object-centric programming model},
  year   = {2024}
}
@misc{Solana2020,
  author = {Anatoly Yakovenko},
  title  = {Solana: A new architecture for a high performance
            blockchain},
  year   = {2020}
}
@misc{Anchor2021,
  author = {Coral and contributors},
  title  = {Anchor: A framework for Solana programs},
  year   = {2021}
}
```
