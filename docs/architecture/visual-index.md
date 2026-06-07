# Visual index

Every Mermaid diagram in the docs, catalogued. Click a thumbnail to
land on the section that explains it.

## Architecture diagrams

| Diagram | File | What it shows |
|---|---|---|
| Three-crate split | [overview.md](overview.md#the-three-crates) | `suwappudb-lane → suwappudb-bridge → suwappudb-state` with the forbidden direct edge |
| Capability-gated mutation sequence | [overview.md](overview.md#the-capability-gated-mutation-path) | Lane submits Intent → bridge validates → BridgeToken → state apply |
| Parallel-vs-sequential split | [overview.md](overview.md#what-runs-in-parallel-what-runs-sequentially) | Aptos Block-STM shape: speculative parallel + sequential validate + retry |
| Full pipeline (top-down) | [data-flow.md](data-flow.md#the-full-pipeline) | Intent → OCC → bundle → state → tree → anchor → store → replay |
| OCC speculative execution | [data-flow.md](data-flow.md#2-speculative-parallel-execution-occ) | rayon par_iter over execute_one |
| Validation + retry loop | [data-flow.md](data-flow.md#3-sequential-validation--retry) | Block-STM convergence loop |
| Cross-VM bundle dispatch | [data-flow.md](data-flow.md#4-cross-vm-bundle-dispatch-when-intentcall) | per-bundle local accumulator interplay with MV store |
| Consolidation via BridgeToken | [data-flow.md](data-flow.md#5-consolidation-through-bridgetoken) | finalise() → apply per (addr, slot) |
| Recovery via replay | [data-flow.md](data-flow.md#9-recovery-via-replay) | BlockStore iter_from → BlockExecutor → defence-in-depth tree rebuild |
| Conventional vs Suwappu DB cross-VM | [dual-projection.md](dual-projection.md#why-this-matters) | Bridge-prone vs canonical-state design |
| BalanceSlot projection shape | [dual-projection.md](dual-projection.md#the-shape) | u128 canonical → EvmBalance + MoveCoinValue |
| VM-shape transactions → canonical state | [dual-projection.md](dual-projection.md#how-vm-shaped-transactions-reach-canonical-state) | EvmTx / MoveTx flattening to Intent |
| Read paths via projectors | [dual-projection.md](dual-projection.md#reads-through-evmprojector--moveprojector) | EvmProjector + MoveProjector delegating to slot_of |
| 256-ary trie | [state-tree.md](state-tree.md#tree-shape) | Internal/Leaf nodes, depth 20 |
| Node enum | [state-tree.md](state-tree.md#node-types) | classDiagram of Node / Commitment |
| Insert path | [state-tree.md](state-tree.md#insert-path) | Update descending through 20 levels |
| Inclusion proof | [state-tree.md](state-tree.md#inclusion-proof-depth-20-slot--some) | Path of ProofSteps from root to leaf |
| Absence proof — early term | [state-tree.md](state-tree.md#absence-proof--early-termination-depth-k--20-slot--none) | Variable-length, terminates at empty child |
| Verifier flavour dispatch | [state-tree.md](state-tree.md#why-three-flavours) | Empty / Inclusion / Absence branches |
| Block-level tree integration | [state-tree.md](state-tree.md#block-level-integration) | BlockExecutor → State::entries → StateTree::from_entries |
| Sprint dependency DAG | [sprint-map.md](sprint-map.md#dependency-graph) | S1–S8 with sprint deps + exit gates |
| IQ decision points | [sprint-map.md](sprint-map.md#iq-decision-points-across-sprints) | IQ-1..IQ-8 vs sprint where they apply |
| Git branch history | [sprint-map.md](sprint-map.md#branch-history) | gitGraph of per-sprint feature branches + merges |
| Launch-readiness backlog | [open-iqs.md](open-iqs.md#deferred-decisions) | IQ-1, 3, 6, 7, 8 in flight |
| Launch-readiness sequence | [open-iqs.md](open-iqs.md#launch-readiness-as-its-own-sprint) | LR-1..LR-4 sprints on top of phase-1 |

## Decision diagrams (IQs)

| Diagram | File | What it shows |
|---|---|---|
| Move VM choice flow | [IQ-3](../iq/IQ-3-move-vm-choice.md) | 5 options → Aptos selected |
| Verkle commitment swap | [IQ-6](../iq/IQ-6-verkle-commitment.md) | BLAKE3 placeholder → IPA over banderwagon |
| Anchor parity phases | [IQ-7](../iq/IQ-7-anchor-parity.md) | In-memory MAC → Solidity + ECDSA |
| IQ decision flow | [iq/README.md](../iq/README.md#decision-flow) | All 8 IQs and their dependencies |
| Anchor auth dispatch | [IQ-7](../iq/IQ-7-anchor-parity.md#rust-integration-surface) | AuthScheme enum branches |
| Parity check sequence | [IQ-7](../iq/IQ-7-anchor-parity.md#parity-check-semantics) | Node → L1AnchorReader → eth_call → compare |

## Ecosystem + deployment diagrams

| Diagram | File | What it shows |
|---|---|---|
| Production architecture (current live) | [ecosystem.md](../ECOSYSTEM-AUDIT.md#2-actual-production-architecture-today) | Besu L1 + OP rollup; backend services; suwappu-db not yet wired |
| Deployment topology (target) | [deployment-topology.md](deployment-topology.md) | What deploys where: validator ring + corridor super-nodes + L1 + RPC nodes |
| Dual-ring validator set | [validator-rings.md](validator-rings.md) | Authority Ring + Validator Ring with corruption profiles |
| Sprint Gantt | [sprint-timeline.md](sprint-timeline.md) | Phase-1 (S1–S8) + launch-readiness (S8.5–S12) timeline |
| Request lifecycle | [request-lifecycle.md](request-lifecycle.md) | Wallet RPC → suwappudb-server → state → response |

## Paper-aligned diagrams

| Diagram | File | What it shows |
|---|---|---|
| LTP three-phase lifecycle | [ltp-lifecycle.md](ltp-lifecycle.md) | Commit → Lattice → Materialize with the constant-bandwidth claim |
| LTP six-layer security | [ltp-lifecycle.md](ltp-lifecycle.md#six-layer-security-stack) | Independent security layers |

## Quick stats

- **34 architecture/spec/IQ diagrams** indexed
- **All Mermaid** — renders on GitHub natively, no external tooling
- **All cross-linked** — clicking through any diagram lands on the
  prose that explains it

## How to add a new diagram

1. Drop the Mermaid block in the relevant file under `docs/`
2. Add a row to this index pointing at the diagram's anchor
3. If the diagram is load-bearing (introduces or invariants a new
   concept), also link it from `docs/README.md`
