//! OCC block-execution throughput benchmarks — the numbers behind
//! `BENCHMARKS.md` (gap item G2 in
//! `docs/research/chain-gap-analysis-2026-07.md`).
//!
//! Measures [`BlockExecutor::execute`] (CE-MVCC parallel path) against
//! a sequential [`Bridge::submit`] baseline over the same intent
//! blocks. Two contention regimes: 8 hot addresses (worst case —
//! every transfer conflicts) and 1,024 addresses (payments-shaped
//! low-conflict traffic).
//!
//! Run with:
//!
//! ```text
//! cargo bench -p suwappudb-bridge
//! ```
//!
//! Caveats (also spelled out in BENCHMARKS.md): in-memory
//! `BalanceStore`, `Intent::Transfer` canonicalization only (no real
//! VM bytecode on this path), default BLAKE3 commitments. These are
//! substrate numbers, not chain TPS.

use criterion::{criterion_group, criterion_main, BatchSize, BenchmarkId, Criterion, Throughput};
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use suwappudb_bridge::{BlockExecutor, Bridge, Intent};
use suwappudb_state::{Address, Balance, BridgeToken, State, StateChange};

/// Seed balance per address — high enough that transfers rarely
/// reject on insufficient funds, so the bench measures execution,
/// not rejection.
const SEED_BALANCE: u128 = 1_000_000_000;

/// Deterministic RNG seed so every run benches the same block.
const BLOCK_SEED: u64 = 0x5157_4150_5055; // "SUWAPPU" truncated

fn addr(i: u32) -> Address {
    let mut a = [0u8; 20];
    a[..4].copy_from_slice(&i.to_be_bytes());
    Address(a)
}

fn seeded_state(n_addrs: u32) -> State {
    let mut state = State::default();
    let token = BridgeToken::__for_bridge_only();
    for i in 0..n_addrs {
        state.apply(
            &token,
            &StateChange::SetBalance {
                addr: addr(i),
                to: Balance(SEED_BALANCE),
            },
        );
    }
    state
}

fn transfer_block(n_intents: usize, n_addrs: u32) -> Vec<Intent> {
    let mut rng = StdRng::seed_from_u64(BLOCK_SEED);
    (0..n_intents)
        .map(|_| Intent::Transfer {
            from: addr(rng.gen_range(0..n_addrs)),
            to: addr(rng.gen_range(0..n_addrs)),
            amount: rng.gen_range(1..1_000),
        })
        .collect()
}

fn bench_parallel_block_execute(c: &mut Criterion) {
    let mut group = c.benchmark_group("occ_block_execute");
    for (n_intents, n_addrs, label) in [
        (1_000_usize, 8_u32, "1k_intents/8_addrs_high_conflict"),
        (1_000, 1_024, "1k_intents/1k_addrs_low_conflict"),
        (10_000, 1_024, "10k_intents/1k_addrs_low_conflict"),
    ] {
        group.throughput(Throughput::Elements(n_intents as u64));
        let block = transfer_block(n_intents, n_addrs);
        group.bench_function(BenchmarkId::from_parameter(label), |b| {
            b.iter_batched(
                || seeded_state(n_addrs),
                |mut state| BlockExecutor.execute(&mut state, &block),
                BatchSize::LargeInput,
            );
        });
    }
    group.finish();
}

fn bench_sequential_baseline(c: &mut Criterion) {
    let mut group = c.benchmark_group("sequential_submit_baseline");
    let n_intents = 1_000_usize;
    let n_addrs = 1_024_u32;
    group.throughput(Throughput::Elements(n_intents as u64));
    let block = transfer_block(n_intents, n_addrs);
    group.bench_function("1k_intents/1k_addrs", |b| {
        b.iter_batched(
            || seeded_state(n_addrs),
            |mut state| {
                let mut bridge = Bridge::new(&mut state);
                for intent in &block {
                    let _ = bridge.submit(intent.clone());
                }
            },
            BatchSize::LargeInput,
        );
    });
    group.finish();
}

criterion_group!(
    benches,
    bench_parallel_block_execute,
    bench_sequential_baseline
);
criterion_main!(benches);
