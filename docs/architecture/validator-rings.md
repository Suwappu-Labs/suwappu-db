# Dual-ring validator set

Per the GSX DAG L1 paper §5: the validator set is decomposed into
two concentric quorums with independent admission gates and
independent corruption profiles.

## The two rings

```mermaid
flowchart TB
    subgraph Outer[Validator Ring - PoS]
        direction LR
        V1[100-500 stake-weighted<br/>open participants]
        V2[Genesis stake: 25,000 GSX]
        V3[Slashing: 5-30% stake-weight]
    end
    subgraph Inner[Authority Ring - PoA]
        direction LR
        A1[30-50 licensed institutional<br/>entities]
        A2[Per-node stake: 100,000 GSX]
        A3[Slashing: 100% + expulsion]
    end
    subgraph SN[Corridor super-nodes - subset]
        direction LR
        SNi[Per jurisdiction]
        SNj[6 consolidated authorities]
    end

    Outer --> Inner --> SN

    style Outer fill:#cef
    style Inner fill:#fed
    style SN fill:#fcd
```

## What each ring does

```mermaid
flowchart LR
    subgraph Auth[Authority Ring]
        Cert[Produce certificates<br/>into the DAG]
        Sign[Sign compliance<br/>attestations]
        Fast[Fast-path quorum<br/>2/3+1]
    end
    subgraph Val[Validator Ring]
        Order[Ratify ordering on<br/>Mysticeti commits]
        Slash[Enforce slashing]
        Watch[Open monitoring<br/>+ challenge]
    end
    Cert --> Mysticeti[Mysticeti-C linearisation]
    Order --> Mysticeti
    Sign --> Compliance[Compliance attestations<br/>on-chain]
    Fast --> SingleOwner[Single-owner-object<br/>FastPay path]
    Slash --> Penalties[Validator-set penalties]
```

## Why two rings (paper §5.3)

A single-ring chain forces a choice between:

- **Closed validator set** — compliance trust at the cost of
  distributed economic security
- **Open validator set** — economic security at the cost of
  identifiable regulated counterparties

The dual-ring construction sidesteps this by separating the
functions across two quorums whose admission gates and corruption
profiles are independent.

## Safety theorem (paper §11, Theorem 2)

```mermaid
flowchart LR
    Attack[Conflicting commit attempt]
    QA{Authority-Ring<br/>quorum?}
    QV{Validator-Ring<br/>quorum?}
    Reject1[Rejected at<br/>certificate-DAG layer]
    Reject2[Rejected at<br/>linearisation layer]
    Success[Safety violated]

    Attack --> QA
    Attack --> QV
    QA -- &lt;1/3 corrupted --> Reject1
    QV -- &lt;1/3 stake corrupted --> Reject2
    QA -- &geq;1/3 corrupted AND --> AND[AND]
    QV -- &geq;1/3 stake corrupted --> AND
    AND --> Success
    style Success fill:#fcc
    style Reject1 fill:#cfc
    style Reject2 fill:#cfc
```

A safety violation requires Byzantine corruption of **both** ring
quorums **simultaneously**. The AND-gate is the structural property
institutional counterparties demand.

## Why the corruption events are independent (paper §11)

| Independence axis | Authority Ring | Validator Ring |
|---|---|---|
| Admission gate | regulatory licensure (KYC, jurisdiction) | open stake (any party with 25,000 GSX) |
| Operator population | licensed institutions | open market participants |
| Slashing incentive | per-Authority-Node 100% + expulsion | stake-weighted 5–30% |
| Geographic distribution | concentrated in regulated jurisdictions | unbounded (any IP) |

These independencies justify multiplying corruption probabilities
when bounding the joint attack probability.

## Super-node consolidated role (paper §9)

A super node is a **single permissioned legal entity** that holds,
for an assigned jurisdictional corridor, all six authorities
simultaneously:

```mermaid
flowchart TB
    SN[Super node<br/>per corridor]
    SN --> A1[1 — PoA Authority Node operator]
    SN --> A2[2 — LTP attestation witness]
    SN --> A3[3 — Issuer Studio registry authority]
    SN --> A4[4 — DID write authority]
    SN --> A5[5 — Reserve-attestation witness opt]
    SN --> A6[6 — Corridor emergency-response authority]

    style SN fill:#fcd
```

### Why consolidation

Aligns cryptographic, economic, and legal accountability surfaces —
the same legal entity that operates an Authority Node also signs
issuer-registry writes, issues DID credentials, and serves as LTP
attestation witness for its corridor. Eliminates the categorical
class of attacks that exploit divergence between bridge operators
and identity authorities.

### Concentration risk (paper §9)

A super-node compromise propagates across all six authority surfaces
simultaneously. Bounded by four mitigations:

1. **Quorum tolerance** — 7-of-9 LTP attestation quorum tolerates
   sub-quorum minority of compromised witnesses
2. **Dual-bond posture** — base-chain PoS stake + LTP corridor bonds
   sized to 5% of 90-day average attestation notional
3. **Recovery posture** — corridor-scoped suspension within 72 hours,
   fail-closed registered-issuer precompile, corridor reconstitution
   under fresh keys
4. **Governance staging** — from Phase G3 forward, super-node
   admission moves under Concord Council binding authority

## How gsx-db sees the validator set

```mermaid
flowchart LR
    subgraph Outside[Outside gsx-db]
        Mysticeti[Mysticeti-C consensus]
        VKey[Validator key registry<br/>maintained by Authority Ring]
    end
    subgraph GsxDB[gsx-db state surface]
        BridgeToken[BridgeToken capability]
        AnchorReg[AnchorDispatcher per ChainId]
        VSet[(Validator set state<br/>polymorphic balance map<br/>+ stake bonds)]
    end
    Mysticeti --> BridgeToken --> VSet
    VKey -.bootstrap.-> VSet
    AnchorReg -.signs anchors with.-> VKey
```

gsx-db's substrate doesn't enforce ring membership — that's the
consensus layer's job. The substrate holds the validator-set state
(stake bonds, slashing balances, attestation keys) in the same
polymorphic balance map as every other on-chain field. Ring-specific
behaviour (admission, slashing, ratification) sits in the consensus
crate (`gsxbft`).
