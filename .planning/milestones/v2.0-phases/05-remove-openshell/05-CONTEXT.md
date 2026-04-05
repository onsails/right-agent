# Phase 5: Remove OpenShell - Context

**Gathered:** 2026-03-24
**Status:** Ready for planning

<domain>
## Phase Boundary

Strip all OpenShell code from the codebase. Shell wrappers launch `claude` directly. No sandbox create/destroy lifecycle. No policy.yaml requirement. Agent discovery simplified to IDENTITY.md only. The `--no-sandbox` flag is repurposed (not removed) to control CC native sandbox in Phase 6.

This is a subtraction phase — compiler-guided deletion with targeted modifications.

</domain>

<decisions>
## Implementation Decisions

### --no-sandbox Flag
- **D-01:** Keep `--no-sandbox` flag on `rightclaw up`, repurpose it. In Phase 5, it becomes a no-op (all agents run without OpenShell anyway). Phase 6 will wire it to generate `sandbox.enabled: false` in settings.json.
- **D-02:** Do NOT remove the flag — it provides a dev/testing escape hatch when bubblewrap is unavailable.

### Permissions Model
- **D-03:** `--dangerously-skip-permissions` remains always-on (Phase 2 D-08 unchanged). CC native sandbox is the security layer. bypass + sandbox = autonomous agents with OS-level guardrails.
- **D-04:** No per-agent toggle for permissions — all agents bypass CC permission prompts.

### Agent Validation
- **D-05:** `discovery.rs` validation changes: IDENTITY.md is the only required file (was IDENTITY.md + policy.yaml).
- **D-06:** `AgentDef.policy_path` field removed from the struct.
- **D-07:** Phase 6 may add settings.json-related validation when sandbox config generation is implemented.

### Shell Wrapper Simplification
- **D-08:** Wrapper template removes all OpenShell conditional blocks. Single code path: `claude` invocation with flags.
- **D-09:** Wrapper still passes `--append-system-prompt-file`, `--dangerously-skip-permissions`, `--channels` (when Telegram configured), system prompt file (when present). Only the `openshell sandbox create` wrapping is removed.

### Runtime Lifecycle Simplification
- **D-10:** `RuntimeState` drops `no_sandbox: bool` (becomes irrelevant when there's no sandbox to track).
- **D-11:** `AgentState` drops `sandbox_name` field (no sandbox names to track).
- **D-12:** `destroy_sandboxes()` function removed entirely. `cmd_down()` becomes: stop process-compose, done.
- **D-13:** `sandbox.rs` module removed entirely.

### Dependency & Doctor Changes
- **D-14:** `verify_dependencies()` removes openshell check. Only checks: process-compose, claude.
- **D-15:** `doctor.rs` removes openshell binary check and installation URL. New sandbox deps (bubblewrap, socat) added in Phase 7.

### Init & Templates
- **D-16:** `init.rs` removes policy.yaml template creation (both base and Telegram variants).
- **D-17:** Template files `templates/right/policy.yaml` and `templates/right/policy-telegram.yaml` deleted.
- **D-18:** `init.rs` no longer needs path expansion for policy files (OpenShell required absolute paths, CC sandbox doesn't).

### Claude's Discretion
- Exact order of file-by-file removal (compiler will guide)
- Test refactoring approach for shell_wrapper_tests.rs and sandbox_tests.rs
- Whether to keep sandbox_tests.rs as a test file or merge remaining tests elsewhere

</decisions>

<canonical_refs>
## Canonical References

**Downstream agents MUST read these before planning or implementing.**

### OpenShell code to remove (complete inventory)
- `crates/rightclaw/src/runtime/sandbox.rs` — destroy_sandboxes(), RuntimeState, AgentState (DELETE entire file)
- `crates/rightclaw/src/runtime/sandbox_tests.rs` — sandbox state tests (DELETE entire file)
- `crates/rightclaw/src/runtime/mod.rs` — exports sandbox module (MODIFY: remove sandbox exports)
- `crates/rightclaw/src/runtime/deps.rs` — openshell check in verify_dependencies() (MODIFY: remove openshell check)
- `crates/rightclaw/src/codegen/shell_wrapper.rs` — passes no_sandbox to template (MODIFY: remove no_sandbox context)
- `crates/rightclaw/src/codegen/shell_wrapper_tests.rs` — sandbox-specific tests (MODIFY: remove sandbox test variants)
- `crates/rightclaw/src/agent/discovery.rs` — policy.yaml requirement (MODIFY: remove policy validation)
- `crates/rightclaw/src/agent/types.rs` — AgentDef.policy_path field (MODIFY: remove field)
- `crates/rightclaw/src/doctor.rs` — openshell binary check (MODIFY: remove check)
- `crates/rightclaw/src/init.rs` — policy template constants, policy file creation (MODIFY: remove policy generation)
- `crates/rightclaw-cli/src/main.rs` — --no-sandbox flag handling, destroy_sandboxes call (MODIFY)
- `templates/agent-wrapper.sh.j2` — openshell conditional block (MODIFY: remove conditional)
- `templates/right/policy.yaml` — OpenShell policy template (DELETE)
- `templates/right/policy-telegram.yaml` — OpenShell policy template with Telegram (DELETE)

### Phase 2 context (decisions that carry forward)
- `.planning/phases/02-cli-runtime-and-sandboxing/02-CONTEXT.md` — D-05 through D-18, especially agent launch pattern

### Research
- `.planning/research/ARCHITECTURE.md` — v2.0 architecture changes, component modification scope
- `.planning/research/SUMMARY.md` — synthesis of all research findings

</canonical_refs>

<code_context>
## Existing Code Insights

### Removal Scope (11 files, 2 deletions)
- **DELETE:** sandbox.rs, sandbox_tests.rs, policy.yaml, policy-telegram.yaml (4 files)
- **MODIFY:** main.rs, shell_wrapper.rs, shell_wrapper_tests.rs, discovery.rs, types.rs, deps.rs, doctor.rs, init.rs, mod.rs, agent-wrapper.sh.j2 (10 files)

### Established Patterns
- Clap derive API for CLI flags (main.rs) — --no-sandbox stays but changes semantics
- minijinja for template rendering (shell_wrapper.rs) — template simplifies
- `which` crate for binary detection (deps.rs, doctor.rs) — openshell removed from checks
- `include_str!` for embedded templates (init.rs) — policy templates no longer embedded

### Key Data Flow Changes
- `cmd_up()`: no longer passes `no_sandbox` to `generate_wrapper()`, no longer creates RuntimeState with sandbox fields
- `cmd_down()`: no longer calls `destroy_sandboxes()`, just stops process-compose
- `generate_wrapper()`: template context drops `no_sandbox`, `policy_path` variables
- `discover_agents()`: drops policy.yaml existence check from validation

</code_context>

<specifics>
## Specific Ideas

No specific requirements — compiler-guided deletion with targeted modifications. The Rust compiler will catch all dangling references when sandbox.rs and policy_path are removed.

</specifics>

<deferred>
## Deferred Ideas

None — discussion stayed within phase scope

</deferred>

---

*Phase: 05-remove-openshell*
*Context gathered: 2026-03-24*
