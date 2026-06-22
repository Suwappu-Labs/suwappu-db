# Integrating with suwappu-db

> **Coming from the [README](./README.md)?** You're in the right
> place. This is the integrator front door — wallet, indexer,
> custodial relayer, parallel verifier, or auditor — whoever is
> wiring suwappu-db into a downstream stack.

It pairs with the [README](./README.md) (orientation),
[CHANGELOG](./CHANGELOG.md) (per-release deltas),
[ARCHITECTURE](./ARCHITECTURE.md) (one-page system overview), and
[docs/spec/](./docs/spec/) (deep specs per subsystem).

If you're contributing to suwappu-db itself, see [CONTRIBUTING.md](./CONTRIBUTING.md).

---

## Who this is for

You're integrating **with** suwappu-db (not contributing **to** it) if any
of these match:

- Building a wallet, indexer, block-explorer, or chain-listener
  against the Suwappu DAG.
- Wiring an upstream service (custody, settlement, RWA tokenisation)
  to read state from a suwappudb node or write intents into one.
- Running a parallel verifier implementation that needs the same
  parity guarantees as the canonical Rust path.
- Auditing pre-launch — you want to see the load-bearing invariants
  and where they're enforced.

## Stability promises

| Surface | Stability |
|---|---|
| `Anchor` struct (chainId, height, stateRoot, parent, mac) | **Frozen.** Byte-for-byte stable with Solidity `LTPAnchorRegistry.Anchor`. |
| `AuthScheme` discriminants (0=Blake3Mac, 1=Sp1ZkProof, 2=EcdsaSecp256k1, 3=MlDsa65Hybrid) | **Frozen.** New variants are additive only. |
| ECDSA EIP-191 payload (`eth_signed_message_hash`) | **Frozen.** Cross-impl proptest pins 16 vectors. |
| `LTPAnchorRegistry` Solidity ABI | **Frozen.** Published as `contracts/abi/*.abi.json` per release. |
| Workspace Rust crate names (`suwappudb-state`, `suwappudb-bridge`, `suwappudb-server`) | **Stable.** Public types may shift before `v1.0.0`. |
| JSON-RPC method names (`suwappu_getBalance`, `suwappu_getStateRoot`, `suwappu_getParity`) | **Frozen.** Future versions add methods, never repurpose. |
| Internal traits (`MoveExecutor`, `CommitmentScheme`, `BlockStore`) | **Volatile.** Don't depend; use the front-door types. |
| Anything under `#[doc(hidden)]` or `__for_tests` | **Volatile.** Test helpers; will change. |

Pre-`v1.0.0` (which gates on the suwappu-types crate freeze, see C5),
minor bumps **may include breaking changes**. We will call them out
in CHANGELOG. From `v1.0.0` onward, strict SemVer.

## Front-door surfaces

### Rust (depend on this crate via git pin)

```toml
[dependencies]
suwappudb-bridge = { git = "https://github.com/Suwappu-Labs/suwappu-db", tag = "v0.1.0-pre" }
suwappudb-state  = { git = "https://github.com/Suwappu-Labs/suwappu-db", tag = "v0.1.0-pre" }
```

Frozen public types (re-exported from `suwappudb_bridge::anchor` and
`suwappudb_state`):

```rust
// Anchor surface (S11)
suwappudb_bridge::anchor::{Anchor, AnchorHash, AnchorEntry, AuthScheme, ChainId, GENESIS_PARENT}
suwappudb_bridge::anchor::{AnchorAuthCredential, ExpectedVerifier, VerifierConfig}
suwappudb_bridge::anchor::{AnchorDispatcher, AnchorLog, ParityResult}
suwappudb_bridge::anchor::{AnchorSigner, EcdsaSecp256k1Signer, SignerError}
suwappudb_bridge::anchor::{verify_credential, verify_ecdsa, eth_signed_message_hash, EthAddress}

// State surface (S2 / S6 / S12)
suwappudb_state::{Address, Balance, BalanceSlot, BridgeToken, Commitment, State, StateChange}
suwappudb_state::{StateTree, Proof, ProofStep, Node}
suwappudb_state::snapshot::{StateSnapshot, SnapshotManager}
suwappudb_state::dag::{DagBlock, DagStore, BlockHash}

// Metrics surface (S12.3)
suwappudb_state::Metrics  // .to_prometheus_text()
```

