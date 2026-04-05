# Phase 40: Wire Cloudflared into Process-Compose — Discussion Log

> **Audit trail only.** Do not use as input to planning, research, or execution agents.
> Decisions captured in CONTEXT.md — this log preserves the discussion.

**Date:** 2026-04-05
**Phase:** 40-wire-cloudflared-into-process-compose
**Mode:** discuss
**Areas discussed:** Restart policy, depends_on, binary pre-flight

## Gray Areas Discussed

### Restart Policy
| Question | Options Presented | Decision |
|----------|------------------|----------|
| Cloudflared PC process restart policy | `on_failure` vs `always` | `on_failure` — matches bot agents |

User clarified: "we use the same mode as before" — confirmed `on_failure` to match existing bot agent policy.

### depends_on
| Question | Decision |
|----------|----------|
| Bot agents depend on cloudflared? | No dependency — bots work without tunnel |

### Binary Pre-flight
| Question | Decision |
|----------|----------|
| Fail fast if cloudflared missing when TunnelConfig present? | Yes — fail fast via `which::which` in `cmd_up` |

## Corrections / Clarifications

### User Confusion (resolved)
- **Original assumption presented:** Phase 40 is first time cloudflared gets into PC
- **User:** "Didn't we already have cloudflared on our process compose?"
- **Resolved:** Confirmed Phase 40 IS the first wiring — `main.rs:733` placeholder confirms the gap. User accepted.
