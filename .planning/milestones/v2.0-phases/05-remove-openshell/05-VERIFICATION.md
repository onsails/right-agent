---
phase: 05-remove-openshell
verified: 2026-03-24T14:00:00Z
status: passed
score: 10/10 must-haves verified
re_verification: false
---

# Phase 5: Remove OpenShell Verification Report

**Phase Goal:** Agents launch via direct `claude` invocation instead of OpenShell sandbox wrappers -- no OpenShell dependency required
**Verified:** 2026-03-24T14:00:00Z
**Status:** passed
**Re-verification:** No -- initial verification

## Goal Achievement

### Observable Truths

| # | Truth | Status | Evidence |
|---|-------|--------|----------|
| 1 | cargo check --workspace passes with zero openshell references in production code | VERIFIED | `rg openshell crates/ templates/` returns only negative test assertions (lines testing ABSENCE of openshell). Zero references in production code paths. |
| 2 | AgentDef no longer has a policy_path field | VERIFIED | `crates/rightclaw/src/agent/types.rs` -- AgentDef struct confirmed at lines 44-69, no `policy_path` field present. `rg policy_path crates/` returns zero results. |
| 3 | RuntimeState no longer has a no_sandbox field | VERIFIED | `crates/rightclaw/src/runtime/state.rs` -- RuntimeState struct at lines 7-11 contains only `agents`, `socket_path`, `started_at`. No `no_sandbox`. |
| 4 | AgentState no longer has a sandbox_name field | VERIFIED | `crates/rightclaw/src/runtime/state.rs` -- AgentState struct at lines 15-17 contains only `name`. No `sandbox_name`. |
| 5 | Shell wrapper template has a single direct-claude code path (no openshell conditional) | VERIFIED | `templates/agent-wrapper.sh.j2` -- single `exec "$CLAUDE_BIN"` path at line 19. No `openshell`, no `no_sandbox` conditional. |
| 6 | verify_dependencies() takes no parameters and does not check for openshell | VERIFIED | `crates/rightclaw/src/runtime/deps.rs` line 14: `pub fn verify_dependencies() -> miette::Result<()>` -- zero params. Body checks only `process-compose` and `claude`/`claude-bun`. |
| 7 | cmd_down() does not call destroy_sandboxes() | VERIFIED | `crates/rightclaw-cli/src/main.rs` lines 405-427: `cmd_down` reads state, calls `client.shutdown()`, prints message. No `destroy_sandboxes` call. `rg destroy_sandboxes crates/` returns zero results. |
| 8 | init_rightclaw_home() does not create policy.yaml | VERIFIED | `crates/rightclaw/src/init.rs` -- no `DEFAULT_POLICY` constant, no `policy.yaml` in the `files` array (line 43-49). Test at line 313 explicitly asserts `!agents_dir.join("policy.yaml").exists()`. |
| 9 | doctor does not check for openshell binary | VERIFIED | `crates/rightclaw/src/doctor.rs` lines 42-55: `run_doctor` checks 3 binaries: rightclaw, process-compose, claude. No openshell. Test at line 348 asserts `!binary_names.contains(&"openshell")`. |
| 10 | discover_agents() does not require policy.yaml | VERIFIED | `crates/rightclaw/src/agent/discovery.rs` lines 82-136: discovery checks only for `IDENTITY.md` (line 109). No `policy.yaml` check. `rg policy_path crates/rightclaw/src/agent/ ` returns zero results. |

**Score:** 10/10 truths verified

### Required Artifacts

| Artifact | Expected | Status | Details |
|----------|----------|--------|---------|
| `crates/rightclaw/src/runtime/state.rs` | Simplified RuntimeState, AgentState, write_state, read_state | VERIFIED | 39 lines. Contains `pub struct RuntimeState` (3 fields), `pub struct AgentState` (1 field), `write_state`, `read_state`. No sandbox fields. |
| `templates/agent-wrapper.sh.j2` | Direct claude invocation wrapper | VERIFIED | 27 lines. Contains `exec "$CLAUDE_BIN"` at line 19. Single code path, no openshell conditional. |

