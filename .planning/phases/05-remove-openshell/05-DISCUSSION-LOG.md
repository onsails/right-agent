# Phase 5: Remove OpenShell - Discussion Log

> **Audit trail only.** Do not use as input to planning, research, or execution agents.
> Decisions are captured in CONTEXT.md — this log preserves the alternatives considered.

**Date:** 2026-03-24
**Phase:** 05-remove-openshell
**Areas discussed:** --no-sandbox flag fate, bypassPermissions stance, Agent validation change

---

## --no-sandbox Flag Fate

| Option | Description | Selected |
|--------|-------------|----------|
| Remove entirely | Phase 5 strips it. Phase 6 adds CC sandbox via settings.json — no CLI flag needed since sandbox is always-on via generated config. | |
| Keep, repurpose | Repurpose as --no-sandbox to generate settings.json with sandbox.enabled=false. Useful for dev/testing when bubblewrap isn't available. | ✓ |
| Remove now, add back in Phase 6 | Clean removal in Phase 5. Phase 6 decides if a disable mechanism is needed based on sandbox config design. | |

**User's choice:** Keep, repurpose
**Notes:** Flag becomes a no-op in Phase 5 (no OpenShell to skip), then gets wired to CC sandbox config in Phase 6.

---

## bypassPermissions Stance

| Option | Description | Selected |
|--------|-------------|----------|
| Always-on (keep D-08) | CC sandbox is the security layer now. bypass + sandbox = autonomous agents with OS-level guardrails. Same philosophy, different enforcer. | ✓ |
| Per-agent toggle | Add bypass_permissions: bool to agent.yaml. Some agents might want CC permission prompts as extra safety. Default: true (bypass). | |
| Drop bypass entirely | Rely on CC sandbox + CC permissions together. More conservative but agents get prompted for many operations. | |

**User's choice:** Always-on (keep D-08)
**Notes:** Security model unchanged: always bypass CC permissions, sandbox enforces at OS level.

---

## Agent Validation Change

| Option | Description | Selected |
|--------|-------------|----------|
| IDENTITY.md only | Minimal: if it has IDENTITY.md, it's an agent. Same as v1 minus the policy requirement. Clean. | |
| IDENTITY.md + agent.yaml | Require agent.yaml too — every agent must have explicit config. More structured but adds friction for simple agents. | |
| IDENTITY.md now, revisit in Phase 6 | Remove policy.yaml requirement. Phase 6 may add settings.json-related validation when sandbox config is generated. | ✓ |

**User's choice:** IDENTITY.md now, revisit in Phase 6
**Notes:** Minimal change for Phase 5 — just remove policy.yaml requirement. Phase 6 decides if settings.json validation is needed.

---

## Claude's Discretion

- Exact order of file-by-file removal (compiler-guided)
- Test refactoring approach for shell_wrapper_tests.rs and sandbox_tests.rs
- Whether to keep sandbox_tests.rs or merge remaining tests

## Deferred Ideas

None — discussion stayed within phase scope
