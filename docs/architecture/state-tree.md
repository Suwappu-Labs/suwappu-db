# State tree

Per-block commitment to the canonical state. 256-ary trie shape
(Verkle-aligned). Hash-based commitments now (BLAKE3 per
[IQ-6](../iq/IQ-6-verkle-commitment.md)); polynomial
commitments at launch.

## Tree shape

```mermaid
flowchart TB
    Root["Root (Internal)"]
    Root --> N0["byte 0x01<br/>(Internal)"]
    Root --> N1["byte 0x42<br/>(Internal)"]
    Root --> N2["byte 0xff<br/>(Internal)"]
    N0 --> N00["byte 0x01<br/>(Internal)"]
    N0 --> N01["byte 0x09<br/>(Internal)"]
    N1 --> N10["byte 0x00<br/>(Internal)"]
    N00 --> Leaf1["...18 more levels...<br/>Leaf(slot=42)"]
    N01 --> Leaf2["...18 more levels...<br/>Leaf(slot=99)"]
    N10 --> Leaf3["...18 more levels...<br/>Leaf(slot=7)"]
```

- **256-ary**: each internal node has up to 256 children, keyed by
  one byte of the address path
- **Depth 20**: address is 20 bytes; leaf depth is exactly the
  address length
- **Sparse**: `BTreeMap<u8, Box<Node>>` per internal node — only
  populated children take memory, ordered keys give deterministic
  commitment input
- **Empty subtrees collapse**: an unallocated subtree is `Node::Empty`
  with a fixed precomputed commitment

## Node types

```mermaid
classDiagram
    class Node {
        <<enum>>
    }
    Node : Empty
    Node : Leaf(BalanceSlot)
    Node : Internal(BTreeMap~u8, Box~Node~~)
    class Commitment {
        +[u8; 32]
    }
    Node --> Commitment : commit_node
```

## Commitment scheme

```
empty_commitment       = BLAKE3("Suwappudb-TREE/EMPTY")
commit(Empty)          = empty_commitment
commit(Leaf(slot))     = BLAKE3("Suwappudb-TREE/LEAF_" | slot.canonical().to_be_bytes())
commit(Internal(kids)) = BLAKE3("Suwappudb-TREE/INT__"
                                | for (byte, child) in sorted(kids):
                                    byte (1B) | commit(child) (32B))
```

Domain-separated tags (`EMPTY`, `LEAF_`, `INT__`) prevent cross-type
collisions. Internal nodes iterate via `BTreeMap` for deterministic
ordering — same logical state always produces the same commitment
regardless of how children were inserted.

## Insert path

```mermaid
sequenceDiagram
    participant U as Update(addr, slot)
    participant N0 as Node @ depth 0
    participant N1 as Node @ depth 1
    participant N19 as Node @ depth 19
    U->>N0: byte = addr[0], descend
    Note over N0: Empty → Internal{}<br/>via promotion
    N0->>N1: byte = addr[1], descend
    Note over N1: ...
    N1->>N19: byte = addr[19], descend
    N19->>N19: replace with Leaf(slot)
```

Updates allocate intermediate `Internal` nodes lazily as the path
descends. Empty subtrees promote to `Internal({})` on first write.

## Proof shape

Two flavours: inclusion (full depth) and absence (early termination).

### Inclusion proof (depth 20, slot = Some)

```mermaid
flowchart LR
    Root --> Step0[ProofStep<br/>byte=addr0<br/>siblings={...}]
    Step0 --> Step1[ProofStep<br/>byte=addr1<br/>siblings={...}]
    Step1 --> Stepn[...]
    Stepn --> Step19[ProofStep<br/>byte=addr19<br/>siblings={}]
    Step19 --> Leaf[Leaf(slot)]
```

The bottom step's byte contributes the leaf commitment to its
parent's reconstruction. Verifier walks root → leaf, rebuilding each
internal commitment from `current` + `siblings`.

### Absence proof — early termination (depth K < 20, slot = None)

```mermaid
flowchart LR
    Root --> Step0[ProofStep<br/>byte=addr0<br/>siblings={1: c1, 5: c5}]
    Step0 --> Stepk[ProofStep at depth K<br/>byte=addrK<br/>NO CHILD AT byte]
    Stepk -.terminates here.-> X[no further<br/>steps recorded]
```

The proof terminates at the first depth where `addr_byte` has no
child. Verifier excludes that byte from the parent's reconstructed
commitment (which is what the actual tree did).

### Absence proof in empty tree

`path` is empty. Verifier checks `root == empty_commitment`. Done.

## Why three flavours

```mermaid
flowchart TB
    Root{Verifier::verify}
    Root -- proof.path empty + slot None --> Empty[root == empty_commitment?]
    Root -- proof.path full + slot Some --> Inclusion[reconstruct including bottom byte]
    Root -- proof.path partial + slot None --> Absence[reconstruct excluding bottom byte]
```

Distinguishing them was the bug found mid-S6: an absence proof
that pads to full depth ends up adding phantom (byte, computed)
entries to parent commitments, breaking verification. Variable-
length proofs + an `include_self_in_parent` flag at the bottom
step is the fix.

## Block-level integration

```mermaid
sequenceDiagram
    participant BE as BlockExecutor
    participant State as State (post-consolidation)
    participant Tree as StateTree
    BE->>State: entries()
    State-->>BE: Vec<(Address, BalanceSlot)>
    BE->>Tree: from_entries(...)
    Tree-->>BE: StateTree
    BE->>Tree: root()
    Tree-->>BE: Commitment
    BE->>BE: BlockReport.state_root = root
```

Phase-1 rebuilds the entire tree from full state every block. S6.5
will introduce incremental updates touching only changed paths;
trait surface unchanged.

## Witness size — phase-1 vs launch

| Case | Phase-1 (BLAKE3) worst case | Launch Verkle (IPA) |
|---|---|---|
| Inclusion | 20 levels × 255 sib × 32B = ~163KB | ~200B |
| Absence (early term) | depth × 255 × 32B, depth ≤ 20 | ~200B |
| Empty tree | 0B | 0B |

Stateless light clients are gated on the swap to real Verkle. Per
IQ-6, this is a launch-readiness item — the property tests
(`cross_tree_root_agreement`, `tampered_slot_rejected`, etc.) stay
green under the swap because they test commitment semantics, not
witness size.

## What gets verified at 10k cases

- Determinism: same state ⇒ same root, regardless of insert order
- Sensitivity: any state change ⇒ different root
- Inclusion: every inserted address has a verifying proof
- Absence: every uninserted address has a verifying absence proof
- Tamper resistance: bumped-slot claims are rejected
- Cross-tree agreement: sequential vs from-effective-map produce the
  same root and both verify all inclusions
