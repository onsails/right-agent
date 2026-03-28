# Phase 21: Fix & Verification - Research

**Researched:** 2026-03-28
**Domain:** Claude Code subagent model selection, deferred tool availability in subagent context, startup_prompt fix strategy
**Confidence:** HIGH

---

<user_constraints>
## User Constraints (from CONTEXT.md)

### Locked Decisions

- **D-01:** Root cause is CC's `iv6` subscriber not calling `M6()` when idle after SubagentStop.
  The fix cannot patch CC internals — it must work around the gap from rightclaw's side.
- **D-02:** The intended workaround already exists: rightcron's `*/5 * * * *` reconciler job
  (created via CronCreate during bootstrap) fires every 5 minutes → `M6()` called → Telegram
  messages drained. The reconciler job was NOT being created because CronCreate was unavailable
  to the background sub-agent.
- **D-03:** Debug log shows `ToolSearchTool: select failed — none found: CronCreate, CronList, CronDelete`
  during the background agent's rightcron bootstrap (line 458, session 21:21-21:22).
- **D-04:** Earlier in the log (line 154): `Tool search disabled for model 'claude-haiku-4-5-20251001'`
  fires during session init — before the first real API call.
- **D-05:** OPEN QUESTION — research must confirm: is the Haiku model coming from (a) CC's default
  for Agent tool sub-agents regardless of parent model, (b) a hook system that runs on Haiku, or
  (c) `--model sonnet` in agent.yaml not resolving correctly to claude-sonnet-4-6?
  The fix strategy depends on which of these is true.
- **D-06:** The fix is to ensure the background sub-agent that runs rightcron bootstrap uses Sonnet
  (or at minimum a model that supports tool_reference blocks / deferred tools). Once it can find
  CronCreate, it will create the `*/5 * * * *` reconciler job and the freeze is resolved.
- **D-07:** Implementation options to evaluate — see Section "Fix Strategy" below.
- **D-08:** Fallback — if ensuring Sonnet still doesn't make CronCreate available, change
  `startup_prompt` so the background agent loops rather than exits.
- **D-09:** Pass criterion: send a Telegram message after rightcron bootstrap completes, receive
  a response within 5 minutes.
- **D-10:** Confirmation Test A from DIAGNOSIS.md is the primary test.
- **D-11:** 5 minutes is acceptable for the verification test.
- **D-12:** Regression: existing behavior (messages sent during rightcron run) must still work.

### Claude's Discretion

Implementation option selection (D-07: A, B, or C) based on what research reveals.

### Deferred Ideas (OUT OF SCOPE)

- Requirements text update (DIAG-02 sandbox specificity, DIAG-03 stale element names)
- Filing CC bug report for the iv6/M6 gap
</user_constraints>

<phase_requirements>
## Phase Requirements

| ID | Description | Research Support |
|----|-------------|------------------|
| FIX-01 | Telegram commands receive responses from agent when sandbox is enabled | Fix startup_prompt to ensure reconciler job is created via CronCreate |
| FIX-02 | Fix does not regress --no-sandbox behavior or existing test suite | Startup_prompt change is additive; shell_wrapper_tests.rs needs updating |
| VERIFY-01 | Manual end-to-end test: send Telegram message → agent responds with sandbox on | D-10 Confirmation Test A protocol |
</phase_requirements>

---

## Summary

Research resolved D-05 definitively: the `claude-haiku-4-5-20251001` line in the debug log (line 154) is a **red herring** unrelated to the subagent model. It fires during quota check preparation — CC's `uqY()` function uses `nH()` (the small fast model = haiku) for quota checks before the first real API call. The tool_reference check runs as part of that preparation and logs "disabled" because haiku doesn't support deferred tools. This is normal behavior and does not affect the subagent.

The background sub-agent spawned via the Agent tool **inherits the parent model** (claude-sonnet-4-6). CC's `Ik6()` function confirms: when the Agent tool input specifies no `model` param and the general-purpose agent definition has no `model` field, it calls `iI()` which returns `mainLoopModel` — the parent's model. So the subagent IS sonnet. And `--model sonnet` in the wrapper template resolves correctly: `rK("sonnet")` → `ST()` → `claude-sonnet-4-6`.

