---
phase: 31-e2e-verification
verified: 2026-04-02T22:30:00Z
status: human_needed
score: 1/3 must-haves verified (3rd programmatically verified; 1st and 2nd need live agent)
re_verification: false
human_verification:
  - test: "Bot subprocess sandbox confirmation (VER-01)"
    expected: "Run `tests/e2e/verify-sandbox.sh <agent-name>` with a live agent that has Telegram configured. Stage 4 CC smoke test exits 0 with valid JSON output. CC stderr log shows no sandbox warning or degradation message."
    why_human: "Requires `rightclaw up` to have been run for a real agent, CC binary in PATH, valid license, and bubblewrap/Seatbelt working on the host. Cannot simulate in a context without those prerequisites."
  - test: "Cron subprocess sandbox confirmation (VER-02)"
    expected: "Same as VER-01 — single CC smoke test covers both bot and cron paths per D-06 (they share the same claude binary, same settings.json, same env vars). Run `tests/e2e/verify-sandbox.sh <agent-name>` and confirm exit 0."
    why_human: "Same prerequisites as VER-01. The script's Stage 4 uses the identical CC invocation pattern as both worker.rs and cron.rs — one run covers both."
---

# Phase 31: E2E Verification — Verification Report

**Phase Goal:** Full rightclaw up → doctor green → CC sandbox ON → Telegram → cron flow is verified with all three sandbox dependencies (rg, socat, bwrap) explicitly confirmed
**Verified:** 2026-04-02T22:30:00Z
**Status:** human_needed
**Re-verification:** No — initial verification

## Goal Achievement

### Observable Truths (from ROADMAP Success Criteria)

| # | Truth | Status | Evidence |
|---|-------|--------|----------|
| 1 | Bot subprocess (teloxide → `claude -p --agent`) completes a Telegram reply with CC stderr showing no sandbox warning or degradation | ? HUMAN | Script exists and mirrors worker.rs invocation exactly, but live agent run required to confirm sandbox actually engages |
| 2 | Cron subprocess (`claude -p --agent`) fires on schedule with CC stderr showing no sandbox warning | ? HUMAN | Same script covers this path (D-06: bot+cron share same binary/settings); live run required |
| 3 | Repeatable verification script exists in `tests/e2e/` covering rg, socat, bwrap — re-runnable after CC version bumps | ✓ VERIFIED | `tests/e2e/verify-sandbox.sh` exists, executable, passes `bash -n`, explicit Stage 2 loop checks `rg socat bwrap` via `command -v` |

**Score:** 1/3 truths verified programmatically; 2/3 require human verification with live agent

### Required Artifacts

| Artifact | Expected | Status | Details |
|----------|----------|--------|---------|
| `tests/e2e/verify-sandbox.sh` | 4-stage bash pipeline confirming sandbox engagement | ✓ VERIFIED | 234 lines, executable (`rwxr-xr-x`), committed in d8142cd |
| `tests/e2e/.gitignore` | Excludes last-run.log from git | ✓ VERIFIED | Contains `last-run.log`, committed in d8142cd |

### Key Link Verification

| From | To | Via | Status | Details |
|------|----|-----|--------|---------|
| Stage 4 CC invocation | `worker.rs:371-408` pattern | subshell `cd "$AGENT_DIR"` + `HOME` + `USE_BUILTIN_RIPGREP=0` + all flags | ✓ WIRED | Script mirrors worker.rs exactly: same env vars, same `--dangerously-skip-permissions`, `--output-format json`, `--agent`, `--model haiku`, `--json-schema` with inlined schema |
| Stage 1 doctor check | `main.rs:247-271` exit code | `rightclaw doctor 2>&1 \|\| DOCTOR_EXIT=$?` + non-zero abort | ✓ WIRED | Doctor exits non-zero on any Fail check per main.rs; script captures with `\|\| DOCTOR_EXIT=$?` under `set -euo pipefail` |
| Stage 1 doctor grep | `doctor.rs:30` output format | `grep -q ' FAIL '` | ✓ WIRED | Format `{:<20} {:<6}` renders `FAIL  ` padded — ` FAIL ` substring present in all Fail lines. Belt-and-suspenders per plan note. |
| Stage 2 dep check | rg/socat/bwrap | `command -v` loop | ✓ WIRED | Explicit `for dep in rg socat bwrap` loop — all three named. POSIX-portable, works under `set -u`. |
| Stage 4 sandbox proof | `failIfUnavailable:true` in settings.json | CC exit 0 = sandbox engaged | ✓ WIRED | Proof strategy documented in script comments: SBOX-03 sets failIfUnavailable:true so exit 0 means bwrap did not fail. Non-brittle vs stderr grep. |

