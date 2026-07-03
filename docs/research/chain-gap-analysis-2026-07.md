# Competitive landscape + gap analysis — Tempo, Arc, Robinhood Chain

**Date:** 2026-07-03
**For:** the go-public decision. What the purpose-built chains that
launched over the last year actually ship, where the Suwappu stack is
behind, where it is ahead, and a prioritized close-the-gap backlog.
**Honesty rule:** same as [ECOSYSTEM-AUDIT.md](../ECOSYSTEM-AUDIT.md) —
this document is honest, not optimistic. Facts below were researched
2026-07-03 from primary sources (official docs, repos, benchmark
dashboards); anything unverifiable is marked UNCONFIRMED. Full source
list at the bottom.

**Scoping note.** suwappu-db is a *substrate*, not a chain. The
comparable unit is the three-repo Suwappu stack (`suwappu-dag`
consensus + `suwappu-db` state/execution + `suwappu-lattice-protocol`
attestation). Each gap below names the repo that owns it; items owned
by sibling repos are listed so the stack-level picture is complete,
but only suwappu-db items enter this repo's backlog.

---

## 1. The three chains in one paragraph each

**Tempo** (tempo.xyz — Stripe + Paradigm; CEO Matt Huang) is a
payments L1: Reth-SDK execution (EVM, Osaka hardfork) under Simplex
BFT consensus (Commonware), ~0.5 s blocks with deterministic
finality. Mainnet live 2026-03-18. No native token — gas is paid in
whitelisted USD stablecoins through an enshrined fee AMM. Protocol
level payments features: TIP-20 (ERC-20 + ISO 20022-aligned memos +
pause/freeze/allow/blocklists), dedicated payment lanes with a
separate gas limit, 2D nonces, an enshrined stablecoin DEX, and MPP
("OAuth for money") for agent payments. Permissioned validators
(Visa, Stripe, Zodia Custody first externals). Fully open source
(Rust, dual Apache-2.0/MIT, ~2,700 commits) with a **public nightly
benchmark dashboard** (perf.tempo.xyz): 20,380 TPS median settled on
the TIP-20 transfer scenario, 503 ms average block time, against a
100k+ TPS marketing claim.

**Arc** (arc.io / arc.network — Circle) is a stablecoin-finance L1:
Malachite BFT (Rust Tendermint, Circle-maintained) over a Reth-based
EL, fixed 500 ms block cadence, sub-second deterministic finality, no
reorgs by construction. Testnet-only as of July 2026 (mainnet
targeted summer 2026; ~244M testnet txs; 100+ institutional
participants). USDC is native gas (EIP-1559 + EWMA smoothing,
~$0.01/tx target). Circle StableFX gives 24/7 RFQ FX with atomic PvP
settlement; "Arc Privacy Sector" (TEE-based confidential transfers
with regulator view keys) is whitepapered but not live. Permissioned
PoA of regulated institutions with a PoA→permissioned-PoS roadmap
(ARC token: $222M presale at $3B FDV). Node software open source
(Apache-2.0, github.com/circlefin/arc-node). New nodes bootstrap from
compressed snapshots, not genesis replay.

**Robinhood Chain** (docs.robinhood.com/chain) is an RWA L2:
Arbitrum Orbit/Nitro settling to Ethereum with EIP-4844 blob DA (full
rollup, not AnyTrust), EVM + first-class ERC-4337, ETH gas, chain ID
4663. Mainnet launched 2026-07-01. Centralized sequencer
(reported AWS-hosted, Offchain-Labs-operated infra) with
first-come-first-served ordering and ~100 ms block latency claims.
The product is tokenized stocks: ERC-20 "Stock Tokens" issued as
Jersey-law debt securities (economic exposure only), minted/redeemed
by KYB'd authorized participants, freely composable on-chain
(Uniswap + proprietary Pleiades AMM), corporate actions via an
on-chain shares-per-token multiplier, geo-blocked at the distribution
layer (no US/UK/CA). Permissionless to deploy on; standard Ethereum
tooling.

## 2. Comparison matrix

"Suwappu" column = the three-repo stack; **(db)** marks what this
repo owns. Competitor cells are their *shipped* state, not roadmap,
except where marked.

