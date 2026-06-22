# Insertion: DAG L1 paper, new §11.3

**Where:** end of §11 (Safety and Liveness), after Proposition 3.

---

## 11.3 Empirical reinforcement of the formal arguments

Theorem 2 and Proposition 1 establish safety on the consensus and
execution sides analytically. The Suwappu-DB substrate of §7.4
discharges the implementation-level burden of those theorems with
randomized property tests at 10,000 cases per invariant. Table 4
maps each formal claim to a test that empirically falsifies its
negation across the seeded input space.

### Table 4: Property-tested invariants

| # | Formal claim | Suwappu-DB test | Cases |
|---|---|---|---|
| 1 | Lane separation (§7.4.1) | `scripts/check-lane-separation.sh` | — (structural) |
| 2 | Dual-projection (Prop. 4) | `redb_preserves_dual_projection` | 10,000 |
| 3 | Cross-VM canonical equivalence (Prop. 1) | `interleaved_evm_move_preserves_invariant` | 10,000 |
| 4 | Schedule determinism (Prop. 5) | `parallel_equals_sequential` | 10,000 |
| 5 | Bundle atomicity (§7.4.3) | `bundle_atomicity` | 10,000 |
| 6 | Tree determinism (Prop. 6) | `cross_tree_root_agreement` | 10,000 |
| 7 | Cross-chain anchor parity (§7.4.7) | `cross_chain_parity_holds` | 10,000 |
| 8 | Replay equivalence (Prop. 7) | `recover_matches_live_state` | 10,000 |

The workspace ships 259 tests in total; the eight rows above are the
load-bearing exit gates for the corresponding sprint deliverables
(S1–S8). Each exit gate is a randomized falsification attempt
against a precisely stated invariant; a failing run shrinks to a
minimal counterexample that is recorded in
`proptest-regressions/`.

### Why property testing here

The conventional alternative — example-based unit testing — does
not interact well with the structural claims in §7.3 and §11. The
adversary in Definition 3 controls scheduling and message delay
within $\Delta$ and may interleave EVM-shape and Move-shape
transactions adversarially. Example-based tests cover hand-chosen
points in this space; property tests sample it uniformly with seeds
drawn from the strategy combinators of the `proptest` crate. Where
the adversary's strategy space is finite, exhaustion is plausible
at 10,000 cases; where it is infinite (transaction-set sizes,
sequence lengths), 10,000 cases provides empirical confidence
without claiming formal proof.

### What this does not establish

The property tests run against the Suwappu-DB substrate in isolation.
Properties of the full chain — that the Mysticeti certificate DAG
produces a unique linearization, that the fast-path lane converges
with the main lane within $K$ rounds, that the SCION transport
preserves the bound $\Delta$ — are inherited from the cited prior
art and are not separately re-tested here. The substrate guarantees
hold conditional on the consensus layer behaving as specified.
