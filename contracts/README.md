# LTPAnchorRegistry — Solidity surface

This directory holds the Solidity side of [IQ-7's](../docs/iq/IQ-7-anchor-parity.md)
cross-chain anchor parity. `gsxdb-bridge` is the Rust counterpart; both
must accept/reject identical inputs.

## Contents

| Path | Purpose |
|---|---|
| `src/LTPAnchorRegistry.sol` | Full implementation. Anchor acceptance + MAC + signer set. |
| `src/ILTPAnchorRegistry.sol` | **S11.4** — external-integrator interface for downstream Solidity consumers. |
| `script/Deploy.s.sol` | **S11.4** — Foundry deploy script. Reads `PRIVATE_KEY`, optional `INITIAL_SIGNER` / `INITIAL_CHAIN_ID` / `INITIAL_CHAIN_KEY` from env. |
| `abi/LTPAnchorRegistry.abi.json` | **S11.4** — committed ABI for off-chain integrators. |
| `abi/ILTPAnchorRegistry.abi.json` | **S11.4** — interface-only ABI (subset). |

## Build

Foundry is required (https://book.getfoundry.sh).

```sh
forge install foundry-rs/forge-std    # one-time, into ./lib/ (gitignored)
forge build
```

The `out/` directory (also gitignored) holds full compiler output;
the canonical artifact for external consumers is `abi/*.abi.json`.

## Deploy

### Local anvil

```sh
anvil &
forge script script/Deploy.s.sol \
    --rpc-url http://127.0.0.1:8545 \
    --private-key 0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80 \
    --broadcast --legacy
```

### Sepolia testnet

```sh
export PRIVATE_KEY=0x…
export SEPOLIA_RPC_URL=https://sepolia.infura.io/v3/…
export ETHERSCAN_API_KEY=…
export INITIAL_SIGNER=0x…   # optional: the gsxdb-bridge ECDSA signer
forge script script/Deploy.s.sol \
    --rpc-url $SEPOLIA_RPC_URL \
    --private-key $PRIVATE_KEY \
    --broadcast --verify
```

The script prints the deployed address; pipe to a file in CI to feed
downstream gsxdb-bridge config.

## Parity model

- Rust `gsxdb_bridge::anchor::types::Anchor` ↔ Solidity `Anchor` struct
  — same five fields, same order, same widths.
- Rust `EcdsaSecp256k1Signer` ↔ Solidity `recoverSigner` — same EIP-191
  prefix on `keccak256(abi.encode(anchor))`.
- Rust `Anchor::hash()` ↔ Solidity `hashAnchor` — both use BLAKE3-tagged
  keccak / blake3 over the same five fields (chainId, height,
  stateRoot, parent, mac). `auth_scheme` is intentionally NOT in the
  digest — see `Anchor::hash` rustdoc.
- Rust `compute_mac` (BLAKE3-keyed) ≠ Solidity `computeMac` (keccak256
  placeholder). S11 follow-on: drop BLAKE3 from the Rust side, use
  keccak everywhere, OR add a BLAKE3 precompile shim on the
  Solidity side. Currently the parity test fixtures account for
  this difference.

The end-to-end producer flow is S11.3's `dispatch_with_signer`;
the consumer flow is `acceptAnchor` here.
