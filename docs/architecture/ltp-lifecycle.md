# LTP three-phase lifecycle

Per the LTP companion paper. GSX-DB integrates with LTP as both
**producer of anchors** (outbound, via `AnchorDispatcher`) and
**consumer of attestations** (inbound, via `L1AnchorReader`).

## The three phases

```mermaid
flowchart LR
    subgraph S[Sender]
        D[Payload D]
    end
    subgraph N[Commitment network nodes]
        S1[Shard 1]
        S2[Shard 2]
        Sn[Shard n]
    end
    subgraph R[Receiver]
        D2[Reconstructed D]
    end

    D -- Phase 1: Commit --> RS[Reed-Solomon n,k encode]
    RS -- encrypt CEK --> S1
    RS --> S2
    RS --> Sn

    S -- Phase 2: Lattice<br/>constant 1.3 kB envelope<br/>ML-KEM-768 sealed --> R

    R -- Phase 3: Materialize<br/>fetch any k of n --> S1
    R --> S2
    S1 --> D2
    S2 --> D2

    style D fill:#cef
    style D2 fill:#cef
```

### Phase 1 — Commit

Sender encrypts and erasure-codes the payload, distributes shards
across the commitment network, anchors a Signed Tree Head on-chain:

```mermaid
sequenceDiagram
    actor Sender
    participant CN as Commitment Network
    participant L as Public Commitment Log

    Sender->>Sender: 1. Sample fresh CEK (32 bytes)
    Sender->>Sender: 2. Compute eid = SHA3-256(D + shape + ts + pkS)
    Sender->>Sender: 3. Reed-Solomon encode: D → (s1..sn)
    Sender->>Sender: 4. Encrypt each shard: ci = AEAD(si, CEK, nonce_i)
    Sender->>CN: 5. Distribute (c1..cn) via loc(eid, i)
    Sender->>L: 6. Append Signed Tree Head ML-DSA-65
```

### Phase 2 — Lattice (the constant-bandwidth phase)

Sender transmits a constant-size sealed envelope to the receiver,
independent of payload size:

```mermaid
flowchart LR
    Sender --> Env["Envelope ~1300 bytes<br/>ML-KEM-768 ciphertext + lattice payload"]
    Env --> Receiver

    subgraph Inside[Inside the envelope]
        eid[eid 32 B]
        cek[CEK 32 B]
        rho[STH reference 32 B]
        policy[access policy ≤64 B]
    end
    Env -.contents.-> Inside
```

After this transmission the sender may disconnect entirely. The
receiver has everything needed to reconstruct.

### Phase 3 — Materialize

Receiver unseals the envelope, fetches any k of n shards from the
geographically nearest nodes, decodes, and verifies:

```mermaid
sequenceDiagram
    actor Receiver
    participant CN as Commitment Network
    participant L as Public Commitment Log

    Receiver->>Receiver: 1. Decap envelope → CEK, eid, rho
    Receiver->>L: 2. Fetch Signed Tree Head at rho
    Receiver->>CN: 3. Fetch any k shards of n via loc(eid, i)
    CN-->>Receiver: ci_1..ci_k
    Receiver->>Receiver: 4. Decrypt each: si = AEAD-decrypt(ci, CEK, nonce_i)
    Receiver->>Receiver: 5. Reed-Solomon decode: D' = RS^-1(si1..sik)
    Receiver->>Receiver: 6. Verify Hc(D' + ... + pkS) = eid
```

## Six-layer security stack

Each layer addresses one threat surface; compromise of any single
layer leaves the others intact.

```mermaid
flowchart TB
    L6[Layer 6 — Programmable access policy]
    L5[Layer 5 — Sealed envelope ML-KEM-768]
    L4[Layer 4 — Shard-level AEAD]
    L3[Layer 3 — Optional ZK mode Groth16 over BLS12-381<br/>not on default settlement path]
    L2[Layer 2 — Cryptographic integrity SHA3-256 + ML-DSA-65]
    L1[Layer 1 — Reed-Solomon threshold below k<br/>information-theoretic]

    L6 --> L5 --> L4 --> L3 --> L2 --> L1
    style L1 fill:#cfc
    style L2 fill:#cfc
    style L4 fill:#cfc
    style L5 fill:#cfc
    style L3 fill:#fed
    style L6 fill:#cef
```

The plaintext-indistinguishability theorem (paper §5.2) relies on
Layers 1, 4, 5. Layers 2, 3, 6 strengthen orthogonal surfaces
without weakening the central confidentiality argument.

## Bandwidth profile

```mermaid
flowchart LR
    subgraph TCP[Conventional TCP/HTTP]
        D1[Payload D] --> SBig[Sender]
        SBig -- Bandwidth ∝ |D| --> RBig[Receiver]
        RBig --> D1r[D received]
    end
    subgraph LTP[LTP]
        D2[Payload D] --> S[Sender]
        S -- Phase 1: distribute<br/>shards to CN --> CN[(Commitment<br/>Network)]
        S -- Phase 2: 1.3 kB envelope<br/>regardless of D --> R[Receiver]
        R -- Phase 3: fetch k shards<br/>locally proximate --> CN
        CN --> D2r[D reconstructed]
    end
```

**The wire-cost of the sender-to-receiver path is constant.** Bandwidth
proportional to `|D|` is distributed across the commitment network
at commit time and reconstructed receiver-side at materialize time.

## The February 2026 critical finding

The original protocol design exposed shard identifiers in three
independent locations:

```mermaid
flowchart LR
    L1[Leak 1 — Envelope in transit] -.bypass.-> SI[Shard IDs]
    L2[Leak 2 — Commitment log at rest] -.bypass.-> SI
    L3[Leak 3 — Nodes serving unencrypted shards] -.bypass.-> SI
    SI --> Attack[Plaintext recovery<br/>without envelope]

    style Attack fill:#fcc
```

The fix (Option C in paper §6.2): encrypted shards with derivable
metadata. Receiver derives shard locations from the entity
identifier alone via `loc(eid, i)`; shard identifiers live only as
Merkle leaves in the commitment record. The lattice key shrank from
~869 B to ~160 B and closed all three leakage points.

## How gsx-db integrates

```mermaid
flowchart LR
    subgraph GsxDB[gsx-db substrate]
        Bridge[AnchorDispatcher] --> Anchor[Anchor 32B state_root<br/>+ chain_id + height]
        Server[L1AnchorReader] --> Verify[verify state_root parity]
    end
    subgraph LTP[LTP attestation pipeline - paper §10]
        Witness[Super-node BFT attestation quorum]
        Reg[LTPAnchorRegistry.sol]
        Witness -- 7-of-9 BLS aggregate --> Reg
    end
    Anchor -- submit anchor --> Witness
    Reg -- eth_call --> Verify
```

GSX-DB's anchor is a small input (`Anchor` ≈ 96 B) to the LTP
attestation pipeline. The LTP commitment on the base chain
(constant ~1,600 B per anchor) compresses regardless of payload
complexity.
