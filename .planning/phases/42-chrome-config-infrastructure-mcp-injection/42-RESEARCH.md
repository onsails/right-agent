# Phase 42: Chrome Config Infrastructure + MCP Injection - Research

**Researched:** 2026-04-06
**Domain:** Rust config extension, MCP JSON generation, sandbox settings generation
**Confidence:** HIGH

## Summary

Phase 42 is a pure Rust codebase extension with no greenfield work. Every pattern required — two-level config deserialization, optional-feature injection into MCP JSON, additive sandbox override merging — already exists in the codebase and has a concrete template to follow. The work is mechanical: replicate `TunnelConfig`/`RawTunnelConfig` for Chrome, extend two generator functions with an `Option<&ChromeConfig>` parameter, wire both into `cmd_up()`.

The `chrome-devtools-mcp` package installs a binary named `chrome-devtools-mcp` when installed globally via npm. The `--no-sandbox` flag it accepts is a Chrome browser flag (not an MCP server flag) — it is required when Chrome runs as root or inside bubblewrap where the inner Chrome sandbox is disabled by the outer bwrap sandbox. The `--isolated` flag creates a temporary throw-away user-data-dir; since this phase passes `--userDataDir`, `--isolated` and `--userDataDir` coexist fine (isolated just doesn't auto-generate the dir — the explicit path wins).

The `write_global_config()` function in `config.rs` does manual YAML string building (serde-saphyr is deserialize-only). Phase 42 must extend that function to emit the `chrome:` section when present.

**Primary recommendation:** Follow the `TunnelConfig` pattern exactly. Two files to extend (`config.rs`, `codegen/mcp_config.rs`, `codegen/settings.rs`) plus two call sites to update in `main.rs`. All patterns are in-tree.

---

<user_constraints>
## User Constraints (from CONTEXT.md)

### Locked Decisions

**D-01:** Add `ChromeConfig` struct to `crates/rightclaw/src/config.rs` alongside existing `TunnelConfig`

**D-02:** Fields: `chrome_path: PathBuf` (Chrome browser binary) and `mcp_binary_path: PathBuf` (absolute path to `chrome-devtools-mcp` binary)

**D-03:** Both fields are required within `ChromeConfig`; the entire `chrome` section is optional in `GlobalConfig` (`pub chrome: Option<ChromeConfig>`)

**D-04:** Follow the existing `RawGlobalConfig`/`RawTunnelConfig` deserialization pattern — add `RawChromeConfig` with the same two fields as `Option<PathBuf>`

**D-05:** `mcp_binary_path` comes from `config.yaml` — set by `rightclaw init` (Phase 43). Phase 42 only reads it.

**D-06:** Extend `generate_mcp_config()` in `crates/rightclaw/src/codegen/mcp_config.rs` with an optional `chrome_config: Option<&ChromeConfig>` parameter

**D-07:** When `chrome_config` is `Some`: inject a `chrome-devtools` entry into `.mcp.json` using the read-modify-write pattern already in place. Key: `"chrome-devtools"`, command: absolute `mcp_binary_path`, args: `["--executablePath", "<chrome_path>", "--headless", "--isolated", "--no-sandbox", "--userDataDir", "<agent_dir>/.chrome-profile"]`

**D-08:** MCP entry uses `command` + `args` fields (no `npx`, no `env` block needed)

**D-09:** Extend `generate_settings()` in `crates/rightclaw/src/codegen/settings.rs` with an optional `chrome_config: Option<&ChromeConfig>` parameter

**D-10:** When `chrome_config` is `Some`: extend `allowed_commands` with `chrome_path.to_string_lossy()`, extend `allow_write` with `agent.path.join(".chrome-profile").display().to_string()`

**D-11:** Chrome overrides are added AFTER user `SandboxOverrides` from `agent.yaml` using the same `Vec::extend` pattern (lines 49-56 in settings.rs) — additive, never clobbering

**D-12:** In `cmd_up()` (main.rs ~line 526), after reading `global_cfg` (~line 706): extract `let chrome_cfg = global_cfg.chrome.as_ref()`

**D-13:** Pass `chrome_cfg` into `generate_settings()` call (~line 618) and `generate_mcp_config()` call (~line 701)

**D-14:** If `chrome` is not set in global config → skip Chrome injection entirely (no warn needed at this phase; INJECT-03 graceful degradation is Phase 43)

### Claude's Discretion

- Whether to add a `chrome_profile` field to `ChromeConfig` or hardcode the `.chrome-profile` subdirectory name
- Whether `.chrome-profile` dir is pre-created by rightclaw or left to Chrome to create on first launch
- Whether the `chrome-devtools-mcp` entry is idempotent-safe (overwrite if already present — yes, follow existing pattern)

### Deferred Ideas (OUT OF SCOPE)

- Per-agent `chrome.enabled: false` opt-out in `agent.yaml` — deferred to future milestone
- Chrome as a separate process-compose service — v3.5
- Chrome version enforcement — warn-only, deferred
- `rightclaw init` Chrome detection logic — Phase 43
- Doctor check and AGENTS.md template — Phase 44
</user_constraints>

---

<phase_requirements>
## Phase Requirements

| ID | Description | Research Support |
|----|-------------|------------------|
| INJECT-01 | `rightclaw up` injects `chrome-devtools` entry into per-agent `.mcp.json` when `chrome.chrome_path` is set; uses absolute path to `chrome-devtools-mcp` binary (never `npx`) | D-06, D-07, D-08 + existing `generate_mcp_config()` read-modify-write pattern |
| INJECT-02 | Generated entry passes `--executablePath`, `--headless`, `--isolated`, `--no-sandbox`, `--userDataDir <agent_dir>/.chrome-profile` | D-07 exact arg list; `--no-sandbox` is a Chrome browser flag needed inside bwrap |
| SBOX-01 | `generate_settings()` adds Chrome binary to `allowedCommands`, `.chrome-profile` dir to `allowWrite` | D-09, D-10 + existing `Vec::extend` pattern in settings.rs lines 49-56 |
| SBOX-02 | Chrome sandbox overrides are additive — merged with existing `SandboxOverrides` from `agent.yaml` using `Vec::extend` | D-11 + verified in settings.rs source code |
</phase_requirements>

---

## Standard Stack

### Core (no new dependencies needed)

All implementation uses existing in-tree dependencies.

| Library | Version | Purpose | Source |
|---------|---------|---------|--------|
| serde-saphyr | in Cargo.toml | YAML deserialization for `RawChromeConfig` | existing |
| serde_json | in Cargo.toml | `.mcp.json` read-modify-write | existing |
| miette | in Cargo.toml | Error propagation | existing |
| std::path::PathBuf | stdlib | Chrome path types | existing |

**No new Cargo dependencies.** [VERIFIED: codebase inspection]

### External Binary

| Tool | Install | Binary Name After `npm install -g` | Location |
|------|---------|-----------------------------------|----|
| chrome-devtools-mcp | `npm install -g chrome-devtools-mcp` | `chrome-devtools-mcp` | typically `/usr/local/bin/chrome-devtools-mcp` or `~/.npm-global/bin/chrome-devtools-mcp` |

[CITED: github.com/ChromeDevTools/chrome-devtools-mcp package.json bin field — `"chrome-devtools-mcp": "./build/src/bin/chrome-devtools-mcp.js"`]

---

## Architecture Patterns

### Recommended File Touch List

```
crates/rightclaw/src/
├── config.rs                   # Add ChromeConfig, RawChromeConfig; extend GlobalConfig, RawGlobalConfig, read_global_config(), write_global_config()
├── codegen/
│   ├── mcp_config.rs           # Extend generate_mcp_config() with chrome_config: Option<&ChromeConfig>
│   └── settings.rs             # Extend generate_settings() with chrome_config: Option<&ChromeConfig>
crates/rightclaw-cli/src/
└── main.rs                     # cmd_up(): extract chrome_cfg, pass to both generator calls
```

### Pattern 1: Two-Level Config Deserialization (TunnelConfig template)

**What:** `Raw*` struct with `Option<String>` or `Option<PathBuf>` fields deserialized from YAML, then validated and converted to concrete typed struct.

**When to use:** Any new section in `config.yaml`. Required because serde-saphyr is deserialize-only (write is manual).

**Template from `config.rs` lines 41-55 + 70-88:**
```rust
// Source: crates/rightclaw/src/config.rs
#[derive(Debug, Deserialize)]
struct RawChromeConfig {
    #[serde(default)]
    chrome_path: String,
    #[serde(default)]
    mcp_binary_path: String,
}

#[derive(Debug, Clone)]
pub struct ChromeConfig {
    pub chrome_path: PathBuf,
    pub mcp_binary_path: PathBuf,
}

// In read_global_config(): conversion with validation
raw.chrome
    .map(|c| -> miette::Result<ChromeConfig> {
        if c.chrome_path.is_empty() || c.mcp_binary_path.is_empty() {
            return Err(miette::miette!("chrome config missing chrome_path or mcp_binary_path"));
        }
        Ok(ChromeConfig {
            chrome_path: PathBuf::from(c.chrome_path),
            mcp_binary_path: PathBuf::from(c.mcp_binary_path),
        })
    })
    .transpose()?
```

### Pattern 2: MCP Entry Injection (rightmemory template)

**What:** Read `.mcp.json`, ensure `mcpServers` object exists, insert/overwrite key, write back. Idempotent.

**Template from `mcp_config.rs` lines 38-49:**
```rust
// Source: crates/rightclaw/src/codegen/mcp_config.rs
if let Some(chrome) = chrome_config {
    let profile_dir = agent_path.join(".chrome-profile");
    servers.insert(
        "chrome-devtools".to_string(),
        serde_json::json!({
            "command": chrome.mcp_binary_path.to_string_lossy(),
            "args": [
                "--executablePath", chrome.chrome_path.to_string_lossy().as_ref(),
                "--headless",
                "--isolated",
                "--no-sandbox",
                "--userDataDir", profile_dir.to_string_lossy().as_ref()
            ]
        }),
    );
}
```

### Pattern 3: Additive Sandbox Override (Vec::extend template)

**What:** Extend `allow_write` and `allowed_commands` after user overrides — Chrome overrides go last.

**Insertion point — after lines 49-56 in `settings.rs`:**
```rust
// Source: crates/rightclaw/src/codegen/settings.rs (after user override block)
if let Some(chrome) = chrome_config {
    allow_write.push(agent.path.join(".chrome-profile").display().to_string());
    // allowed_commands doesn't exist yet as a Vec in settings.rs —
    // see PITFALL #1 below for the correct CC settings field name
}
```

### Pattern 4: write_global_config() Extension

**What:** serde-saphyr is deserialize-only. `write_global_config()` builds YAML by hand with `String::push_str`. Must extend to emit the `chrome:` section.

**Template from `config.rs` lines 96-103:**
```rust
// Source: crates/rightclaw/src/config.rs
if let Some(ref chrome) = config.chrome {
    content.push_str("chrome:\n");
    let chrome_path = chrome.chrome_path.display().to_string().replace('"', "\\\"");
    let mcp_path = chrome.mcp_binary_path.display().to_string().replace('"', "\\\"");
    content.push_str(&format!("  chrome_path: \"{chrome_path}\"\n"));
    content.push_str(&format!("  mcp_binary_path: \"{mcp_path}\"\n"));
}
```

### Anti-Patterns to Avoid

- **Using `npx` in the MCP command field:** INJECT-01 explicitly prohibits this. Always use the absolute `mcp_binary_path` stored in config.
- **Replacing user SandboxOverrides instead of extending:** The `Vec::extend` pattern is additive — never reassign the Vec.
- **Reading `global_cfg` inside the per-agent loop:** Current code reads `global_cfg` once after the loop (line 706). Chrome config extraction must stay outside the loop too. [VERIFIED: main.rs inspection]
- **Skipping `write_global_config()` extension:** Phase 43 writes Chrome config via `rightclaw init` — if `write_global_config` doesn't emit the `chrome:` section, Phase 43 cannot persist it.

---

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| YAML writing | Custom serializer | Manual `String::push_str` (existing pattern) | serde-saphyr is deserialize-only; existing `write_global_config()` already does this |
| JSON read-modify-write for `.mcp.json` | Custom merge | `serde_json::Value` map insert (existing pattern in `mcp_config.rs`) | Already idempotent, handles all edge cases |
| Chrome path quoting | Custom escaping | `.replace('"', "\\\"")` (existing pattern in `write_global_config`) | Same pattern as `TunnelConfig` serialization |

---

## Common Pitfalls

### Pitfall 1: `allowedCommands` vs `excludedCommands` — Wrong Field Name

**What goes wrong:** Injecting Chrome binary path into `excludedCommands` instead of `allowedCommands`. These are opposite.

**Why it happens:** `settings.rs` has an `excluded_commands: Vec<String>` local variable and a `excludedCommands` JSON field. The Chrome binary needs to be ALLOWED, not excluded. CC's `allowedCommands` (distinct from `allowWrite`) is the relevant sandbox field for executable allowlisting.

**How to avoid:** Check the actual CC `settings.json` schema. Looking at `settings.rs`, there is currently NO `allowedCommands` field being emitted. SBOX-01 requires adding one. The field name in CC's sandbox JSON is `allowedCommands` (camelCase). [ASSUMED — CC settings.json schema; verify by reading CC documentation or existing settings output before implementing]

**Warning signs:** Agent fails to run Chrome binary with a sandbox permission error after `rightclaw up`.

### Pitfall 2: `--isolated` + `--userDataDir` Interaction

**What goes wrong:** Assuming `--isolated` conflicts with `--userDataDir` and removing one.

**Why it happens:** `--isolated` creates a temporary auto-cleaned profile by default; but when `--userDataDir` is explicitly provided, the explicit path is used (isolated just doesn't auto-generate). They coexist correctly.

**How to avoid:** Pass both as specified in D-07. [CITED: github.com/ChromeDevTools/chrome-devtools-mcp docs/cli.md — "Isolated is enabled by default unless --userDataDir is provided"]

**Impact:** If `--isolated` is dropped, Chrome won't be properly isolated. If `--userDataDir` is dropped, Chrome creates profile in default system location, polluting agent isolation.

### Pitfall 3: `global_cfg` Read Position vs Per-Agent Loop

**What goes wrong:** Moving the `global_cfg` read inside the per-agent loop, reading config.yaml once per agent instead of once total.

**Why it happens:** The per-agent loop (lines 616-703) runs before `read_global_config()` (line 706). When wiring chrome_cfg into the loop, it's tempting to move global_cfg into the loop.

**How to avoid:** Read `global_cfg` before the per-agent loop (hoisting from line 706 to before line 616), or extract `chrome_cfg` before the loop and close over it. [VERIFIED: main.rs inspection — current structure puts global_cfg read after the per-agent loop]

**Warning signs:** `rightclaw up` reads config.yaml N times for N agents — harmless but wasteful.

### Pitfall 4: `write_global_config()` Not Extended

**What goes wrong:** Phase 42 adds `ChromeConfig` but doesn't extend `write_global_config()`, so Phase 43 (`rightclaw init`) cannot persist chrome config to disk.

**Why it happens:** Phase 42's success criteria are observable at `rightclaw up` time (reading config). Writing chrome config is Phase 43's job — but the writer must be ready.

**How to avoid:** Extend `write_global_config()` in the same PR as the struct addition. The function is short (lines 91-108) and the pattern is identical to the tunnel section. [VERIFIED: config.rs inspection]

### Pitfall 5: `AgentConfig` has `#[serde(deny_unknown_fields)]`

**What goes wrong:** Adding a `chrome` field to `AgentConfig` or `SandboxOverrides` for per-agent overrides breaks deserialization of all existing `agent.yaml` files that don't have that field.

**Why it happens:** Both structs use `#[serde(deny_unknown_fields)]` (types.rs lines 28, 49). Any field not in the struct causes a parse error.

**How to avoid:** Don't add chrome fields to `AgentConfig` or `SandboxOverrides` in this phase. Chrome overrides are global-only for v3.4. [VERIFIED: agent/types.rs inspection]

---

## Code Examples

### Full ChromeConfig addition to config.rs

```rust
// Source: pattern from crates/rightclaw/src/config.rs TunnelConfig

#[derive(Debug, Clone)]
pub struct ChromeConfig {
    pub chrome_path: PathBuf,
    pub mcp_binary_path: PathBuf,
}

#[derive(Debug, Deserialize)]
struct RawChromeConfig {
    #[serde(default)]
    chrome_path: String,
    #[serde(default)]
    mcp_binary_path: String,
}

// GlobalConfig gets: pub chrome: Option<ChromeConfig>,
// RawGlobalConfig gets: chrome: Option<RawChromeConfig>,
// read_global_config() map: raw.chrome.map(|c| {...}).transpose()?
// write_global_config() gets chrome section emitter
```

### generate_mcp_config() signature change

```rust
// Source: crates/rightclaw/src/codegen/mcp_config.rs
pub fn generate_mcp_config(
    agent_path: &Path,
    binary: &Path,
    agent_name: &str,
    rightclaw_home: &Path,
    chrome_config: Option<&ChromeConfig>,  // NEW
) -> miette::Result<()>
```

### generate_settings() signature change

```rust
// Source: crates/rightclaw/src/codegen/settings.rs
pub fn generate_settings(
    agent: &AgentDef,
    no_sandbox: bool,
    host_home: &Path,
    rg_path: Option<PathBuf>,
    chrome_config: Option<&ChromeConfig>,  // NEW
) -> miette::Result<serde_json::Value>
```

### cmd_up() wiring (main.rs)

```rust
// Hoist global_cfg read to before the per-agent loop (currently at line 706)
let global_cfg = rightclaw::config::read_global_config(home)?;
let chrome_cfg = global_cfg.chrome.as_ref();

// Inside per-agent loop — settings call (~line 618):
let settings = rightclaw::codegen::generate_settings(
    agent, no_sandbox, &host_home, rg_path.clone(), chrome_cfg
)?;

// Inside per-agent loop — mcp_config call (~line 701):
rightclaw::codegen::generate_mcp_config(
    &agent.path, &self_exe, &agent.name, home, chrome_cfg
)?;
```

---

## State of the Art

| Old Approach | Current Approach | Notes |
|--------------|------------------|-------|
| `npx chrome-devtools-mcp@latest` | Absolute path to globally-installed binary | Decided in v3.4 research — no npx in .mcp.json |
| `serde_yaml` | `serde-saphyr` | serde_yaml deprecated March 2024 |

---

## Assumptions Log

| # | Claim | Section | Risk if Wrong |
|---|-------|---------|---------------|
| A1 | CC `settings.json` sandbox accepts an `allowedCommands` field to whitelist executable paths for Chrome | Pitfall 1, SBOX-01 | If field name is different (e.g. `allowedBinaries`, or Chrome allowlisting works differently), SBOX-01 implementation is wrong — need to verify CC settings schema |

**Note on A1:** Current `settings.rs` has `excludedCommands` (a list of commands to block from sandbox). SBOX-01 says "Chrome binary path added to `allowedCommands`" — this is a CC sandbox concept that may or may not be implemented as a simple list field. The existing code does not emit `allowedCommands` anywhere, so this is net-new. Verify against CC settings.json documentation before implementing.

---

## Open Questions

1. **Does CC `settings.json` have an `allowedCommands` field for sandbox executable whitelisting?**
   - What we know: SBOX-01 requires Chrome binary in `allowedCommands`. Current `settings.rs` emits no such field.
   - What's unclear: The exact field name and schema position in CC's sandbox config. Existing code has `excludedCommands` (block list), which is the opposite.
   - Recommendation: Read current CC settings.json schema docs or inspect a running agent's settings.json before implementing SBOX-01. If the field doesn't exist, the allowWrite for `.chrome-profile` is still required and implementable.

2. **Should `write_global_config()` be extended in Phase 42 or Phase 43?**
   - What we know: Phase 42 adds the reader; Phase 43 uses the writer. If not extended in Phase 42, Phase 43 will fail to persist config.
   - Recommendation: Extend `write_global_config()` in Phase 42 alongside the struct addition. Low cost, prevents a blocker in Phase 43.

---

## Environment Availability

| Dependency | Required By | Available | Version | Fallback |
|------------|------------|-----------|---------|----------|
| `chrome-devtools-mcp` binary | INJECT-01 (reading installed path) | NOT_IN_PATH | — | Phase 42 only reads the path from config — binary absence doesn't block compilation or testing |
| Rust toolchain | All | Available | (existing project) | — |

**Missing dependencies with no fallback:** None that block Phase 42 implementation. The binary path is stored in config.yaml by Phase 43 init — Phase 42 only reads it.

---

## Security Domain

Phase has no new security surface beyond what's already covered by existing sandbox generation. Chrome-specific items:

| ASVS Category | Applies | Standard Control |
|---------------|---------|-----------------|
| V5 Input Validation | yes | `chrome_path` and `mcp_binary_path` are read as strings and stored as `PathBuf` — no shell expansion, passed as JSON array args (not shell string) |
| V4 Access Control | yes | `allowWrite` for `.chrome-profile` scoped to agent dir only — not a global write grant |

**`--no-sandbox` Chrome flag:** This disables Chrome's internal sandbox, intentional because bubblewrap (outer bwrap sandbox) is the enforcement layer. This is consistent with existing architecture decisions recorded in MEMORY.md: "Chrome sandbox: `--no-sandbox` arg (bubblewrap is outer sandbox)". [CITED: .planning/STATE.md decisions section]

---

## Sources

### Primary (HIGH confidence)
- `crates/rightclaw/src/config.rs` — full TunnelConfig/RawTunnelConfig pattern verified by inspection
- `crates/rightclaw/src/codegen/mcp_config.rs` — generate_mcp_config() read-modify-write pattern verified
- `crates/rightclaw/src/codegen/settings.rs` + `settings_tests.rs` — additive Vec::extend pattern, exact insertion point
- `crates/rightclaw/src/agent/types.rs` — SandboxOverrides deny_unknown_fields constraint verified
- `crates/rightclaw-cli/src/main.rs` lines 526-710 — cmd_up() integration points verified
- `github.com/ChromeDevTools/chrome-devtools-mcp package.json` — binary name `chrome-devtools-mcp` confirmed

### Secondary (MEDIUM confidence)
- [ChromeDevTools/chrome-devtools-mcp docs/cli.md](https://github.com/ChromeDevTools/chrome-devtools-mcp/blob/main/docs/cli.md) — `--isolated` + `--userDataDir` interaction, `--no-sandbox` as Chrome flag
- [Issue #261: Headless isolated launch fails as root without --no-sandbox](https://github.com/ChromeDevTools/chrome-devtools-mcp/issues/261) — confirms `--no-sandbox` is Chrome browser flag, needed inside bwrap

### Tertiary (LOW confidence)
- None

---

## Metadata

**Confidence breakdown:**
- Config pattern replication: HIGH — exact template in crates/rightclaw/src/config.rs
- MCP injection: HIGH — exact template in mcp_config.rs, pattern is trivial extension
- Sandbox settings injection: MEDIUM — `allowedCommands` field name needs CC schema verification (A1)
- cmd_up() wiring: HIGH — both call sites identified and verified

**Research date:** 2026-04-06
**Valid until:** 2026-05-06 (stable patterns, only risk is CC settings.json schema for A1)
