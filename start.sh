#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
IDENTITY_FILE="$SCRIPT_DIR/identity/IDENTITY.md"
WORKSPACE="${1:-$HOME}"

if [ ! -f "$IDENTITY_FILE" ]; then
  echo "error: identity file not found: $IDENTITY_FILE" >&2
  exit 1
fi

# TODO: wrap in openshell when available
# openshell sandbox create --policy "$SCRIPT_DIR/policies/default.yaml" -- \

exec claude \
  --append-system-prompt-file "$IDENTITY_FILE" \
  --dangerously-skip-permissions \
  -p "$WORKSPACE"
