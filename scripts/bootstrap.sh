#!/usr/bin/env bash
# scripts/bootstrap.sh — multi-purpose project lifecycle script.
#
# Subcommands:
#   smoke         sanity-check the build/test loop end to end
#   init-repo     create the GitHub repo and push initial scaffold
#   deploy-aws    deploy validator shadow to AWS (S8 — placeholder)
#   release       cut a release (use /release slash command instead)
#
# Permission tiers (from claude-code/settings.json):
#   smoke         — allowed silently
#   init-repo     — asks before running (creates remote state)
#   deploy-aws    — asks before running
#   release       — asks before running
#
# Usage:  ./scripts/bootstrap.sh <subcommand>

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

cmd="${1:-help}"
shift || true

usage() {
  sed -n '2,16p' "$0" | sed 's/^# \{0,1\}//'
}

case "$cmd" in
    smoke)
        echo "[smoke] cargo check"
        cargo check --workspace --all-targets

        echo "[smoke] cargo test"
        cargo test --workspace

        echo "[smoke] cargo fmt --check"
        cargo fmt --all -- --check

        echo "[smoke] lane-separation"
        ./scripts/check-lane-separation.sh

        echo "[smoke] OK"
        ;;

    init-repo)
        # Creates the GitHub repository and pushes main.
        # Uses GH_REPO env var or defaults to Suwappu-Labs/suwappu-db.
        repo="${GH_REPO:-Suwappu-Labs/suwappu-db}"

        if git remote get-url origin >/dev/null 2>&1; then
            echo "init-repo: origin already set to $(git remote get-url origin)" >&2
            echo "Refusing to re-initialise. Use 'git remote set-url' if needed." >&2
            exit 1
        fi

        echo "init-repo: would run:"
        echo "  gh repo create $repo --private --source=. --remote=origin"
        echo "  git push -u origin main"
        echo ""
        echo "Re-run with INIT_REPO_CONFIRM=1 to actually do this."
        if [[ "${INIT_REPO_CONFIRM:-}" == "1" ]]; then
            gh repo create "$repo" --private --source=. --remote=origin
            git push -u origin main
            echo "init-repo: done"
        fi
        ;;

    deploy-aws)
        echo "deploy-aws: pending S8 — DAG store + recovery + telemetry sprint." >&2
        echo "See CLAUDE.md sprint backlog and /aws-status for current infra state." >&2
        exit 78  # EX_CONFIG: not yet implemented
        ;;

    release)
        echo "release: use the /release slash command in Claude Code." >&2
        echo "It runs the full pre-flight (clean tree, CI green, /check) before tagging." >&2
        exit 78
        ;;

    help|-h|--help)
        usage
        ;;

    *)
        echo "unknown subcommand: $cmd" >&2
        usage
        exit 64  # EX_USAGE
        ;;
esac
