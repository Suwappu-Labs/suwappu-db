# Deployment topology

What runs where, today (phase-1 substrate live, chain not yet) and
at launch (full GSX DAG L1 per the academic paper).

## Today (phase-1 substrate + live experiments)

```mermaid
flowchart TB
    Devs[Developers + ops]
    GH[(GitHub<br/>GlobalSettlementNetwork)]
    Devs -->|push| GH

    subgraph AWS[AWS us-east-2]
        Besu[GSX Testnet<br/>4× Besu validator<br/>QBFT<br/>chain 103115120]
        OPStack[GSN L2<br/>op-reth + op-node<br/>chain 103218544<br/>RPC 18.226.17.168:8545]
        Besu -->|L1 settlement| OPStack
    end

    GH -.deploy.-> Besu
    GH -.deploy.-> OPStack

    subgraph Backend[Backend services]
        API[gsn-backend api - Go]
        ChainListen[chain-listener - Go]
        Wallet[wallet-service - Python]
        FB[fireblocks-service - Python]
    end

    OPStack <--RPC--> ChainListen
    API --> ChainListen
    API --> Wallet
    API --> FB

    subgraph Frontend[End users]
        CBDC[cbdc-studio]
        RWA[gsx-rwa-frontend]
        Identity[canton-gsxid]
    end

    CBDC -->|HTTPS| API
    RWA -->|HTTPS| API
    Identity -->|HTTPS| API

    GsxDB[gsx-db substrate<br/>not deployed yet]
    GsxDB -.shadow target.-> OPStack

    style GsxDB fill:#fed
    style Besu fill:#cfc
    style OPStack fill:#cfc
```

**Status:**
- Besu testnet + OP rollup are live; backend services deployed
- `gsx-db` is the candidate substrate but **not deployed against any
  live chain yet**

## Option A — Shadow deployment (1–2 weeks; pre-launch)

```mermaid
flowchart LR
    subgraph Live[Live GSN L2]
        OP[op-reth<br/>:8545]
    end
    subgraph Shadow[gsx-db shadow]
        Syncer[L2StateSyncer<br/>polls every N seconds]
        State[(gsxdb-state)]
        Server[gsxdb-server<br/>JSON-RPC]
        Tree[(StateTree)]
    end
    OP -- eth_getBalance --> Syncer
    OP -- eth_getTransactionCount --> Syncer
    Syncer --> State
    State --> Tree
    Server -- gsx_getBalance --> State
    Server -- gsx_getStateRoot --> Tree
    Auditor[Auditor / dashboard]
    Auditor --> Server
    Auditor --> OP
    Auditor -.compare roots.-> Tree
```

Cross-validation layer. gsx-db consumes published RPC state; any
divergence between gsx-db and op-reth surfaces in the auditor view.

## Target — full GSX DAG L1 (per the academic paper)

```mermaid
flowchart TB
    subgraph Ring1[Authority Ring - 30-50 PoA]
        A1[Authority Node 1]
        A2[Authority Node 2]
        Adots[...]
        AN[Authority Node N]
    end
    subgraph Ring2[Validator Ring - 100-500 PoS]
        V1[Validator 1]
        V2[Validator 2]
        Vdots[...]
        VN[Validator M]
    end
    subgraph SuperNodes[Corridor super nodes - subset of Authority Ring]
        SN1[Super node A<br/>US corridor]
        SN2[Super node B<br/>EU corridor]
        SN3[Super node C<br/>APAC corridor]
    end

    DAG[(Mysticeti-C certificate DAG)]
    A1 --> DAG
    A2 --> DAG
    AN --> DAG
    V1 -- ratify --> DAG
    V2 -- ratify --> DAG
    VN -- ratify --> DAG

    subgraph Node[Per-validator node]
        Consensus[gsxbft consensus]
        Exec[gsx-revm + gsx-db<br/>dual-VM execution]
        Anchor[LTP anchor pipeline]
    end
    DAG --> Consensus --> Exec --> Anchor

    Anchor --> SN1
    Anchor --> SN2
    Anchor --> SN3

    SN1 -- attest --> L1[External L1s<br/>Ethereum, BSC, etc]
    SN2 -- attest --> L1
    SN3 -- attest --> L1
```

Refer to the academic paper §4 (architecture) and §10 (LTP
integration) for the formal description.

## Network specifics (per paper §13 hardware envelope)

| Param | Value |
|---|---|
| Validators in benchmark | 100 (30 PoA + 70 PoS) |
| Network | 100 Gbps dual-port LACP per node |
| RAM | 512 GB per node |
| GPU | NVIDIA H100-80GB per node |
| Throughput (simple transfer) | 72,000 TPS |
| Throughput (full DEX-load) | 17,000 TPS |
| Throughput (extrapolated full PoA) | 114,000 TPS |
| Inter-validator transport | SCION (BGP-class attack-resistant) |
| Block propagation | RaptorQ erasure coding (RFC 6330) |

## What gsx-db owns in the target topology

```mermaid
flowchart LR
    Tx[Transactions<br/>EVM or Move shape] --> Consensus[Mysticeti-C<br/>certificate DAG]
    Consensus --> BlockBuilder[BlockBuilder trait]
    BlockBuilder --> Bridge[gsxdb-bridge<br/>OCC + bundles + anchor]
    Bridge --> State[(gsxdb-state<br/>canonical balance map<br/>+ Verkle tree)]
    Bridge --> Anchor[AnchorDispatcher]
    State --> Tree[(StateTree)]
    Anchor -.LTP attestation.-> SuperNode[Corridor super node]
```

gsx-db owns the boxed-out execution and state surfaces. The
consensus layer (above) is `gsxbft-consensus-only-demo` or the full
`gsx-bft` repository; the LTP attestation pipeline (below) is
documented in the LTP companion paper.