The actual failure is different: `ToolSearchTool: select failed — none found: CronCreate, CronList, CronDelete`. Tool search ran (sonnet supports it) but the Cron tools were not returned in results. This indicates that CronCreate/CronList/CronDelete (CC built-in deferred tools with `shouldDefer: true`) are **not available in the subagent's tool list** — they are only available in the main session context. Deferred tool search in a subagent looks at the tools passed to it by the parent; if the parent didn't pass the Cron tools to the subagent, they won't appear in its search results.

**Primary recommendation:** Change `startup_prompt` so rightcron bootstrap runs in the main thread directly (not via the Agent tool), or use the Agent tool but ensure the subagent can access Cron tools. The simplest correct fix is to remove "Use the Agent tool" from the startup_prompt and let rightcron run synchronously in the main thread — which keeps the step cycle running anyway and doesn't have the subagent tool-isolation problem.

---

## D-05 Answer: Root Cause of the Haiku Log Line

**Source:** CC cli.js (cc_version 2.1.86), lines 277, 1633, 4396, 7675

Line 154 fires from this chain:
1. CC starts, prepares the first message (startup_prompt)
2. Before the first API call, CC runs `uqY()` — a quota check using `nH()` = `aW6()` = `claude-haiku-4-5-20251001`
3. During quota check preparation, `isToolSearchEnabled("claude-haiku-4-5-20251001", ...)` runs
4. `c68("claude-haiku-4-5-20251001")` checks: is haiku in the unsupported model list? YES → returns false
5. "Tool search disabled for model 'claude-haiku-4-5-20251001'" logged

This is a **quota check side effect**, not the subagent model. The subagent model is sonnet.

**Evidence:**
- Line 140: `model=claude-sonnet-4-6` (main session auto-mode check)
- Line 154: haiku log fires
- Line 160: `source=quota_check` first API call (uses haiku by design)
- Lines 437, 452, 468: `source=agent:builtin:general-purpose` (subagent calls — no haiku log)

---

## Root Cause of CronCreate Failure

**Source:** CC cli.js lines 1392, 3866, 4033, 2685

`CronCreate`, `CronList`, `CronDelete` are CC built-in deferred tools (`shouldDefer: true`). In subagent sessions, deferred tools are only available if they were included in the tools passed to the subagent by the parent. The ToolSearch ran (tool_reference blocks work because the subagent is sonnet), but the Cron tools were absent from its search index.

**Why Cron tools are absent from subagent context:**

The general-purpose agent definition has `tools: ["*"]` — all tools the parent has access to. However, CronCreate has a specific lifecycle: it's tied to the **main session's task scheduler** (`ScheduledTasks` component). When a subagent runs, it gets a copy of the parent's tool list at the time of spawning, but the Cron tools may be filtered because:

1. CronCreate `isEnabled()` returns `wk()` = `!n6(process.env.CLAUDE_CODE_DISABLE_CRON)` — enabled globally.
2. BUT: Cron tools are only meaningful for the main session's scheduler. CC may not expose them inside subagent contexts to prevent subagents from scheduling tasks in the outer session.

The debug log confirms: the subagent ran ToolSearch and found `mcp__plugin_telegram_telegram__reply` (line 470) but not CronCreate. MCP tools are found; built-in deferred Cron tools are not. This is a subagent context restriction.

**Conclusion:** The fix must not rely on the subagent calling CronCreate. The fix must run rightcron bootstrap in the **main thread** where Cron tools are available.

---

## Fix Strategy

**D-05 is resolved: the model is NOT the problem.** Options A and B from CONTEXT.md D-07 are based on a misdiagnosis. The correct fix is:

### Correct Fix: Remove Agent Tool From startup_prompt

The `startup_prompt` currently says:
```
"Use the Agent tool to run this in the background: Run /rightcron to bootstrap..."
```

This spawns a subagent that lacks access to CronCreate. The fix is to remove "Use the Agent tool" so rightcron runs in the **main thread**:

```
"Run /rightcron to bootstrap the cron reconciler. Create crons/ directory if missing, schedule the reconciler job, and recover any persisted cron specs. Do this silently without messaging the user."
```

