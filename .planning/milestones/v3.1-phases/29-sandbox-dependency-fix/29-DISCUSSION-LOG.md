# Phase 29: Sandbox Dependency Fix - Discussion Log

> **Audit trail only.** Do not use as input to planning, research, or execution agents.
> Decisions are captured in CONTEXT.md — this log preserves the alternatives considered.

**Date:** 2026-04-02
**Phase:** 29-sandbox-dependency-fix
**Areas discussed:** rg path resolution, failIfUnavailable scope, Atomicity strategy

---

## rg Path Resolution

### Q1: Where should rg path be resolved?

| Option | Description | Selected |
|--------|-------------|----------|
| In generate_settings() as param (Recommended) | generate_settings() gains rg_path: Option<PathBuf>. cmd_up resolves once via which::which. Clean separation — settings.rs stays pure. | ✓ |
| Inside generate_settings() | generate_settings() calls which::which internally. Simpler but adds IO. | |
| In process-compose env | Inject PATH with rg's parent dir. CC finds rg via PATH. Less reliable than sandbox.ripgrep.command. | |

**User's choice:** In generate_settings() as param (Recommended)
**Notes:** None

### Q2: What happens when rg is not found?

| Option | Description | Selected |
|--------|-------------|----------|
| Warn and continue (Recommended) | Log tracing::warn, pass None, no sandbox.ripgrep.command in JSON. | ✓ |
| Error and abort | rightclaw up fails. Strict but may annoy on macOS. | |
| Warn + set failIfUnavailable: false | Auto-downgrade. Complex. | |

**User's choice:** Warn and continue — but only if agent then fails without sandbox, not silently disables sandbox
**Notes:** User's condition is met by always setting failIfUnavailable: true. CC will crash the agent if sandbox can't start, preventing silent degradation.

---

## failIfUnavailable Scope

### Q3: failIfUnavailable: true — always or only when sandbox enabled?

| Option | Description | Selected |
|--------|-------------|----------|
| Always true (Recommended) | Unconditional. When sandbox disabled, CC ignores it (inert). Zero branching. | ✓ |
| Only when sandbox enabled | Conditional. Cleaner JSON for --no-sandbox but adds branching without real benefit. | |

**User's choice:** Always true (Recommended)
**Notes:** None

---

## Atomicity Strategy

### Q4: How to commit 4 fix sites?

| Option | Description | Selected |
|--------|-------------|----------|
| Single commit (Recommended) | All 4 changes atomic. No intermediate broken state. | ✓ |
| Two commits | Prerequisites first, then settings.rs. Possible intermediate breakage. | |
| Per-site commits | Maximum granularity but dangerous intermediate states. | |

**User's choice:** Single commit (Recommended)
**Notes:** None

---

## Claude's Discretion

None — all areas had explicit user decisions.

## Deferred Ideas

None — discussion stayed within phase scope.
