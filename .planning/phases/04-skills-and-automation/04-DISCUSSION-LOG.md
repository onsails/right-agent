# Phase 4: Skills and Automation - Discussion Log

> **Audit trail only.** Do not use as input to planning, research, or execution agents.
> Decisions are captured in CONTEXT.md — this log preserves the alternatives considered.

**Date:** 2026-03-22
**Phase:** 4-Skills and Automation
**Areas discussed:** ClawHub API, Policy gate, CronSync design, Skill install path

---

## ClawHub API

**User's choice:** Research actual endpoints during plan-phase research

---

## Skill Install Path

| Option | Description | Selected |
|--------|-------------|----------|
| Agent's skills/ dir | Per-agent isolation | ✓ |
| .claude/skills/ | Shared across agents | |
| Both options | Default per-agent, --global for shared | |

**User's choice:** Agent's skills/ dir — per-agent isolation

---

## Policy Gate

| Option | Description | Selected |
|--------|-------------|----------|
| Block and explain | Refuse, show what's needed vs allowed | ✓ |
| Warn and confirm | Show mismatch, ask confirmation | |
| You decide | Claude's discretion | |

**User's choice:** Block and explain — security-first

---

## CronSync Bootstrap

### Activation mechanism

| Option | Description | Selected |
|--------|-------------|----------|
| Auto-register via /loop | | |
| Manual setup | User runs /loop themselves | |
| HEARTBEAT.md driven | | |
| Start prompt | agent.yaml start_prompt | |
| Generated system prompt | run/<agent>-system.md, non-editable | ✓ |

**User's choice:** Generated system prompt file at run/<agent>-system.md. User wanted it non-modifiable — system prompt passed via --append-system-prompt-file, regenerated on each `rightclaw up`. Confirmed no policy change needed since wrapper reads the file before sandbox launch.

---

## Claude's Discretion

- ClawHub API error handling
- CronSync conversation style
- Lock file edge cases
- System prompt wording

## Deferred Ideas

None
