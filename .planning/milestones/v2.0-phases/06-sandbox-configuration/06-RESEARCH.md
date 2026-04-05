# Phase 6: Sandbox Configuration - Research

**Researched:** 2026-03-24
**Domain:** Claude Code settings.json generation with per-agent sandbox configuration
**Confidence:** HIGH

## Summary

Phase 6 generates per-agent `.claude/settings.json` files during `rightclaw up` to configure CC's native sandbox. This is a codegen task building on the existing pattern in `codegen/shell_wrapper.rs` and `codegen/process_compose.rs` -- a new `codegen/settings.rs` module takes an `AgentDef` and a `no_sandbox` flag, returns a `serde_json::Value`, and the caller writes it to the agent's `.claude/` directory. The existing settings generation in `init.rs` (lines 78-103) is extracted and superseded by this new module.

The CC settings.json sandbox schema is well-documented and stable. Arrays in sandbox config (allowWrite, allowedDomains, denyRead) merge across scopes -- RightClaw writes to the project-level `.claude/settings.json` inside each agent dir, and CC merges those with any user-level settings. The `AgentConfig` struct needs a new optional `sandbox: Option<SandboxOverrides>` field with `deny_unknown_fields`, following the existing pattern.

**Primary recommendation:** Build `codegen/settings.rs` as a pure function `generate_settings(agent: &AgentDef, no_sandbox: bool) -> miette::Result<serde_json::Value>`, hook it into `cmd_up()` between wrapper generation and process-compose generation, and refactor `init.rs` to call it for the default agent.

<user_constraints>

## User Constraints (from CONTEXT.md)

### Locked Decisions
- **D-01:** Generated settings.json always includes: `sandbox.enabled: true`, `sandbox.autoAllowBashIfSandboxed: true`, `sandbox.allowUnsandboxedCommands: false`
- **D-02:** Filesystem: `sandbox.filesystem.allowWrite` scoped to agent's own directory only (`~/.rightclaw/agents/<name>/`)
- **D-03:** Network: `sandbox.network.allowedDomains` includes generous defaults: `api.anthropic.com`, `github.com`, `npmjs.org`, `crates.io`, `agentskills.io`, `api.telegram.org`
- **D-04:** Non-sandbox settings always included: `skipDangerousModePermissionPrompt: true`, `spinnerTipsEnabled: false`, `prefersReducedMotion: true`
- **D-05:** Telegram `enabledPlugins` included conditionally (same logic as current init.rs -- detected via `.mcp.json` presence)
- **D-06:** Nested `sandbox:` section in agent.yaml for per-agent overrides
- **D-07:** Override fields: `allow_write: Vec<String>`, `allowed_domains: Vec<String>`, `excluded_commands: Vec<String>`
- **D-08:** Override arrays MERGE with (not replace) generated defaults. User additions are appended.
- **D-09:** `AgentConfig` struct gets new optional `sandbox: Option<SandboxOverrides>` field
- **D-10:** `SandboxOverrides` is a new struct with `deny_unknown_fields` for strict validation
- **D-11:** CC path prefixes apply: `//` = absolute from root (legacy), `~/` = relative to HOME, `/` = absolute from root (current standard)
- **D-12:** When `--no-sandbox` flag is set, settings.json is still generated but with `sandbox.enabled: false`
- **D-13:** All other settings (skipDangerousModePermissionPrompt, spinnerTipsEnabled, etc.) still apply
- **D-14:** The `no_sandbox` flag is passed from `cmd_up()` to the settings generation function
- **D-15:** `rightclaw up` always regenerates `.claude/settings.json` for every discovered agent -- deterministic, no drift
- **D-16:** All customization goes through `agent.yaml` -- manual edits to `.claude/settings.json` are overwritten
- **D-17:** `rightclaw init` also generates initial `.claude/settings.json` for the default "right" agent (extracted from current init.rs logic)
- **D-18:** Settings generation extracted to new `codegen/settings.rs` module, called from both `init.rs` and `cmd_up()`
- **D-19:** New module `crates/rightclaw/src/codegen/settings.rs` -- `pub fn generate_settings(agent: &AgentDef, no_sandbox: bool) -> miette::Result<serde_json::Value>`
- **D-20:** Function returns `serde_json::Value`, caller handles file write (pattern consistent with other codegen modules)
- **D-21:** Hook in `cmd_up()` agent loop: after wrapper generation, before process-compose generation
- **D-22:** `std::fs::create_dir_all(agent.path.join(".claude"))` before write (idempotent)

