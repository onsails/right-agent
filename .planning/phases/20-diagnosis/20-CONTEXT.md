# Phase 20: Diagnosis - Context

**Gathered:** 2026-03-28
**Status:** Ready for planning

<domain>
## Phase Boundary

Analyze existing logs and CC TUI output to identify why the agent stops responding to Telegram
messages after the rightcron background agent completes. Produce DIAGNOSIS.md naming the root
cause and proposing a fix for Phase 21.

NOT in scope: implementing the fix (Phase 21), changing sandbox config, changing Telegram config.

</domain>

<decisions>
## Implementation Decisions

### What we know from evidence

- **D-01:** Telegram DOES work in sandbox — "hi" and "hi again" were processed correctly within
  seconds of session start, while rightcron was still running as background agent.
- **D-02:** The freeze is NOT a systematic "Telegram never works in sandbox" — it's time/event
  triggered.
- **D-03:** Trigger is confirmed as: agent goes idle AFTER rightcron background agent completes.
  Messages sent while rightcron is running work; messages sent ~1h after rightcron completes
  do not.
- **D-04:** Process is alive (confirmed via process-compose TUI — 525 MiB, 3.9% CPU, Running,
  no restarts). True freeze, not a crash/restart.
- **D-05:** Debug log ends after idle_prompt at 21:23:25. Session ran for ~1 more hour but
  produced no further log entries. The "notion mcp" message (22:22) produced no log entry
  at all — CC never received or processed it.

### TUI evidence (from process-compose CC output)

Exact sequence from live TUI:
1. Session starts, rightcron launched as background agent
2. "Ready. How can I help you?" — main thread idle, background agent running
3. ← telegram · brainsmith: hi → reply sent ✓
4. ← telegram · brainsmith: hi again → reply sent ✓
5. "Agent 'Bootstrap rightcron reconciler' completed"
6. "Rightcron bootstrap done — crons/ directory is ready, no persisted jobs to recover."
7. Empty `❯` prompt — session waiting
8. (1 hour later) "I need you to add notion mcp" — NO CC response, not shown in TUI

### Two competing hypotheses

- **D-06 (Hypothesis A — CC Event Loop):** After background agent completion, CC's main
  thread enters a state that doesn't wake up for channel notifications. Messages during
  background execution work because the main thread is in "active" state. After SubagentStop,
  the event loop enters a reduced polling mode that drops channel notifications silently.

- **D-07 (Hypothesis B — socat TCP timeout):** The Telegram plugin uses long polling to
  api.telegram.org. In sandbox mode, CC routes connections through socat (required for
  allowedDomains enforcement). After ~1 hour of idle, socat drops the TCP connection.
  The plugin can't silently reconnect and stops receiving messages. Without --no-sandbox,
  no socat proxy — direct connection stays alive indefinitely.
  Evidence: "works without --no-sandbox" fits a network-layer difference, not an event loop
  difference. The Telegram plugin subprocess's network path changes under sandbox.

### Diagnostic approach

- **D-08:** Work with existing logs — no need for fresh reproduction. Evidence is sufficient
  to evaluate both hypotheses and propose a fix.
- **D-09:** DIAGNOSIS.md should name which hypothesis is more likely, provide confirmation
  tests, and propose the fix approach. This is the deliverable for Phase 21.

### Output format

- **D-10:** Phase 20 produces a `DIAGNOSIS.md` file in the phase directory with:
  - Evidence summary (log + TUI findings)
  - Root cause evaluation (A vs B, with reasoning)
  - Confirmation test (how to verify which hypothesis is correct)
  - Fix proposal (for Phase 21 to implement)

</decisions>

<canonical_refs>
## Canonical References

**Downstream agents MUST read these before planning or implementing.**

### Existing log files
- `/home/wb/.rightclaw/run/right-debug.log` — most recent session debug log (21:21–21:23 window)

### Key source files
- `crates/rightclaw/src/codegen/shell_wrapper.rs` — startup_prompt (rightcron bootstrap launch)
- `crates/rightclaw/src/codegen/settings.rs` — sandbox config generation, DEFAULT_ALLOWED_DOMAINS
- `templates/agent-wrapper.sh.j2` — how CC is launched, env vars, sandbox flow

### Project context
- `.planning/PROJECT.md` — project constraints, sandbox architecture decisions
- `.planning/REQUIREMENTS.md` — DIAG-01..03 requirements for this phase

</canonical_refs>

<code_context>
## Existing Code Insights

### Reusable Assets
- `generate_settings()` in settings.rs — generates the settings.json with sandbox config including `network.allowedDomains`. If socat is the enforcement mechanism, this is where the constraint originates.
- `agent-wrapper.sh.j2` — shell wrapper template. If the fix involves adding env vars or socat flags, this is the integration point.

### Established Patterns
- Sandbox is enforced via `.claude/settings.json` per-agent, regenerated on every `rightclaw up`
- `api.telegram.org` is already in DEFAULT_ALLOWED_DOMAINS — so domain filtering is not the issue
- socat + bubblewrap are required for Linux sandbox (doctor enforces this)

### Integration Points
- The Telegram plugin runs as a stdio MCP subprocess spawned by CC
- Channel notifications flow: plugin polls api.telegram.org → sends MCP notification to CC → CC processes and responds
- The break point is somewhere between the plugin polling successfully and CC processing the notification

</code_context>

<specifics>
## Specific Ideas

- The "works without --no-sandbox" clue strongly points to Hypothesis B (socat) — the network path
  is the key differentiator between the two modes
- Look at socat default timeout for idle TCP connections — this is a well-known gotcha
- Check if the Telegram plugin has a reconnect mechanism or relies on persistent TCP connections

</specifics>

<deferred>
## Deferred Ideas

- Folded todo: "Investigate CC session hang with sandbox + background agents" — this IS this phase
- Other todo "Document CC gotcha — Telegram messages dropped while agent is streaming" — separate
  issue, different trigger (streaming vs idle). Defer to documentation task after fix.

</deferred>

---

*Phase: 20-diagnosis*
*Context gathered: 2026-03-28*
