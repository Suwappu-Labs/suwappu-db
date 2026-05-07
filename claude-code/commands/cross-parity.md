---
description: Run the 3-way LTPAnchorRegistry parity test (Solidity ↔ Rust ↔ shadow)
allowed-tools: Bash, Read, Agent
---

# Cross-parity verification

The 3-way parity check verifies that anchor validation rules are identical across:

1. **Solidity** — `LTPAnchorRegistry` contract (canonical)
2. **Rust** — `gsxdb-anchor` crate (production validator)
3. **Shadow** — testnet shadow instance behavior

For Phase 1 close, all 36 entity-state-machine pairs must be green.

## Phase 1: Verify deps available

```bash
which forge       # Solidity tests
cargo --version   # Rust tests
ls contracts/LTPAnchorRegistry.sol 2>/dev/null || echo "MISSING contract"
ls gsxdb-anchor/src/lib.rs 2>/dev/null || echo "MISSING crate"
```

If any dep is missing, stop and report.

## Phase 2: Run the parity matrix

```bash
# Rust side
cargo test --package gsxdb-anchor --features parity-fixtures -- --nocapture

# Solidity side
(cd contracts && forge test --match-contract AnchorParity -vv)

# Cross-fixture comparison
./scripts/cross-parity.sh   # diffs Rust output against Solidity output for all 36 pairs
```

## Phase 3: Delegate to parity-checker

If any pair diverges, invoke the `parity-checker` subagent with:

- The failing pair(s) and their inputs
- The Rust validation function
- The Solidity validation function
- Request: "Identify the divergence — is it a Rust bug, a Solidity bug, or a spec ambiguity?"

## Phase 4: Report

```
Pair (entity × state × machine)                Status
Anchor × Pending × OnRamp                      ✓
Anchor × Pending × OffRamp                     ✓
... (36 pairs)
```

End with: `36/36 GREEN — Phase 1 parity gate met` or `<n>/36 GREEN — <m> divergences need resolution`.
