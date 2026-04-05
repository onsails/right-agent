# Phase 10: Doctor & Managed Settings - Discussion Log

> **Audit trail only.** Do not use as input to planning, research, or execution agents.
> Decisions are captured in CONTEXT.md — this log preserves the alternatives considered.

**Date:** 2026-03-25
**Phase:** 10-doctor-managed-settings
**Areas discussed:** CLI shape, sudo strategy, macOS support, Doctor conflict detail

---

## Scope Call (pre-discussion)

User asked what strict-sandbox is and why it exists. Explained:
- RightClaw already works without it — per-agent settings.json handles sandbox per session
- `allowManagedDomainsOnly: true` adds a machine-wide policy layer (all CC sessions, not just RightClaw)
- Requires sudo, affects all claude invocations on the machine, is opt-in only by design

| Option | Description | Selected |
|--------|-------------|----------|
| Ship as planned | Config command + doctor check, completes v2.1 | ✓ |
| Defer strict-sandbox, keep doctor | Only TOOL-02, drop TOOL-01 | |
| Drop Phase 10 entirely | Call v2.1 done after Phase 9 | |

**User's choice:** Ship it as planned.

---

## CLI Shape

| Option | Description | Selected |
|--------|-------------|----------|
| Nested subcommand | `Config` variant with `ConfigCommands` sub-enum | ✓ |
| Flat command | `StrictSandbox` directly in top-level `Commands` | |

**User's choice:** Nested subcommand — `rightclaw config strict-sandbox`
**Notes:** Extensible namespace for future config commands.

---

## sudo Strategy

| Option | Description | Selected |
|--------|-------------|----------|
| Attempt write, clear error | Try write, surface permission denied with sudo hint | ✓ |
| Re-exec via sudo | Detect uid != 0, spawn `sudo rightclaw ...` as child | |

**User's choice:** Attempt write, surface clear error.
**Notes:** Simpler, no re-exec complexity.

---

## macOS Support

| Option | Description | Selected |
|--------|-------------|----------|
| Cross-platform | Write to /etc/claude-code/ on Linux and macOS both | ✓ |
| Linux-only | Gate behind cfg(target_os = "linux") | |

**User's choice:** Cross-platform.
**Notes:** /etc/ exists on macOS. Planner to verify CC reads same path on macOS.

---

## Doctor Conflict Detail

| Option | Description | Selected |
|--------|-------------|----------|
| Rich warning | Parse file, check allowManagedDomainsOnly:true, specific message | ✓ |
| Minimal warning | Just note file exists | |

**User's choice:** Rich warning — read and parse the file content.
**Notes:** If `allowManagedDomainsOnly: true`, warn that per-agent allowedDomains may be overridden.

---

## Claude's Discretion

- JSON parsing approach (serde_json vs string contains) — planner decides based on dep availability
- Whether to extract `check_managed_settings()` function or inline — prefer extracted for testability
- Exact success message text for `config strict-sandbox`

## Deferred Ideas

- `rightclaw config unset strict-sandbox` — remove managed-settings.json
- Domain allowlist management — out of RightClaw scope
