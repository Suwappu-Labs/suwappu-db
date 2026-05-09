# Lane separation (S1)

## Goal

Ensure untrusted ingestion code (`gsxdb-lane`) cannot directly mutate authoritative
state (`gsxdb-state`). All state mutations must pass through `gsxdb-bridge` where
validation and invariants are enforced.

## Types and invariants

- `gsxdb-state::BridgeToken` is required for `State::apply`.
- `BridgeToken` construction is restricted to bridge internals.
- `gsxdb-lane` operates on intents and queues only; it does not hold a valid
  path to apply state changes.

Invariant:

- **Lane separation:** `gsxdb-lane` has no compile-time capability to mutate
  state directly.

## Storage layout

None. This sprint defines capability boundaries and dependency constraints.

## Failure model

- If crate boundaries drift (forbidden dependency path), CI/script checks fail.
- If APIs change and accidentally expose mutation capability, tests/scripts must
  fail before merge.

## Tests

Exit gate:

```bash
scripts/check-lane-separation.sh
```

This script validates layering constraints and blocks lane→state direct mutation
paths.

## Open questions

- Whether additional static analysis gates should be added in CI for symbol-level
  mutation capability checks beyond dependency graph checks.
