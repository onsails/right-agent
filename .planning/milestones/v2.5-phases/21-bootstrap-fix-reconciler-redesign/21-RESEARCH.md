# Phase 21: Bootstrap Fix + Reconciler Redesign - Research

**Researched:** 2026-03-29
**Domain:** Rust constant edit + Markdown skill redesign
**Confidence:** HIGH

## Summary

Phase 21 is two tightly scoped changes. First, a one-line Rust constant edit in `shell_wrapper.rs` removes the "Use the Agent tool" prefix so rightcron runs inline on the main thread. Second, the skill source at `skills/cronsync/SKILL.md` is restructured to make the reconciler algorithm more explicit and guard against Agent tool delegation for CronCreate/CronDelete/CronList calls.

The root cause (confirmed in Phase 20 diagnosis) is that CronCreate is a main-thread-only tool — background agents dispatched via the Agent tool cannot see it. The startup_prompt currently tells CC to "Use the Agent tool to run this in the background", which means the bootstrap agent never finds CronCreate and the reconciler job is never scheduled.

Both fixes are small in scope but require TDD discipline per CLAUDE.md: write failing tests first for the startup_prompt change, then apply the fix.

**Primary recommendation:** Edit `skills/cronsync/SKILL.md` (source) and `shell_wrapper.rs` (constant). Do NOT touch the installed runtime copy at `~/.rightclaw/agents/right/.claude/skills/rightcron/SKILL.md`.

<user_constraints>
## User Constraints (from CONTEXT.md)

### Locked Decisions

- **D-01:** New startup_prompt (replaces current string in `shell_wrapper.rs` line ~54):
  ```
  "Run /rightcron to bootstrap the cron reconciler. Create crons/ directory if missing, schedule the reconciler job, and recover any persisted cron specs. Do this silently without messaging the user."
  ```
  Remove "Use the Agent tool to run this in the background:" prefix AND remove the "IMPORTANT: run this as a background agent so the main thread stays free for incoming messages." note entirely.

- **D-02:** The new prompt runs rightcron INLINE (main thread). Bootstrap is ~5-10 seconds — acceptable. After bootstrap completes, the `*/5 * * * *` reconciler job exists and CronFire will call M6() every 5 minutes.

- **D-03:** The cron still fires `Run /rightcron reconcile` (unchanged). The reconciler is internally split into two steps within the SAME conversation turn:
  1. CHECK step — Read desired state (YAML files) + actual state (CronList) + tracked state (state.json). Compute diff internally. NO CronCreate/CronDelete calls in this step.
  2. RECONCILE step — Based on computed diff, call CronCreate/CronDelete directly in this conversation. Write updated state.json. Report.

- **D-04:** SKILL.md restructures Reconciliation Algorithm into two named sections:
  - "Step A: Compute diff (CHECK)" — steps 1-3 from current algorithm, plus diff computation
  - "Step B: Apply changes (RECONCILE)" — step 4 from current algorithm (CronCreate/CronDelete), plus steps 5-6

- **D-05:** Add a prominent CRITICAL constraint at the top of the Reconciliation Algorithm section:
  ```
  CRITICAL: NEVER use the Agent tool for CronCreate, CronDelete, or CronList calls. These
  tools are only available in the main conversation thread. Any delegation to a background
  agent will silently fail — CronCreate will not be found by the background agent's ToolSearch.
  All cron tool calls MUST happen directly in this conversation turn.
  ```

- **D-06:** Bootstrap step 2 ("Schedule the reconciler job via CronCreate") should add a note: "Call CronCreate directly in this turn — do NOT use the Agent tool."

- **D-07:** Write failing tests FIRST for the startup_prompt change before editing the constant.
  - Test: `startup_prompt_does_not_use_agent_tool` — assert `!startup_prompt.contains("Agent tool")`
  - Test: `startup_prompt_invokes_rightcron` — assert `startup_prompt.contains("/rightcron")`
  - File: `crates/rightclaw/src/codegen/shell_wrapper_tests.rs`

### Claude's Discretion

None stated — all decisions are locked.

### Deferred Ideas (OUT OF SCOPE)

- Document CC gotcha "Telegram messages dropped while agent is streaming"
- Making startup_prompt configurable per-agent via agent.yaml
- Multi-agent cron isolation
</user_constraints>

<phase_requirements>
## Phase Requirements

| ID | Description | Research Support |
|----|-------------|------------------|
| BOOT-01 | `rightclaw up` results in a `*/5 * * * *` reconciler cron job existing in the agent session | Inline startup_prompt enables bootstrap to call CronCreate on main thread |
| BOOT-02 | `startup_prompt` does not delegate to a background Agent tool — rightcron runs inline | One-line constant change at `shell_wrapper.rs` line 54 |
| RECON-01 | rightcron skill separates reconciler into CHECK (read-only, no CronCreate/CronDelete) and RECONCILE (direct calls in main thread) | SKILL.md restructuring of Steps 1-6 into Step A + Step B |
| RECON-02 | After cron fires, jobs defined in `crons/*.yaml` are created/updated/deleted correctly without any Agent tool delegation | CRITICAL guard + CHECK/RECONCILE split in SKILL.md |
</phase_requirements>

