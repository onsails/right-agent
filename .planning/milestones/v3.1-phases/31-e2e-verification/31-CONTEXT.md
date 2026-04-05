# Phase 31: E2E Verification - Context

**Gathered:** 2026-04-02
**Status:** Ready for planning

<domain>
## Phase Boundary

Full rightclaw up → doctor green → CC sandbox ON → Telegram → cron flow is verified with all three sandbox dependencies (rg, socat, bwrap) explicitly confirmed. Deliverable is a repeatable shell script in `tests/e2e/` that can be re-run after CC version bumps.

</domain>

<decisions>
## Implementation Decisions

### Verification Format
- **D-01:** Shell script (bash) in `tests/e2e/`. Not a Rust integration test, not a markdown checklist. Programmatic pass/fail output.
- **D-02:** Script takes agent name as argument and uses that agent's real settings.json and agent definition from `~/.rightclaw/agents/`. Requires `rightclaw up` to have been run first — script verifies state, doesn't create it.
- **D-03:** Script exits with error if settings.json doesn't exist for the target agent, with message "run rightclaw up first".

### Verification Pipeline
- **D-04:** Three-stage pipeline: (1) `rightclaw doctor` pre-flight — parse for Fail/Warn on sandbox-related checks, abort if doctor fails; (2) dependency availability checks — rg, socat, bwrap in PATH; (3) CC smoke test — real `claude -p` invocation with sandbox.
- **D-05:** If doctor pre-flight fails, skip CC smoke test entirely. No point testing CC if dependencies are broken.

### CC Smoke Test
- **D-06:** Single CC invocation using `claude -p --agent <name> --model haiku` with `--json-schema` for structured output. One test covers both VER-01 and VER-02 — bot and cron subprocesses use the same claude binary, same settings.json, same env vars.
- **D-07:** Use `--model haiku` to minimize token cost.
- **D-08:** Parse structured JSON output for success. Exit code 0 + valid JSON = sandbox engaged and working. Non-zero exit = sandbox or CC failure (failIfUnavailable:true ensures sandbox failure is fatal).

### Logging
- **D-09:** CC stderr captured to `tests/e2e/last-run.log`. On failure, script prints log contents for debugging. On success, log kept for auditing.

### Claude's Discretion
- Exact structured output JSON schema for the smoke test prompt
- Specific doctor output parsing strategy (regex vs jq vs string match)
- Script argument handling (flags, defaults, help text)
- Color/formatting of pass/fail output

</decisions>

<canonical_refs>
## Canonical References

**Downstream agents MUST read these before planning or implementing.**

### Requirements
- `.planning/REQUIREMENTS.md` — VER-01 (bot subprocess sandbox), VER-02 (cron subprocess sandbox), VER-03 (repeatable verification script)

### Phase 29 Implementation (sandbox fix)
- `crates/rightclaw/src/codegen/settings.rs` — `generate_settings()` with `rg_path`, `failIfUnavailable: true`, `sandbox.ripgrep.command`
- `crates/bot/src/telegram/worker.rs:403` — `USE_BUILTIN_RIPGREP=0` env var
- `crates/bot/src/cron.rs:231` — `USE_BUILTIN_RIPGREP=0` env var

### Phase 30 Implementation (doctor checks)
- `crates/rightclaw/src/doctor.rs` — `check_rg_in_path()`, `check_ripgrep_in_settings()`, existing sandbox checks (bwrap, socat)

### Agent Definition
- `crates/rightclaw/src/codegen/agent_def.rs` — agent definition codegen, `--agent` flag usage
- `crates/bot/src/telegram/worker.rs` — `invoke_cc()` for CC subprocess invocation pattern

### Roadmap
- `.planning/ROADMAP.md` — Phase 31 success criteria

</canonical_refs>

<code_context>
## Existing Code Insights

### Reusable Assets
- `which` crate in workspace deps — same rg/socat/bwrap resolution as doctor.rs
- `rightclaw doctor` already has structured output with check names and severity — parseable
- `--json-schema` / `--agent` flags already used in worker.rs and cron.rs CC invocations
- `reply-schema.json` generated per agent — pattern for structured CC output exists

### Established Patterns
- CC invocation: `claude -p --agent <name> --model <model> --json-schema <path>` with `USE_BUILTIN_RIPGREP=0` env var
- Agent dir as cwd for CC invocations
- settings.json at `<agent_dir>/.claude/settings.json`

### Integration Points
- No new Rust code needed — this is a shell script artifact
- `tests/e2e/` directory (new) — doesn't exist yet
- Script invokes `rightclaw doctor` and `claude` CLI binaries

</code_context>

<specifics>
## Specific Ideas

- User wants structured output + haiku model for cost-effective programmatic verification
- User confirmed single CC smoke test is sufficient — bot and cron paths share the same CC binary/settings
- Doctor pre-flight gates the CC smoke test — fail early if deps are broken

</specifics>

<deferred>
## Deferred Ideas

### Reviewed Todos (not folded)
- "Document CC gotcha — Telegram messages dropped while agent is streaming" — docs task, unrelated to sandbox verification scope

</deferred>

---

*Phase: 31-e2e-verification*
*Context gathered: 2026-04-02*