### Claude's Discretion
- Exact JSON structure for settings (key naming matches CC's schema)
- Test strategy for settings generation (unit tests with mock AgentDef)
- Whether to add `sandbox.filesystem.denyRead` or `sandbox.filesystem.denyWrite` defaults
- How to handle the `//` prefix convention in path generation (resolve at generation time or pass through)

### Deferred Ideas (OUT OF SCOPE)
None -- discussion stayed within phase scope

</user_constraints>

<phase_requirements>

## Phase Requirements

| ID | Description | Research Support |
|----|-------------|------------------|
| SBCF-01 | `rightclaw up` generates per-agent `.claude/settings.json` in each agent directory with sandbox enabled | Codegen pattern from shell_wrapper.rs/process_compose.rs; CC settings.json schema fully documented; hook point identified in cmd_up() at line ~298 |
| SBCF-02 | Generated settings.json includes filesystem restrictions (allowWrite scoped to agent dir + workspace) | CC `sandbox.filesystem.allowWrite` field verified in official docs; path prefix `/` = absolute, `~/` = home-relative confirmed |
| SBCF-03 | Generated settings.json includes network restrictions (allowedDomains for required services) | CC `sandbox.network.allowedDomains` field verified; supports wildcards (e.g., `*.npmjs.org`) |
| SBCF-04 | Generated settings.json sets `allowUnsandboxedCommands: false` and `autoAllowBashIfSandboxed: true` | Both fields confirmed in official docs; `allowUnsandboxedCommands: false` disables the `dangerouslyDisableSandbox` escape hatch |
| SBCF-05 | User can override sandbox settings per-agent via `agent.yaml` sandbox section | AgentConfig struct uses `deny_unknown_fields`; new `SandboxOverrides` struct follows same pattern; serde-saphyr handles deserialization |
| SBCF-06 | agent.yaml sandbox overrides merge with (not replace) generated defaults | `Vec::extend()` for array merge; CC also merges arrays across scopes, so both layers are additive |

</phase_requirements>

## Standard Stack

### Core (already in workspace)

| Library | Version | Purpose | Why Standard |
|---------|---------|---------|--------------|
| serde_json | 1.0 | Build settings.json as `serde_json::Value` | Already in workspace deps; JSON construction via `serde_json::json!()` macro is the standard approach for dynamic JSON |
| serde | 1.0 | Derive `Deserialize` for `SandboxOverrides` struct | Already in workspace deps; `deny_unknown_fields` attribute gives strict validation |
| serde-saphyr | 0.0 | Parse `sandbox:` section from agent.yaml | Already used for `AgentConfig` deserialization |
| miette | 7.6 | Error reporting from settings generation | Consistent with all other codegen modules |

### No New Dependencies

This phase requires zero new crate dependencies. All required functionality is covered by the existing workspace:
- JSON construction: `serde_json::json!()` macro
- YAML parsing: `serde-saphyr` (extends existing `AgentConfig`)
- File I/O: `std::fs` (stdlib)
- Path manipulation: `std::path` (stdlib)

## Architecture Patterns

### Recommended Module Structure

```
crates/rightclaw/src/codegen/
  mod.rs                  # Add: pub mod settings; pub use settings::generate_settings;
  settings.rs             # NEW: generate_settings() function
  settings_tests.rs       # NEW: unit tests (extracted per project convention)
  shell_wrapper.rs        # Existing (unchanged)
  process_compose.rs      # Existing (unchanged)
  system_prompt.rs        # Existing (unchanged)
```

### Pattern 1: Codegen Function Signature

**What:** All codegen modules follow the same signature pattern: take `&AgentDef` (plus config args), return `Result<T>`.
**When to use:** Always for new codegen modules.

```rust
// Source: Existing pattern from shell_wrapper.rs and process_compose.rs

/// Generate a `.claude/settings.json` value for an agent.
///
/// When `no_sandbox` is true, sandbox.enabled is set to false but all other
/// settings are still generated (D-12, D-13).
pub fn generate_settings(agent: &AgentDef, no_sandbox: bool) -> miette::Result<serde_json::Value> {
    // ...
}
```

### Pattern 2: Settings JSON Structure

**What:** The exact JSON structure RightClaw generates for each agent.
**When to use:** This is the core output of `generate_settings()`.

```rust
// Source: CC official docs at https://code.claude.com/docs/en/settings

let mut settings = serde_json::json!({
    // Non-sandbox settings (D-04)
    "skipDangerousModePermissionPrompt": true,
    "spinnerTipsEnabled": false,
    "prefersReducedMotion": true,

    // Sandbox configuration (D-01)
    "sandbox": {
        "enabled": !no_sandbox,
        "autoAllowBashIfSandboxed": true,
        "allowUnsandboxedCommands": false,

        // Filesystem restrictions (D-02)
        "filesystem": {
            "allowWrite": base_allow_write  // Agent dir path
        },

        // Network restrictions (D-03)
        "network": {
            "allowedDomains": base_allowed_domains
        }
    }
});

// Telegram plugin (D-05) -- conditional on .mcp.json presence
if agent.mcp_config_path.is_some() {
    settings["enabledPlugins"] = serde_json::json!({
        "telegram@claude-plugins-official": true
    });
}
```

### Pattern 3: SandboxOverrides Struct

**What:** New struct added to `agent/types.rs` for user-facing sandbox overrides in `agent.yaml`.
**When to use:** Deserialized from the `sandbox:` section of `agent.yaml`.

```rust
// Source: Project convention from AgentConfig (deny_unknown_fields)

/// Per-agent sandbox overrides defined in agent.yaml.
///
/// All arrays MERGE with generated defaults (D-08).
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SandboxOverrides {
    /// Additional paths to allow writing (appended to defaults).
    #[serde(default)]
    pub allow_write: Vec<String>,

    /// Additional domains to allow (appended to defaults).
    #[serde(default)]
    pub allowed_domains: Vec<String>,

    /// Commands to exclude from sandbox (appended to defaults).
    #[serde(default)]
    pub excluded_commands: Vec<String>,
}
```

And added to `AgentConfig`:

```rust
pub struct AgentConfig {
    // ... existing fields ...
    pub sandbox: Option<SandboxOverrides>,
}
```

### Pattern 4: Merge Logic

**What:** User overrides extend (not replace) the generated defaults.
**When to use:** When building the final settings.json from defaults + agent.yaml overrides.

```rust
// Build base arrays
let mut allow_write = vec![agent_dir_path.clone()];
let mut allowed_domains = vec![
    "api.anthropic.com".to_string(),
    "github.com".to_string(),
    "npmjs.org".to_string(),
    "crates.io".to_string(),
    "agentskills.io".to_string(),
    "api.telegram.org".to_string(),
];
let mut excluded_commands: Vec<String> = vec![];

// Merge user overrides (D-08)
if let Some(ref config) = agent.config {
    if let Some(ref overrides) = config.sandbox {
        allow_write.extend(overrides.allow_write.iter().cloned());
        allowed_domains.extend(overrides.allowed_domains.iter().cloned());
        excluded_commands.extend(overrides.excluded_commands.iter().cloned());
    }
}
```

### Pattern 5: cmd_up() Integration Point

**What:** Where settings generation hooks into the agent launch flow.
**When to use:** In `cmd_up()` in `main.rs`, inside the agent loop.

```rust
// In cmd_up(), inside `for agent in &agents { ... }` loop:

// 1. Generate combined prompt (existing)
let combined_content = rightclaw::codegen::generate_combined_prompt(agent)?;
// ... write prompt file ...

// 2. Generate shell wrapper (existing)
let wrapper_content = rightclaw::codegen::generate_wrapper(agent, &prompt_path_str, debug_log.as_deref())?;
// ... write wrapper file ...

// 3. Generate settings.json (NEW -- D-21)
let settings = rightclaw::codegen::generate_settings(agent, no_sandbox)?;
let claude_dir = agent.path.join(".claude");
std::fs::create_dir_all(&claude_dir)
    .map_err(|e| miette::miette!("failed to create .claude dir for '{}': {e:#}", agent.name))?;
std::fs::write(
    claude_dir.join("settings.json"),
    serde_json::to_string_pretty(&settings)
        .map_err(|e| miette::miette!("failed to serialize settings for '{}': {e:#}", agent.name))?,
)
.map_err(|e| miette::miette!("failed to write settings.json for '{}': {e:#}", agent.name))?;
tracing::debug!(agent = %agent.name, "wrote settings.json");

// 4. Generate process-compose.yaml (existing, after loop)
```

### Pattern 6: init.rs Refactoring

**What:** Extract inline settings JSON from `init.rs` and delegate to `codegen::generate_settings()`.
**When to use:** During the `init_rightclaw_home()` refactor.

The current `init.rs` lines 78-103 build settings inline. After refactoring:

```rust
// In init.rs, replace inline settings block with:

// Build a minimal AgentDef for the default "right" agent.
let agent_def = AgentDef {
    name: "right".to_string(),
    path: agents_dir.clone(),
    identity_path: agents_dir.join("IDENTITY.md"),
    config: None,
    mcp_config_path: if telegram_token.is_some() {
        Some(agents_dir.join(".mcp.json"))
    } else {
        None
    },
    // ... other optional fields set to None ...
};

let settings = crate::codegen::generate_settings(&agent_def, false)?;
let claude_dir = agents_dir.join(".claude");
std::fs::create_dir_all(&claude_dir)?;
std::fs::write(
    claude_dir.join("settings.json"),
    serde_json::to_string_pretty(&settings)?,
)?;
```

**Subtlety:** In `init.rs`, the `.mcp.json` file doesn't exist yet at the time settings are generated. But the `AgentDef` is constructed with `mcp_config_path: Some(...)` when telegram_token is provided -- this controls the Telegram plugin logic. The actual `.mcp.json` file is created later in init. This is fine because `generate_settings()` only checks `agent.mcp_config_path.is_some()`, it doesn't read the file.

### Anti-Patterns to Avoid

- **Embedding settings template as a .json file:** Don't use `include_str!()` for a JSON template. Build JSON programmatically with `serde_json::json!()` -- it's type-safe, composable, and handles the merge logic cleanly.
- **Reading existing settings.json before overwriting:** D-15 says settings are always regenerated from agent.yaml as the single source of truth. Never read-modify-write the settings file.
- **Hardcoding the agent HOME path in allowWrite:** Use the agent's actual `path` field from `AgentDef`, not a hardcoded `~/.rightclaw/agents/<name>`. The path is already absolute from discovery.
- **Using `~` prefix for agent dir in allowWrite:** Since agent dir paths are absolute from `AgentDef.path`, use absolute paths in allowWrite. The `~/` prefix would be wrong if HOME is ever overridden (v2.1).

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| JSON construction | Manual string concatenation | `serde_json::json!()` macro | Type-safe, handles escaping, nesting, and null values correctly |
| YAML parsing for SandboxOverrides | Custom parser | `serde-saphyr` + `#[derive(Deserialize)]` | Already used for AgentConfig; deny_unknown_fields catches typos |
| Array merge semantics | Custom merge algorithm | `Vec::extend()` | Simple, correct, and CC also merges arrays across scopes |
| Pretty-printing JSON | Manual formatting | `serde_json::to_string_pretty()` | Human-readable output, consistent indentation |

## Common Pitfalls

### Pitfall 1: Path Prefix Confusion for allowWrite

**What goes wrong:** Using `~/` prefix for the agent directory path in `sandbox.filesystem.allowWrite` when the path should be absolute.
**Why it happens:** CC docs show `~/` for home-relative paths, tempting developers to use `~/.rightclaw/agents/<name>/`. But if HOME is ever overridden (v2.1), `~/` resolves to the wrong location.
**How to avoid:** Always use the absolute path from `AgentDef.path.display().to_string()`. Absolute paths with `/` prefix are unambiguous.
**Warning signs:** If settings.json contains `~/` for the agent directory, it's a bug.

### Pitfall 2: deny_unknown_fields Breaks Forward Compatibility

**What goes wrong:** Adding fields to `SandboxOverrides` later breaks existing agent.yaml files that don't know about the new fields.
**Why it happens:** `deny_unknown_fields` is strict by design -- but it only rejects fields not in the struct, not missing optional fields. This is actually fine because `#[serde(default)]` handles missing fields.
**How to avoid:** New fields in `SandboxOverrides` MUST have `#[serde(default)]` so existing agent.yaml files without them still parse. This is already the pattern used in `AgentConfig`.
**Warning signs:** None -- just remember to add `#[serde(default)]` on every new field.

### Pitfall 3: init.rs AgentDef Construction Timing

**What goes wrong:** In `init.rs`, the AgentDef is constructed before files exist on disk. If `generate_settings()` tries to read files (e.g., check if `.mcp.json` actually exists), it fails.
**Why it happens:** `init_rightclaw_home()` creates files in sequence -- the AgentDef is synthetic, not from `discover_agents()`.
**How to avoid:** `generate_settings()` must only read fields from the `AgentDef` struct, never stat files on disk. The `mcp_config_path: Option<PathBuf>` field is treated as a boolean signal (`.is_some()`), not a file to read.
**Warning signs:** If `generate_settings()` calls `std::fs::exists()` or `std::fs::read()`, it's a design error.

### Pitfall 4: no_sandbox Must Not Skip Settings Generation

**What goes wrong:** Developer skips settings.json generation entirely when `--no-sandbox` is set.
**Why it happens:** Natural assumption: "no sandbox = no settings needed."
**How to avoid:** D-12 and D-13 are explicit: settings.json is ALWAYS generated. With `--no-sandbox`, only `sandbox.enabled` changes to `false`. All other settings (skipDangerousModePermissionPrompt, spinnerTipsEnabled, Telegram plugin, etc.) still apply.
**Warning signs:** If the code has `if !no_sandbox { generate_settings(...) }`, it's wrong.

### Pitfall 5: Forgetting to Wire no_sandbox Through to generate_settings

**What goes wrong:** The `no_sandbox` flag exists in `cmd_up()` but isn't passed to the new function.
**Why it happens:** Currently `let _ = no_sandbox;` silences the unused warning. Easy to forget to actually use it.
**How to avoid:** Remove the `let _ = no_sandbox;` line and pass it to `generate_settings()`. Compiler will catch if unused after that.
**Warning signs:** The `let _ = no_sandbox;` line still present after Phase 6 implementation.

### Pitfall 6: .claude Directory Already Exists from Skills

**What goes wrong:** `create_dir_all` is called on `.claude/` but the directory may already exist (init.rs creates `.claude/skills/`).
**Why it happens:** Multiple code paths create `.claude/` subdirectories.
**How to avoid:** `create_dir_all` is idempotent by design (D-22). It creates the directory if missing, succeeds if it already exists. No issue here -- just be aware.
**Warning signs:** None. Using `create_dir` (without `_all`) would fail if the dir exists.

## Code Examples

### Complete generate_settings() Implementation

```rust
// Source: Synthesized from CC official docs + CONTEXT.md decisions

use crate::agent::AgentDef;

/// Generate a `.claude/settings.json` value for an agent.
///
/// Produces sandbox configuration with filesystem and network restrictions.
/// When `no_sandbox` is true, `sandbox.enabled` is `false` but all other
/// settings remain (agents still need skipDangerousModePermissionPrompt, etc.).
///
/// User overrides from `agent.yaml` `sandbox:` section are merged with
/// generated defaults (arrays are extended, not replaced).
pub fn generate_settings(agent: &AgentDef, no_sandbox: bool) -> miette::Result<serde_json::Value> {
    // Base filesystem allowWrite: agent's own directory (absolute path)
    let mut allow_write = vec![agent.path.display().to_string()];

    // Base allowed domains (D-03)
    let mut allowed_domains = vec![
        "api.anthropic.com".to_string(),
        "github.com".to_string(),
        "npmjs.org".to_string(),
        "crates.io".to_string(),
        "agentskills.io".to_string(),
        "api.telegram.org".to_string(),
    ];

    let mut excluded_commands: Vec<String> = vec![];

    // Merge user overrides from agent.yaml (D-08)
    if let Some(ref config) = agent.config {
        if let Some(ref overrides) = config.sandbox {
            allow_write.extend(overrides.allow_write.iter().cloned());
            allowed_domains.extend(overrides.allowed_domains.iter().cloned());
            excluded_commands.extend(overrides.excluded_commands.iter().cloned());
        }
    }

    let mut settings = serde_json::json!({
        // Non-sandbox settings (D-04)
        "skipDangerousModePermissionPrompt": true,
        "spinnerTipsEnabled": false,
        "prefersReducedMotion": true,

        // Sandbox config (D-01, D-12)
        "sandbox": {
            "enabled": !no_sandbox,
            "autoAllowBashIfSandboxed": true,
            "allowUnsandboxedCommands": false,
            "filesystem": {
                "allowWrite": allow_write,
            },
            "network": {
                "allowedDomains": allowed_domains,
            },
        }
    });

    // Add excludedCommands only if non-empty
    if !excluded_commands.is_empty() {
        settings["sandbox"]["excludedCommands"] = serde_json::json!(excluded_commands);
    }

    // Telegram plugin (D-05)
    if agent.mcp_config_path.is_some() {
        settings["enabledPlugins"] = serde_json::json!({
            "telegram@claude-plugins-official": true
        });
    }

    Ok(settings)
}
```

### agent.yaml with sandbox overrides

```yaml
# Example agent.yaml showing sandbox override usage
restart: on_failure
max_restarts: 5
model: sonnet

sandbox:
  allow_write:
    - "/tmp/builds"
    - "~/.cargo"
  allowed_domains:
    - "registry.npmjs.org"
    - "*.crates.io"
  excluded_commands:
    - "docker"
```

### SandboxOverrides Deserialization Test

```rust
#[test]
fn agent_config_with_sandbox_overrides() {
    let yaml = r#"
restart: on_failure
sandbox:
  allow_write:
    - "/tmp/builds"
  allowed_domains:
    - "registry.npmjs.org"
  excluded_commands:
    - "docker"
"#;
    let config: AgentConfig = serde_saphyr::from_str(yaml).unwrap();
    let sandbox = config.sandbox.unwrap();
    assert_eq!(sandbox.allow_write, vec!["/tmp/builds"]);
    assert_eq!(sandbox.allowed_domains, vec!["registry.npmjs.org"]);
    assert_eq!(sandbox.excluded_commands, vec!["docker"]);
}

#[test]
fn agent_config_without_sandbox_section() {
    let yaml = "restart: never";
    let config: AgentConfig = serde_saphyr::from_str(yaml).unwrap();
    assert!(config.sandbox.is_none());
}

#[test]
fn sandbox_overrides_rejects_unknown_fields() {
    let yaml = r#"
sandbox:
  allow_write: ["/tmp"]
  unknown_field: "bad"
"#;
    let result: Result<AgentConfig, _> = serde_saphyr::from_str(yaml);
    assert!(result.is_err());
}
```

### generate_settings() Unit Tests

```rust
#[test]
fn generates_sandbox_enabled_by_default() {
    let agent = make_test_agent("test-agent", None);
    let settings = generate_settings(&agent, false).unwrap();
    assert_eq!(settings["sandbox"]["enabled"], true);
    assert_eq!(settings["sandbox"]["autoAllowBashIfSandboxed"], true);
    assert_eq!(settings["sandbox"]["allowUnsandboxedCommands"], false);
}

#[test]
fn no_sandbox_disables_sandbox_only() {
    let agent = make_test_agent("test-agent", None);
    let settings = generate_settings(&agent, true).unwrap();
    assert_eq!(settings["sandbox"]["enabled"], false);
    // Other settings still present
    assert_eq!(settings["skipDangerousModePermissionPrompt"], true);
    assert_eq!(settings["spinnerTipsEnabled"], false);
}

#[test]
fn merges_user_overrides_with_defaults() {
    let overrides = SandboxOverrides {
        allow_write: vec!["/tmp/custom".to_string()],
        allowed_domains: vec!["custom.example.com".to_string()],
        excluded_commands: vec!["docker".to_string()],
    };
    let config = AgentConfig {
        sandbox: Some(overrides),
        ..Default::default()
    };
    let agent = make_test_agent("test-agent", Some(config));
    let settings = generate_settings(&agent, false).unwrap();

    let allow_write = settings["sandbox"]["filesystem"]["allowWrite"]
        .as_array().unwrap();
    // Default (agent dir) + user override
    assert!(allow_write.len() >= 2);
    assert!(allow_write.iter().any(|v| v == "/tmp/custom"));

    let domains = settings["sandbox"]["network"]["allowedDomains"]
        .as_array().unwrap();
    assert!(domains.iter().any(|v| v == "custom.example.com"));
    // Defaults still present
    assert!(domains.iter().any(|v| v == "api.anthropic.com"));

    // excludedCommands present
    assert_eq!(settings["sandbox"]["excludedCommands"][0], "docker");
}

#[test]
fn includes_telegram_plugin_when_mcp_present() {
    let mut agent = make_test_agent("test-agent", None);
    agent.mcp_config_path = Some(PathBuf::from("/fake/.mcp.json"));
    let settings = generate_settings(&agent, false).unwrap();
    assert_eq!(
        settings["enabledPlugins"]["telegram@claude-plugins-official"],
        true
    );
}

#[test]
fn omits_telegram_plugin_when_no_mcp() {
    let agent = make_test_agent("test-agent", None);
    let settings = generate_settings(&agent, false).unwrap();
    assert!(settings.get("enabledPlugins").is_none());
}
```

## State of the Art

| Old Approach | Current Approach | When Changed | Impact |
|--------------|------------------|--------------|--------|
| `//path` prefix for absolute paths in sandbox settings | `/path` for absolute (standard convention) | CC docs updated, `//` still works as legacy | Use `/path` for new code; `//` backwards-compatible but non-standard |
| OpenShell `policy.yaml` for sandbox config | `.claude/settings.json` with `sandbox.*` fields | v2.0 (Phase 5 removed OpenShell) | RightClaw generates settings.json instead of policy.yaml |
| `--dangerously-skip-permissions` CLI flag for autonomous mode | `defaultMode: "bypassPermissions"` in settings.json | CC mid-2025 | Can set in settings.json; RightClaw still uses CLI flag but settings approach is cleaner |
| `serde_yaml` for YAML parsing | `serde-saphyr` | serde_yaml archived March 2024 | Already migrated in project |

**CC sandbox settings verified against official docs (2026-03-24):**
- All fields in STACK.md schema remain current and accurate
- Path prefix convention clarified: `/` = absolute (standard), `//` = absolute (legacy, still works)
- Array merge behavior confirmed: arrays merge across scopes (user, project, managed)
- `sandbox.enabled` default is `false` -- must be explicitly set to `true`

## Open Questions

1. **Should `sandbox.filesystem.denyRead` include sensitive paths by default?**
   - What we know: CC docs suggest `denyRead: ["~/.aws/credentials", "~/.ssh"]` as best practice. STACK.md research recommended `denyRead` for SSH, AWS, GPG, Docker config.
   - What's unclear: CONTEXT.md D-02 only specifies `allowWrite`, not `denyRead`. This is left to Claude's discretion.
   - Recommendation: Include `denyRead` defaults for `~/.ssh`, `~/.aws`, `~/.gnupg` since RightClaw positions itself as "security-first." Use absolute paths (not `~/`) to be safe if HOME is later overridden. However, note that `~` resolution depends on the runtime HOME, so for v2.0 (no HOME override), `~/` works fine and is more portable than hardcoded `/home/<user>/`. **Recommendation: use `~/.ssh`, `~/.aws`, `~/.gnupg` with `~/` prefix for now.** This will resolve correctly in the current setup and will need revisiting in v2.1 when HOME isolation is added.

2. **Should `excludedCommands` be omitted or set to empty array when no overrides?**
   - What we know: CC defaults to `[]` when omitted. Including an empty array is harmless but verbose.
   - Recommendation: Omit `excludedCommands` from generated JSON when empty (cleaner output). Only include when user overrides provide entries.

3. **`AgentConfig` Default trait for test helpers**
   - What we know: `AgentConfig` doesn't derive `Default` currently (it has `default_max_restarts()` and `default_backoff_seconds()` custom defaults). Adding `Default` would simplify test helpers.
   - Recommendation: Leave as-is for production code (constructing `AgentConfig` explicitly is safer). For tests, create a `make_test_agent()` helper function that builds a minimal `AgentDef`.

## Sources

### Primary (HIGH confidence)
- [Claude Code Settings Reference](https://code.claude.com/docs/en/settings) - Complete sandbox settings.json schema, path prefixes, array merge behavior. Verified 2026-03-24.
- [Claude Code Sandboxing Docs](https://code.claude.com/docs/en/sandboxing) - Sandbox behavior, filesystem/network isolation, how sandbox.enabled interacts with permissions. Verified 2026-03-24.
- Existing codebase: `codegen/shell_wrapper.rs`, `codegen/process_compose.rs`, `agent/types.rs`, `init.rs` - Codegen patterns, AgentConfig schema, current settings generation.

### Secondary (MEDIUM confidence)
- `.planning/research/STACK.md` - Full settings.json schema with field reference table. Researched 2026-03-23.
- `.planning/research/FEATURES.md` - Feature landscape, sandbox config details, HOME override implications. Researched 2026-03-23.

## Metadata

**Confidence breakdown:**
- Standard stack: HIGH - No new dependencies, all crates verified in workspace
- Architecture: HIGH - Pattern directly follows existing codegen modules (shell_wrapper, process_compose)
- Pitfalls: HIGH - Well-understood from v1.0 experience, CC official docs, and CONTEXT.md decisions
- CC sandbox schema: HIGH - Verified against official docs 2026-03-24

**Research date:** 2026-03-24
**Valid until:** 2026-04-24 (CC sandbox schema is stable; settings.json fields rarely change)
