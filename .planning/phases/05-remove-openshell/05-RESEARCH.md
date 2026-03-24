# Phase 5: Remove OpenShell - Research

**Researched:** 2026-03-24
**Domain:** Rust codebase surgery -- compiler-guided deletion of OpenShell dependency
**Confidence:** HIGH

## Summary

Phase 5 is a pure subtraction phase. The goal is to remove every OpenShell reference from the RightClaw codebase so that `rightclaw up` launches agents via direct `claude` invocation instead of `openshell sandbox create` wrapping. This is the foundational step of the v2.0 sandbox migration -- removing the old system before adding the new one (Phase 6: settings.json generation, Phase 7: doctor/installer updates).

The scope is well-defined: 4 file deletions, 10 file modifications, and corresponding test updates. The Rust compiler is the primary verification tool -- removing `policy_path` from `AgentDef` and `sandbox.rs` functions from `runtime/mod.rs` will cascade compilation errors through every call site. The CONTEXT.md decisions are precise: `--no-sandbox` becomes a no-op (kept for Phase 6), `AgentDef.policy_path` is removed, `RuntimeState` drops sandbox fields, `destroy_sandboxes()` is deleted entirely, and the shell wrapper template collapses to a single direct-claude code path.

**Primary recommendation:** Start by removing `sandbox.rs` and `policy_path` from `AgentDef`, then let the compiler guide you through every file that references them. The Rust type system makes this phase low-risk -- if it compiles and tests pass, the removal is complete.

<user_constraints>
## User Constraints (from CONTEXT.md)

