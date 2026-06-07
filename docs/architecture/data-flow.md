# Data flow

End-to-end: from a user submitting an `Intent` to the chain
committing post-block state, anchoring it, and persisting the block.

## The full pipeline

```mermaid
flowchart TB
    User([User])
    Lane[suwappudb-lane]
    BlockExec[BlockExecutor::<br/>execute_with_registry]
    OCC{OCC scheduler}
    MvStore[(MvStore)]
    Registry[ContractRegistry]
    BundleExec[Bundle execution<br/>per Call]
    Bridge[Bridge::submit<br/>+ BridgeToken]
    Store[(BalanceStore)]
    Tree[StateTree::from_state]
    Anchor[AnchorDispatcher::<br/>dispatch]
    Logs[AnchorLogs<br/>per chain]
    Block[Block]
    BlockStore[(BlockStore)]

    User -- "Vec&lt;Intent&gt;" --> Lane
    Lane -- block --> BlockExec
    BlockExec --> OCC
    OCC -- "speculative<br/>execute_one" --> MvStore
    OCC -- "Intent::Call" --> Registry
    Registry -- generator --> BundleExec
    BundleExec --> MvStore
    OCC -- consolidate --> Bridge
    Bridge --> Store
    BlockExec --> Tree
    Tree -.read full state.-> Store
    BlockExec -- "(height, root)" --> Anchor
    Anchor --> Logs
    BlockExec --> Block
    Block --> BlockStore
```

## Step-by-step

### 1. Intent submission (lane → bridge)

```mermaid
sequenceDiagram
    actor User
    participant Lane as suwappudb-lane
    participant Block as BlockExecutor
    User->>Lane: submit intents
    Lane->>Lane: collect into a block (Vec<Intent>)
    Lane->>Block: execute_with_registry(state, &block, &registry)
```

A block is just `Vec<Intent>`. Lane code accumulates intents in
whatever way it wants (per round, per timer, per length); the block
executor doesn't care.

### 2. Speculative parallel execution (OCC)

```mermaid
sequenceDiagram
    participant BE as BlockExecutor
    participant rayon as rayon scope
    participant E1 as execute_one(0)
    participant E2 as execute_one(1)
    participant En as execute_one(N-1)
    participant MV as MvStore
    BE->>rayon: par_iter pending
    rayon->>E1: speculatively execute
    rayon->>E2: speculatively execute
    rayon->>En: speculatively execute
    E1->>MV: read(addr, idx=0)
    E1->>MV: write(addr, slot, idx=0)
    E2->>MV: read(addr, idx=1)
    E2->>MV: write(addr, slot, idx=1)
    En->>MV: read/write at idx=N-1
    rayon-->>BE: Vec<Txn>
```

Every txn writes its own version into `MvStore` keyed by `TxnIdx`.
Reads return the highest-versioned write strictly below the
reader's index, falling through to canonical state.

### 3. Sequential validation + retry

```mermaid
flowchart LR
    Validate{Every Txn:<br/>read set<br/>still valid?} -- yes --> Done[converged]
    Validate -- no --> Clear[clear_writes for stale txn]
    Clear --> Spec[re-execute pending]
    Spec --> Validate
```

Block-STM proves logarithmic iterations under random workloads. Cap
is `2 * block_len + 4`; exceeding it panics as an algorithmic bug.

### 4. Cross-VM bundle dispatch (when Intent::Call)

```mermaid
sequenceDiagram
    participant E as execute_one(idx)
    participant Reg as ContractRegistry
    participant Gen as BundleGenerator
    participant MV as MvStore
    participant Local as Per-bundle local
    E->>Reg: get(target)
    alt target registered
        Reg-->>E: Arc<dyn BundleGenerator>
        E->>Gen: generate(CallCtx)
        Gen-->>E: Bundle { steps }
        loop each step in bundle
            E->>Local: read addr (intra-bundle visible)
            Local-->>E: slot or fall through
            E->>MV: read(addr, idx) if not in local
            E->>Local: insert(addr, new_slot)
        end
        E->>MV: publish all local writes at idx
        Note over E: On any step's failure:<br/>mv.clear_writes(idx)<br/>return rejected Txn
    else target not registered
        Reg-->>E: None
        E->>MV: plain transfer fallback
    end
```

