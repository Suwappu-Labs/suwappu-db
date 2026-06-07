# Paper additions — Suwappu DB as the state substrate

## Visuals

Presentation-ready SUWAPPU visuals are available in the DAG repo for context:

- [SUWAPPU Visual Index](../../../suwappu-dag/docs/visuals/index.html)
- [SUWAPPU Ecosystem Atlas](../../../suwappu-dag/docs/visuals/suwappu-ecosystem-atlas.html)
- [SUWAPPU DAG presentation](../../../suwappu-dag/docs/visuals/suwappu-dag.html)
- [SUWAPPU DB presentation](../../../suwappu-dag/docs/visuals/suwappu-db.html)
- [LTP presentation](../../../suwappu-dag/docs/visuals/ltp.html)

This directory holds proposed insertions to the two academic papers
to integrate **Suwappu DB** as the named implementation of the execution
substrate described abstractly in those papers.

## Source papers (v7)

- `suwappu_dag_l1_academic_v7.pdf` — *SUWAPPU DAG Layer 1*
- `suwappu_ltp_academic_v7.pdf` — *Lattice Transfer Protocol*

## Files in this directory

| File | What it inserts | Where in the paper |
|---|---|---|
| [`dag-l1-section-7-4.md`](dag-l1-section-7-4.md) | New §7.4 — *State substrate: Suwappu DB* | DAG L1 paper, end of §7 |
| [`dag-l1-section-11-empirical.md`](dag-l1-section-11-empirical.md) | New §11.3 — empirical reinforcement | DAG L1 paper, end of §11 |
| [`dag-l1-section-12-row.md`](dag-l1-section-12-row.md) | Two rows for Table 1 (exception zones) | DAG L1 paper, §12 Table 1 |
| [`dag-l1-related-work.md`](dag-l1-related-work.md) | New related-work paragraph | DAG L1 paper, §2 (Related Work) |
| [`ltp-section-7-4.md`](ltp-section-7-4.md) | New §7.4 — Rust integration surface | LTP paper, end of §7 |
| [`ltp-section-8-row.md`](ltp-section-8-row.md) | Update to Table 2 (test coverage) | LTP paper, §8 Table 2 |

## Editorial notes

- All insertions match the papers' tone and structure: numbered
  constructions, propositions, "Why X" subheaders, conservative
  claims with limitations explicit.
- Every numeric or structural claim maps to a specific module path,
  test name, or commit in the Suwappu DB repo. No claim is uncheckable.
- Phase-1 substrate facts only. Claims about real Verkle, real Move
  VM, and deployed `LTPAnchorRegistry.sol` are deferred to launch-
  readiness — the paper's existing §12 (exception zones) and the new
  Table 1 rows in `dag-l1-section-12-row.md` carry those.

## Verification trail

The numeric claims in the additions are reproducible:

```bash
cd suwappu-db
cargo test --workspace                     # 259 tests
PROPTEST_CASES=10000 cargo test --workspace --release
./scripts/check-lane-separation.sh
./scripts/cross-parity.sh
```

## Cite as

In LaTeX, suggested entry (replace year/version when published):

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