This runs rightcron synchronously in the main thread where CronCreate IS available. It also:
- Keeps the step cycle running during bootstrap (so channel messages received during bootstrap are processed)
- After bootstrap completes, the `*/5 * * * *` reconciler job is created → M6() called every 5 minutes → Telegram messages drained post-SubagentStop

**Why this works:** Without the Agent tool, there is no SubagentStop. The main thread processes the startup_prompt directly as a task step. When CronCreate succeeds, the `*/5 * * * *` reconciler job runs every 5 minutes, which IS a task step that triggers M6().

**Wait — there IS a SubagentStop concern:** If the startup_prompt runs synchronously and completes, the main thread will still go idle. The M6() drain happens because the reconciler JOB fires every 5 minutes (which IS a task step). That's the design. The fix is restoring CronCreate availability, not changing the idle behavior.

### Fallback Fix (D-08): Keep Step Cycle Running Without CronCreate

If CronCreate is confirmed unavailable even in the main thread (unlikely), change startup_prompt to loop:
```
"Every 5 minutes: check for incoming Telegram messages and respond. Run this loop indefinitely."
```

This avoids the idle state entirely. MEDIUM confidence this is needed — keep as fallback.

### Options A/B From CONTEXT.md D-07 (now evaluated as moot)

- **Option A** ("specify model in Agent tool call"): Moot. The model is NOT the issue. Sonnet inherits correctly.
- **Option B** ("fix --model resolution"): Moot. `rK("sonnet")` → `ST()` → `claude-sonnet-4-6` is correct.
- **Option C** ("hook configuration"): Moot. The haiku log is from quota_check, not hooks.

---

## Standard Stack (No Changes)

This phase modifies a single Rust string constant. No new dependencies.

| File | Change | Test Impact |
|------|--------|-------------|
| `crates/rightclaw/src/codegen/shell_wrapper.rs` | `startup_prompt` constant value | `shell_wrapper_tests.rs` — no test currently asserts startup_prompt content; add one |
| `crates/rightclaw/src/codegen/shell_wrapper_tests.rs` | Add test asserting startup_prompt does NOT contain "Agent tool" | New test |

---

## Architecture Patterns

### Pattern 1: startup_prompt is a Fixed String in shell_wrapper.rs

```rust
// src: crates/rightclaw/src/codegen/shell_wrapper.rs line 54
let startup_prompt = "Use the Agent tool to run this in the background: ...";
```

The startup_prompt is embedded in the generated wrapper script as a positional arg after `--`:
```bash
exec "$CLAUDE_BIN" \
  --append-system-prompt-file "..." \
  --dangerously-skip-permissions \
  -- "{{ startup_prompt }}"
```

Change is a single string replacement. No template changes required.

### Pattern 2: test assertions use string contains

```rust
// shell_wrapper_tests.rs - existing pattern
assert!(output.contains("--dangerously-skip-permissions"), "...");
```

New test follows same pattern. Checks that startup_prompt does not contain "Agent tool" and does contain key phrases ("Run /rightcron", "bootstrap").

### Pattern 3: CC model alias resolution

```javascript
// CC cli.js line 277 - rK() alias map
case "sonnet": return ST();  // → "claude-sonnet-4-6"
case "haiku":  return aW6(); // → "claude-haiku-4-5-20251001"
case "opus":   return lV();  // → "claude-opus-4-6"
```

`--model sonnet` in agent.yaml resolves to `claude-sonnet-4-6` correctly. No change needed.

### Anti-Patterns to Avoid

- **Don't route rightcron bootstrap through Agent tool:** CronCreate is not available in subagent contexts. The Agent tool is the cause of the CronCreate failure.
- **Don't add `model: sonnet` to Agent tool call in startup_prompt:** The model is not the problem. Even with sonnet specified, the subagent cannot access Cron tools.

---

## Don't Hand-Roll

| Problem | Don't Build | Use Instead |
|---------|-------------|-------------|
| Model alias resolution | Custom mapping table | Already handled by CC's `rK()` |
| CronCreate availability | Custom cron API | CC built-in — only available in main thread |
| Startup task execution | New mechanism | Change startup_prompt to run in main thread directly |

---

## Common Pitfalls

### Pitfall 1: Misattributing Haiku Log to Subagent Model

