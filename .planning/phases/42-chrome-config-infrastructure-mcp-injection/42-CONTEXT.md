# Phase 42: Chrome Config Infrastructure + MCP Injection - Context

**Gathered:** 2026-04-06
**Status:** Ready for planning

<domain>
## Phase Boundary

Per-agent `.mcp.json` carries a working `chrome-devtools` entry on every `rightclaw up` when Chrome is configured. Covers: ChromeConfig struct in global config, MCP injection into `.mcp.json`, sandbox overrides in `settings.json`.

Chrome init detection (`rightclaw init` logic, `which` discovery, standard path fallbacks) is Phase 43. Doctor check and AGENTS.md template are Phase 44.

</domain>

<decisions>
## Implementation Decisions

### ChromeConfig struct
- **D-01:** Add `ChromeConfig` struct to `crates/rightclaw/src/config.rs` alongside existing `TunnelConfig`
- **D-02:** Fields: `chrome_path: PathBuf` (Chrome browser binary) and `mcp_binary_path: PathBuf` (absolute path to `chrome-devtools-mcp` binary)
- **D-03:** Both fields are required within `ChromeConfig`; the entire `chrome` section is optional in `GlobalConfig` (`pub chrome: Option<ChromeConfig>`)
- **D-04:** Follow the existing `RawGlobalConfig`/`RawTunnelConfig` deserialization pattern — add `RawChromeConfig` with the same two fields as `Option<PathBuf>`

### MCP injection
- **D-05:** `mcp_binary_path` comes from `config.yaml` — set by `rightclaw init` (Phase 43). Phase 42 only reads it.
- **D-06:** Extend `generate_mcp_config()` in `crates/rightclaw/src/codegen/mcp_config.rs` with an optional `chrome_config: Option<&ChromeConfig>` parameter
- **D-07:** When `chrome_config` is `Some`: inject a `chrome-devtools` entry into `.mcp.json` using the read-modify-write pattern already in place. Key: `"chrome-devtools"`, command: absolute `mcp_binary_path`, args: `["--executablePath", "<chrome_path>", "--headless", "--isolated", "--no-sandbox", "--userDataDir", "<agent_dir>/.chrome-profile"]`
- **D-08:** MCP entry uses `command` + `args` fields (no `npx`, no `env` block needed)

### Sandbox settings injection
- **D-09:** Extend `generate_settings()` in `crates/rightclaw/src/codegen/settings.rs` with an optional `chrome_config: Option<&ChromeConfig>` parameter
- **D-10:** When `chrome_config` is `Some`: extend `allowed_commands` with `chrome_path.to_string_lossy()`, extend `allow_write` with `agent.path.join(".chrome-profile").display().to_string()`
- **D-11:** Chrome overrides are added AFTER user `SandboxOverrides` from `agent.yaml` using the same `Vec::extend` pattern (lines 49-56 in settings.rs) — additive, never clobbering

### rightclaw up integration
- **D-12:** In `cmd_up()` (main.rs ~line 526), after reading `global_cfg` (~line 706): extract `let chrome_cfg = global_cfg.chrome.as_ref()`
- **D-13:** Pass `chrome_cfg` into `generate_settings()` call (~line 618) and `generate_mcp_config()` call (~line 701)
- **D-14:** If `chrome` is not set in global config → skip Chrome injection entirely (no warn needed at this phase; INJECT-03 graceful degradation is Phase 43)

### Claude's Discretion
- Whether to add a `chrome_profile` field to `ChromeConfig` or hardcode the `.chrome-profile` subdirectory name
- Whether `.chrome-profile` dir is pre-created by rightclaw or left to Chrome to create on first launch
- Whether the `chrome-devtools-mcp` entry is idempotent-safe (overwrite if already present — yes, follow existing pattern)

</decisions>

<canonical_refs>
## Canonical References

**Downstream agents MUST read these before planning or implementing.**

### Requirements
- `.planning/REQUIREMENTS.md` — INJECT-01, INJECT-02, SBOX-01, SBOX-02 (exact arg list, binary path rule, additive merge requirement)

### MCP config generation
- `crates/rightclaw/src/codegen/mcp_config.rs` — `generate_mcp_config()`: read-modify-write pattern, existing `rightmemory` entry as template for `chrome-devtools` entry
- `crates/rightclaw/src/codegen/mod.rs` — public exports to update

### Settings generation
- `crates/rightclaw/src/codegen/settings.rs` — `generate_settings()`: additive Vec::extend merge pattern (lines 48-56); where Chrome overrides slot in

### Global config
- `crates/rightclaw/src/config.rs` — `GlobalConfig`, `TunnelConfig`, `RawGlobalConfig` pattern to follow for `ChromeConfig`/`RawChromeConfig`

### Agent types
- `crates/rightclaw/src/agent/types.rs` — `SandboxOverrides`, `AgentConfig`, `AgentDef`

### Up command
- `crates/rightclaw-cli/src/main.rs` — `cmd_up()` lines ~526-710: global_cfg read point (~706), settings call (~618), mcp_config call (~701)

</canonical_refs>

<code_context>
## Existing Code Insights

### Reusable Assets
- `generate_mcp_config()` in `mcp_config.rs`: already idempotent read-modify-write — `chrome-devtools` entry follows the exact same shape as `rightmemory`
- `generate_settings()` in `settings.rs`: additive Vec::extend merge already implemented for user SandboxOverrides; Chrome overrides use the same mechanism
- `TunnelConfig` / `RawTunnelConfig` in `config.rs`: exact pattern to follow for `ChromeConfig` / `RawChromeConfig`

### Established Patterns
- **Config deserialization**: Two-level structs (`Raw*` with `Option<PathBuf>` → convert to concrete type with validation). Follow `TunnelConfig` pattern exactly.
- **Optional feature injection**: Read global config first, extract optional subsection, pass as `Option<&T>` to generators. Pattern set by tunnel config in `cmd_up()`.
- **Additive sandbox merging**: `Vec::extend()` after user overrides — Chrome overrides go last (lowest priority relative to user overrides in terms of ordering, but since they don't overlap, order doesn't matter).

### Integration Points
- `cmd_up()` in `main.rs`: two injection points — `generate_settings()` call and `generate_mcp_config()` call
- Both generator functions get the same `Option<&ChromeConfig>` parameter — no need for separate chrome detection at each call site

</code_context>

<specifics>
## Specific Ideas

- `mcp_binary_path` resolution strategy: `rightclaw init` (Phase 43) does `which chrome-devtools-mcp` first, then falls back to standard locations (Linux: `/usr/local/bin`, `~/.npm-global/bin`; macOS: `/usr/local/bin`, homebrew bin, `~/.npm-global/bin`). Phase 42 only reads the stored value — no discovery here.
- Args order for `chrome-devtools` entry (from INJECT-02): `["--executablePath", "<chrome_path>", "--headless", "--isolated", "--no-sandbox", "--userDataDir", "<agent_dir>/.chrome-profile"]`

</specifics>

<deferred>
## Deferred Ideas

- Per-agent `chrome.enabled: false` opt-out in `agent.yaml` — deferred to future milestone (CHROME-AGENT-01 in requirements Out of Scope)
- Chrome as a separate process-compose service — v3.5 (CHROME-EXT-01)
- Chrome version enforcement — warn-only, deferred

</deferred>

---

*Phase: 42-chrome-config-infrastructure-mcp-injection*
*Context gathered: 2026-04-06*
