# Insertion: LTP paper, new §7.4

**Where:** end of §7 (On-Chain Settlement), after §7.3 (Entity
state machine).

---

## 7.4 Rust integration surface

The Solidity registry of §7.1 anchors LTP commitments on-chain. The
authoritative LTP signer state (the materialize-side and the
attestation-side) lives off-chain, in a runtime that produces and
consumes anchor records and submits them to `LTPAnchorRegistry`. We
describe the Rust integration surface here for completeness; the
detailed substrate is the companion *Suwappu DB* implementation
[Suwappu DB, 2026].

### 7.4.1 Anchor production

For each block of the host chain, the Suwappu DB `AnchorDispatcher`
emits one `Anchor` per registered target chain:

```rust
pub struct Anchor {
    pub chain_id:   ChainId,
    pub height:     u64,
    pub state_root: Commitment,
    pub parent:     AnchorHash,
    pub mac:        [u8; 32],
}
```

Per-chain logs are append-only: `AnchorLog::append` validates chain
identity, parent-hash linkage, height monotonicity, and authentication
under that chain's key. The validation rules in this struct mirror
the five validation rules of §7.1 (replay rejection, signer
authorization, sequence monotonicity, temporal expiry, state-
transition validity) and are pre-positioned for the Solidity-Rust
parity discipline of §8.

### 7.4.2 Cross-chain parity verification

The parity check at any height returns one of three outcomes:

```rust
pub enum ParityResult {
    Agreed   { state_root: Commitment },
    Disagreed { divergent: Vec<(ChainId, Commitment)>,
                missing:   Vec<ChainId> },
}
```

`Agreed` requires every registered chain to have a valid anchor at
that height with matching `state_root` and verifying authentication.
`Disagreed` surfaces the divergent set with chain identifiers, which
the BFT attestation pipeline of §4 ingests as a slashable signal
under the corridor super-node accountability surface.

The parity-check semantics are verified at 10,000 cases in the
substrate test suite (`cross_chain_parity_holds`); the test
falsifies the negation, that an honest dispatch produces a parity
violation, across randomly generated state-root sequences over $n
\geq 3$ chains.

### 7.4.3 Solidity-Rust parity discipline

Per §7 and §8.2, all 36 entity-state-machine pairs are
parity-tested across the Python and Solidity implementations. The
Rust substrate inherits this discipline through the
`solidity_anchor_parity` integration test, which encodes the
Solidity contract's deterministic logic — Keccak256-MAC, encoding
order, transition matrix — as Rust fixtures and verifies that the
substrate's `Anchor::verify_mac` agrees with the Solidity contract's
expected output on each pair. Failing fixtures are caught at CI
time before they reach the registry.

### 7.4.4 Authentication primitive at launch

Per §9.1 (classical-cryptography exception zones), the launch path
swaps the substrate's BLAKE3 keyed-MAC primitive in
`Anchor::compute_mac` for a hybrid ECDSA-secp256k1 + ML-DSA-65
signature, aligning with the host chain's account signing surface
of [SUWAPPU DAG L1, 2026, §12, Table 1]. The primitive swap touches a
single function and is invisible to the per-chain `AnchorLog` and
`AnchorDispatcher` types, and to the parity-check semantics of
§7.4.2.

### References to add

```bibtex
@misc{suwappudb2026,
  author       = {Toma Natsagdorj and Javier Calderon Jr.
                  and the SUWAPPU Engineering Team},
  title        = {{Suwappu DB}: A Polymorphic Dual-VM State Substrate
                  with Capability-Gated Mutation},
  howpublished = {Companion implementation to the SUWAPPU DAG Layer 1
                  paper},
  year         = {2026},
  url          = {https://github.com/suwappu/suwappu-db}
}
```
