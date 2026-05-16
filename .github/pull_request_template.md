<!--
Thanks for sending a PR. Before submitting:

1. Sign your commits: `git commit -s` (DCO; checked by CI). See
   CONTRIBUTING.md for the full policy.
2. One scope per PR. Reference the IQ / sprint / audit ledger this
   addresses if applicable.
3. Run `cargo clippy --workspace -- -D warnings` and
   `cargo test --workspace` locally — green CI is required before
   review.
-->

## Summary

<!-- One or two sentences: what does this PR change and why. -->

## Scope + references

- Surface(s) touched:
- IQ / sprint / audit-ledger entry (if any):
- Related issue: closes #

## What changed

<!-- A few bullets. Files + behaviour, not a diff narration. -->

-
-
-

## Tests

<!-- Tick what applies; expand any "no" with a reason. -->

- [ ] Unit / integration tests added or updated
- [ ] Property tests still pass at default cases
- [ ] (Optional) Property tests pass at `PROPTEST_CASES=10000`
- [ ] Solidity tests still pass (if `contracts/` touched)
- [ ] Manual smoke test ran locally

## Stability

- [ ] No change to frozen public types
  ([INTEGRATORS.md "Stability promises"](../INTEGRATORS.md#stability-promises))
- [ ] Breaks a frozen surface — CHANGELOG + INTEGRATORS updated;
      maintainer approval secured

## Checklist

- [ ] Commits signed off (`git commit -s`)
- [ ] `cargo clippy --workspace -- -D warnings` passes
- [ ] `cargo test --workspace` passes
- [ ] CHANGELOG updated (under `[Unreleased]` or a new version
      section)
- [ ] Relevant docs (spec / IQ / audit ledger / per-crate README)
      updated
