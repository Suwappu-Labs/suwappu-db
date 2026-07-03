//! State-tree commitment, proof-generation, and snapshot benchmarks —
//! the numbers behind `BENCHMARKS.md` (gap item G2 in
//! `docs/research/chain-gap-analysis-2026-07.md`).
//!
//! Run with:
//!
//! ```text
//! cargo bench -p suwappudb-state
//! ```
//!
//! Default features bench the BLAKE3 commitment scheme (the phase-1
//! default). The banderwagon+IPA path (`production-verkle`) is
//! benchmarked separately via the `verkle_parity` exit-gate timings —
//! see BENCHMARKS.md for why it is not wired into this harness yet.

use criterion::{criterion_group, criterion_main, BatchSize, BenchmarkId, Criterion, Throughput};
use suwappudb_state::snapshot::StateSnapshot;
use suwappudb_state::{Address, Balance, BridgeToken, State, StateChange, StateTree};

fn addr(i: u32) -> Address {
    let mut a = [0u8; 20];
    a[..4].copy_from_slice(&i.to_be_bytes());
    Address(a)
}

fn seeded_state(n: u32) -> State {
    let mut state = State::default();
    let token = BridgeToken::__for_bridge_only();
    for i in 0..n {
        state.apply(
            &token,
            &StateChange::SetBalance {
                addr: addr(i),
                to: Balance(u128::from(i) * 7 + 1),
            },
        );
    }
    state
}

/// Full tree build + root commitment over the whole state — the
/// per-block cost shape today (`StateTree::from_state(state).root()`
/// is exactly what `BlockExecutor` computes at commit).
fn bench_tree_commit(c: &mut Criterion) {
    let mut group = c.benchmark_group("state_tree_commit");
    group.sample_size(20);
    for n in [1_000_u32, 10_000, 100_000] {
        group.throughput(Throughput::Elements(u64::from(n)));
        let state = seeded_state(n);
        group.bench_function(BenchmarkId::from_parameter(format!("{n}_accounts")), |b| {
            b.iter(|| StateTree::from_state(&state).root());
        });
    }
    group.finish();
}

/// Inclusion-proof generation against a 10k-account tree — the
/// per-query witness cost an integrator pays on `proof(addr)`.
fn bench_proof_generation(c: &mut Criterion) {
    let state = seeded_state(10_000);
    let tree = StateTree::from_state(&state);
    let target = addr(1_234);
    c.bench_function("proof_generation/10k_accounts", |b| {
        b.iter(|| tree.proof(&target));
    });
}

/// Snapshot capture (`from_state`) and restore (`restore_into_state`)
/// over a 10k-account state — the operator-facing bootstrap path
/// exercised by `suwappudb-snapshot export` / `import`.
fn bench_snapshot_roundtrip(c: &mut Criterion) {
    let mut group = c.benchmark_group("snapshot");
    group.throughput(Throughput::Elements(10_000));

    let state = seeded_state(10_000);
    group.bench_function("capture/10k_accounts", |b| {
        b.iter(|| StateSnapshot::from_state(&state, 1, None));
    });

    let snap = StateSnapshot::from_state(&state, 1, None);
    let token = BridgeToken::__for_bridge_only();
    group.bench_function("restore/10k_accounts", |b| {
        b.iter_batched(
            State::default,
            |mut fresh| {
                snap.restore_into_state(&mut fresh, &token)
                    .expect("restore")
            },
            BatchSize::LargeInput,
        );
    });
    group.finish();
}

criterion_group!(
    benches,
    bench_tree_commit,
    bench_proof_generation,
    bench_snapshot_roundtrip
);
criterion_main!(benches);
