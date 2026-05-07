---
description: Run all local verifications — fmt, clippy, test, deny, lane-separation
allowed-tools: Bash, Read
---

# Local verification suite

Run each step. Report pass/fail with the exact failing output for any step that doesn't succeed. Do **not** auto-fix unless the user asks — verify first, propose fix second.

```bash
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
cargo deny check
./scripts/check-lane-separation.sh
```

Additional checks if the relevant files exist:

```bash
# If anchor parity tests are wired up
[ -f scripts/cross-parity.sh ] && ./scripts/cross-parity.sh

# If integration tests are gated
[ -d tests/integration ] && cargo test --test '*' --features integration
```

## Reporting

Format the result as:

```
Step          Status
fmt           ✓ / ✗
clippy        ✓ / ✗
test          ✓ / ✗
deny          ✓ / ✗
lane-sep      ✓ / ✗
parity        ✓ / ✗ / —
integration   ✓ / ✗ / —
```

For any ✗, paste the first 30 lines of the failing output and propose a fix. Stop there — let the user decide whether to apply.
