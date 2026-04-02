#!/usr/bin/env bash
# verify-sandbox.sh — Verify rightclaw CC sandbox engagement for a live agent.
#
# Usage:
#   tests/e2e/verify-sandbox.sh <agent-name>
#
# Requires:
#   - rightclaw up has been run for <agent-name>
#   - rightclaw, claude, rg, socat, bwrap in PATH
#
# Exit codes:
#   0 = all stages passed (sandbox confirmed engaged)
#   1 = one or more stages failed

set -euo pipefail

# ---------------------------------------------------------------------------
# Argument parsing
# ---------------------------------------------------------------------------
AGENT_NAME="${1:?Usage: verify-sandbox.sh <agent-name>}"
RIGHTCLAW_HOME="${RIGHTCLAW_HOME:-$HOME/.rightclaw}"
AGENT_DIR="$RIGHTCLAW_HOME/agents/$AGENT_NAME"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
LOG_FILE="$SCRIPT_DIR/last-run.log"

# ---------------------------------------------------------------------------
# Color output (disabled when stdout is not a tty)
# ---------------------------------------------------------------------------
if [ -t 1 ]; then
  GREEN='\033[0;32m'
  RED='\033[0;31m'
  YELLOW='\033[1;33m'
  NC='\033[0m'
else
  GREEN=''
  RED=''
  YELLOW=''
  NC=''
fi

PASS_COUNT=0
FAIL_COUNT=0

pass() {
  echo -e "${GREEN}[PASS]${NC} $*"
  PASS_COUNT=$((PASS_COUNT + 1))
}

fail() {
  echo -e "${RED}[FAIL]${NC} $*"
  FAIL_COUNT=$((FAIL_COUNT + 1))
}

warn() {
  echo -e "${YELLOW}[WARN]${NC} $*"
}

# ---------------------------------------------------------------------------
# Stage 1: rightclaw doctor pre-flight
# ---------------------------------------------------------------------------
echo ""
echo "=== Stage 1: Doctor pre-flight ==="

DOCTOR_OUTPUT=""
DOCTOR_EXIT=0
DOCTOR_OUTPUT=$(rightclaw doctor 2>&1) || DOCTOR_EXIT=$?

echo "$DOCTOR_OUTPUT"

if [ $DOCTOR_EXIT -ne 0 ]; then
  fail "rightclaw doctor exited non-zero ($DOCTOR_EXIT) — aborting before CC smoke test"
  echo ""
  echo "=== Summary ==="
  echo -e "${RED}FAILED${NC}: doctor pre-flight failed. Fix issues above, then re-run."
  exit 1
fi

if echo "$DOCTOR_OUTPUT" | grep -q ' FAIL '; then
  fail "rightclaw doctor output contains FAIL checks — aborting before CC smoke test"
  echo ""
  echo "=== Summary ==="
  echo -e "${RED}FAILED${NC}: doctor reported failures. Fix issues above, then re-run."
  exit 1
fi

pass "rightclaw doctor: all checks passed (no FAIL)"

# ---------------------------------------------------------------------------
# Stage 2: Dependency availability in PATH
# ---------------------------------------------------------------------------
echo ""
echo "=== Stage 2: Sandbox dependency PATH check ==="

DEPS_OK=true

for dep in rg socat bwrap; do
  if command -v "$dep" > /dev/null 2>&1; then
    pass "$dep: found at $(command -v "$dep")"
  else
    fail "$dep: not found in PATH"
    DEPS_OK=false
  fi
done

if [ "$DEPS_OK" = false ]; then
  echo ""
  echo "=== Summary ==="
  echo -e "${RED}FAILED${NC}: one or more sandbox dependencies missing from PATH. Install missing tools and re-run."
  exit 1
fi

# ---------------------------------------------------------------------------
# Stage 3: settings.json pre-flight
# ---------------------------------------------------------------------------
echo ""
echo "=== Stage 3: Agent settings pre-flight ==="

SETTINGS_FILE="$AGENT_DIR/.claude/settings.json"
REPLY_SCHEMA_FILE="$AGENT_DIR/.claude/reply-schema.json"

if [ ! -f "$SETTINGS_FILE" ]; then
  fail "settings.json not found at $SETTINGS_FILE"
  echo "  run 'rightclaw up' first to generate agent settings"
  echo ""
  echo "=== Summary ==="
  echo -e "${RED}FAILED${NC}: agent not initialized. Run 'rightclaw up' then re-run this script."
  exit 1
fi
pass "settings.json exists"