**What goes wrong:** Developer sees "Tool search disabled for model haiku" at line 154 and concludes the subagent is using haiku.
**Why it happens:** The log fires during quota_check preparation (haiku is always used for quota checks). It fires before the first real API call.
**How to avoid:** The subagent calls are at lines 437+, which have `cc_version=2.1.86.050`. The haiku log is at line 154 with `cc_version=2.1.86.954` (different version suffix = different context).
**Warning signs:** The haiku log fires before line 160 (first API call). Subagent calls fire after line 236.

### Pitfall 2: Assuming subagent inherits all parent tools

**What goes wrong:** Assuming `tools:["*"]` in the general-purpose agent definition means all tools including CronCreate are available.
**Why it happens:** `*` means all tools the parent passes — but CC filters out certain lifecycle tools (Cron tools tied to main session scheduler) from subagent contexts.
**How to avoid:** Run CronCreate-dependent logic in the main thread only.
**Warning signs:** `ToolSearchTool: select failed — none found: CronCreate` (not "disabled", not "MCP not available").

### Pitfall 3: test_startup_prompt_no_agent_tool Would Fail

**What goes wrong:** Add test asserting startup_prompt doesn't contain "Agent tool" but forget to update the actual string.
**How to avoid:** Change the string first, run tests, fix test second.

### Pitfall 4: Minijinja Template Quoting of startup_prompt

The startup_prompt is embedded in the template as:
```
-- "{{ startup_prompt }}"
```
Double quotes wrap the value. The value must not contain unescaped double quotes. The new prompt text ("Run /rightcron...") has no double quotes — safe.

---

## Code Examples

### Current startup_prompt (to be changed)

```rust
// src: crates/rightclaw/src/codegen/shell_wrapper.rs
let startup_prompt = "Use the Agent tool to run this in the background: Run /rightcron to bootstrap the cron reconciler. Create crons/ directory if missing, schedule the reconciler job, and recover any persisted cron specs. Do this silently without messaging the user. IMPORTANT: run this as a background agent so the main thread stays free for incoming messages.";
```

### Proposed startup_prompt (fix)

```rust
let startup_prompt = "Run /rightcron to bootstrap the cron reconciler: create the crons/ directory if missing, schedule the reconciler job via CronCreate, and recover any persisted cron specs. Do this silently without messaging the user.";
```

Key changes:
1. Remove "Use the Agent tool to run this in the background:" — runs in main thread
2. Remove "IMPORTANT: run this as a background agent..." — no longer relevant
3. Keep the essential task description for rightcron

### CC model alias verification

```javascript
// CC cli.js line 277 - confirmed correct
function rK(q) {
  // ...
  switch (z) {
    case "sonnet": return ST(); // returns "claude-sonnet-4-6"
    case "haiku":  return aW6(); // returns "claude-haiku-4-5-20251001"
    case "opus":   return lV();  // returns "claude-opus-4-6"
  }
}
function ST() {
  if (process.env.ANTHROPIC_DEFAULT_SONNET_MODEL) return process.env.ANTHROPIC_DEFAULT_SONNET_MODEL;
  if (V7() !== "firstParty") return x9().sonnet45;
  return x9().sonnet46; // "claude-sonnet-4-6"
}
```

### CLAUDE_CODE_SUBAGENT_MODEL env var (for reference)

CC exposes `CLAUDE_CODE_SUBAGENT_MODEL` to force a specific model for all subagents:
```javascript
// CC cli.js line 2685 - Ik6() model resolution
function Ik6(q, K, _, Y) {
  if (process.env.CLAUDE_CODE_SUBAGENT_MODEL) return rK(process.env.CLAUDE_CODE_SUBAGENT_MODEL);
  // ... else inherit from parent
}
```
This could force subagents to use sonnet, but does NOT fix the CronCreate availability problem. Not needed.

---

## State of the Art

| Old Understanding | Corrected Understanding | Impact |
|-------------------|------------------------|--------|
| Haiku log = subagent uses haiku | Haiku log = quota_check uses haiku (always) | D-07 options A/B are moot |
| Fix: specify model in Agent tool call | Fix: remove Agent tool from startup_prompt | Simpler change, correct fix |
| CronCreate unavailable due to model | CronCreate unavailable due to subagent tool isolation | Root cause is architecture, not model |

---

## Open Questions