### Data-Flow Trace (Level 4)

Not applicable — this is a bash script, not a component rendering dynamic data. No state/props to trace.

### Behavioral Spot-Checks

| Behavior | Command | Result | Status |
|----------|---------|--------|--------|
| Syntax valid | `bash -n tests/e2e/verify-sandbox.sh` | exits 0, no output | ✓ PASS |
| Usage error on no args | `verify-sandbox.sh` (no args) | exits 1, prints `1: Usage: verify-sandbox.sh <agent-name>` | ✓ PASS |
| Stage 1 abort on missing rightclaw binary | `RIGHTCLAW_HOME=/tmp/nonexistent verify-sandbox.sh fake-agent` | exits 1, prints `[FAIL] rightclaw doctor exited non-zero (127)` | ✓ PASS |
| Full live run with real agent | requires `rightclaw up` + CC binary | SKIPPED — no live agent in context | ? SKIP |
| commit d8142cd exists | `git show d8142cd --stat` | 2 files, 235 insertions, message matches | ✓ PASS |

### Requirements Coverage

| Requirement | Source Plan | Description | Status | Evidence |
|-------------|-------------|-------------|--------|----------|
| VER-01 | 31-01-PLAN.md | Bot subprocess (`claude -p`) runs with sandbox enabled — CC stderr shows no sandbox warning | ? HUMAN | Script Stage 4 is the verification mechanism; requires live run to confirm outcome |
| VER-02 | 31-01-PLAN.md | Cron subprocess (`claude -p --agent`) runs with sandbox enabled — CC stderr shows no sandbox warning | ? HUMAN | Same Stage 4 covers this path per D-06; requires live run |
| VER-03 | 31-01-PLAN.md | Repeatable verification script checks rg + socat + bwrap availability in agent launch environment | ✓ SATISFIED | Stage 2 of `verify-sandbox.sh` explicitly checks all three with `command -v`; script is executable and re-runnable |

No orphaned requirements — REQUIREMENTS.md maps VER-01, VER-02, VER-03 all to Phase 31, all claimed by 31-01-PLAN.md.

### Anti-Patterns Found

| File | Line | Pattern | Severity | Impact |
|------|------|---------|----------|--------|
| None found | — | — | — | — |

Script was scanned for TODO/FIXME, `return null`, empty handlers, hardcoded empty data — none present. All four stages have real implementations. `set -euo pipefail` enforces fail-fast. Exit code capture pattern (`CC_EXIT=0 / ... \|\| CC_EXIT=$?`) is correct for non-zero inspection under strict mode.

One notable design constraint in the script: Stage 4 uses `grep -qi "sandbox"` on stderr as informational only (WARN, not FAIL). This is intentional per D-08 and documented in the script comments — not a stub.

### Human Verification Required

#### 1. Full E2E Smoke Test (VER-01 + VER-02)

**Test:** With a real agent initialized via `rightclaw up <agent-name>`, run:
```
tests/e2e/verify-sandbox.sh <agent-name>
```
**Expected:**
- All stages pass with `[PASS]` prefix
- Final output: `ALL CHECKS PASSED`
- VER-01 and VER-02 confirmation lines printed
- `tests/e2e/last-run.log` created with CC stderr (should be minimal/empty on success)
- Exit code 0

**Why human:** Requires `rightclaw up` to have been run for a real agent (produces settings.json with `failIfUnavailable:true` and `reply-schema.json`), CC binary in PATH with valid license, and bubblewrap not blocked by AppArmor. Cannot simulate in a static analysis context. The script's proof strategy (exit 0 under `failIfUnavailable:true`) is only meaningful with a real CC + real bwrap execution.

**Failure hint if Stage 4 fails:** Check `tests/e2e/last-run.log`. If bwrap is blocked by AppArmor (Ubuntu 24.04+), see MEMORY.md — unprivileged bubblewrap requires kernel sysctl `kernel.apparmor_restrict_unprivileged_userns=0`.

### Gaps Summary

No implementation gaps. The deliverable (`tests/e2e/verify-sandbox.sh`) is complete, substantive, executable, and correctly wired to all canonical references. The human verification items are not gaps — they are the verification mechanism itself. VER-01 and VER-02 are satisfied by the script's existence and design; their truth can only be confirmed by running the script against a live system.

---

_Verified: 2026-04-02T22:30:00Z_
_Verifier: Claude (gsd-verifier)_
