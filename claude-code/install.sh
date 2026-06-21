#!/usr/bin/env bash
# Install Claude Code config from claude-code/ templates into .claude/.
#
# Default (project scope): copies settings, slash commands, and subagents to ./.claude/
# With --user: also copies slash commands and subagents to ~/.claude/
#
# Idempotent. Re-running overwrites the active config from the templates.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SRC="$REPO_ROOT/claude-code"
DEST_PROJECT="$REPO_ROOT/.claude"
DEST_USER="$HOME/.claude"

if [[ ! -d "$SRC" ]]; then
  echo "error: $SRC not found — are you running from the suwappu-db repo?" >&2
  exit 1
fi

USER_INSTALL=false
for arg in "$@"; do
  case "$arg" in
    --user) USER_INSTALL=true ;;
    -h|--help)
      sed -n '2,8p' "$0" | sed 's/^# //;s/^#//'
      exit 0
      ;;
    *) echo "unknown arg: $arg" >&2; exit 1 ;;
  esac
done

install_to() {
  local dest="$1"
  local include_settings="$2"

  mkdir -p "$dest" "$dest/commands" "$dest/agents"

  if [[ "$include_settings" == "true" && -f "$SRC/settings.json" ]]; then
    cp "$SRC/settings.json" "$dest/settings.json"
    echo "  ✓ $dest/settings.json"
  fi

  if [[ -d "$SRC/commands" ]]; then
    for f in "$SRC/commands"/*.md; do
      [[ -e "$f" ]] || continue
      cp "$f" "$dest/commands/"
      echo "  ✓ $dest/commands/$(basename "$f")"
    done
  fi

  if [[ -d "$SRC/agents" ]]; then
    for f in "$SRC/agents"/*.md; do
      [[ -e "$f" ]] || continue
      cp "$f" "$dest/agents/"
      echo "  ✓ $dest/agents/$(basename "$f")"
    done
  fi
}

echo "Installing Claude Code config to project scope: $DEST_PROJECT"
install_to "$DEST_PROJECT" "true"

if [[ "$USER_INSTALL" == "true" ]]; then
  echo ""
  echo "Installing slash commands + subagents to user scope: $DEST_USER"
  echo "(settings.json NOT copied to user scope — keep your personal user settings intact)"
  install_to "$DEST_USER" "false"
fi

echo ""
echo "Done. Reload Claude Code or open a new session in this repo to pick up changes."
