## IQ-7: Anchor log — in-memory + MAC in phase-1, on-chain + signatures at launch readiness

**Status:** Accepted
**Date:** 2026-05-08
**Sprint context:** S7 (anchor log)

### Question

S7's user-facing claim is "the chain anchors its state root to multiple
external chains so any anchored chain can verify the chain's state."
Real implementation: a Solidity `LTPAnchorRegistry` contract on
Ethereum (and BSC, Polygon, etc.); ECDSA signatures from authorized
validators; on-chain dispute resolution.

Same scope question as IQ-3 (Move VM) and IQ-6 (Verkle): pull
production cryptography + chain integration into phase-1 code, or ship
the abstraction now and swap in real chain integration at launch
readiness.

### Decision

Phase-1 ships:

- In-memory `AnchorLog` per chain id (no real RPC, no gas, no Solidity)
- BLAKE3 keyed-hash MAC (not ECDSA/EdDSA)
- `AnchorDispatcher` writes to all registered chains in-process
- `parity_check` reads from in-memory logs

Real launch ships:

- Solidity `LTPAnchorRegistry` contract per target chain
- Validator-signed anchors (ECDSA over secp256k1 on EVM chains)
- Dispatcher writes via signed RPC transactions
- Parity verification reads anchors via RPC `eth_call`
- Slashing for divergent anchors

### Why this works for phase-1

The property test exercises the **load-bearing claim**: anchors at the
same logical height across N chains must agree on the state root, and
tampering with any chain's log must be detected. The MAC primitive is
adequate for this — the property test doesn't care whether
authentication is symmetric (HMAC) or asymmetric (ECDSA); both fail
under tampering.

The swap to real Solidity is mechanical from the perspective of the
property test: replace the in-memory log with an RPC adapter that
reads/writes a Solidity contract; replace `compute_mac` with an ECDSA
signing function. Tree shape unchanged, dispatch interface unchanged,
parity-check semantics unchanged.

### Consequences

- **Code:** `gsxdb-bridge::anchor` ships in-memory + MAC.
- **Tests:** 10k cases of `cross_chain_parity_holds` pass.
- **Spec:** `docs/spec/anchor-log.md` documents the future on-chain
  swap surface explicitly.
- **`scripts/cross-parity.sh`:** real implementation that runs the
  10k-case property test.

### What this leaves open

- **Launch-readiness Solidity contract.** Will need its own design doc
  + audit + deploy story.
- **Validator key management.** ECDSA keys, rotation, revocation.
- **Cross-chain time/height mapping.** Phase-1 uses a single logical
  height; production needs chain-specific height windows.
- **Slashing mechanics.** Detection (parity_check) is implemented;
  punishment is launch-readiness.

### Propagation checklist

- [x] Code: `AnchorLog`, `AnchorDispatcher`, `parity_check`
- [x] Tests: 10k cross-chain parity property test
- [x] `scripts/cross-parity.sh` real implementation
- [x] Spec doc `docs/spec/anchor-log.md`
- [ ] Launch-readiness checklist: add Solidity contract + signatures