The per-bundle local accumulator is the critical fix from S5. The MV
store deliberately excludes same-idx reads (OCC validation
correctness); bundles need step-N+1 to see step-N writes, so the
executor maintains an in-bundle map consulted before MV.

### 5. Consolidation through BridgeToken

```mermaid
sequenceDiagram
    participant BE as BlockExecutor
    participant MV as MvStore
    participant Token as BridgeToken
    participant State as State::apply
    BE->>MV: finalise()
    MV-->>BE: Vec<(Address, BalanceSlot)>
    BE->>Token: __for_bridge_only()
    loop each (addr, slot)
        BE->>State: apply(&token, StateChange::SetBalance)
    end
```

This is the only path through which speculative writes reach
canonical state. The capability gate is the single mutation entry
point.

### 6. State tree + block report

```mermaid
flowchart LR
    State -- entries --> Tree[StateTree::from_state]
    Tree -- root --> Report[BlockReport]
    Report -- state_root --> Caller
    Report -- outcomes --> Caller
    Report -- iterations,aborts --> Caller
```

Phase-1 rebuilds the tree from full state per block. S6.5 / S8 will
introduce incremental updates without changing the trait surface.

### 7. Anchor dispatch

```mermaid
flowchart LR
    BR[BlockReport.state_root] --> Disp[AnchorDispatcher::dispatch]
    Disp --> A1[Anchor for ChainId 1]
    Disp --> A2[Anchor for ChainId 2]
    Disp --> A3[Anchor for ChainId 3]
    A1 --> L1[(Log for chain 1)]
    A2 --> L2[(Log for chain 2)]
    A3 --> L3[(Log for chain 3)]
```

One anchor per chain, MAC'd under that chain's key, linked to the
previous anchor on that chain. `parity_check(height)` reads all logs
and returns `Agreed` iff every chain matches.

### 8. Block persistence

```mermaid
flowchart LR
    BlockBuild[Build Block { height, parent, state_root, intents }] --> BlockHash[block.hash]
    BlockHash --> BlockPut[BlockStore::put]
    BlockPut --> BS[(BlockStore)]
```

Block hash is BLAKE3 of canonical encoding (height | parent |
state_root | intent_count | each intent with type tag). Block store
indexes by hash and by height.

### 9. Recovery via replay

```mermaid
sequenceDiagram
    participant Fresh as Fresh State
    participant Replay as recovery::replay
    participant BS as BlockStore
    participant BE as BlockExecutor
    participant Tree as StateTree
    Fresh->>Replay: replay(store, &mut state, registry, from=0)
    Replay->>BS: iter_from(0)
    BS-->>Replay: Vec<Block> in height order
    loop every block
        Replay->>Replay: verify parent linkage
        Replay->>BE: execute_with_registry(state, &block.intents, registry)
        BE-->>Replay: BlockReport
        Replay->>Replay: assert recorded == computed state_root
        Replay->>Tree: from_state(state).root
        Tree-->>Replay: live root
        Replay->>Replay: defence-in-depth: assert live == recorded
    end
    Replay-->>Fresh: state at latest height
```

Replay is the determinism property's stress test. Any mismatch
surfaces as `RecoveryError::StateRootMismatch` — either the store
was tampered or the executor isn't deterministic (which would be a
serious S4 regression).

## Invariants enforced at every step

| Layer | Invariant | Verified by |
|---|---|---|
| Type | `EvmBalance == MoveCoinValue` for any slot | `projections_always_agree` (S2) |
| In-memory store | Same op sequence ⇒ same state | `op_replay_is_deterministic` (S2) |
| Persistent store | Round-trip through redb preserves slot | `redb_preserves_dual_projection` (S2) |
| Block executor | Parallel ≡ sequential | `parallel_equals_sequential` (S4) |
| Bundle | Atomic — fail step ⇒ no state change | `bundle_atomicity` (S5) |
| Tree | Same state ⇒ same root | `cross_tree_root_agreement` (S6) |
| Anchors | Same root ⇒ Agreed across all chains | `cross_chain_parity_holds` (S7) |
| Recovery | Replay ≡ live execution | `recover_matches_live_state` (S8) |

Each row is a 10,000-case property test in dev or release.
