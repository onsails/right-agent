# Phase 20: Diagnosis - Discussion Log

> **Audit trail only.** Do not use as input to planning, research, or execution agents.
> Decisions are captured in CONTEXT.md — this log preserves the alternatives considered.

**Date:** 2026-03-28
**Phase:** 20-diagnosis
**Areas discussed:** Log coverage gap, How to reproduce, What 'queue empty' means, Scope of log analysis

---

## Log Coverage Gap

| Option | Description | Selected |
|--------|-------------|----------|
| Sent messages after 17:09, no response | Session alive but not responding | |
| Session ended too fast to test | Log from session that exited before testing | |
| Have a longer log too | Longer session log available | ✓ |

**User's choice:** User ran a new session, captured both TUI output and debug log.
**Notes:** New session confirmed agent responds to early messages (hi, hi again) but not to
third message sent ~1 hour later. Confirmed process alive (not restarted).

---

## How to Reproduce

| Option | Description | Selected |
|--------|-------------|----------|
| Reproduce manually + capture new log | Start, wait for rightcron, send Telegram | |
| Analysis of existing logs only | Work with existing logs + TUI output | ✓ |

**User's choice:** Work with existing logs. TUI output provided as additional evidence.
**Notes:** TUI output showed exact sequence — hi/hi again processed WHILE rightcron running,
agent completed, then session frozen for 1 hour before "notion mcp" message ignored.

---

## What 'Queue Empty' Means

| Option | Description | Selected |
|--------|-------------|----------|
| Checked Telegram getUpdates directly | Message polled from Telegram queue confirmed | |
| Inferred from CC logs | Assumption from absence of error | |
| Not sure | General observation | ✓ |

**User's choice:** User clarified they never said "queue empty" — that was from the original TODO
description. User only reported that the third message got no response.
**Notes:** "Queue empty" language in the TODO was imprecise. Actual observation: agent alive,
messages sent, no reply received.

---

## Scope of Log Analysis

| Option | Description | Selected |
|--------|-------------|----------|
| Doc with findings + hypothesis | DIAGNOSIS.md in phase dir | ✓ |
| Skip doc, fix directly in Phase 21 | No written artifact | |

**User's choice:** Write DIAGNOSIS.md with findings and hypothesis. Gives Phase 21 a clear target.

---

## Trigger for Freeze

| Option | Description | Selected |
|--------|-------------|----------|
| After going idle (~60s) | idle_prompt fires, channel notifications stop | |
| After rightcron background agent completes | SubagentStop is the trigger | ✓ |
| After long period (1h+) | Time-based degradation | |
| Don't know | Need controlled test | |

**User's choice:** After rightcron background agent completes.
**Notes:** TUI shows sequence — both messages received while rightcron running, then freeze
after completion. Consistent with SubagentStop being the trigger.

---

## Claude's Discretion

- Confirmed two competing hypotheses (A: CC event loop, B: socat TCP timeout) without user
  input — user approved both-hypothesis framing
- Chose to cite "works without --no-sandbox" as evidence favoring Hypothesis B
