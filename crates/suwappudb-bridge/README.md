# suwappudb-bridge

[![License: Apache-2.0](https://img.shields.io/badge/License-Apache_2.0-blue.svg)](../../LICENSE)

**The only writer to `suwappudb-state`.** Validates intents, runs OCC
parallel execution, dispatches anchors to the cross-chain registry.

> Downstream code should depend on **[`suwappudb-types`](../suwappudb-types)**,
> not on this crate directly. `suwappudb-bridge` is internal — its public
> APIs may evolve in any pre-1.0 minor bump.

## What it owns

- **Intent validation** (`Bridge::submit`) — the type-system gate
  that mints a `BridgeToken` and applies it to `suwappudb-state`.
- **CE-MVCC OCC executor** (`occ::block_executor`) — Aptos
  Block-STM-style parallel execution with sequential validation +
  clear-and-retry. `parallel_equals_sequential` proptest at 10k
  cases.
- **Cross-VM intent bundles** (`bundle::`) — atomic save-and-restore
  multi-step transactions with `Intent::Call` dispatch.
- **Anchor pipeline** (`anchor::`) — per-chain dispatcher,
  append-only log, four `AuthScheme` variants (Blake3Mac / ECDSA /
  Hybrid / Sp1), unified `verify_credential` dispatch.
- **ECDSA signer** (`anchor::signing`) — `EcdsaSecp256k1Signer`
  produces EIP-191-prefixed signatures that Solidity
  `LTPAnchorRegistry.recoverSigner` consumes byte-for-byte.
- **Recovery** (`recovery::`) — block store + deterministic replay,
  redb-backed in production.
- **Sync** (`sync::l2`) — pulls EVM state from op-reth for shadow
  cross-validation.

## Feature flags

| Flag | Pulls in | Default |
|---|---|---|
| `production-pqc` | `pqcrypto-mldsa` for ML-DSA-65 hybrid verifier | off |
| `production-move-executor` | Propagates the same flag to `suwappudb-state` | off |

## Tests

```sh
cargo test -p suwappudb-bridge                                      # 164 unit/integration
cargo test --test cross_parity                                  # cross-chain parity
cargo test --features production-move-executor --test aptos_move_vm_parity   # full Move VM
PROPTEST_CASES=10000 cargo test -p suwappudb-bridge --release       # exit-gate
```

Cross-impl differential test (Rust ECDSA → Solidity recoverSigner):

```sh
cargo run --example gen_parity_vectors --release -- contracts/test/fixtures/parity_vectors.json
forge test --root contracts --match-contract LTPAnchorRegistryParityTest
```

## Specs

- [`docs/spec/anchor-log.md`](../../docs/spec/anchor-log.md)
- [`docs/spec/ce-mvcc-occ.md`](../../docs/spec/ce-mvcc-occ.md)
- [`docs/spec/cross-vm-intent-queue.md`](../../docs/spec/cross-vm-intent-queue.md)
- [`docs/spec/move-execution.md`](../../docs/spec/move-execution.md)
- [`docs/spec/recovery.md`](../../docs/spec/recovery.md)
