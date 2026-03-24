# Phase 6: Sandbox Configuration - Context

**Gathered:** 2026-03-24
**Status:** Ready for planning

<domain>
## Phase Boundary

Generate per-agent `.claude/settings.json` with CC native sandbox configuration during `rightclaw up`. Users can override sandbox settings via nested `sandbox:` section in `agent.yaml`. Overrides merge with generated defaults. `--no-sandbox` generates settings with `sandbox.enabled: false`. Settings overwritten on every `rightclaw up` — agent.yaml is the single source of truth for customization.

</domain>

<decisions>
## Implementation Decisions

### Default Sandbox Settings
- **D-01:** Generated settings.json always includes: `sandbox.enabled: true`, `sandbox.autoAllowBashIfSandboxed: true`, `sandbox.allowUnsandboxedCommands: false`
- **D-02:** Filesystem: `sandbox.filesystem.allowWrite` scoped to agent's own directory only (`~/.rightclaw/agents/<name>/`)
- **D-03:** Network: `sandbox.network.allowedDomains` includes generous defaults: `api.anthropic.com`, `github.com`, `npmjs.org`, `crates.io`, `agentskills.io`, `api.telegram.org`
- **D-04:** Non-sandbox settings always included: `skipDangerousModePermissionPrompt: true`, `spinnerTipsEnabled: false`, `prefersReducedMotion: true`
- **D-05:** Telegram `enabledPlugins` included conditionally (same logic as current init.rs — detected via `.mcp.json` presence)

### agent.yaml Override Format
- **D-06:** Nested `sandbox:` section in agent.yaml for per-agent overrides
- **D-07:** Override fields: `allow_write: Vec<String>`, `allowed_domains: Vec<String>`, `excluded_commands: Vec<String>`
- **D-08:** Override arrays MERGE with (not replace) generated defaults. User additions are appended.
- **D-09:** `AgentConfig` struct gets new optional `sandbox: Option<SandboxOverrides>` field
- **D-10:** `SandboxOverrides` is a new struct with `deny_unknown_fields` for strict validation
- **D-11:** CC path prefixes apply: `//` = absolute from root, `~/` = relative to HOME, `/` = relative to settings file dir

### --no-sandbox Wiring
- **D-12:** When `--no-sandbox` flag is set, settings.json is still generated but with `sandbox.enabled: false`
- **D-13:** All other settings (skipDangerousModePermissionPrompt, spinnerTipsEnabled, etc.) still apply
- **D-14:** The `no_sandbox` flag is passed from `cmd_up()` to the settings generation function

### Settings Lifecycle
- **D-15:** `rightclaw up` always regenerates `.claude/settings.json` for every discovered agent — deterministic, no drift
- **D-16:** All customization goes through `agent.yaml` — manual edits to `.claude/settings.json` are overwritten
- **D-17:** `rightclaw init` also generates initial `.claude/settings.json` for the default "right" agent (extracted from current init.rs logic)
- **D-18:** Settings generation extracted to new `codegen/settings.rs` module, called from both `init.rs` and `cmd_up()`

### Architecture
- **D-19:** New module `crates/rightclaw/src/codegen/settings.rs` — `pub fn generate_settings(agent: &AgentDef, no_sandbox: bool) -> miette::Result<serde_json::Value>`
- **D-20:** Function returns `serde_json::Value`, caller handles file write (pattern consistent with other codegen modules)
- **D-21:** Hook in `cmd_up()` agent loop: after wrapper generation, before process-compose generation
- **D-22:** `std::fs::create_dir_all(agent.path.join(".claude"))` before write (idempotent)

### Claude's Discretion
- Exact JSON structure for settings (key naming matches CC's schema)
- Test strategy for settings generation (unit tests with mock AgentDef)
- Whether to add `sandbox.filesystem.denyRead` or `sandbox.filesystem.denyWrite` defaults
- How to handle the `//` prefix convention in path generation (resolve at generation time or pass through)

</decisions>

<canonical_refs>
## Canonical References

**Downstream agents MUST read these before planning or implementing.**

### Existing codegen (pattern to follow)
- `crates/rightclaw/src/codegen/mod.rs` — module structure, public exports
- `crates/rightclaw/src/codegen/shell_wrapper.rs` — generate_wrapper() pattern: takes AgentDef, returns Result
- `crates/rightclaw/src/codegen/process_compose.rs` — another codegen module pattern

### Agent config schema
- `crates/rightclaw/src/agent/types.rs` — AgentConfig with deny_unknown_fields, where SandboxOverrides goes

### Settings generation source (to refactor)
- `crates/rightclaw/src/init.rs` — current settings.json generation at lines 78-103 (extract to codegen/settings.rs)

### Integration points
- `crates/rightclaw-cli/src/main.rs` — cmd_up() agent loop where settings generation hooks in
- `crates/rightclaw/src/agent/discovery.rs` — AgentDef construction, where new sandbox config is populated

### CC sandbox settings reference
- `.planning/research/STACK.md` — full settings.json schema (17 fields documented)
- `.planning/research/FEATURES.md` — feature landscape, sandbox config details

### Phase 5 context (build on this)
- `.planning/phases/05-remove-openshell/05-CONTEXT.md` — decisions D-01 through D-18

</canonical_refs>

<code_context>
## Existing Code Insights

### Reusable Assets
- `init.rs` lines 78-103: Settings JSON construction with serde_json::Value — extract to codegen/settings.rs
- `shell_wrapper.rs`: Telegram detection via `agent.mcp_config_path.is_some()` — reuse for conditional allowedDomains
- `serde_json` already in workspace dependencies — no new crates needed

### Established Patterns
- Codegen functions take `&AgentDef` and return `Result<T>` (shell_wrapper, process_compose, system_prompt)
- `deny_unknown_fields` on all serde structs with user-facing YAML
- `include_str!` for embedded templates — not needed here (JSON built programmatically)

### Integration Points
- `cmd_up()` agent loop (main.rs ~line 298): insert after generate_wrapper(), before generate_process_compose()
- `init_rightclaw_home()` in init.rs: replace inline settings generation with call to codegen/settings.rs
- `AgentConfig` struct: add `sandbox: Option<SandboxOverrides>` field

</code_context>

<specifics>
## Specific Ideas

- The `//` prefix convention from CC (double-slash for absolute paths) should be documented in agent.yaml comments or a template
- Merge semantics: `Vec::extend()` for array fields — user additions appended after defaults
- Consider adding `sandbox.enabled` to agent.yaml overrides too, so specific agents can opt out without global --no-sandbox

</specifics>

<deferred>
## Deferred Ideas

None — discussion stayed within phase scope

</deferred>

---

*Phase: 06-sandbox-configuration*
*Context gathered: 2026-03-24*
