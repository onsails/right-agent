# Phase 30: Doctor Diagnostics - Discussion Log

> **Audit trail only.** Do not use as input to planning, research, or execution agents.
> Decisions are captured in CONTEXT.md — this log preserves the alternatives considered.

**Date:** 2026-04-02
**Phase:** 30-doctor-diagnostics
**Areas discussed:** PATH simulation strategy, Settings.json validation scope, Check timing & integration, Severity & exit behavior

---

## PATH Simulation Strategy

| Option | Description | Selected |
|--------|-------------|----------|
| which::which in current env | Same approach as cmd_up — if rg is in doctor's PATH, it'll be in PC's PATH too. Simple, matches actual rg_path resolution. | ✓ |
| Parse process-compose.yaml env | Read generated PC yaml and extract PATH from agent environment blocks. More accurate for edge cases but PC yaml may not exist. | |
| Spawn subprocess mimicking PC | Launch a child process mimicking PC inheritance, check for rg inside it. Most accurate but complex. | |

**User's choice:** which::which in current env (Recommended)
**Notes:** Same shell session guarantee makes this sufficient.

---

## Settings.json Validation Scope

| Option | Description | Selected |
|--------|-------------|----------|
| Disk only | Read each agent's .claude/settings.json, check sandbox.ripgrep.command. Warn if file missing. No codegen dependency. | ✓ |
| Generate in-memory + validate | Call generate_settings() and validate output. Catches codegen bugs but couples doctor to codegen pipeline. | |
| Both: disk check + codegen drift | Read disk AND generate expected, compare. Detects manual edits or stale files. Most thorough but complex. | |

**User's choice:** Disk only (Recommended)
**Notes:** No codegen coupling in doctor path.

---

## Check Timing & Integration

| Option | Description | Selected |
|--------|-------------|----------|
| Doctor only | Add checks to run_doctor(). cmd_up already handles rg inline. Doctor = diagnostics, up = launcher. | ✓ |
| Doctor + up pre-flight | Run doctor before launching agents in cmd_up. Catches stale settings. | |
| Doctor + up --check flag | Optional --check flag for explicit opt-in doctor in up. | |

**User's choice:** Doctor only (Recommended)
**Notes:** cmd_up already resolves rg_path and fails if missing.

---

## Severity & Exit Behavior

| Option | Description | Selected |
|--------|-------------|----------|
| Warn (per requirements) | DOC-01 explicitly says Warn on Linux. Non-blocking — user decides whether to fix. | ✓ |
| Fail for rg, Warn for settings | Missing rg = Fail (agents crash). Invalid settings = Warn. Diverges from spec. | |
| Warn both with summary line | Both Warn per spec, add summary: "Sandbox will fail without rg". | |

**User's choice:** Warn (per requirements)
**Notes:** Per DOC-01/DOC-02 spec. Doctor stays non-blocking.

---

## Claude's Discretion

- Check ordering within run_doctor()
- Fix hint wording
- Whether to validate sandbox.ripgrep.args or just command

## Deferred Ideas

None — discussion stayed within phase scope