### JSON-RPC (external integrators)

`suwappudb-server` binds `0.0.0.0:8660` and serves:

| Method | Params | Returns |
|---|---|---|
| `suwappu_getBalance` | `[address: 0x... or hex 40 chars]` | `{address, balance}` |
| `suwappu_getCoinValue` | `[address]` | `{address, coin_value}` (Move-side projection) |
| `suwappu_getStateRoot` | `[]` | `{state_root}` (32-byte hex) |
| `suwappu_getParity` | `[height: u64]` | `{parity: agreed|disagreed, state_root, height, ...}` |
| `suwappu_getBlock` | `[height: u64]` | placeholder (not yet wired) |
| `suwappu_submitIntent` | tbd | placeholder (not yet wired) |

`GET /health` returns `{status: "ok"}`. `GET /metrics` returns a
Prometheus exposition-format text body (S12.3 quantile summaries).

Auth posture: `/health` and `/metrics` are unauthenticated. `/rpc`
gates behind `Authorization: Bearer <token>` when
`Suwappudb_BEARER_TOKEN` is set (B6 / `docs/architecture/deployment-topology.md`).

### Solidity (downstream contracts)

```solidity
import {ILTPAnchorRegistry} from "@suwappudb/ILTPAnchorRegistry.sol";

contract YourBridgeReceiver {
    ILTPAnchorRegistry public registry;
    constructor(address _registry) {
        registry = ILTPAnchorRegistry(_registry);
    }
    function check(uint32 chainId, uint64 height) external view returns (bool ok) {
        ILTPAnchorRegistry.Anchor memory a = registry.getLastAnchor(chainId);
        return a.height >= height;
    }
}
```

The `ILTPAnchorRegistry` interface ships in `contracts/src/`; the
runtime ABIs ship in `contracts/abi/*.abi.json` (committed) plus each
GitHub Release bundles them as artefacts.

## Worked example: read balance + verify an anchor

```sh
# 1. Clone + pin to a tag
git clone https://github.com/Suwappu-Labs/suwappu-db
cd suwappu-db
git checkout v0.1.0-pre

# 2. Run a local server (in-memory state by default)
cargo run --release --bin suwappudb-server

# 3. Read state via JSON-RPC
curl -sS -d '{"jsonrpc":"2.0","method":"suwappu_getStateRoot","params":[],"id":1}' \
    http://localhost:8660/rpc

# 4. Read an address balance
curl -sS -d '{"jsonrpc":"2.0","method":"suwappu_getBalance","params":["0x1111111111111111111111111111111111111111"],"id":2}' \
    http://localhost:8660/rpc

# 5. Query Prometheus metrics
curl -sS http://localhost:8660/metrics | head -20
```

### Same balance via Rust:

```rust
use suwappudb_state::{Address, State};

let state = State::default();
let addr = Address([0x11; 20]);
let balance = state.balance_of(&addr);
println!("balance = {}", balance.0);
```

### Verify an anchor signature off-chain:

```rust
use suwappudb_bridge::anchor::{
    Anchor, AuthScheme, ChainId, EthAddress, ExpectedVerifier,
    verify_credential, AnchorAuthCredential, GENESIS_PARENT,
};
use suwappudb_state::Commitment;

let anchor = Anchor::ecdsa(ChainId(7), 0, Commitment([0; 32]), GENESIS_PARENT);
let credential = AnchorAuthCredential::EcdsaSecp256k1 {
    signature: [0u8; 65], // 65-byte recoverable sig from your signer
};
let expected = ExpectedVerifier::EcdsaSecp256k1 {
    signer: EthAddress([0xAA; 20]), // address you expect
};
match verify_credential(&anchor, &credential, &expected) {
    Ok(()) => println!("accepted"),
    Err(e) => println!("rejected: {:?}", e),
}
```

## Cross-impl parity reference

