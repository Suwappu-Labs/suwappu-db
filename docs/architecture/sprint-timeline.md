# Sprint timeline

Phase-1 closed + launch-readiness sprints in flight. Visual record
of when each sprint landed and what's still queued.

## Gantt

```mermaid
gantt
    title Suwappu-DB delivery sprints
    dateFormat YYYY-MM-DD
    axisFormat %b '%y

    section Phase 1
    S1 workspace + lane sep      :done, s1, 2026-04-23, 7d
    S2 BalanceSlot + redb        :done, s2, after s1, 7d
    S3 VM-shape tx + projectors  :done, s3, after s2, 7d
    S4 CE-MVCC OCC               :done, s4, after s3, 7d
    S5 Cross-VM intent bundles   :done, s5, after s4, 7d
    S6 State-tree commitment     :done, s6, after s5, 7d
    S7 Anchor log + parity       :done, s7, after s6, 7d
    S8 Block store + recovery    :done, s8, after s7, 7d

    section Launch readiness
    S8.5 redb BlockStore         :done, s85, 2026-05-08, 5d
    S9 Real Move VM (Aptos)      :active, s9, 2026-05-09, 14d
    S10 Real Verkle / IPA        :active, s10, after s9, 21d
    S11 Solidity LTPAnchorReg    :active, s11, after s10, 14d
    S12 DAG + snapshots + shadow :s12, after s11, 21d

    section Mainnet
    Audit + freeze               :crit, audit, after s12, 30d
    Mainnet launch               :milestone, mn, after audit, 0d
```

Phase-1 closed in 8 weeks (S1–S8); launch-readiness path adds
approximately 10–12 more weeks plus audit.

## Sprint exit gates (each is a 10,000-case property test)

```mermaid
flowchart TB
    S1[S1] -- lane-separation script --> S2[S2]
    S2 -- redb_preserves_dual_projection --> S3[S3]
    S3 -- interleaved_evm_move_preserves_invariant --> S4[S4]
    S4 -- parallel_equals_sequential --> S5[S5]
    S5 -- bundle_atomicity --> S6[S6]
    S6 -- cross_tree_root_agreement --> S7[S7]
    S7 -- cross_chain_parity_holds --> S8[S8]
    S8 -- recover_matches_live_state --> S85[S8.5]
    S85 -- redb persistence + restart proptest --> S9[S9]
    S9 -- dual_projection w/ real Move bytecode --> S10[S10]
    S10 -- IPA witness differential parity --> S11[S11]
    S11 -- 36-pair Solidity-Rust parity matrix --> S12[S12]
    S12 -- shadow E2E zero invariant violations --> Mainnet[Mainnet ready]

    style S1 fill:#cfc
    style S2 fill:#cfc
    style S3 fill:#cfc
    style S4 fill:#cfc
    style S5 fill:#cfc
    style S6 fill:#cfc
    style S7 fill:#cfc
    style S8 fill:#cfc
    style S85 fill:#cfc
    style S9 fill:#fed
    style S10 fill:#fed
    style S11 fill:#fed
    style S12 fill:#fee
    style Mainnet fill:#fcd
```

Legend: green = closed, yellow = partial / in flight, pink = not
started.

## What each sprint introduced

| Sprint | Crate touched | Key types | LOC delta (approx) |
|---|---|---|---|
| S1 | workspace | `Address`, `Balance`, `BridgeToken`, `State`, `Bridge` | +1,200 |
| S2 | suwappudb-state | `BalanceSlot`, `BalanceStore`, `InMemoryBalanceStore`, `RedbBalanceStore` | +2,400 |
| S3 | suwappudb-state, suwappudb-bridge | `EvmTx`, `MoveTx`, `EvmProjector`, `MoveProjector`, `MockEvm`, `MockMove` | +1,800 |
| S4 | suwappudb-bridge | `MvStore`, `Txn`, `Validator`, `BlockExecutor`, `BlockReport` | +2,100 |
| S5 | suwappudb-bridge | `Bundle`, `BundleExecutor`, `ContractRegistry`, `BundleGenerator`, `Intent::Call` | +1,600 |
| S6 | suwappudb-state | `Node`, `Commitment`, `Proof`, `StateTree` | +1,400 |
| S7 | suwappudb-bridge | `Anchor`, `AnchorLog`, `AnchorDispatcher`, `ParityResult` | +1,200 |
| S8 | suwappudb-bridge | `Block`, `BlockStore`, `InMemoryBlockStore`, `replay` | +1,800 |
| S8.5 | suwappudb-bridge | `RedbBlockStore`, `BlockStoreError` | +600 |
| S9 | suwappudb-state | `MoveExecutor`, `AccountNonce`, `MoveAddress`, `address_shape` | +800 |
| S10 | suwappudb-state | `tree::verkle::GroupElement` (placeholder) | +200 |
| S11 | suwappudb-bridge | `AuthScheme`, `L1AnchorReader`, `LTPAnchorRegistry.sol` | +1,500 |
| S12 | suwappudb-state, suwappudb-bridge | `DagBlock`, `DagStore`, `SnapshotManager`, telemetry, shadow E2E | +1,800 |

## Test count growth

```mermaid
xychart-beta
    title Tests passing per sprint
    x-axis [S1, S2, S3, S4, S5, S6, S7, S8, S8.5, S9, S10, S11, S12]
    y-axis "Tests" 0 --> 300
    bar [6, 36, 52, 79, 104, 134, 157, 178, 181, 220, 230, 250, 269]
```

(Counts reproduced from sprint closeouts plus cumulative
integration tests.)

## What's still queued (post-S12)

- **Audit + freeze.** 30–60 days. Two independent audits expected
  (one on substrate, one on consensus + LTP).
- **Mainnet launch.** Gated on audit pass + IQ-1 RocksDB swap
  decision.
- **Phase-2.** Mempool, fee market, JSON-RPC `eth_*` compatibility,
  reorg handling at DAG level. Each its own sprint.