## Standard Stack

No new dependencies. Both changes use what already exists.

### Files to Modify

| File | Type | Change |
|------|------|--------|
| `crates/rightclaw/src/codegen/shell_wrapper_tests.rs` | Rust tests | Add 2 failing tests (TDD first) |
| `crates/rightclaw/src/codegen/shell_wrapper.rs` | Rust source | Change `startup_prompt` constant at line 54 |
| `skills/cronsync/SKILL.md` | Markdown skill | Restructure Reconciliation Algorithm section |

## Architecture Patterns

### How the Skill Source Gets Deployed

`skills/cronsync/SKILL.md` is embedded at compile time via `include_str!` in `skills.rs`:

```rust
// crates/rightclaw/src/codegen/skills.rs line 4
const SKILL_RIGHTCRON: &str = include_str!("../../../../skills/cronsync/SKILL.md");
```

`install_builtin_skills(agent_path)` then writes this constant to `<agent_path>/.claude/skills/rightcron/SKILL.md`. It is called during `rightclaw up` for each agent. This means:

- **Edit the source** at `skills/cronsync/SKILL.md`
- The compile-time `include_str!` picks it up on next `cargo build`
- `rightclaw up` overwrites every agent's installed copy on every launch (always-overwrite, not create-if-absent)
- The runtime copy at `~/.rightclaw/agents/right/.claude/skills/rightcron/SKILL.md` is NOT the source of truth — editing it manually is pointless; it gets overwritten on next `rightclaw up`

### startup_prompt Pattern

The constant is a plain `&str` at line 54 of `shell_wrapper.rs`. It is passed into the minijinja template as the `startup_prompt` variable and rendered into the shell wrapper script as a positional argument to `claude`.

Current value (line 54):
```rust
let startup_prompt = "Use the Agent tool to run this in the background: Run /rightcron to bootstrap the cron reconciler. Create crons/ directory if missing, schedule the reconciler job, and recover any persisted cron specs. Do this silently without messaging the user. IMPORTANT: run this as a background agent so the main thread stays free for incoming messages.";
```

Tests in `shell_wrapper_tests.rs` assert on `output.contains("some string")` patterns, where `output` is the full rendered shell script. The new TDD tests should assert on the startup_prompt content directly — but since the constant isn't exported, tests must assert on the rendered wrapper output containing (or not containing) the target strings.

### Test Helper Signature

```rust
fn make_agent(name: &str, start_prompt: Option<&str>) -> AgentDef
```

The `start_prompt` parameter is the agent-level start prompt from `agent.yaml`, not the hardcoded `startup_prompt` constant. Tests call `generate_wrapper(&agent, DUMMY_PROMPT_PATH, None)` and assert on the returned `String`. New tests for startup_prompt follow this same pattern — no new helper needed.

### SKILL.md Reconciliation Algorithm Restructure

Current structure (Steps 1-6 flat):
- Step 1: Read desired state
- Step 2: Read actual state
- Step 3: Read tracked state
- Step 4: Reconcile (CronCreate/CronDelete calls)
- Step 5: Write updated state.json
- Step 6: Report

Target structure (CHECK + RECONCILE split):
```
CRITICAL: NEVER use the Agent tool for CronCreate/CronDelete/CronList
  [see D-05 for exact wording]

Step A: Compute diff (CHECK)
  - Step 1: Read desired state (YAML files)
  - Step 2: Read actual state (CronList) — no CronCreate/CronDelete here
  - Step 3: Read tracked state (state.json)
  - Compute diff: new jobs, changed jobs, unchanged jobs, orphaned jobs

Step B: Apply changes (RECONCILE)
  - Step 4: For each diff entry, call CronCreate/CronDelete directly in this turn
  - Step 5: Write updated state.json
  - Step 6: Report
```

Bootstrap section step 2 gets inline note per D-06:
> "Call CronCreate directly in this turn — do NOT use the Agent tool."

## Don't Hand-Roll

| Problem | Don't Build | Use Instead |
|---------|-------------|-------------|
| Skill deployment | Manual file copy commands | `install_builtin_skills()` — already handles always-overwrite + idempotency |
| Shell escaping in startup_prompt | Custom escaping | minijinja template already handles it |

## Common Pitfalls

