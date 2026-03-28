# Phase 21: Bootstrap Fix + Reconciler Redesign - Context

**Gathered:** 2026-03-28
**Status:** Ready for planning

<domain>
## Phase Boundary

Two changes delivered together:
1. Rust code: change `startup_prompt` constant in `shell_wrapper.rs` so rightcron bootstrap runs inline (no Agent tool delegation).
2. SKILL.md redesign: split reconciler into CHECK + RECONCILE in same conversation turn, add CRITICAL constraint against Agent tool use for CronCreate/CronDelete/CronList.

NOT in scope: changing reconciler interval, rightcron conversational job creation/removal, any changes to lock file logic, state.json format, or YAML spec format.

</domain>

<decisions>
## Implementation Decisions

### startup_prompt (BOOT-02)

- **D-01:** New startup_prompt (replaces the current string in `shell_wrapper.rs` line ~54):
  ```
  "Run /rightcron to bootstrap the cron reconciler. Create crons/ directory if missing, schedule the reconciler job, and recover any persisted cron specs. Do this silently without messaging the user."
  ```
  Remove "Use the Agent tool to run this in the background:" prefix AND remove the "IMPORTANT: run this as a background agent so the main thread stays free for incoming messages." note entirely.

- **D-02:** The new prompt runs rightcron INLINE (main thread). Bootstrap is ~5-10 seconds — acceptable. After bootstrap completes, the `*/5 * * * *` reconciler job exists and CronFire will call M6() every 5 minutes.

### Reconciler split (RECON-01)

- **D-03:** The cron still fires `Run /rightcron reconcile` (unchanged). The reconciler is internally split into two steps within the SAME conversation turn:
  1. **CHECK step** — Read desired state (YAML files) + actual state (CronList) + tracked state (state.json). Compute a diff internally. NO CronCreate/CronDelete calls in this step.
  2. **RECONCILE step** — Based on the computed diff, call CronCreate/CronDelete directly in this conversation. Write updated state.json. Report.

- **D-04:** The SKILL.md should restructure the Reconciliation Algorithm into two named sections:
  - "Step A: Compute diff (CHECK)" — steps 1-3 from current algorithm, plus diff computation
  - "Step B: Apply changes (RECONCILE)" — step 4 from current algorithm (CronCreate/CronDelete), plus steps 5-6

### SKILL.md guard rule (RECON-01, RECON-02)

- **D-05:** Add a prominent CRITICAL constraint at the top of the Reconciliation Algorithm section:
  ```
  CRITICAL: NEVER use the Agent tool for CronCreate, CronDelete, or CronList calls. These
  tools are only available in the main conversation thread. Any delegation to a background
  agent will silently fail — CronCreate will not be found by the background agent's ToolSearch.
  All cron tool calls MUST happen directly in this conversation turn.
  ```

### Bootstrap section in SKILL.md

- **D-06:** Bootstrap step 2 ("Schedule the reconciler job via CronCreate") should add a note:
  "Call CronCreate directly in this turn — do NOT use the Agent tool."
  This reinforces D-05 specifically at the call site.

### TDD (per CLAUDE.md)

- **D-07:** Write failing tests FIRST for the startup_prompt change before editing the constant.
  Test: `startup_prompt_does_not_use_agent_tool` — assert `!startup_prompt.contains("Agent tool")`.
  Test: `startup_prompt_invokes_rightcron` — assert `startup_prompt.contains("/rightcron")`.
  File: `crates/rightclaw/src/codegen/shell_wrapper_tests.rs`

</decisions>

<canonical_refs>
## Canonical References

**Downstream agents MUST read these before planning or implementing.**

### Files to modify
- `crates/rightclaw/src/codegen/shell_wrapper.rs` — `startup_prompt` constant (~line 54)
- `crates/rightclaw/src/codegen/shell_wrapper_tests.rs` — add failing tests before fix
- `/home/wb/.rightclaw/agents/right/.claude/skills/rightcron/SKILL.md` — reconciler redesign

### Context
- `.planning/phases/20-diagnosis/DIAGNOSIS.md` (archived at `milestones/v2.4-phases/20-diagnosis/DIAGNOSIS.md`) — root cause: CronCreate is main-thread-only, background agents can't access it
- `.planning/REQUIREMENTS.md` — BOOT-01, BOOT-02, RECON-01, RECON-02

### SKILL.md location note
The rightcron skill is installed at runtime into each agent's `.claude/skills/rightcron/SKILL.md`.
The source is in the rightclaw repo at: `skills/rightcron/SKILL.md` (check actual path — may be `crates/rightclaw/src/` or a top-level `skills/` directory).
Downstream agents must find and edit the SOURCE skill, not the installed runtime copy.

</canonical_refs>

<code_context>
## Existing Code Insights

### startup_prompt location
`shell_wrapper.rs` line ~54:
```rust
let startup_prompt = "Use the Agent tool to run this in the background: Run /rightcron...";
```
Single constant string. The fix is a one-line change.

### Test file pattern
`shell_wrapper_tests.rs` uses a `make_agent()` helper to construct test fixtures.
Existing tests assert on `generated_wrapper.contains("some string")` patterns.
New tests follow the same pattern.

### SKILL.md source location
The installed skill at `~/.rightclaw/agents/right/.claude/skills/rightcron/SKILL.md` is a copy.
The source that gets installed needs to be found and updated. Check:
- `skills/rightcron/SKILL.md` (top-level)
- Or wherever `cmd_up`'s `install_builtin_skills()` reads from

</code_context>

<specifics>
## Specific Ideas

- The new startup_prompt is intentionally minimal — no threading commentary, no IMPORTANT notes. The simpler the better; the LLM just runs /rightcron directly.
- The CRITICAL guard in SKILL.md should be placed BEFORE the reconciliation algorithm steps, not buried at the end. It's the most important constraint.
- The CHECK + RECONCILE split makes the algorithm clearer to read even if the behavior is unchanged in practice — "first compute what needs doing, then do it."

</specifics>

<deferred>
## Deferred Ideas

- Document CC gotcha todo ("Telegram messages dropped while agent is streaming") — low relevance to this phase, keep pending
- Making startup_prompt configurable per-agent via agent.yaml (currently hardcoded) — future milestone
- Multi-agent cron isolation — future milestone

</deferred>

---

*Phase: 21-bootstrap-fix-reconciler-redesign*
*Context gathered: 2026-03-28*
