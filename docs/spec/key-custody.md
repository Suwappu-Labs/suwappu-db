# Validator key custody — HSM-only profile

**Sprint context:** S11 (Solidity LTPAnchorRegistry + ECDSA), reinforced
in S12 (shadow E2E).
**Source for the practice:** Avalanche validator FAQ, Hyperledger Besu
`--security-module` plugin, Sui validator key incident history.

## Why this exists

The 30–50 PoA Authority Ring members are licensed institutions
operating under regulatory licensure. Compliance review will block
any deployment where signing keys live on the validator host
filesystem. Avalanche's enterprise guidance is explicit on this
point:

- [Avalanche validator FAQ](https://support.avax.network/en/articles/6187511-validator-faq)
  — "Do not keep the signing key on the validator host."
- [Besu QBFT docs](https://besu.hyperledger.org/private-networks/how-to/configure/consensus/qbft)
  — `--security-module` integrates an external key manager.

Suwappu DB needs an analogue and a written profile.

## Profile (binding)

An Authority Node MUST:

1. Hold its validator signing key in one of:
   - AWS CloudHSM (PKCS#11 interface)
   - YubiHSM 2 (with FIPS 140-2 mode enabled)
   - Fireblocks MPC (with policy-based approval)
2. Emit a **key-attestation JSON** in its handshake that proves the
   key is in such a module. Required fields:
   - `module_type`: `"cloudhsm" | "yubihsm" | "fireblocks-mpc"`
   - `module_id`: opaque module identifier (operator-chosen)
   - `attestation_signature`: signature over the module ID by the
     module's hardware-rooted attestation key
3. Reject peers whose attestation fails to parse or whose
   `module_type` is not in the accepted set.

## Trait surface

```mermaid
classDiagram
    class ValidatorKeyHandle {
        <<trait>>
        +sign(message: &[u8]) Vec~u8~
        +pubkey() Vec~u8~
        +attestation() Attestation
    }
    class CloudHsmKeyHandle
    class YubiHsmKeyHandle
    class FireblocksMpcKeyHandle
    class InsecureSoftKey {
        test-only
    }
    ValidatorKeyHandle <|.. CloudHsmKeyHandle
    ValidatorKeyHandle <|.. YubiHsmKeyHandle
    ValidatorKeyHandle <|.. FireblocksMpcKeyHandle
    ValidatorKeyHandle <|.. InsecureSoftKey
```

`InsecureSoftKey` exists only for `#[cfg(test)]` — the production
build gates it behind a feature flag that defaults off, and the CI
build rejects production binaries that linked it in.

## Failure modes

| Mode | Detection | Response |
|---|---|---|
| HSM unreachable at startup | Module returns error on `sign()` probe | Refuse to start node |
| HSM key revoked mid-flight | First failed `sign()` after success | Halt block production, raise alert, request governance rotation |
| Attestation expired in peer handshake | Verifier rejects expired signature | Drop the connection |
| Counterparty advertises insecure module | `module_type` not in accepted set | Drop the connection |

## Enforcement model (B5 clarification)

**Operational, not code-level.** The HSM-only profile above is
enforced by **deployment tooling and the per-peer handshake**, not by
a compile-time check inside the suwappudb binary.

| Layer | Mechanism | Where it runs |
|---|---|---|
| Build | `production-pqc` and similar features gate optional crypto deps; **no feature gates `InsecureSoftKey` out of production builds today**. | Cargo |
| Deploy | Terraform / Helm values + bootstrap scripts pin `KEY_SOURCE` env to one of `cloudhsm`/`yubihsm`/`fireblocks-mpc`; AMI image lacks a writable filesystem path for raw keys. | Operator infrastructure |
| Runtime | The attestation-handshake check in the failure-modes table rejects peers whose `module_type` is not in the accepted set, and refuses to start if the local handle cannot produce a valid attestation. | suwappudb node |
| Audit | The validator's signed attestation (handshake JSON) is mirrored into the on-chain anchor / governance log; off-chain compliance can verify the chain of custody after the fact. | LTP super-nodes |

**Why operational and not code-level.** A compile-time gate that
rejects `InsecureSoftKey` in `--release` was considered and rejected
for two reasons:

1. Half a sprint of code-side scaffolding (feature plumbing across
   five crates, build-time CI assertion) to enforce something the
   handshake already rejects at runtime. The runtime gate is
   strictly stronger — it catches keys loaded from any source, not
   just the `InsecureSoftKey` path.
2. Test binaries and local-dev runs need the soft-key path. A
   feature gate that's "off in release" still leaves the cfg-flag
   active in CI; the attack surface is the binary that actually
   ships to the production rollout, and that surface is bounded by
   deployment tooling.

See [`docs/architecture/deployment-topology.md`](../architecture/deployment-topology.md)
for the deployment-layer enforcement specifics (AMI / cloud-init /
secrets manager paths) and [`docs/audit/pass-b-2026-05-16.md`](../audit/pass-b-2026-05-16.md)
for the B5 audit verdict.

## Migration

The phase-1 substrate uses no real validator keys — it's a substrate,
not a chain. Real key handles enter when S11 ships the Solidity
contract. Until then the trait surface is the spec; implementations
are S11 scope.

## Open questions

- **Multi-region disaster recovery.** A single HSM provider per
  jurisdiction may concentrate failure. The corridor super-node §9
  in the paper handles geographic diversity at the legal-entity
  level; individual HSM redundancy is operator-scoped.
- **Hybrid composition with ML-DSA-65.** When S11 lands hybrid
  signatures, both the classical key (ECDSA) and the PQ key
  (ML-DSA-65) must live in HSM-grade modules. Some PQ algorithms
  don't have HSM support yet — track NIST's PQC implementation
  inventory.