| Dimension | Tempo | Arc | Robinhood Chain | Suwappu stack today |
|---|---|---|---|---|
| Status | Mainnet (Mar 2026) | Testnet (mainnet summer 2026) | Mainnet (Jul 2026) | Pre-mainnet; v0.1.0-pre substrate + live Besu/OP-Stack rails not yet powered by suwappu-db |
| Consensus | Simplex BFT (Commonware) | Malachite (Tendermint, Rust) | Single sequencer → Ethereum settlement | Mysticeti-C DAG (`suwappu-dag`; integration not started) |
| Finality | Deterministic, ~0.5 s | Deterministic, <1 s, 500 ms cadence | Soft ~100 ms; hard = Ethereum | Deterministic by design; **no measured number** |
| VM(s) | EVM only | EVM only | EVM only | **EVM + Move, parity-proven at 10k cases (db)** |
| Parallel execution | Reth pipeline | Reth | Nitro | **Block-STM-style CE-MVCC OCC (db)** |
| State commitment | MPT (Reth heritage) | Merkle root per block (Reth) | Nitro MPT | **Verkle: banderwagon + IPA witnesses (db)** |
| Published perf numbers | **Nightly public dashboard: 20.4k TPS median** | 3,000+ TPS @ 20 validators, <350 ms benchmarked | ~100 ms blocks (marketing) | **None** |
| Fee model | Any whitelisted stablecoin + enshrined fee AMM; <$0.001 fixed TIP-20 transfers | Native USDC gas, EWMA, ~$0.01 target | ETH | Undefined (dag-owned) |
| Payments features | TIP-20 memos (ISO 20022), payment lanes, 2D nonces, enshrined DEX, MPP | StableFX PvP, partner stablecoins, EIP-7708 transfer logs | n/a (RWA focus) | None at intent level (db seam exists) |
| Compliance hooks | Protocol-level pause/freeze/allow/blocklist, Travel Rule | Runtime USDC blocklist, view keys (planned), screened validators | Issuer-layer geo-blocking, KYT | None (deliberate decision not yet taken) |
| Privacy | Advertised opt-in confidential transfers (live status UNCONFIRMED) | TEE-based confidential transfers whitepapered, not live | None | None |
| Post-quantum | None announced | Optional PQ wallet sigs at mainnet (roadmap) | None | **ML-DSA-65 hybrid verifier behind `production-pqc` (db)** |
| Cross-chain | Across/Relay/Bungee/Squid + CCIP; Bridge (Stripe) fiat rails | CCTP + Gateway (canonical USDC) | Canonical rollup bridge + LayerZero OFT + CCIP | **LTP anchors: state-root parity across chains, Rust↔Solidity differential-tested (db)** — different primitive: attestation, not asset transfer |
| Node bootstrap | **Official daily snapshots + `tempo download`** | **Snapshot-first bootstrap (~68 GB EL)** | Standard Nitro node from published configs | File-based `StateSnapshot` primitive; **no distribution/tooling** |
| Open source | Yes — Apache-2.0/MIT, active, 84 releases | Yes — Apache-2.0, "testnet alpha" | Nitro is OSS; RH-specific mods UNCONFIRMED | Apache-2.0 (db, dag); **lattice is Elastic 2.0**; **CI workflows present but not running (Actions billing off)** |
| SDKs | tempo-viem (TS), tempo-alloy (Rust), Go, Foundry std | App Kit, thirdweb/QuickNode/Graph/Blockscout | Standard EVM + Alchemy/QuickNode | Rust crate (`suwappudb-types`) + JSON-RPC schema; **no TS SDK** |
| Explorer/indexer | explore.tempo.xyz + hosted SQL indexer (TIDX) | Blockscout | Blockscout | None |
| Validator model | Permissioned → PoS roadmap | Permissioned PoA → PoS roadmap | Centralized sequencer | Permissioned rings (paper §5); comparable posture |

## 3. What the research changes about our story

Three observations that should shape the public positioning:

1. **Nobody else does what the anchor pipeline does.** All three
   competitors move *assets* across chains (CCTP, OFT, canonical
   bridges). None attests *state parity* across heterogeneous
   verifiers the way `LTPAnchorRegistry` + `AnchorDispatcher::parity_check`
   do. That, dual-VM parity, and Verkle witnesses are the
   differentiators; the README should say so explicitly against this
   landscape instead of assuming the reader infers it.

2. **Permissioned-validator launches are now the industry norm, not a
   weakness.** Tempo (Visa/Stripe/Zodia), Arc (regulated-institution
   PoA), and Robinhood (single sequencer) all launched permissioned
   with a decentralization roadmap. The validator-rings posture in the
   paper needs no apology — but it does need the same
   "here-is-the-roadmap-to-opening-the-set" framing the others publish.

3. **The table stakes for "public" are performance evidence, node
   bootstrap tooling, and a runnable network — not more invariants.**
   We are unusually strong on verification (10k-case exit gates,
   differential tests, SBOM) and unusually weak on the three things
   every competitor leads with: a number, a snapshot, an endpoint.

## 4. Gap analysis → close-the-gap backlog

Grouped P0/P1/P2 by "what going public requires." Each item names an
owner repo. suwappu-db items should become IQs/issues before
implementation per convention.

### P0 — before the repo is public (days, not weeks)

