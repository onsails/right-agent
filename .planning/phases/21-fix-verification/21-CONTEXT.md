# Phase 21: Fix & Verification - Context

**Gathered:** 2026-03-28
**Status:** Ready for planning

<domain>
## Phase Boundary

Implement a fix so Telegram channel messages are processed after rightcron bootstrap completes,
then verify end-to-end that Telegram responds to commands in a running agent session.

NOT in scope: changing sandbox config, rewriting rightcron's skill logic, updating stale
REQUIREMENTS.md text (deferred to milestone wrap-up).

</domain>

<decisions>
## Implementation Decisions

### Root Cause (locked from Phase 20)

- **D-01:** Root cause is CC's `iv6` subscriber not calling `M6()` when idle after SubagentStop.
  The fix cannot patch CC internals — it must work around the gap from rightclaw's side.
- **D-02:** The intended workaround already exists: rightcron's `*/5 * * * *` reconciler job
  (created via CronCreate during bootstrap) fires every 5 minutes → `M6()` called → Telegram
  messages drained. The reconciler job was NOT being created because CronCreate was unavailable
  to the background sub-agent.

### Why CronCreate Was Unavailable (research target)

- **D-03:** Debug log shows `ToolSearchTool: select failed — none found: CronCreate, CronList, CronDelete`
  during the background agent's rightcron bootstrap (line 458, session 21:21-21:22).
- **D-04:** Earlier in the log (line 154): `Tool search disabled for model 'claude-haiku-4-5-20251001'`
  fires during session init — before the first real API call. This suggests the background sub-agent
  (spawned by the Agent tool to run rightcron) uses Haiku by default, and tool_reference blocks
  (required for deferred tool search) are Sonnet+ only.
- **D-05:** OPEN QUESTION — research must confirm: is the Haiku model coming from (a) CC's default
  for Agent tool sub-agents regardless of parent model, (b) a hook system that runs on Haiku, or
  (c) `--model sonnet` in agent.yaml not resolving correctly to claude-sonnet-4-6?
  The fix strategy depends on which of these is true.

### Fix Direction

- **D-06:** The fix is to ensure the background sub-agent that runs rightcron bootstrap uses Sonnet
  (or at minimum a model that supports tool_reference blocks / deferred tools). Once it can find
  CronCreate, it will create the `*/5 * * * *` reconciler job and the freeze is resolved.
- **D-07:** Implementation options (researcher should evaluate):
  - **Option A**: Change `startup_prompt` to explicitly request Sonnet for the Agent tool call:
    `"Use the Agent tool (model: claude-sonnet-4-6) to run this in the background: Run /rightcron..."`
  - **Option B**: Fix `--model sonnet` → ensure it maps to `claude-sonnet-4-6` in the wrapper
    (currently `sonnet` might map to Haiku or resolve incorrectly)
  - **Option C**: If hooks are the source of the haiku model log, adjust hook configuration
  The researcher should determine which option applies given how CC selects the sub-agent model.
- **D-08:** Fallback — if ensuring Sonnet still doesn't make CronCreate available (e.g., it's behind
  a disabled feature flag), the fallback is Option A from DIAGNOSIS.md: change `startup_prompt`
  so the background agent loops rather than exits (keep the step cycle running).

### Verification

- **D-09:** Pass criterion: send a Telegram message after rightcron bootstrap completes ("Agent completed"
  appears in TUI), receive a response within 5 minutes.
- **D-10:** Confirmation Test A from DIAGNOSIS.md is the primary test:
  1. `rightclaw up` with Telegram configured
  2. Wait for "Agent 'Bootstrap rightcron reconciler' completed" in TUI
  3. Wait 30 seconds (no messages during rightcron run or immediately after)
  4. Send a Telegram message
  5. Expect response within 5 minutes (one cron fire cycle)
- **D-11:** 5 minutes is acceptable for the verification test — no need to shorten the interval.
- **D-12:** Regression: existing behavior (messages sent during rightcron run) must still work.
  Confirmation Test B from DIAGNOSIS.md as the regression check.

### Folded Todos

- **Todo: "Investigate CC session hang with sandbox + background agents"** — this IS the fix
  being implemented. Move to done after Phase 21 completes.
- **Todo: "Document CC gotcha — Telegram messages dropped while agent is streaming"** — separate
  issue (streaming vs idle), but this phase should add a GOTCHA.md entry describing the iv6/M6
  gap and the fix. The streaming case may have the same root cause.

</decisions>

<canonical_refs>
## Canonical References

**Downstream agents MUST read these before planning or implementing.**

### Phase 20 output
- `.planning/phases/20-diagnosis/DIAGNOSIS.md` — root cause analysis, confirmation tests, fix proposals

### Key source files to modify
- `crates/rightclaw/src/codegen/shell_wrapper.rs` — `startup_prompt` string (fix target)
- `crates/rightclaw/src/codegen/shell_wrapper_tests.rs` — tests to update after startup_prompt change
- `templates/agent-wrapper.sh.j2` — how startup_prompt is passed as `--` positional arg

### Debug evidence
- `/home/wb/.rightclaw/run/right-debug.log` — session log with haiku/CronCreate failure evidence

### Project context
- `.planning/REQUIREMENTS.md` — FIX-01, FIX-02, VERIFY-01

</canonical_refs>

<code_context>
## Existing Code Insights

### Integration Point
The fix lands in `shell_wrapper.rs` — specifically the `startup_prompt` constant. This is the
`--` positional argument passed to CC at launch. Changing it is a single-string change in one
function.

### Test Pattern
`shell_wrapper_tests.rs` contains tests that assert on the generated wrapper script content.
Any startup_prompt change needs corresponding test updates.

### Model in agent.yaml
The agent's `model: sonnet` flows through `generate_wrapper()` → template → `--model sonnet`
CLI flag. If the mapping is wrong (sonnet → haiku), the fix might need to happen in the model
resolution logic rather than the startup_prompt.

</code_context>

<specifics>
## Specific Ideas

- The debug log's `source=agent:builtin:general-purpose` for background agent calls suggests
  CC's builtin Agent tool may always use a cost-optimized model for sub-agents. If so,
  the only reliable fix is explicitly requesting the model in the Agent tool invocation
  from the startup_prompt text.
- The `*/5 * * * *` interval (5 minutes) is acceptable for chat — a user might wait up
  to 5 minutes for a first response after a session restart. If this proves unacceptable
  in practice, the reconciler interval can be shortened in rightcron's SKILL.md.

</specifics>

<deferred>
## Deferred Ideas

- Requirements text update (DIAG-02 sandbox specificity, DIAG-03 stale element names) — defer to
  complete-milestone workflow
- Filing CC bug report for the iv6/M6 gap — out of scope for v2.4, but should be tracked as a
  SEED. The fix in rightclaw is a workaround; the root bug is in CC.

</deferred>

---

*Phase: 21-fix-verification*
*Context gathered: 2026-03-28*
