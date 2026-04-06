---
phase: 01
slug: foundation-and-agent-discovery
status: verified
threats_open: 0
asvs_level: L1
created: 2026-04-05
---

# Phase 01 — Security

> Per-phase security contract: threat register, accepted risks, and audit trail.

---

## Trust Boundaries

| Boundary | Description | Data Crossing |
|----------|-------------|---------------|
| CLI → Filesystem | `rightclaw init` writes template files to `$RIGHTCLAW_HOME/agents/right/` | Static embedded templates only; no user-supplied content written |
| CLI → Filesystem | `discover_agents` reads directory entries under `$RIGHTCLAW_HOME/agents/` | Agent names (directory names) and YAML config files |
| CLI → Environment | `RIGHTCLAW_HOME` env var read in main.rs, passed to `resolve_home` as parameter | Path string only |

---

## Threat Register

*No threat model was defined in Plan 01-01 or Plan 01-02 PLAN.md files.*

This phase is a pure infrastructure scaffold: Cargo workspace setup, data type definitions, YAML deserialization, and local filesystem operations (init/list). No network calls, no authentication, no credentials, no external service interactions.

Inherent mitigations in the implementation:
- `AgentConfig` uses `#[serde(deny_unknown_fields)]` — rejects malformed YAML inputs
- `validate_agent_name` enforces `^[a-zA-Z0-9][a-zA-Z0-9_-]*$` — prevents path traversal via agent directory names
- `resolve_home` receives env values as parameters (not via `std::env::var`) — prevents test environment pollution
- `init_rightclaw_home` checks for existing directory before writing — no silent overwrites
- All templates embedded via `include_str!` at compile time — no runtime template loading from untrusted paths

| Threat ID | Category | Component | Disposition | Mitigation | Status |
|-----------|----------|-----------|-------------|------------|--------|

*No threats registered — phase has no threat model block in PLAN.md.*

---

## Accepted Risks Log

No accepted risks.

---

## Security Audit Trail

| Audit Date | Threats Total | Closed | Open | Run By |
|------------|---------------|--------|------|--------|
| 2026-04-05 | 0 | 0 | 0 | gsd-secure-phase (no formal threat model in PLAN.md) |

---

## Sign-Off

- [x] All threats have a disposition (mitigate / accept / transfer)
- [x] Accepted risks documented in Accepted Risks Log
- [x] `threats_open: 0` confirmed
- [x] `status: verified` set in frontmatter

**Approval:** verified 2026-04-05