### Locked Decisions
- **D-01:** Keep `--no-sandbox` flag on `rightclaw up`, repurpose it. In Phase 5, it becomes a no-op (all agents run without OpenShell anyway). Phase 6 will wire it to generate `sandbox.enabled: false` in settings.json.
- **D-02:** Do NOT remove the flag -- it provides a dev/testing escape hatch when bubblewrap is unavailable.
- **D-03:** `--dangerously-skip-permissions` remains always-on (Phase 2 D-08 unchanged). CC native sandbox is the security layer. bypass + sandbox = autonomous agents with OS-level guardrails.
- **D-04:** No per-agent toggle for permissions -- all agents bypass CC permission prompts.
- **D-05:** `discovery.rs` validation changes: IDENTITY.md is the only required file (was IDENTITY.md + policy.yaml).
- **D-06:** `AgentDef.policy_path` field removed from the struct.
- **D-07:** Phase 6 may add settings.json-related validation when sandbox config generation is implemented.
- **D-08:** Wrapper template removes all OpenShell conditional blocks. Single code path: `claude` invocation with flags.
- **D-09:** Wrapper still passes `--append-system-prompt-file`, `--dangerously-skip-permissions`, `--channels` (when Telegram configured), system prompt file (when present). Only the `openshell sandbox create` wrapping is removed.
- **D-10:** `RuntimeState` drops `no_sandbox: bool` (becomes irrelevant when there's no sandbox to track).
- **D-11:** `AgentState` drops `sandbox_name` field (no sandbox names to track).
- **D-12:** `destroy_sandboxes()` function removed entirely. `cmd_down()` becomes: stop process-compose, done.
- **D-13:** `sandbox.rs` module removed entirely.
- **D-14:** `verify_dependencies()` removes openshell check. Only checks: process-compose, claude.
- **D-15:** `doctor.rs` removes openshell binary check and installation URL. New sandbox deps (bubblewrap, socat) added in Phase 7.
- **D-16:** `init.rs` removes policy.yaml template creation (both base and Telegram variants).
- **D-17:** Template files `templates/right/policy.yaml` and `templates/right/policy-telegram.yaml` deleted.
- **D-18:** `init.rs` no longer needs path expansion for policy files (OpenShell required absolute paths, CC sandbox doesn't).

### Claude's Discretion
- Exact order of file-by-file removal (compiler will guide)
- Test refactoring approach for shell_wrapper_tests.rs and sandbox_tests.rs
- Whether to keep sandbox_tests.rs as a test file or merge remaining tests elsewhere

### Deferred Ideas (OUT OF SCOPE)
None -- discussion stayed within phase scope
</user_constraints>

<phase_requirements>
## Phase Requirements

| ID | Description | Research Support |
|----|-------------|------------------|
| SBMG-01 | User can run `rightclaw up` without OpenShell installed | Remove openshell check from `verify_dependencies()` (D-14). Remove openshell from doctor checks (D-15). Template no longer invokes openshell (D-08). |
| SBMG-02 | All OpenShell code paths removed from codebase | Delete sandbox.rs (D-13), sandbox_tests.rs. Remove `AgentState`, `sandbox_name_for`, `destroy_sandboxes` from runtime exports. Remove policy_path from AgentDef (D-06). Remove openshell conditional from template (D-08). |
| SBMG-03 | Shell wrapper launches `claude` directly | Template collapse to single code path using `$CLAUDE_BIN` (D-08). Remove `{% if not no_sandbox %}` openshell block. Keep all CC flags (D-09). |
| SBMG-04 | `rightclaw down` no longer attempts OpenShell sandbox destroy | Remove `destroy_sandboxes()` call from `cmd_down()` (D-12). Remove `no_sandbox` check from `cmd_down()`. Simplify to: shutdown PC, done. |
| SBMG-05 | OpenShell policy.yaml files removed from default agent template | Delete `templates/right/policy.yaml` and `templates/right/policy-telegram.yaml` (D-17). Remove `include_str!` constants from `init.rs` (D-16). Remove policy creation from `init_rightclaw_home()`. |
</phase_requirements>

## Standard Stack

No new dependencies needed. This phase only removes code and simplifies existing code.

### Existing Stack (Unchanged)
| Library | Purpose | Relevance to Phase |
|---------|---------|-------------------|
| clap | CLI args | `--no-sandbox` flag stays but semantics change |
| minijinja | Template rendering | Wrapper template simplifies |
| which | Binary detection | openshell check removed |
| serde/serde_json | Serialization | RuntimeState struct changes |
| miette/thiserror | Error handling | `AgentError::MissingRequiredFile` still exists but no longer used for policy.yaml |

### Crate Removals
None. The `which` crate is still used for `process-compose` and `claude` checks. No crate becomes unused.

## Architecture Patterns

### Pattern: Compiler-Guided Deletion

**What:** Remove a type or field, then fix every compilation error. The Rust compiler finds all references.

**When to use:** Exactly this phase. Removing `policy_path: PathBuf` from `AgentDef` and `sandbox_name_for()` from sandbox.rs will break every call site.

**Execution strategy:**
1. Delete entire files first (sandbox.rs, sandbox_tests.rs, policy templates)
2. Remove fields from structs (AgentDef.policy_path, RuntimeState.no_sandbox, AgentState.sandbox_name)
3. Remove module re-exports (runtime/mod.rs sandbox exports)
4. `cargo check` -- compiler lists every broken reference
5. Fix each broken reference by removing the code that used the deleted item
6. Repeat until clean compilation

**Example:**
```rust
// Step 1: Remove from types.rs
pub struct AgentDef {
    pub name: String,
    pub path: PathBuf,
    pub identity_path: PathBuf,
    // REMOVED: pub policy_path: PathBuf,
    pub config: Option<AgentConfig>,
    // ... rest unchanged
}

// Step 2: Compiler error in discovery.rs line 129:
//   field `policy_path` not found in `AgentDef`
// Fix: remove the policy_path field from the AgentDef construction

// Step 3: Compiler error in shell_wrapper.rs line 37:
//   no field `policy_path` on type `AgentDef`
// Fix: remove policy_path from template context

// Step 4: Compiler error in shell_wrapper_tests.rs lines 22, 50:
//   field `policy_path` not found
// Fix: remove from test helper functions
```

### Pattern: RuntimeState Backward Compatibility

**What:** The simplified `RuntimeState` must still deserialize old state.json files from v1.0 (which have `no_sandbox`, `sandbox_name` fields).

**When to use:** During RuntimeState struct simplification.

**How to handle:**
```rust
// Option A: #[serde(default)] on the old field during transition
// NOT needed for Phase 5 -- the entire struct changes.

// The new RuntimeState:
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RuntimeState {
    pub agents: Vec<AgentState>,  // AgentState simplified (just name)
    pub socket_path: String,
    pub started_at: String,
    // no_sandbox: REMOVED
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AgentState {
    pub name: String,
    // sandbox_name: REMOVED
}
```

**Backward compat consideration:** If a user has a running v1.0 instance and runs `rightclaw down` with v2.0 binary, `read_state()` will try to deserialize old state.json with `sandbox_name` field. Use `#[serde(default)]` for removed fields OR accept that users must `rightclaw down` before upgrading. Since rightclaw is pre-release, accepting the breaking change is fine. The old state.json has extra fields that serde ignores by default (serde ignores unknown fields unless `deny_unknown_fields` is set). Check: `RuntimeState` does NOT have `deny_unknown_fields`, so old state files with extra `sandbox_name` and `no_sandbox` fields will deserialize fine into the new struct (extra fields are silently ignored).

**Confidence:** HIGH -- verified that `RuntimeState` uses default serde behavior (no `deny_unknown_fields`).

### Recommended Deletion Order

Based on dependency analysis of the codebase:

```
Wave 1: Delete files and remove struct fields (breaks compilation)
  - DELETE: templates/right/policy.yaml
  - DELETE: templates/right/policy-telegram.yaml
  - DELETE: crates/rightclaw/src/runtime/sandbox_tests.rs
  - MODIFY: crates/rightclaw/src/agent/types.rs -- remove policy_path field
  - MODIFY: crates/rightclaw/src/runtime/sandbox.rs -- remove AgentState.sandbox_name,
    RuntimeState.no_sandbox, destroy_sandboxes(), sandbox_name_for()

Wave 2: Fix compilation errors (cascading from Wave 1)
  - MODIFY: crates/rightclaw/src/runtime/mod.rs -- remove sandbox re-exports
  - MODIFY: crates/rightclaw/src/agent/discovery.rs -- remove policy.yaml check,
    remove policy_path from AgentDef construction
  - MODIFY: crates/rightclaw/src/codegen/shell_wrapper.rs -- remove no_sandbox param,
    remove policy_path from context
  - MODIFY: templates/agent-wrapper.sh.j2 -- remove openshell block
  - MODIFY: crates/rightclaw/src/runtime/deps.rs -- remove openshell check,
    remove no_sandbox param
  - MODIFY: crates/rightclaw/src/doctor.rs -- remove openshell binary check,
    remove policy.yaml from agent validation
  - MODIFY: crates/rightclaw/src/init.rs -- remove policy template constants,
    remove policy file creation, remove path expansion
  - MODIFY: crates/rightclaw-cli/src/main.rs -- remove sandbox state creation,
    remove destroy_sandboxes call, simplify cmd_down

Wave 3: Fix tests
  - MODIFY: crates/rightclaw/src/codegen/shell_wrapper_tests.rs
  - MODIFY: crates/rightclaw/src/agent/discovery_tests.rs
  - MODIFY: crates/rightclaw/src/doctor.rs (inline tests)
  - MODIFY: crates/rightclaw/src/init.rs (inline tests)
  - MODIFY: crates/rightclaw/src/runtime/deps.rs (inline tests)
```

### Anti-Patterns to Avoid
- **Partial removal:** Do not leave `policy_path` as `Option<PathBuf>` "for compatibility." The CONTEXT.md says remove it (D-06). A clean break is better than optional fields nobody uses.
- **Premature addition:** Do not add Phase 6 features (settings.json generation, `HOME` override) in this phase. This phase is ONLY removal.
- **Test deletion without replacement:** Do not delete sandbox-related tests without ensuring the remaining runtime state tests still cover `write_state`/`read_state` roundtrips. Move them to sandbox.rs (which becomes state-only) or a new file.

## Don't Hand-Roll

Not applicable for this phase. No new functionality is being added -- only code removal.

## Common Pitfalls

### Pitfall 1: Forgetting to Update the `cmd_pair` Function
**What goes wrong:** `cmd_pair()` in main.rs does not go through the standard `cmd_up` pipeline. It directly constructs a claude invocation. If it references policy_path or sandbox features, it will still compile if those references are through a different code path.
**Why it happens:** `cmd_pair` is a separate command that bypasses wrapper generation. Easy to overlook during compiler-guided deletion since it may not reference the deleted fields directly.
**How to avoid:** Explicitly audit `cmd_pair()` -- verify it has no OpenShell references. Currently it does not use policy_path or sandbox features, so no changes needed. But verify.
**Warning signs:** `cmd_pair` launching an agent that tries to invoke openshell.

### Pitfall 2: `--no-sandbox` Flag Becoming Dead Code
**What goes wrong:** The `--no-sandbox` flag is kept (D-01) but becomes a no-op. If the flag is still passed to `verify_dependencies()` or `generate_wrapper()`, the function signatures still expect it but do nothing with it.
**Why it happens:** The flag's purpose shifts: Phase 5 makes it meaningless, Phase 6 gives it new meaning.
**How to avoid:** In Phase 5, remove `no_sandbox` from `verify_dependencies()` and `generate_wrapper()` function signatures. The CLI flag stays in clap, but the parsed value is not passed anywhere. Phase 6 will wire it to settings.json generation.
**Warning signs:** `#[allow(unused_variables)]` warnings on the `no_sandbox` parameter.

### Pitfall 3: RuntimeState Deserialization Breaking on Old State Files
**What goes wrong:** A v1.0 `state.json` has fields `no_sandbox` and `sandbox_name` per agent. If the new struct can't deserialize these, `rightclaw down` fails on an existing running instance.
**Why it happens:** Struct fields removed without considering existing serialized data.
**How to avoid:** Verified: `RuntimeState` and `AgentState` do NOT use `#[serde(deny_unknown_fields)]`. Serde's default behavior silently ignores extra fields in JSON. However, when REMOVING fields from the struct, the NEW struct will not populate those fields -- which is fine because they're not used. When ADDING `#[serde(default)]` is needed for fields that exist in old JSON but are now optional in the new struct. Since we're removing fields (not making them optional), no `#[serde(default)]` is needed -- serde just ignores the extra JSON keys.
**Warning signs:** `rightclaw down` failing with "unknown field" errors on existing state.json.

### Pitfall 4: init.rs Tests Asserting policy.yaml Existence
**What goes wrong:** Several init.rs tests assert `policy.yaml` is created and has specific content. After removal, these tests must be updated or removed.
**Why it happens:** Tests coupled to the old behavior.
**How to avoid:** Identify all tests that assert on policy.yaml:
  - `init_creates_default_agent_files` -- asserts `policy.yaml` exists
  - `init_without_telegram_uses_base_policy` -- reads policy content
  - `init_with_telegram_uses_telegram_policy` -- reads policy content
  - `init_creates_bootstrap_md` -- unrelated (no change)
  Remove the policy.yaml assertions from tests. The `init_creates_default_agent_files` test should assert `policy.yaml` does NOT exist.
**Warning signs:** Test failures mentioning "policy.yaml not found."

### Pitfall 5: doctor.rs Agent Validation Still Checking policy.yaml
**What goes wrong:** `check_agent_structure()` in doctor.rs checks for both `identity_exists` and `policy_exists` to determine valid agents. After Phase 5, only IDENTITY.md is required.
**Why it happens:** doctor.rs has its own agent validation logic separate from discovery.rs.
**How to avoid:** Update `check_agent_structure()` to check only `identity_exists`. Remove `policy_exists` variable and its usage. Update tests:
  - `run_doctor_with_valid_agent_reports_pass` -- stop creating policy.yaml
  - `run_doctor_reports_missing_required_files` -- change assertion (policy.yaml no longer required)
  - `run_doctor_always_checks_all_four_binaries` -- update to expect 3 binaries (no openshell)
**Warning signs:** `rightclaw doctor` failing on agents without policy.yaml.

### Pitfall 6: sandbox.rs Still Needed for RuntimeState
**What goes wrong:** D-13 says "sandbox.rs module removed entirely," but `RuntimeState`, `write_state`, and `read_state` live in sandbox.rs. These are still needed.
**Why it happens:** The module name "sandbox" is misleading -- it also contains the runtime state persistence logic.
**How to avoid:** Rename or restructure: either (a) keep the file but rename to `state.rs`, (b) move `RuntimeState`/`write_state`/`read_state` to `runtime/mod.rs` or a new `runtime/state.rs` before deleting sandbox.rs, or (c) strip sandbox functions from sandbox.rs and keep only state functions. Option (b) is cleanest -- create `runtime/state.rs` with the simplified structs and functions, then delete `sandbox.rs`.
**Warning signs:** Compile errors when trying to use `read_state`/`write_state` after sandbox.rs deletion.

## Code Examples

### Simplified `generate_wrapper` Signature (After Phase 5)
```rust
// Source: crates/rightclaw/src/codegen/shell_wrapper.rs (proposed)
pub fn generate_wrapper(
    agent: &AgentDef,
    combined_prompt_path: &str,
    debug_log_path: Option<&str>,
) -> miette::Result<String> {
    // no_sandbox and policy_path removed from context
    let channels: Option<&str> = if agent.mcp_config_path.is_some() {
        Some("plugin:telegram@claude-plugins-official")
    } else {
        None
    };
    let model = agent.config.as_ref().and_then(|c| c.model.as_deref());
    let startup_prompt = "Run /rightcron ...";

    tmpl.render(context! {
        agent_name => agent.name,
        working_dir => agent.path.display().to_string(),
        combined_prompt_path => combined_prompt_path,
        channels => channels,
        model => model,
        startup_prompt => startup_prompt,
        debug => debug_log_path.is_some(),
        debug_log_path => debug_log_path.unwrap_or_default(),
    })
    .map_err(|e| miette::miette!("template render error: {e:#}"))
}
```

### Simplified Shell Wrapper Template (After Phase 5)
```bash
#!/usr/bin/env bash
# Generated by rightclaw -- do not edit
# Agent: {{ agent_name }}
set -euo pipefail

# Resolve claude binary (claude or claude-bun)
CLAUDE_BIN=""
for bin in claude claude-bun; do
  if command -v "$bin" &>/dev/null; then
    CLAUDE_BIN="$bin"
    break
  fi
done
if [ -z "$CLAUDE_BIN" ]; then
  echo "error: claude CLI not found in PATH (tried: claude, claude-bun)" >&2
  exit 1
fi

exec "$CLAUDE_BIN" \
  --append-system-prompt-file "{{ combined_prompt_path }}" \
  --dangerously-skip-permissions \
  {% if model %}--model {{ model }} \
  {% endif %}{% if debug %}--debug-file "{{ debug_log_path }}" \
  {% endif %}{% if channels %}--channels {{ channels }} \
  {% endif %}{% if startup_prompt %}-- "{{ startup_prompt }}"
  {% else %}
  {% endif %}
```

### Simplified `cmd_down` (After Phase 5)
```rust
// Source: crates/rightclaw-cli/src/main.rs (proposed)
async fn cmd_down(home: &Path) -> miette::Result<()> {
    let run_dir = home.join("run");
    let state_path = run_dir.join("state.json");

    // Verify instance exists (read state for validation only)
    let _state = rightclaw::runtime::read_state(&state_path).map_err(|_| {
        miette::miette!("No running instance found. Is rightclaw running?")
    })?;

    // Shutdown via REST API
    match rightclaw::runtime::PcClient::new(rightclaw::runtime::PC_PORT) {
        Ok(client) => {
            if let Err(e) = client.shutdown().await {
                tracing::warn!("process-compose shutdown failed: {e:#}");
            }
        }
        Err(e) => {
            tracing::warn!("could not connect to process-compose: {e:#}");
        }
    }

    // No sandbox cleanup needed -- just done.
    println!("All agents stopped.");
    Ok(())
}
```

### Simplified `verify_dependencies` (After Phase 5)
```rust
// Source: crates/rightclaw/src/runtime/deps.rs (proposed)
pub fn verify_dependencies() -> miette::Result<()> {
    which::which("process-compose").map_err(|_| {
        miette::miette!(
            help = "Install: https://f1bonacc1.github.io/process-compose/installation/",
            "process-compose not found in PATH"
        )
    })?;

    find_binary(&["claude", "claude-bun"]).map_err(|_| {
        miette::miette!(
            help = "Install Claude Code CLI: https://docs.anthropic.com/en/docs/claude-code",
            "claude not found in PATH (tried: claude, claude-bun)"
        )
    })?;

    // openshell check: REMOVED (D-14)
    // bubblewrap/socat checks: added in Phase 7

    Ok(())
}
```

## State of the Art

| Old Approach (v1.0) | New Approach (Phase 5) | Impact |
|---------------------|------------------------|--------|
| `openshell sandbox create -- claude` | `exec "$CLAUDE_BIN"` directly | Simpler wrapper, no OpenShell dependency |
| `destroy_sandboxes()` on shutdown | No cleanup needed | Simpler shutdown path |
| `policy.yaml` required per agent | Only `IDENTITY.md` required | Simpler agent setup |
| `verify_dependencies()` checks openshell | Only checks process-compose + claude | Fewer external dependencies |
| `RuntimeState` tracks sandbox names + no_sandbox | Just agent names + socket + timestamp | Simpler state |

**Deprecated/removed:**
- `sandbox_name_for()` -- no sandbox naming needed
- `AgentState.sandbox_name` -- no sandbox identity tracking
- `RuntimeState.no_sandbox` -- no sandbox toggle state
- All `policy.yaml` templates -- no OpenShell policies
- All `openshell` invocations in wrapper template

## Detailed File Change Inventory

### Files to DELETE (4 files)
| File | Lines | Content |
|------|-------|---------|
| `templates/right/policy.yaml` | 177 | OpenShell base policy template |
| `templates/right/policy-telegram.yaml` | 186 | OpenShell Telegram policy template |
| `crates/rightclaw/src/runtime/sandbox_tests.rs` | 67 | Tests for sandbox state (some tests move to state.rs) |

Note: `sandbox.rs` is not deleted -- it is restructured into `state.rs` (see Pitfall 6).

### Files to MODIFY (11 files)
| File | Lines | Key Changes |
|------|-------|------------|
| `crates/rightclaw/src/agent/types.rs` | 141 | Remove `policy_path: PathBuf` from `AgentDef` |
| `crates/rightclaw/src/agent/discovery.rs` | 151 | Remove policy.yaml existence check, remove policy_path from AgentDef construction |
| `crates/rightclaw/src/agent/discovery_tests.rs` | 233 | Remove policy.yaml from test fixtures, update error assertions |
| `crates/rightclaw/src/runtime/sandbox.rs` -> rename to `state.rs` | 83 | Remove `sandbox_name` from AgentState, remove `no_sandbox` from RuntimeState, delete `sandbox_name_for()`, delete `destroy_sandboxes()` |
| `crates/rightclaw/src/runtime/mod.rs` | 7 | Change `sandbox` -> `state` module, update re-exports |
| `crates/rightclaw/src/runtime/deps.rs` | 79 | Remove `no_sandbox` parameter, remove openshell check |
| `crates/rightclaw/src/codegen/shell_wrapper.rs` | 52 | Remove `no_sandbox` param, remove `policy_path` from context |
| `crates/rightclaw/src/codegen/shell_wrapper_tests.rs` | 213 | Remove sandbox-variant tests, remove policy_path from helpers |
| `crates/rightclaw/src/doctor.rs` | 364 | Remove openshell binary check, update agent validation to not require policy.yaml |
| `crates/rightclaw/src/init.rs` | 559 | Remove policy template constants, remove policy file creation, remove path expansion |
| `crates/rightclaw-cli/src/main.rs` | 526 | Remove sandbox state from cmd_up, remove destroy_sandboxes from cmd_down, update verify_dependencies call |
| `templates/agent-wrapper.sh.j2` | 45 | Remove `{% if not no_sandbox %}` openshell block, keep direct claude path |

## Open Questions

1. **sandbox.rs rename to state.rs**
   - What we know: `sandbox.rs` contains both sandbox functions (to be deleted) and state persistence functions (to be kept). D-13 says "sandbox.rs module removed entirely" which conflicts with keeping state functions.
   - What's unclear: Whether the user intended "move state functions elsewhere then delete" or "remove the sandbox-specific functions and keep the file."
   - Recommendation: Rename `sandbox.rs` to `state.rs` and move only the state functions. This aligns with D-13 (sandbox.rs no longer exists) while preserving needed functionality. Update `runtime/mod.rs` exports accordingly. Existing sandbox_tests.rs tests for `write_state`/`read_state` roundtrips should move to `state_tests.rs`.

2. **`Down` command doc comment**
   - What we know: The `Down` variant's doc comment says "Stop all agents and destroy sandboxes."
   - Recommendation: Update to "Stop all agents" -- trivial but easy to miss.

## Sources

### Primary (HIGH confidence)
- Codebase inspection of all 14 affected files (read via Read tool)
- `.planning/phases/05-remove-openshell/05-CONTEXT.md` -- 18 locked decisions
- `.planning/research/ARCHITECTURE.md` -- v2.0 component-level changes
- `.planning/research/SUMMARY.md` -- synthesis and build order

### Secondary (MEDIUM confidence)
- `.planning/REQUIREMENTS.md` -- SBMG-01 through SBMG-05 requirement definitions

## Metadata

**Confidence breakdown:**
- Standard stack: HIGH -- no new dependencies, pure removal
- Architecture: HIGH -- compiler-guided deletion, all affected files identified and read
- Pitfalls: HIGH -- 6 specific pitfalls identified from actual code inspection, particularly the sandbox.rs/state.rs rename issue

**Research date:** 2026-03-24
**Valid until:** No expiry -- this is a snapshot of codebase state for a deletion phase
