# Insertion: LTP paper, additions to Table 2 (§8)

**Where:** Table 2 (Test coverage on LTPAnchorRegistry v5), §8.2 —
append a Rust section so the substrate-level test surface is
recorded alongside the Python and Solidity surfaces.

---

## Updated Table 2

```text
Surface                                          Tests
Solidity unit + integration                         34
Solidity fuzz + invariant + formal                  15
  fuzz iterations / test                           256
  invariant calls / test                         3,840
Python core protocol                               139
Python cryptography                                 89
Python security                                    224
Python enforcement                                 167
Python verification                                 98
Python economics                                    95
Python infrastructure                               96
Python on-chain integration                         31
Python misc                                        228
Rust suwappudb-state lib                               101  ★ NEW
Rust suwappudb-bridge lib                              112  ★ NEW
Rust suwappudb-state integration (state_tree)            6  ★ NEW
Rust suwappudb-bridge integration                       38  ★ NEW
Rust suwappudb-lane lib                                  2  ★ NEW
  proptest cases / invariant                  10,000   ★ NEW
 Total                                          1,603  (was 1,344)
```

The Rust integration tests are partitioned across seven
integration targets in `crates/suwappudb-bridge/tests/`:
`block_executor` (4), `cross_parity` (5), `cross_vm_bundles` (4),
`cross_vm_parity` (6), `e2e_shadow_testnet` (4), `persistent_e2e`
(4), `recovery` (3), `solidity_anchor_parity` (8).

The 10,000-cases-per-invariant figure is the production exit-gate
strength, run via `PROPTEST_CASES=10000 cargo test --release` on
each of the eight load-bearing invariants enumerated in [Suwappu DAG L1,
2026, §11.3, Table 4]. The default CI run uses 256 cases per
invariant for fast feedback; release tags run the 10,000-case
strength end-to-end.

### Note on cross-implementation parity

The `solidity_anchor_parity` integration test (8 tests, 256 default
proptest cases) encodes the Solidity contract's deterministic logic
as Rust fixtures: Keccak256-MAC computation, packed-field encoding
order, and the 11-transition state-machine matrix of §7.3. The Rust
substrate independently validates each fixture, producing a
three-way parity record across Python, Solidity, and Rust. The
parity discipline of §7 and §8.2 is preserved across all three
implementations.
