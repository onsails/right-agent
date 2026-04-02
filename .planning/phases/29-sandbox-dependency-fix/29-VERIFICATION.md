---
phase: 29-sandbox-dependency-fix
verified: 2026-04-02T20:15:00Z
status: passed
score: 5/5 must-haves verified
re_verification: false
---

# Phase 29: Sandbox Dependency Fix Verification Report

**Phase Goal:** CC sandbox actually engages in nix/devenv environments -- all four fix sites land atomically
**Verified:** 2026-04-02T20:15:00Z
**Status:** passed
**Re-verification:** No -- initial verification

## Goal Achievement

### Observable Truths

| # | Truth | Status | Evidence |
|---|-------|--------|----------|
| 1 | Generated settings.json contains sandbox.failIfUnavailable: true unconditionally | VERIFIED | settings.rs:78 has `"failIfUnavailable": true` inside sandbox JSON block; test `includes_fail_if_unavailable_unconditionally` covers both sandbox=true and sandbox=false cases |
| 2 | Generated settings.json contains sandbox.ripgrep.command with absolute rg path when rg is available | VERIFIED | settings.rs:99-104 conditionally injects ripgrep block when `rg_path` is Some; test `injects_ripgrep_command_when_path_provided` asserts command="/usr/bin/rg" and args=[] |
| 3 | Generated settings.json omits sandbox.ripgrep when rg is not available | VERIFIED | settings.rs:99 `if let Some` guard means None skips injection; test `omits_ripgrep_when_path_not_provided` asserts .get("ripgrep").is_none() |
| 4 | USE_BUILTIN_RIPGREP is set to "0" in both worker.rs and cron.rs | VERIFIED | worker.rs:403 `cmd.env("USE_BUILTIN_RIPGREP", "0")` and cron.rs:231 `cmd.env("USE_BUILTIN_RIPGREP", "0")`; rg confirms zero matches for `"1"` value across crates/ |
| 5 | devenv.nix includes pkgs.ripgrep in packages | VERIFIED | devenv.nix:8 `pkgs.ripgrep` with SBOX-04 comment |

**Score:** 5/5 truths verified

### Required Artifacts

| Artifact | Expected | Status | Details |
|----------|----------|--------|---------|
| `crates/rightclaw/src/codegen/settings.rs` | rg_path param, failIfUnavailable, ripgrep injection | VERIFIED | Signature has `rg_path: Option<PathBuf>` at line 31; failIfUnavailable at line 78; ripgrep injection at lines 99-104 |
| `crates/rightclaw/src/codegen/settings_tests.rs` | 3 new tests for ripgrep/failIfUnavailable | VERIFIED | Tests at lines 271, 290, 307; all 14 pre-existing calls updated to 4-arg form (None as 4th) |
| `devenv.nix` | pkgs.ripgrep in packages | VERIFIED | Line 8 |

### Key Link Verification

| From | To | Via | Status | Details |
|------|----|-----|--------|---------|
| main.rs | settings.rs | `which::which("rg")` resolved before loop, passed as rg_path | WIRED | main.rs:379 resolves rg_path via which::which; line 396 passes `rg_path.clone()` to generate_settings |
| settings.rs | settings.json output | serde_json sandbox object | WIRED | Line 78 failIfUnavailable: true; lines 99-104 conditional ripgrep injection |
| init.rs | settings.rs | None as rg_path for template | WIRED | init.rs:97 passes `None` as 4th arg -- correct for template generation |

### Data-Flow Trace (Level 4)

Not applicable -- settings.rs generates config JSON, not a UI component rendering dynamic data.

### Behavioral Spot-Checks

Step 7b: SKIPPED -- requires running `rightclaw up` with external dependencies (process-compose, claude). The code paths are verified via unit tests and static analysis.

### Requirements Coverage

| Requirement | Source Plan | Description | Status | Evidence |
|-------------|------------|-------------|--------|----------|
| SBOX-01 | 29-01 | Inject sandbox.ripgrep.command with resolved system rg path | SATISFIED | settings.rs:99-104 conditional injection; main.rs:379 which::which resolution |
| SBOX-02 | 29-01 | USE_BUILTIN_RIPGREP corrected to "0" in worker.rs and cron.rs | SATISFIED | worker.rs:403 and cron.rs:231 both set "0"; no "1" values remain |
| SBOX-03 | 29-01 | sandbox.failIfUnavailable: true in generated settings.json | SATISFIED | settings.rs:78 unconditional in JSON block |
| SBOX-04 | 29-01 | devenv.nix includes pkgs.ripgrep | SATISFIED | devenv.nix:8 |

No orphaned requirements found -- all 4 SBOX IDs from REQUIREMENTS.md phase 29 mapping are claimed by 29-01-PLAN.md.

### Anti-Patterns Found

| File | Line | Pattern | Severity | Impact |
|------|------|---------|----------|--------|
| None | - | - | - | - |

No TODOs, FIXMEs, placeholders, empty returns, or stub patterns found in modified files.

### Human Verification Required

### 1. Live sandbox engagement in nix environment

**Test:** Run `rightclaw up` in a nix devenv shell, then check the generated `.claude/settings.json` for an agent
**Expected:** settings.json contains `sandbox.failIfUnavailable: true` and `sandbox.ripgrep.command` pointing to the nix-store rg path; CC sandbox actually engages (no silent fallback)
**Why human:** Requires running rightclaw with process-compose and CC in a live nix environment; cannot verify nix store path resolution or CC sandbox runtime behavior statically

### Gaps Summary

No gaps found. All four fix sites verified in the codebase. Commit `6313653` contains all changes atomically. Tests cover the three key behaviors (failIfUnavailable unconditional, ripgrep injection when present, ripgrep omission when absent). The USE_BUILTIN_RIPGREP polarity fix is confirmed in both call sites with no remaining "1" values.

---

_Verified: 2026-04-02T20:15:00Z_
_Verifier: Claude (gsd-verifier)_