### Pitfall 1: Editing Runtime Copy Instead of Source
**What goes wrong:** Editor opens `~/.rightclaw/agents/right/.claude/skills/rightcron/SKILL.md` (the installed copy). Change is visible immediately but gets overwritten on next `rightclaw up`.
**Why it happens:** The CONTEXT.md noted "check actual path — may be in crates/rightclaw/src/ or top-level skills/". Verified: source is `skills/cronsync/SKILL.md` (top-level), not inside `crates/`.
**How to avoid:** Always edit `skills/cronsync/SKILL.md`. The installed copy is derived.

### Pitfall 2: Skill Name Mismatch
**What goes wrong:** Source dir is `skills/cronsync/` but installed path is `.claude/skills/rightcron/SKILL.md`.
**Why it happens:** `skills.rs` maps `"rightcron/SKILL.md"` to the `SKILL_RIGHTCRON` constant which is loaded from `skills/cronsync/SKILL.md`. The source dir name (`cronsync`) differs from the install name (`rightcron`).
**How to avoid:** Edit `skills/cronsync/SKILL.md` as the source. The install mapping in `skills.rs` is correct and does not need to change.

### Pitfall 3: Tests Assert on startup_prompt String But It's Not Exported
**What goes wrong:** Tests try to import or reference `startup_prompt` directly but it's a local `let` binding inside `generate_wrapper()`.
**Why it happens:** The constant isn't a module-level const, it's defined inline in the function body.
**How to avoid:** Tests must call `generate_wrapper(...)` and assert on the rendered output string. The rendered output will contain the startup_prompt verbatim. `output.contains("Agent tool")` and `output.contains("/rightcron")` work correctly.

### Pitfall 4: Forgetting to Rebuild After SKILL.md Edit
**What goes wrong:** `skills/cronsync/SKILL.md` is edited but old compiled binary still has the old content.
**Why it happens:** `include_str!` embeds content at compile time — file change doesn't affect a cached binary.
**How to avoid:** Run `cargo build --workspace` after editing the SKILL.md source. The planner should include a build step after SKILL.md edit.

## Code Examples

### Current startup_prompt (to be replaced)
From `crates/rightclaw/src/codegen/shell_wrapper.rs` line 54:
```rust
let startup_prompt = "Use the Agent tool to run this in the background: Run /rightcron to bootstrap the cron reconciler. Create crons/ directory if missing, schedule the reconciler job, and recover any persisted cron specs. Do this silently without messaging the user. IMPORTANT: run this as a background agent so the main thread stays free for incoming messages.";
```

### New startup_prompt (per D-01)
```rust
let startup_prompt = "Run /rightcron to bootstrap the cron reconciler. Create crons/ directory if missing, schedule the reconciler job, and recover any persisted cron specs. Do this silently without messaging the user.";
```

### Failing test patterns (TDD first, per D-07)
```rust
#[test]
fn startup_prompt_does_not_use_agent_tool() {
    let agent = make_agent("testbot", None);
    let output = generate_wrapper(&agent, DUMMY_PROMPT_PATH, None).unwrap();
    assert!(
        !output.contains("Agent tool"),
        "startup_prompt must NOT delegate to Agent tool:\n{output}"
    );
}

#[test]
fn startup_prompt_invokes_rightcron() {
    let agent = make_agent("testbot", None);
    let output = generate_wrapper(&agent, DUMMY_PROMPT_PATH, None).unwrap();
    assert!(
        output.contains("/rightcron"),
        "startup_prompt must invoke /rightcron:\n{output}"
    );
}
```

### skills.rs install mapping (reference only — no change needed)
From `crates/rightclaw/src/codegen/skills.rs` lines 3-4:
```rust
const SKILL_RIGHTSKILLS: &str = include_str!("../../../../skills/rightskills/SKILL.md");
const SKILL_RIGHTCRON: &str   = include_str!("../../../../skills/cronsync/SKILL.md");
```

## Environment Availability

Step 2.6: SKIPPED (no external dependencies — phase is pure code and Markdown edits with existing toolchain).

## Open Questions

None. All implementation details are fully specified in CONTEXT.md decisions D-01 through D-07.

## Sources

### Primary (HIGH confidence)
- Direct file inspection: `crates/rightclaw/src/codegen/shell_wrapper.rs` — startup_prompt location confirmed at line 54
- Direct file inspection: `crates/rightclaw/src/codegen/skills.rs` — `include_str!("../../../../skills/cronsync/SKILL.md")` confirms source path
- Direct file inspection: `crates/rightclaw/src/codegen/shell_wrapper_tests.rs` — test patterns, `make_agent()` signature confirmed
- Direct file inspection: `skills/cronsync/SKILL.md` — full current skill content for restructuring reference
- Direct file inspection: `21-CONTEXT.md` — all decisions locked

## Metadata

**Confidence breakdown:**
- Standard stack: HIGH — no new deps, all existing code
- Architecture: HIGH — source/install mapping verified by code inspection
- Pitfalls: HIGH — sourced from direct code reading, not inference

**Research date:** 2026-03-29
**Valid until:** Stable — these are internal source files with no external version drift risk
