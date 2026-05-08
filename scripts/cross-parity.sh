#!/usr/bin/env bash
# scripts/cross-parity.sh — runs the cross-chain anchor parity property test.
#
# Phase-1 implementation per S7: in-memory multi-chain anchor logs +
# parity_check at every height + 10k-case property test.
#
# Usage:  ./scripts/cross-parity.sh [--release|--quick]
#
#   default  : 10000 cases, dev profile (~15-20s)
#   --release: 10000 cases, release profile (~5s, but heavy compile)
#   --quick  : default 256 cases (CI smoke)

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

mode="${1:-default}"

case "$mode" in
    --release)
        echo "[cross-parity] PROPTEST_CASES=10000 cargo test --release -p gsxdb-bridge --test cross_parity"
        PROPTEST_CASES=10000 cargo test --release -p gsxdb-bridge --test cross_parity
        ;;
    --quick)
        echo "[cross-parity] cargo test -p gsxdb-bridge --test cross_parity"
        cargo test -p gsxdb-bridge --test cross_parity
        ;;
    default)
        echo "[cross-parity] PROPTEST_CASES=10000 cargo test -p gsxdb-bridge --test cross_parity"
        PROPTEST_CASES=10000 cargo test -p gsxdb-bridge --test cross_parity
        ;;
    *)
        echo "unknown mode: $mode" >&2
        echo "usage: $0 [--release|--quick]" >&2
        exit 64  # EX_USAGE
        ;;
esac

echo "[cross-parity] OK"
