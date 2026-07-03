# Benchmarks

Reproducible performance numbers for the suwappu-db substrate (gap
item G2 in
[`docs/research/chain-gap-analysis-2026-07.md`](./docs/research/chain-gap-analysis-2026-07.md)).
Same honesty rule as the rest of the repo: these are **substrate
microbenchmarks on default features**, not chain TPS. Peer chains
publish end-to-end network numbers (consensus + networking +
signature verification included); nothing here is comparable to a
"20k TPS mainnet" claim and we do not pretend it is.

## How to run

```sh
cargo bench -p suwappudb-bridge    # OCC block execution
cargo bench -p suwappudb-state     # tree commit, proofs, snapshots
```

Criterion, text output only (no HTML reports). Bench sources:
[`crates/suwappudb-bridge/benches/block_throughput.rs`](./crates/suwappudb-bridge/benches/block_throughput.rs),
[`crates/suwappudb-state/benches/state_commit.rs`](./crates/suwappudb-state/benches/state_commit.rs).

## Reference run

2026-07-03, `v0.1.0-pre` + G2/G5 tree, **default features** (BLAKE3
commitments, mock VM canonicalization, in-memory `BalanceStore`).
Hardware: 4-vCPU Intel Xeon @ 2.80 GHz, 16 GB RAM, Linux. Numbers
are criterion means; expect run-to-run variance of a few percent on
shared cloud hardware.

### Block execution (`suwappudb-bridge`)

| Benchmark | Time / block | Throughput |
|---|---|---|
| OCC parallel — 1k transfers, 8 addrs (worst-case conflict) | 13.4 ms | 74.5k intents/s |
| OCC parallel — 1k transfers, 1,024 addrs (low conflict) | 12.7 ms | 78.5k intents/s |
| OCC parallel — 10k transfers, 1,024 addrs | 601 ms | 16.7k intents/s |
| Sequential `Bridge::submit` — 1k transfers, 1,024 addrs | 0.106 ms | 9.4M intents/s |

**Read the sequential row carefully before quoting it.** The
comparison is not apples-to-apples: `BlockExecutor::execute` also
computes the state-tree root and the block report; `Bridge::submit`
commits nothing. But the gap (≈120×) is real and expected on this
workload: with mock-VM canonicalization a transfer is a few memory
operations, so the speculative CE-MVCC machinery (versioned
multi-store, validation, retry loop) costs far more than it
parallelizes away — on 4 cores, for near-free transactions,
sequential wins. This matches the Block-STM literature: optimistic
parallelism pays when per-transaction execution is expensive (real
EVM/Move bytecode), which is exactly the `production-move-executor`
configuration these benches don't yet cover. The 10k-intent row
(worse-than-linear, retry-loop-dominated at ~10 intents/address of
contention) is the same effect. Publishing the unflattering row is
the point of this file; the optimization backlog it implies is
tracked in the gap analysis.

### State tree, proofs, snapshots (`suwappudb-state`)

| Benchmark | Time | Throughput |
|---|---|---|
| Full tree build + BLAKE3 root — 1k accounts | 3.29 ms | 304k accounts/s |
| Full tree build + BLAKE3 root — 10k accounts | 45.7 ms | 219k accounts/s |
| Full tree build + BLAKE3 root — 100k accounts | 696 ms | 144k accounts/s |
| Inclusion-proof generation — 10k-account tree | 20.3 ms/proof | — |
| Snapshot capture — 10k accounts | 0.98 ms | 10.2M entries/s |
| Snapshot restore — 10k accounts | 0.71 ms | 14.1M entries/s |

Notes:

- **Tree commit is a full rebuild** (`StateTree::from_state`) — the
  documented S6 simplification. The per-block cost is O(total
  accounts), not O(touched accounts); incremental commitment with
  dirty-marking is the obvious next optimization and these numbers
  are its baseline.
- **Proof generation at 20 ms/proof** is the per-step witness path.
  The compact multipoint witness follow-on (IQ-6, Phase E) targets
  both size (~12.5 KB → ~200 B) and this generation cost.
- Snapshot capture/restore at ~1 ms per 10k accounts confirms the
  operator bootstrap flow
  ([`docs/architecture/node-bootstrap.md`](./docs/architecture/node-bootstrap.md))
  is I/O-bound, not CPU-bound.

## What is deliberately not here (yet)

- **`production-move-executor` / `production-verkle` numbers.** The
  real Aptos VM and banderwagon+IPA paths change both sides of the
  parallelism trade-off; they need their own bench matrix (and the
  Verkle exit-gate already records prove/verify timings). Tracked
  under G2 follow-on in the gap analysis.
- **End-to-end numbers** (JSON-RPC ingest → block → anchor). Those
  belong to the shadow-testnet harness, not criterion.
- **redb-backed store benches.** The persistence layer's cost shows
  up in recovery/replay, which has its own exit-gate timing.
