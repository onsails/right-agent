#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
IDENTITY_DIR="$SCRIPT_DIR/identity"
WORKSPACE="${1:-$HOME}"

# Concatenate all identity files into a single prompt
PROMPT=$(cat \
  "$IDENTITY_DIR/IDENTITY.md" \
  "$IDENTITY_DIR/SOUL.md" \
  "$IDENTITY_DIR/AGENTS.md" \
)

if [ -z "$PROMPT" ]; then
  echo "error: identity files empty or missing in $IDENTITY_DIR" >&2
  exit 1
fi

# TODO: wrap in openshell when available
# openshell sandbox create --policy "$SCRIPT_DIR/policies/default.yaml" -- \

exec claude \
  --append-system-prompt "$PROMPT" \
  --dangerously-skip-permissions \
  -p "$WORKSPACE"
