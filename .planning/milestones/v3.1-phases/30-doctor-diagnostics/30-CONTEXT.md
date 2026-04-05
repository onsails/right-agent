# Phase 30: Doctor Diagnostics - Context

**Gathered:** 2026-04-02
**Status:** Ready for planning

<domain>
## Phase Boundary

Add two new checks to `rightclaw doctor`: (1) ripgrep availability in the PATH that agent processes will inherit, and (2) validation of `sandbox.ripgrep.command` in each agent's generated settings.json. No changes to `rightclaw up` or other commands.

</domain>

<decisions>
## Implementation Decisions

### PATH Simulation Strategy
- **D-01:** Use `which::which("rg")` in the current environment — same approach as `cmd_up`. Doctor and `rightclaw up` run from the same shell session, so the PATH is identical. No subprocess spawning or process-compose.yaml parsing needed.

### Settings.json Validation Scope
- **D-02:** Read each agent's `.claude/settings.json` from disk only. Do not call `generate_settings()` in the doctor path — no codegen coupling. If the file doesn't exist, emit Warn with "run `rightclaw up` first to generate settings".

### Check Timing & Integration
- **D-03:** New checks run only in `run_doctor()`. No pre-flight checks in `cmd_up` — it already resolves `rg_path` inline and fails if missing. Doctor is the diagnostic tool, `up` is the launcher.

### Severity & Exit Behavior
- **D-04:** Both checks emit `Warn` severity per DOC-01/DOC-02 requirements. Missing rg = Warn (Linux only). Invalid/absent `sandbox.ripgrep.command` in settings.json = Warn. Doctor remains non-blocking.

### Claude's Discretion
- Check ordering within `run_doctor()` — place new checks logically near existing sandbox checks (after bwrap/socat, before agent structure)
- Fix hint wording for both new checks
- Whether to validate `sandbox.ripgrep.args` field or just `command`

</decisions>

<canonical_refs>
## Canonical References

**Downstream agents MUST read these before planning or implementing.**

### Requirements
- `.planning/REQUIREMENTS.md` — DOC-01 (rg PATH check) and DOC-02 (settings.json ripgrep.command validation)

### Phase 29 Implementation (dependency)
- `crates/rightclaw/src/codegen/settings.rs` — `generate_settings()` with `rg_path` parameter, sandbox.ripgrep.command injection logic
- `crates/rightclaw/src/codegen/settings_tests.rs` — Tests for ripgrep injection (injects_ripgrep_command_when_path_provided, omits_ripgrep_when_path_not_provided)

### Existing Doctor
- `crates/rightclaw/src/doctor.rs` — Current run_doctor() with all existing checks, CheckStatus/DoctorCheck types
- `crates/rightclaw-cli/src/main.rs:247` — cmd_doctor() CLI handler
- `crates/rightclaw-cli/src/main.rs:379` — rg_path resolution in cmd_up (which::which pattern to follow)

### Roadmap
- `.planning/ROADMAP.md` — Phase 30 success criteria (2 checks, specific Warn semantics)

</canonical_refs>

<code_context>
## Existing Code Insights

### Reusable Assets
- `check_binary()` in doctor.rs — already checks binaries in PATH with fix hints, can be reused for rg check (DOC-01)
- `CheckStatus::Warn` and `DoctorCheck` types — ready to use
- `serde_json::Value` parsing pattern from `check_managed_settings()` — reusable for parsing settings.json

### Established Patterns
- Doctor checks are sync functions returning `Vec<DoctorCheck>` or `DoctorCheck`
- Linux-only checks gated by `std::env::consts::OS == "linux"` (bwrap/socat pattern)
- Binary checks use `which::which()` — same crate already in dependencies
- Tests use `tempfile::tempdir()` with synthetic agent structures

### Integration Points
- `run_doctor()` in `crates/rightclaw/src/doctor.rs` — new checks added to the existing Vec
- Agent directory discovery reuses `home.join("agents")` pattern already in `check_agent_structure()` and `check_webhook_info_for_agents()`
- settings.json path: `<agent_dir>/.claude/settings.json` (written by cmd_up)

</code_context>

<specifics>
## Specific Ideas

No specific requirements — open to standard approaches

</specifics>

<deferred>
## Deferred Ideas

None — discussion stayed within phase scope

</deferred>

---

*Phase: 30-doctor-diagnostics*
*Context gathered: 2026-04-02*
