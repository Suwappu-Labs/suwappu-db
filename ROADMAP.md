# Roadmap

`suwappu-db` is the storage + execution substrate consumed by
[`suwappu-dag`](https://github.com/suwappu/suwappu-dag) (the
SUWAPPU L1) as a workspace dependency. The roadmap below tracks
substrate-internal milestones; the L1's roadmap (and overall mainnet
plan) lives in
[`suwappu-dag/ROADMAP.md`](https://github.com/suwappu/suwappu-dag/blob/main/ROADMAP.md).

See [`CHANGELOG.md`](./CHANGELOG.md) for the shipped versions and
[`INTEGRATORS.md`](./INTEGRATORS.md) for the front-door API + stability
promises.

---

## Substrate phases

| Phase | Window | Headline | Status |
|---|---|---|---|
| **A** | Q1 2026 | Pass A — S8.5–S12: Redb persistence, real Move VM, Verkle commitments, anchor SLA | ✅ Closed |
| **B** | Apr–May 2026 | Pass B — security audit + hardening | ✅ Closed |
| **C** | May 2026 | Pass C — external-dev readiness (INTEGRATORS surface, SDK examples, fuzz targets) | 🟡 In flight |
| **D** | Q3 2026 | suwappu-db v0.2.0 — extended bridge surface (protocol-owned credit path; unblocks suwappu-dag's `SuwappuDbSubstrate` arms currently stubbed) | ⏳ Next |
| **E** | Q4 2026 | Compact multipoint Verkle witnesses (IQ-6 closure) | ⏳ |
| **GA** | aligned with suwappu-dag mainnet (M18–M24) | suwappu-db `1.0` cut against mainnet genesis | ⏳ |

---

## Connection to the L1 roadmap

The substrate's release cadence is keyed to the L1's needs:

- `suwappu-db v0.1.0-pre` (May 2026) → consumed by `suwappu-dag` v0.1.0–v0.3.0.
- `suwappu-db v0.2.0` (Q3 2026) → unblocks the production-real
  `SuwappuDbSubstrate` arms in `suwappu-dag` (today they're stubbed pending
  the protocol-owned credit path).
- `suwappu-db v1.0` → cut against mainnet genesis.

See the
[`suwappu-dag` ROADMAP](https://github.com/suwappu/suwappu-dag/blob/main/ROADMAP.md)
for the full mainnet plan.

---

## How to contribute

- [`CONTRIBUTING.md`](./CONTRIBUTING.md) — dev workflow.
- [`SECURITY.md`](./SECURITY.md) — coordinated disclosure for
  vulnerabilities (don't open public issues).
- [`INTEGRATORS.md`](./INTEGRATORS.md) — stable API surface +
  compatibility commitments for downstream consumers.