### Key Link Verification

| From | To | Via | Status | Details |
|------|----|-----|--------|---------|
| `crates/rightclaw-cli/src/main.rs` | `crates/rightclaw/src/runtime/state.rs` | `rightclaw::runtime::RuntimeState, AgentState, write_state, read_state` | WIRED | Lines 342, 345, 353, 409 -- all four symbols used in cmd_up and cmd_down |
| `crates/rightclaw/src/codegen/shell_wrapper.rs` | `templates/agent-wrapper.sh.j2` | `include_str!` template rendering | WIRED | Line 4: `include_str!("../../../../templates/agent-wrapper.sh.j2")` |
| `crates/rightclaw/src/agent/discovery.rs` | `crates/rightclaw/src/agent/types.rs` | AgentDef construction | WIRED | Line 116: `AgentDef {` with all required fields, no policy_path |

### Requirements Coverage

| Requirement | Source Plan | Description | Status | Evidence |
|-------------|------------|-------------|--------|----------|
| SBMG-01 | 05-01, 05-02 | User can run `rightclaw up` without OpenShell installed | SATISFIED | `verify_dependencies()` checks only process-compose and claude -- no openshell check. Shell wrapper invokes claude directly. |
| SBMG-02 | 05-01, 05-02 | All OpenShell code paths removed from codebase | SATISFIED | `rg openshell crates/ templates/` returns only negative test assertions. `sandbox.rs` deleted. `destroy_sandboxes`, `sandbox_name_for` functions deleted. |
| SBMG-03 | 05-01 | Shell wrapper launches `claude` directly instead of wrapping with `openshell sandbox create` | SATISFIED | `templates/agent-wrapper.sh.j2` contains `exec "$CLAUDE_BIN"` with direct flags. No openshell wrapping. |
| SBMG-04 | 05-01 | `rightclaw down` no longer attempts OpenShell sandbox destroy | SATISFIED | `cmd_down()` in main.rs only calls `client.shutdown()` for process-compose. No `destroy_sandboxes` call. `rg destroy_sandboxes crates/` returns zero. |
| SBMG-05 | 05-01 | OpenShell policy.yaml files removed from default agent template | SATISFIED | `templates/right/policy.yaml` and `templates/right/policy-telegram.yaml` deleted. `init_rightclaw_home()` does not create policy.yaml. Test asserts absence. |

### Anti-Patterns Found

| File | Line | Pattern | Severity | Impact |
|------|------|---------|----------|--------|
| (none) | - | - | - | No anti-patterns found in any modified files |

### Human Verification Required

### 1. Agent Launch Without OpenShell

**Test:** Run `rightclaw up` on a system without OpenShell installed
**Expected:** Agents launch successfully via direct claude invocation, process-compose TUI shows running agents
**Why human:** Requires actual process-compose and claude CLI installed, real agent directory, and observing TUI output

### 2. Agent Shutdown Behavior

**Test:** Run `rightclaw down` after agents are running
**Expected:** Process-compose stops cleanly, no errors about missing openshell or sandbox destruction
**Why human:** Requires running instance to verify clean shutdown path

### Gaps Summary

No gaps found. All 10 observable truths verified. All 5 requirements (SBMG-01 through SBMG-05) satisfied. All artifacts exist, are substantive, and are wired. All key links verified. No anti-patterns detected. Commits confirmed in git log.

The `--no-sandbox` CLI flag is intentionally preserved as a no-op (`let _ = no_sandbox;`) for Phase 6 to repurpose for CC native sandbox configuration bypass -- this is a documented design decision, not a gap.

---

_Verified: 2026-03-24T14:00:00Z_
_Verifier: Claude (gsd-verifier)_
