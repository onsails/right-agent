# Phase 1: Foundation and Agent Discovery - Discussion Log

> **Audit trail only.** Do not use as input to planning, research, or execution agents.
> Decisions are captured in CONTEXT.md — this log preserves the alternatives considered.

**Date:** 2026-03-21
**Phase:** 1-Foundation and Agent Discovery
**Areas discussed:** Project structure, Agent dir layout, Validation strictness, Devenv setup

---

## Project Structure

| Option | Description | Selected |
|--------|-------------|----------|
| Two crates | rightclaw/ (library) + rightclaw-cli/ (binary) | ✓ |
| Three crates | rightclaw-core/ + rightclaw/ + rightclaw-cli/ | |
| Single crate | One crate, split later | |

**User's choice:** Two crates in `crates/` directory
**Notes:** User specified crates should live in `crates/` subdirectory

### Agent Lookup Location

| Option | Description | Selected |
|--------|-------------|----------|
| ~/.rightclaw/agents/ | Global agents dir, system-level tool | ✓ |
| Project-local first | <project>/agents/ first, fallback to global | |
| Both merged | Global + project-local | |

**User's choice:** ~/.rightclaw/agents/ only — RightClaw is NOT project-scoped
**Notes:** User clarified there is no "project path" concept. `rightclaw up` always reads from RIGHTCLAW_HOME. Customizable via --home flag or env var.

---

## Agent Dir Layout

### Default Agent Shipping

| Option | Description | Selected |
|--------|-------------|----------|
| Embedded in binary | `rightclaw init` extracts templates | ✓ |
| Separate in repo | install.sh copies from repo | |
| Template command | `rightclaw new-agent` generates from template | |

**User's choice:** `rightclaw init` with templates embedded at compile time
**Notes:** User was between options 1 and 3. Recommended combining both — `rightclaw init` for initial setup, future `rightclaw new-agent` for creating additional agents.

---

## Validation Strictness

### Minimum Valid Agent

| Option | Description | Selected |
|--------|-------------|----------|
| IDENTITY.md + policy.yaml | Both required | ✓ |
| IDENTITY.md only | policy.yaml optional with defaults | |
| Any .md file | Loose detection | |

**User's choice:** IDENTITY.md + policy.yaml — no agent runs without sandbox policy

### Invalid Config Behavior

| Option | Description | Selected |
|--------|-------------|----------|
| Fail fast | Refuse to start ANY agents | ✓ |
| Skip invalid | Log warning, skip broken agent | |
| Start with defaults | Ignore invalid fields | |

**User's choice:** Fail fast — per CLAUDE.rust.md FAIL FAST principle

---

## Devenv Setup

| Option | Description | Selected |
|--------|-------------|----------|
| Rust + process-compose | Toolchain + PC for testing | ✓ |
| Rust + PC + OpenShell | Also include OpenShell | |
| Minimal | Just Rust toolchain | |

**User's choice:** Rust + process-compose. OpenShell too new for nix — require installed separately.
**Notes:** User said to include OpenShell in devenv only if it becomes available as a nix package.

---

## Claude's Discretion

- Module organization within crates
- Exact clap command structure
- Test organization and fixtures

## Deferred Ideas

None — discussion stayed within phase scope

## Additional Notes

- User requested: copy CLAUDE.rust.md from ~/dev/tpt/ and reference it in project CLAUDE.md
