# Mock Inventory & Replacement Plan

Audit of every mock, stub, in-memory store, and explicit placeholder in
the repo, cross-checked against the sprint backlog in `CLAUDE.md` and
the accepted IQs in `docs/iq/`. Generated 2026-05-15.

Each entry has:

- **Where** — file path(s) and the symbol or comment.
- **What it stands in for** — production behavior it mocks.
- **Scheduled replacement** — sprint + IQ if planned, or `UNSCHEDULED`.

---

## 1. Named mocks (production code, trait-swappable)

### 1.1 `MockEvm`, `MockMove`

- **Where:** `crates/gsxdb-bridge/src/vm/executor.rs:56,85`
  (re-exported via `crates/gsxdb-bridge/src/vm/mod.rs:15` and
  `crates/gsxdb-bridge/src/lib.rs:46`).
- **What it stands in for:** real revm EVM execution and real Aptos
  Move VM execution. Both currently canonicalise their VM-shape tx to
  `Intent::Transfer` and route through `Bridge::submit`.
- **Scheduled replacement:** **S9** (IQ-2 Part 2, IQ-3, IQ-4).
  IQ-2 binds "mock at S3, real VM at launch"; IQ-3 selects Aptos as
  the Move dialect; IQ-4 binds address-shape/nonce semantics that the
  real VMs require.

### 1.2 `MockMoveExecutor`

- **Where:** `crates/gsxdb-state/src/vm/executor.rs:58`
  (default impl of `MoveExecutor` trait at line 42).
- **What it stands in for:** Aptos `move-vm-runtime` bytecode
  execution. Mock passes input state through unchanged with no
  bytecode interpretation.
- **Scheduled replacement:** **S9** via `AptosMoveExecutor`
  (skeleton already exists at `vm/executor.rs:75-91`, feature-gated
  on `production-move-executor`; body is a `TODO(S9)`). Backlog row
  S9 explicitly cites IQ-3/4/5.

### 1.3 `MockL1AnchorReader`

- **Where:** `crates/gsxdb-bridge/src/anchor/l1_reader.rs:23`
  (implements `L1AnchorReader` trait at line 16).
- **What it stands in for:** RPC reads of the Solidity
  `LTPAnchorRegistry` on chain 103115120. Mock keeps anchors in a
  `BTreeMap<(ChainId, u64), Anchor>` populated by tests.
- **Scheduled replacement:** **S11** (IQ-7 Part 2).
  Production alternative `RpcL1AnchorReader` already exists in the
  same file (line 59), but its `call_get_anchor` body uses two
  in-file placeholders flagged separately below (§3.2).

---

## 2. In-memory stores (default for tests; production swap exists or is planned)

### 2.1 `InMemoryBalanceStore`

- **Where:** `crates/gsxdb-state/src/store.rs:64`
  (implements `BalanceStore` trait; default for `State::default()`).
- **What it stands in for:** persistent balance storage.
- **Status:** Production counterpart `RedbBalanceStore` already
  exists at `crates/gsxdb-state/src/redb_store.rs`. The two are
  cross-checked by the property test `redb_matches_in_memory`
  (`redb_store.rs:446`). **No replacement work pending** — this is the
  intended dual-impl pattern; in-memory remains as the test fixture.

### 2.2 `InMemoryBlockStore`

- **Where:** `crates/gsxdb-bridge/src/recovery/store.rs:62`
  (implements `BlockStore` trait).
- **What it stands in for:** crash-recoverable block log.
- **Scheduled replacement:** **S8.5** (IQ-8).
  `RedbBlockStore` is already exported alongside it
  (`recovery/mod.rs:26`); S8.5 hardens replay persistence and makes
  redb the default for production callers. In-memory stays as the
  test fixture.

### 2.3 Lane `Mempool` (FIFO)

- **Where:** `crates/gsxdb-lane/src/lib.rs:32-39`.
  Comment: *"Simple FIFO mempool. Phase-1 placeholder; S5 replaces
  this with the crash-recoverable cross-VM intent queue Q."*
- **What it stands in for:** the crash-recoverable cross-VM intent
  queue Q described in `docs/spec/cross-vm-intent-queue.md`.
- **Scheduled replacement:** ⚠️ **STALE COMMENT.** S5 has closed
  (backlog: ✅ Closed), but its exit gate is *"Cross-VM intent
  bundles + Intent::Call dispatch; bundle_atomicity @ 10k passing"* —
  bundles, not a crash-recoverable mempool. The lane `Mempool` was
  not replaced in S5 and is **UNSCHEDULED** in the current backlog.
  Either the comment needs to be retargeted at a later sprint (likely
  S12 launch-hardening) or a fresh IQ should record the actual queue
  plan. Flag for human decision.

---

## 3. Cryptographic / commitment placeholders

### 3.1 Verkle commitment scaffolding

- **Where:**
  - `crates/gsxdb-state/src/tree/verkle.rs:22` — `GroupElement([u8; 32])` "Phase 1 placeholder; S10 wraps actual elliptic-curve arithmetic."
  - `crates/gsxdb-state/src/tree/commit.rs:45` — `Commitment([0; 32])` placeholder constant; real value produced by `empty_commitment()`.
