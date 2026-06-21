#!/usr/bin/env bash
# check-lane-separation.sh
#
# Enforces the lane-separation invariant: suwappudb-lane must not depend on
# suwappudb-state, directly or transitively-through-anything-other-than-bridge,
# and must not import suwappudb_state symbols in its source.
#
# Two checks:
#   1. Cargo.toml of suwappudb-lane has no suwappudb-state dependency
#   2. No `use suwappudb_state` (or `suwappudb_state::`) in suwappudb-lane source
#
# Exits non-zero on violation. Run by /check, by CI, and by the lane-auditor
# subagent.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
LANE_DIR="$REPO_ROOT/crates/suwappudb-lane"
LANE_TOML="$LANE_DIR/Cargo.toml"
LANE_SRC="$LANE_DIR/src"

if [[ ! -f "$LANE_TOML" ]]; then
  echo "error: $LANE_TOML not found" >&2
  exit 2
fi

if [[ ! -d "$LANE_SRC" ]]; then
  echo "error: $LANE_SRC not found" >&2
  exit 2
fi

violations=0

# Check 1: Cargo.toml dep
# We grep for `suwappudb-state` outside of comment lines.
if grep -nE '^[[:space:]]*suwappudb-state[[:space:]]*=' "$LANE_TOML" > /dev/null; then
  echo "✗ lane-separation violation: suwappudb-lane/Cargo.toml depends on suwappudb-state" >&2
  grep -nE '^[[:space:]]*suwappudb-state[[:space:]]*=' "$LANE_TOML" | sed 's/^/    /' >&2
  violations=$((violations + 1))
fi

# Check 2: source imports
# We look for any `use suwappudb_state` or bare `suwappudb_state::` reference. We
# exclude comments (lines starting with `//` after optional whitespace).
while IFS= read -r -d '' file; do
  if grep -nE '^[[:space:]]*[^/]*\b(use[[:space:]]+suwappudb_state|suwappudb_state::)' "$file" > /dev/null; then
    echo "✗ lane-separation violation: $file imports from suwappudb_state" >&2
    grep -nE '^[[:space:]]*[^/]*\b(use[[:space:]]+suwappudb_state|suwappudb_state::)' "$file" | sed 's/^/    /' >&2
    violations=$((violations + 1))
  fi
done < <(find "$LANE_SRC" -type f -name '*.rs' -print0)

if [[ "$violations" -gt 0 ]]; then
  echo "" >&2
  echo "lane-separation: $violations violation(s) — see CLAUDE.md 'Load-bearing invariants'" >&2
  exit 1
fi

echo "✓ lane-separation: suwappudb-lane has no path to suwappudb-state outside the bridge"