The Solidity `LTPAnchorRegistry.recoverSigner` and the Rust
`verify_ecdsa` consume **byte-identical** EIP-191 payloads. The S11.5
differential test (`contracts/test/LTPAnchorRegistryParity.t.sol`)
ingests 16 Rust-signed vectors and asserts Solidity recovers each
vector to the embedded signer address. If you're building a parallel
implementation, regenerate the vectors via:

```sh
cargo run --example gen_parity_vectors --release -- \
    contracts/test/fixtures/parity_vectors.json
```

…and your implementation should pass the same fixture.

Three documented divergences between Rust and Solidity verifiers are
recorded in [`docs/audit/pass-b-2026-05-16.md`](./docs/audit/pass-b-2026-05-16.md#b7--anchor--credential--l1_reader-deep-review)
under B7 / D1–D3. None affects soundness; they bound what a parallel
implementation must mirror vs. what's intentionally divergent.

## Compile-time feature flags

| Feature | Crate | What it enables |
|---|---|---|
| `production-move-executor` | `suwappudb-state` | Real Aptos Move VM via `move-vm-runtime` (S9). Adds ~100 transitive deps from `aptos-core` git pin. Default off. |
| `production-verkle` | `suwappudb-state` | Real banderwagon-IPA Verkle commitments (S10). Adds `banderwagon` + `ipa-multipoint` git deps. Default off. |
| `production-pqc` | `suwappudb-bridge` | ML-DSA-65 PQ verifier (S11 hybrid). Adds `pqcrypto-mldsa` C shim. Default off. |

Default builds compile only the phase-1 substrate: BLAKE3 state-tree,
mock Move executor, ECDSA-only anchor verifier. Flip features on for
launch-readiness behaviour.

## Solidity ABI artefacts (C7)

The `LTPAnchorRegistry` Solidity ABI is published in two forms:

- **Committed in-repo** under [`contracts/abi/`](./contracts/abi/):
  - `LTPAnchorRegistry.abi.json` — full contract ABI (24 entries).
  - `ILTPAnchorRegistry.abi.json` — interface-only ABI (21
    entries). Use this if you only need to call the contract,
    not link against its storage layout.
- **Bundled in each GitHub Release** alongside the cargo binaries
  by `.github/workflows/release.yml`. The release artefact
  manifest includes SHA-256 checksums; the embedded `metadata`
  field also pins the compiler version (Solc 0.8.20).

Use cases:

- **TypeScript / ethers.js / viem:** `import abi from
  '@suwappudb/abi/LTPAnchorRegistry.abi.json'` (vendor the file from
  the release bundle).
- **Rust / ethers-rs:** `abigen!(...)` with the ABI path or the
  release URL.
- **Solidity downstream:** import
  [`contracts/src/ILTPAnchorRegistry.sol`](./contracts/src/ILTPAnchorRegistry.sol)
  directly.

The contract is not yet live on any chain. The deploy script lives
at `contracts/script/Deploy.s.sol`; expected mainnet address is TBD
and will be documented here at deploy time.

## Distribution — Docker (C8)

The release pipeline publishes a multi-arch container image
(`linux/amd64` + `linux/arm64`) to GitHub Container Registry on
every `v*` tag:

```sh
docker pull ghcr.io/globalsettlementnetwork/suwappu-db:v0.1.0-pre
```

Tagging rules (per
`.github/workflows/publish-image.yml`):

- Every `v*` tag publishes the literal tag (e.g. `v0.1.0-pre`)
  and the version-only form (`0.1.0-pre`).
- `latest` moves only for **non-pre-release** tags. `v0.1.0-pre`
  does not get `latest`; a future `v1.0.0` will.

### Minimal-config run

```sh
docker run --rm -p 8660:8660 -p 9660:9660 \
    -e RUST_LOG=info \
    ghcr.io/globalsettlementnetwork/suwappu-db:v0.1.0-pre
```

The container exposes:

- `8660` — JSON-RPC (`POST /v1/rpc`) + health/metrics
  (`GET /health`, `GET /metrics`).
- `9660` — reserved for the side-car metrics port; currently
  shares 8660 since metrics are served from the same router.

Health check is built-in (`curl /health` every 30s); orchestrators
that read the Docker HEALTHCHECK field don't need separate probes.

### Hardened production run

Layer the B6 bearer-token auth + the deployment-topology firewall
recommendations on top:

```sh
docker run --rm \
    -p 127.0.0.1:8660:8660 \                # bind localhost only
    -e Suwappudb_BEARER_TOKEN=$(openssl rand -hex 32) \  # B6 auth
    -e RUST_LOG=info \
    ghcr.io/globalsettlementnetwork/suwappu-db:v0.1.0-pre
```

…then expose 8660 to the outside world only through nginx /
Cloudflare Access / ALB authentication. See
[`docs/architecture/deployment-topology.md`](./docs/architecture/deployment-topology.md)
"RPC endpoint auth posture" for the full pattern.

### Building locally

```sh
docker build -t suwappudb-server:dev .
```

`Dockerfile` is multi-stage; the final image is `debian:bookworm-slim`
with the suwappudb-server binary + `ca-certificates` + `curl` for the
HEALTHCHECK probe. Total image ~80 MB.

## License

This repository is [Apache-2.0](./LICENSE).

### Cross-repo license posture

The Suwappu stack spans three repositories with non-uniform licenses:

| Repository | License | Notes |
|---|---|---|
| `suwappu-db` (this repo) | Apache-2.0 | Permissive; redistribute freely. |
| `suwappu-dag` (sibling) | Apache-2.0 | Same posture. |
| `suwappu-lattice-protocol` (sibling) | Elastic License 2.0 | Non-commercial-redistribution clause; if your product uses lattice-protocol, you must consult that repo's terms separately. |

The license mismatch with `suwappu-lattice-protocol` is intentional —
it covers the corridor super-node attestation surface, which has
licensing characteristics distinct from the substrate. Downstream
Rust consumers that depend on **all three** must accept Elastic 2.0's
constraints on the lattice-protocol portion.

## Reporting issues / proposing changes

- **Issue tracker:** https://github.com/Suwappu-Labs/suwappu-db/issues
- **Security disclosures:** see [SECURITY.md](./SECURITY.md) (private
  disclosure path; do not file public issues for vulnerabilities).
- **PRs welcome** under the `CONTRIBUTING.md` workflow (DCO sign-off
  required).
- **API change requests** should land as an issue first with a
  concrete use case; we prefer to discuss before code.

## Known gaps (Phase-1 → mainnet)

These are explicit pre-launch limitations; consult the linked audit
ledgers when planning a deployment.

- **Sp1 zkVM verifier** — `AuthScheme::Sp1ZkProof` is wire-shape
  only; the verifier rejects with `UnsupportedScheme`. Pending
  Track 1.3 toolchain decision (see IQ-7).
- **Compact multipoint IPA witnesses** — S10 ships per-step IPA
  witnesses (~12.5 KB per inclusion proof at depth 20). The ~200 B
  multipoint-aggregated witness is a follow-on optimization (see
  `docs/iq/IQ-6-verkle-commitment.md`).
- **Single-signer-per-chain (Rust) vs approved-set (Solidity)** —
  documented divergence D1 in
  [`docs/audit/pass-b-2026-05-16.md`](./docs/audit/pass-b-2026-05-16.md).
- **BLAKE3-keyed MAC (Rust) vs keccak256 stub (Solidity)** —
  documented divergence D2; ECDSA chains bypass the MAC.

## Reference index

| Doc | Use it for |
|---|---|
| [README.md](./README.md) | Project orientation |
| [CHANGELOG.md](./CHANGELOG.md) | Per-release deltas |
| [docs/spec/](./docs/spec/) | Subsystem specs (anchor, state-tree, OCC, recovery, etc.) |
| [docs/iq/](./docs/iq/) | Investigation Question records (decisions) |
| [docs/architecture/](./docs/architecture/) | Topology, deployment, request lifecycle |
| [docs/audit/](./docs/audit/) | Pass A / B / C verdict ledgers |
| [contracts/README.md](./contracts/README.md) | Solidity side: build, deploy, parity |
| [contracts/abi/](./contracts/abi/) | Committed ABI artefacts |

---

Build status, dependency graph, and the live shadow-testnet
endpoint (`18.226.17.168:8545`) are documented in
[`docs/architecture/deployment-topology.md`](./docs/architecture/deployment-topology.md).