- **What it stands in for:** Verkle commitments + IPA witnesses.
- **Scheduled replacement:** **S10** (IQ-6 Part 2). Backlog row S10:
  *"Real Verkle commitments + IPA witnesses + parity harness."*
  Phase-1 actually commits via BLAKE3 per IQ-6 Part 1.

### 3.2 `LTPAnchorRegistry` hash + MAC placeholders

- **Where:**
  - `contracts/src/LTPAnchorRegistry.sol:246` — uses Keccak256 instead of BLAKE3 ("real deployment requires actual BLAKE3 via precompile").
  - `crates/gsxdb-bridge/src/anchor/parity_test.rs:4` — Rust parity tests deliberately match the Keccak256 placeholder.
  - `crates/gsxdb-bridge/src/anchor/l1_reader.rs:86` — `RpcL1AnchorReader` calls with function signature `0x12345678` (stand-in ABI, "real ABI needed").
  - `crates/gsxdb-bridge/src/anchor/l1_reader.rs:183-187` — `placeholder_key = [0u8; 32]` HMAC key in RPC reader.
- **What it stands in for:** real BLAKE3 hashing + real `getAnchor`
  ABI + real per-chain HMAC keys delivered via the anchor dispatcher.
- **Scheduled replacement:** **S11** (IQ-7 Part 2).
  Backlog row S11: *"Solidity LTPAnchorRegistry + ECDSA parity."*
  Note: S11 covers the Solidity hash swap and ECDSA, but the
  `RpcL1AnchorReader` ABI + HMAC-key plumbing in
  `l1_reader.rs:86,183` is **not explicitly enumerated** in any IQ
  exit gate. Likely fine to fold into S11, but worth recording as a
  sub-task there.

---

## 4. Other placeholders

### 4.1 `/metrics` HTTP handler

- **Where:** `crates/gsxdb-server/src/main.rs:77` — *"Handler: GET /metrics (placeholder)."*
- **What it stands in for:** Prometheus scrape endpoint backed by
  OpenTelemetry meters.
- **Scheduled replacement:** **S12** (IQ-9, "structured telemetry").
  IQ-9 spells out `opentelemetry-prometheus` exporter wiring.

### 4.2 Replay parent-hash placeholder

- **Where:** `crates/gsxdb-bridge/src/recovery/replay.rs:78-80` —
  uses `GENESIS_PARENT` as the parent of `from` because the real
  parent must come from outside the call. Caller-contract, not a
  mock; documented as deliberate.
- **Scheduled replacement:** none needed (this is a defined contract,
  not a stand-in for production behavior).

### 4.3 Test-only fixture

- `crates/gsxdb-bridge/src/recovery/store.rs:640` — `fake_hash = [0x42; 32]`. Test data, not production code. Not relevant.

---

## 5. Coverage summary

| Mock / placeholder | Sprint | IQ | Status |
|---|---|---|---|
| `MockEvm`, `MockMove` | S9 | IQ-2, IQ-3, IQ-4 | ✅ Scheduled |
| `MockMoveExecutor` | S9 | IQ-3, IQ-4 | ✅ Scheduled (skeleton present) |
| `MockL1AnchorReader` | S11 | IQ-7 | ✅ Scheduled |
| `InMemoryBalanceStore` | — | IQ-1 | ✅ Dual-impl by design (kept as test fixture) |
| `InMemoryBlockStore` | S8.5 | IQ-8 | ✅ Scheduled |
| Lane `Mempool` (FIFO) | — | — | ⚠️ **UNSCHEDULED** — comment references S5 but S5 closed without replacing it |
| `GroupElement` / empty `Commitment` | S10 | IQ-6 | ✅ Scheduled |
| Solidity Keccak256-for-BLAKE3 | S11 | IQ-7 | ✅ Scheduled |
| `RpcL1AnchorReader` ABI sig `0x12345678` | S11 | IQ-7 | ⚠️ **Implicit** — not enumerated in IQ-7 exit gate; fold into S11 sub-task |
| `RpcL1AnchorReader` placeholder HMAC key | S11 | IQ-7 | ⚠️ **Implicit** — same as above; depends on dispatcher key plumbing |
| `/metrics` handler | S12 | IQ-9 | ✅ Scheduled |
| `recovery/replay.rs` `GENESIS_PARENT` use | — | — | ✅ Not a mock — defined contract |

---

## 6. Gaps to resolve

1. **Lane `Mempool` is unscheduled.** The source comment promises S5
   replacement, but S5 closed against a different exit gate. Either:
   - retarget the comment at S12 (intent queue Q fits under
     launch-hardening), or
   - file a fresh IQ for the crash-recoverable queue Q and schedule
     a sprint for it.
2. **`RpcL1AnchorReader` two sub-placeholders are implicit in S11.**
   The Solidity Keccak→BLAKE3 swap is named in S11; the ABI signature
   and HMAC-key plumbing are not. Add them to the S11 PR checklist or
   amend IQ-7 Part 2's propagation list.

Everything else is accounted for: a sprint owns it, and the
replacement type or feature gate already exists in-tree.
