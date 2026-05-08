# Anchor log + cross-chain parity (S7)

## Goal

Per-block commitment to the canonical state, replicated to N external
chains, so that any anchored chain's local copy can be used to verify
what the chain claimed for any height.

Phase-1 ships in-memory + MAC per IQ-7. Real Solidity
`LTPAnchorRegistry` + ECDSA signatures is a launch-readiness item.

## Types and invariants

### Anchor

```rust
pub struct Anchor {
    pub chain_id: ChainId,
    pub height: u64,
    pub state_root: Commitment,
    pub parent: AnchorHash,
    pub mac: [u8; 32],
}
```

Per-chain commitment to `(state_root, height)` plus a back-pointer to
the previous anchor on the same chain. The MAC binds these fields
under the chain's authenticator key.

`hash()` returns the BLAKE3 of the anchor's canonical encoding,
including the MAC. Used as the `parent` field of the next anchor.

### AnchorLog

```rust
pub struct AnchorLog { chain_id: ChainId, entries: Vec<Anchor> }
```

Append-only per-chain log. `append` validates:

1. Chain id matches the log's chain id
2. Parent hash matches the previous anchor's hash (or `GENESIS_PARENT`)
3. Height is `prev.height + 1` (or 0)
4. MAC verifies under the provided key

Any failure aborts the append; the log is unchanged.

### AnchorDispatcher

```rust
pub struct AnchorDispatcher {
    logs: BTreeMap<ChainId, AnchorLog>,
    keys: BTreeMap<ChainId, [u8; 32]>,
}
```

Multi-chain writer. `dispatch(height, state_root)` builds one anchor
per registered chain (each with its own parent + key) and appends to
each chain's log.

`parity_check(height)` reads every chain's anchor at `height` and
returns:

- `Agreed { state_root }` if every chain has an anchor at that height,
  every MAC verifies, and every anchor's `state_root` matches.
- `Disagreed { divergent, missing }` otherwise.

### Cross-chain parity invariant

For any sequence of `(height, state_root)` dispatches via the
dispatcher, with no tampering:

```
for every height h:
    parity_check(h) == Agreed { state_root: <whatever was dispatched> }
```

Tampering with any chain's log is detected:

```
forge(chain_c, height_h, _) ⇒ parity_check(h) == Disagreed { ... }
```

## Storage layout

In-memory `Vec<Anchor>` per chain. Persistent storage is **not in S7
scope** — would arrive with S8.5 (when `RedbBlockStore` lands) or
launch readiness, whichever comes first.

## Failure model

- **MAC failure** at append time: `AppendError::BadMac`, log unchanged.
- **MAC failure** at parity-check time: chain marked tampered, parity
  returns `Disagreed`.
- **Height gap**: `AppendError::HeightGap`. Logs are dense; gaps are a
  contract violation by the dispatcher.
- **Parent mismatch**: `AppendError::ParentMismatch`. Detected at
  append; the dispatcher uses `latest()` so this only fires under
  external misuse.

## Tests

### Exit gate

```text
PROPTEST_CASES=10000 cargo test --test cross_parity cross_chain_parity_holds
```

10,000 cases of randomly generated `(height, state_root)` sequences
across 3 chains. Asserts every height returns `Agreed`. Run via
`./scripts/cross-parity.sh`.

### Sub-properties

- `dispatched_anchors_appear_on_all_chains`
- `parity_detects_tampering`
- `anchor_chain_is_linked` — every anchor's parent matches the previous
  anchor's hash on the same chain

### Inline unit tests

- 8 anchor type tests (MAC round-trip, key sensitivity, tamper
  detection, hash determinism, distinctness)
- 8 anchor-log tests (append, validation, error paths)
- 7 dispatcher tests (registration, dispatch, parity, tamper)

## Real-deploy swap surface (per IQ-7)

The trait/struct surface stays. What changes:

- `compute_mac` → `sign_ecdsa(privkey, encoding)`
- `verify_mac` → `verify_ecdsa(pubkey, encoding, sig)`
- `AnchorLog::append` storage → Solidity contract write via RPC
- `AnchorLog::at` storage → Solidity contract read via `eth_call`

The property-test invariants are dialect-independent and stay green
under the swap.

## Open questions

- **Cross-chain time/height mapping.** Phase-1 uses one logical height;
  real chains have different block times. IQ-7 follow-up.
- **Slashing for divergent anchors.** Detection is here; punishment
  needs validator-set semantics. Launch readiness.
- **Anchor persistence.** Phase-1 in-memory only. S8.5 / launch.