| # | Gap | Evidence from landscape | Action | Owner |
|---|---|---|---|---|
| G1 | **CI badges reference workflows that do not run** (Actions billing disabled). A public repo whose top-of-README badges are dead reads as abandonware next to tempoxyz/tempo's green 84-release wall. | All three competitors' public repos run CI | Enable Actions billing (or move CI to a free runner tier) before flipping visibility; verify `ci.yml`, `security.yml`, `sbom.yml`, `scorecard.yml` actually pass; drop any badge that can't be backed by a real run (README already does this for Scorecard — extend the same honesty to CI/Security badges) | org / db |
| G2 | **Zero published performance numbers.** Tempo publishes a nightly public benchmark dashboard; Arc publishes TPS-vs-validator-count benchmarks; even Robinhood leads with a latency number. We publish proptest case counts, which prove correctness, not speed. | perf.tempo.xyz; Arc "3,000+ TPS @ 20 validators" | Add a reproducible `cargo bench`/criterion harness over `BlockExecutor` (OCC batch throughput, state-tree commit latency, anchor dispatch latency) + a `BENCHMARKS.md` with honest numbers on named hardware. The existing `dag_snapshot_exit_gate` 10k-in-1.12s figure shows we already collect timings — formalize it. **Landed 2026-07-03** (`BENCHMARKS.md` + criterion benches in bridge/state); production-feature bench matrix is the follow-on. | **db** |
| G3 | **No landscape positioning in README.** Reader can't tell why this exists when Tempo/Arc/Reth exist. | All three lead with "purpose-built for X" | One README section: what we are relative to payments L1s / stablecoin L1s / RWA L2s — dual-VM parity, Verkle witnesses, anchor attestation, PQ-ready are the four differentiators. Link this doc. | **db** |
| G4 | **Elastic-2.0 lattice license needs louder placement.** Competitors are uniformly Apache/MIT; our three-repo stack has a non-commercial-redistribution clause in one corner. Already documented in INTEGRATORS.md — but a public reader hits README first. | Tempo dual Apache/MIT; Arc Apache-2.0 | Keep the current README license paragraph (it's good), add the same call-out to docs/README.md; no license change proposed here — that's a business decision. | db / org |

### P1 — fast follow (Phase D window, Q3 2026)

| # | Gap | Evidence | Action | Owner |
|---|---|---|---|---|
| G5 | **No node-bootstrap/snapshot distribution story.** `StateSnapshot` capture/restore exists and is byte-idempotent, but an external operator has no `tempo download`-equivalent, no published snapshot cadence. | Tempo daily snapshots + downloader; Arc snapshot-first bootstrap (genesis replay explicitly avoided) | Ship a `suwappudb snapshot export/import` CLI on the existing SnapshotManager + document an operator bootstrap flow (snapshot restore → replay tail from RedbBlockStore). Recovery spec already guarantees deterministic replay — this is packaging, not research. **CLI + flow landed 2026-07-03** (`suwappudb-snapshot`, `docs/architecture/node-bootstrap.md`); published-cadence distribution waits for a public network. | **db** |
| G6 | **No TypeScript SDK.** Every competitor has first-class TS tooling (tempo-viem, App Kit, standard viem). Wallet/indexer devs are TS-first; our front door is a Rust crate + raw JSON-RPC schema. | tempo-viem, tempo-alloy split mirrors exactly our suwappudb-types situation | Generate a thin typed TS client from `docs/api/rpc-schema.json` (openrpc/quicktype-style), publish as `@suwappu/db-client`; keep it schema-derived so it can't drift. | **db** |
| G7 | **No transfer memo field.** ISO 20022-aligned memos are protocol-level in TIP-20 and table stakes for any payments corridor use-case (LTP's home turf). | Tempo TIP-20 memos + Travel Rule | IQ: add optional bounded memo bytes to `Intent::Transfer` + anchor payload implications (memo must NOT enter the frozen EIP-191 payload without an IQ — that surface is frozen). Cheap at intent level, expensive if retrofitted after the surface freezes at 1.0. | **db** (IQ first) |
| G8 | **Fee/gas abstraction is undefined at the intent layer.** Both payments L1s made "no native token / stablecoin gas" a headline. Whether Suwappu wants stablecoin gas is a dag/tokenomics call, but the substrate should not *preclude* it. | Tempo enshrined fee AMM; Arc native-USDC gas | IQ: verify `Intent` + OCC + bundle surfaces are fee-token-agnostic (they appear to be — fees are currently absent entirely); document the seam where a fee lane would attach so dag can build on it in v0.2.0's extended-bridge work. | dag + **db** (seam doc) |
| G9 | **Explorer/indexer story absent.** Arc and Robinhood both launched on Blockscout day one; Tempo built a hosted SQL indexer. | Blockscout on 2/3 chains | Decide: minimal eth-JSON-RPC compatibility shim in suwappudb-server so stock Blockscout can index, vs. documented "bring your own indexer via suwappu_* methods." Scope it in the same IQ as G6. | **db** |

### P2 — post-GA / deliberate decisions (Phase E+)

| # | Gap | Evidence | Action | Owner |
|---|---|---|---|---|
| G10 | **Confidential transfers with auditability.** Both stablecoin L1s advertise it (neither has shipped it — Arc whitepapered TEE+view-keys, Tempo advertised, UNCONFIRMED live). This is where the market is going, and nobody has landed it yet. | Arc Privacy Sector whitepaper; Tempo design goal | IQ post-GA: view-key selective disclosure over the Verkle tree. Note our Verkle commitments are a *better* substrate for this than their MPTs (witness-friendly). Do not rush; being third-to-ship-but-audited beats first-to-whitepaper. | **db** + dag |
| G11 | **Protocol-level compliance hooks** (pause/freeze/blocklist). Tempo enshrined them; Arc enforces the USDC blocklist at EVM runtime. Adopting them changes the neutrality posture of the whole stack. | TIP-20; Arc runtime blocklist | IQ with explicit governance sign-off either way. The wrong outcome is drifting into them piecemeal. | dag (policy), db (mechanism) |
| G12 | **State-growth pricing.** Tempo charges storage creation outside the gas limit (TIP-1000/1016 reservoir model). We have no state-rent/pricing concept. | Tempo TIPs 1000/1016 | Track; becomes real only at sustained public load. | dag |
| G13 | **Compact multipoint Verkle witnesses** (~200 B vs today's ~12.5 KB). Already on the roadmap (Phase E, IQ-6); the landscape makes it more valuable: it is the one performance number where we can beat every MPT chain rather than chase them. | No competitor has Verkle at all | Keep Phase E as planned; when it lands, publish witness-size benchmarks alongside G2's throughput numbers. | **db** (scheduled) |

### Non-goals — gaps we deliberately do not close

- **Chasing headline TPS.** Tempo's own dashboard shows 20k real vs
  100k claimed. Publish honest measured numbers (G2) and the
  witness-size story (G13); do not enter the marketing-TPS race.
- **Native stablecoin gas in the substrate.** That is chain policy
  (dag). The substrate's job is to not preclude it (G8).
- **Dropping Move to simplify.** All three competitors are EVM-only;
  dual-VM parity is a differentiator precisely because it is hard and
  proven, not despite it.
- **An enshrined DEX/AMM.** Application-layer on our stack; Tempo's
  enshrined orderbook serves their single-purpose thesis, not ours.

## 5. Suggested sequencing

1. **This week (pre-public):** G1 (CI truthfulness), G3 (README
   positioning), G4 (license call-out). All docs/infra, no code risk.
2. **Next two sprints:** G2 (bench harness + BENCHMARKS.md) and G5
   (snapshot CLI) — both package existing, tested machinery.
3. **Phase D alongside v0.2.0:** G6 (TS client), G7 (memo IQ), G8
   (fee-seam doc), G9 (indexer decision).
4. **Phase E and beyond:** G13 as scheduled; G10–G12 as IQs when the
   dag-side policy questions have owners.

## 6. Sources

Primary sources reviewed 2026-07-03. Competitor facts not in these
sources are marked UNCONFIRMED above.

**Tempo:** tempo.xyz/blog/{mainnet,testnet,mpp-sessions,tip20};
docs.tempo.xyz (performance, TIPs, developer-tools, node
system-requirements); perf.tempo.xyz (nightly benchmark, run of
2026-07-03); github.com/tempoxyz/{tempo,mpp-specs};
snapshots.tempoxyz.dev; CoinDesk 2026-03-18; The Defiant + Ledger
Insights (validators, 2026-04-14); The Block (RedStone); Sentora
technical review.

**Arc:** docs.arc.io (system-overview, gas-and-fees,
evm-differences, node-requirements, running-a-node);
github.com/circlefin/{arc-node,malachite}; circle.com blog
(Arc intro, StableFX + partner stablecoins, CCTP/Gateway);
arc.io/blog (testnet reliability, deterministic finality); Arc
whitepaper + privacy whitepaper; CoinDesk 2026-04-06 (PQ); The Block
(StableFX); token-sale coverage (Yahoo Finance, Unchained).

**Robinhood Chain:** docs.robinhood.com/chain (about, connecting,
stock-tokens, bridging, run-a-full-node); robinhood.com newsroom
(testnet 2026-02-10, mainnet 2026-07-01); blog.arbitrum.io
(testnet + mainnet posts); The Block, CoinDesk, Unchained,
QuickNode (mainnet coverage); status.robinhoodchain.offchain.io;
eco.com support (product mechanics).
