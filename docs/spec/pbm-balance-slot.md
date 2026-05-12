# PBM balance slot and dual projection (S2)

## Goal

Define the canonical per-address balance record (`BalanceSlot`) that carries both
EVM and Move views while enforcing projection equality by construction.

## Types and invariants

- `BalanceSlot` stores `evm_balance` and `move_coin_value`.
- Mutations (`deposit`, `withdraw`, replacement through store writes) preserve
  the invariant:

`evm_balance == move_coin_value`

- `BalanceStore` abstraction provides backend-independent access.
- `InMemoryBalanceStore` and `RedbBalanceStore` must return equivalent logical
  values for identical write histories.

## Storage layout

- In-memory backend: map keyed by `Address`.
- redb backend: tables keyed by address bytes; slot fields encoded into stable
  byte forms and round-tripped through the store API.

## Failure model

- Overflow on deposit and underflow on withdraw are rejected atomically.
- Backend open/read/write errors are surfaced as store-level errors.
- Backend differences are treated as correctness failures (covered by parity
  tests/properties).

## Tests

Representative gates:

- `redb_preserves_dual_projection` (backend persistence + invariant)
- `store_preserves_dual_projection` (property test)
- atomicity tests for rejected mutations

## Open questions

- Future resource-model changes for real Move VM integration may require
  expanding slot schema while preserving canonical equivalence.