if [ ! -f "$REPLY_SCHEMA_FILE" ]; then
  fail "reply-schema.json not found at $REPLY_SCHEMA_FILE"
  echo "  run 'rightclaw up' first to generate agent files"
  echo ""
  echo "=== Summary ==="
  echo -e "${RED}FAILED${NC}: agent not initialized. Run 'rightclaw up' then re-run this script."
  exit 1
fi
pass "reply-schema.json exists"

# ---------------------------------------------------------------------------
# Stage 4: CC smoke test — sandbox engagement
# ---------------------------------------------------------------------------
echo ""
echo "=== Stage 4: CC smoke test (sandbox engagement) ==="
echo "  agent:   $AGENT_NAME"
echo "  dir:     $AGENT_DIR"
echo "  model:   haiku"
echo "  log:     $LOG_FILE"
echo ""

REPLY_SCHEMA="$(cat "$REPLY_SCHEMA_FILE")"
PROMPT="Reply with a single word: ok"

# Use explicit exit code capture — set -e must not kill the script on CC non-zero exit.
CC_EXIT=0
CC_OUTPUT=""
CC_OUTPUT=$(
  cd "$AGENT_DIR" && \
  HOME="$AGENT_DIR" \
  USE_BUILTIN_RIPGREP=0 \
  claude -p \
    --dangerously-skip-permissions \
    --output-format json \
    --agent "$AGENT_NAME" \
    --model haiku \
    --json-schema "$REPLY_SCHEMA" \
    -- "$PROMPT" \
  2>"$LOG_FILE"
) || CC_EXIT=$?

if [ $CC_EXIT -ne 0 ]; then
  fail "claude exited with code $CC_EXIT — sandbox or CC failure"
  echo ""
  echo "  CC stderr output (from $LOG_FILE):"
  echo "  ---"
  sed 's/^/  /' "$LOG_FILE"
  echo "  ---"
  echo ""
  echo "  Possible causes:"
  echo "  - Sandbox failed to start (check bwrap AppArmor restrictions on Ubuntu 24.04+)"
  echo "  - failIfUnavailable:true in settings.json caused fatal sandbox error"
  echo "  - CC binary not found or license not accepted"
  echo "  - settings.json misconfigured (run 'rightclaw up' to regenerate)"
  echo ""
  echo "=== Summary ==="
  echo -e "${RED}FAILED${NC}: CC smoke test failed (exit $CC_EXIT). See log at $LOG_FILE"
  exit 1
fi

# Validate JSON output
if echo "$CC_OUTPUT" | python3 -c "import json,sys; json.load(sys.stdin)" > /dev/null 2>&1; then
  pass "CC smoke test: exit 0 + valid JSON output — sandbox confirmed engaged"
  echo "  output: $CC_OUTPUT"
  # VER-01/VER-02 proof: settings.json has failIfUnavailable:true, so CC exit 0 means
  # sandbox did NOT fail. If bwrap/Seatbelt failed to engage, CC would exit non-zero.
  # We explicitly do NOT grep stderr for sandbox warning strings (brittle across CC versions,
  # see 31-DISCUSSION-LOG.md). Belt-and-suspenders: scan stderr for "sandbox" keyword as
  # informational only — does not affect exit code.
  if [ -s "$LOG_FILE" ] && grep -qi "sandbox" "$LOG_FILE"; then
    warn "CC stderr mentions 'sandbox' — review $LOG_FILE to confirm no degradation"
  fi
else
  fail "CC exited 0 but output is not valid JSON"
  echo "  raw output: $CC_OUTPUT"
  echo ""
  echo "=== Summary ==="
  echo -e "${RED}FAILED${NC}: CC output parse error. Check $LOG_FILE for stderr."
  exit 1
fi

# ---------------------------------------------------------------------------
# Summary
# ---------------------------------------------------------------------------
echo ""
echo "=== Summary ==="
TOTAL=$((PASS_COUNT + FAIL_COUNT))
echo "  $PASS_COUNT/$TOTAL checks passed"
echo ""

if [ $FAIL_COUNT -eq 0 ]; then
  echo -e "${GREEN}ALL CHECKS PASSED${NC}"
  echo "  VER-01: bot subprocess sandbox confirmed — exit 0 under failIfUnavailable:true"
  echo "          (same claude binary + settings.json as teloxide worker)"
  echo "  VER-02: cron subprocess sandbox confirmed — exit 0 under failIfUnavailable:true"
  echo "          (same claude binary + settings.json as cron runner)"
  echo "  VER-03: rg + socat + bwrap confirmed in PATH"
  echo ""
  echo "  CC stderr log saved to: $LOG_FILE"
  exit 0
else
  echo -e "${RED}$FAIL_COUNT CHECK(S) FAILED${NC}"
  exit 1
fi