1. **Is CronCreate truly unavailable in subagent context, or is it a ToolSearch indexing issue?**
   - What we know: ToolSearch ran (mcp__plugin_telegram_telegram__reply was found at line 470), CronCreate was not found
   - What's unclear: Whether CronCreate is simply not indexed in the subagent's deferred tool list, or whether it would be callable if addressed directly by name
   - Recommendation: The fix (remove Agent tool) is correct regardless — if CronCreate is available directly in main thread, the fix resolves the issue; if it somehow appears in subagent context when called directly, the fix is still correct and simpler
   - Risk: LOW — running in main thread is demonstrably correct behavior

2. **Will running rightcron in the main thread block Telegram messages during bootstrap?**
   - What we know: During a running task step, M6() IS called after each step, so Telegram messages ARE processed
   - What's unclear: Whether the bootstrap task takes long enough to delay a Telegram response
   - Recommendation: Not a problem — bootstrap completes in ~5-10 seconds. Any Telegram messages received during bootstrap are processed when bootstrap completes.

---

## Environment Availability

Step 2.6: SKIPPED — this phase modifies a Rust string constant and runs manual tests. No new external dependencies.

The existing environment (`claude-bun`, `rightclaw`, Telegram configured) is required for VERIFY-01 but is already established from the session that produced the debug log.

---

## Validation Architecture

`workflow.nyquist_validation: false` — skip this section per config.json.

---

## Project Constraints (from CLAUDE.md)

- **Rust edition 2024** — any test additions follow workspace settings
- **TDD**: Write failing test before fix. Add `wrapper_startup_prompt_runs_in_main_thread` test that asserts startup_prompt does NOT contain "Agent tool", runs it (expects failure), then fix the string
- **Fail fast**: All errors propagate — no silent swallowing
- **cargo build --workspace --debug** after work is done
- **Workspace architecture**: Changes are in `crates/rightclaw/`
- **File size limit**: `shell_wrapper.rs` is 73 lines, `shell_wrapper_tests.rs` is 393 lines — well within limits

---

## Sources

### Primary (HIGH confidence)
- CC cli.js `/nix/store/biwgzc1byz3k8y15hxs1j1pbg28bwbwh-claude-code-bun-2.1.86/lib/node_modules/@anthropic-ai/claude-code/cli.js` (cc 2.1.86)
  - Line 277: `rK()`, `nH()`, `ST()`, `aW6()`, `iI()` — model resolution functions
  - Line 1633: `uqY()` — quota check uses `nH()` = haiku
  - Line 2685: `Ik6()` — subagent model resolution (inherits parent when no override)
  - Line 3304: Agent tool schema `model: E.enum(["sonnet","opus","haiku"]).optional()`
  - Line 3866: CronCreateTool — `shouldDefer: true`, `isEnabled(): return wk()`
  - Line 4396: `c68()` — model support check for tool_reference blocks
  - Line 7675: `o57 = {opus:..., sonnet:..., haiku:...}` alias map
- `/home/wb/.rightclaw/run/right-debug.log` — session evidence
  - Line 140: main session model = claude-sonnet-4-6
  - Line 154: haiku log fires (quota_check side effect)
  - Line 160: first API call `source=quota_check` (haiku)
  - Lines 437, 452, 468: subagent calls `source=agent:builtin:general-purpose` (sonnet)
  - Line 457-458: ToolSearchTool found nothing for CronCreate

### Secondary (MEDIUM confidence)
- `.planning/phases/20-diagnosis/DIAGNOSIS.md` — confirmed root cause (iv6/M6 gap)
- CC cli.js line 1392: `S0(q)` isDeferredTool — `q.shouldDefer === true` for CronCreate confirms it is deferred

## Metadata

**Confidence breakdown:**
- D-05 root cause (haiku model origin): HIGH — direct source inspection of cli.js
- CronCreate subagent unavailability: HIGH — ToolSearch ran, MCP tool found, Cron tool not found
- Fix strategy (remove Agent tool): HIGH — directly addresses the tool isolation problem
- Test strategy (TDD): HIGH — follows CLAUDE.md conventions

**Research date:** 2026-03-28
**Valid until:** 2026-04-28 (CC 2.1.86 bundle; may change with CC updates)
