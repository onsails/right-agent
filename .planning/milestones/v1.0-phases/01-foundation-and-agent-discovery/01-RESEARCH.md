# Phase 1: Foundation and Agent Discovery - Research

**Researched:** 2026-03-21
**Domain:** Rust CLI scaffold, YAML config parsing, filesystem-based agent discovery
**Confidence:** HIGH

## Summary

Phase 1 is a greenfield Rust project scaffold with two crates in a Cargo workspace: a library crate (`rightclaw`) containing types, config parsing, and directory scanning, and a binary crate (`rightclaw-cli`) providing clap-based CLI commands. No runtime behavior -- just types, parsing, validation, and a `--help` command.

The stack is well-established: clap 4.6 for CLI, serde + serde-saphyr for YAML deserialization, thiserror + miette for error handling. The key technical challenge is serde-saphyr being pre-1.0 (0.0.22), but it is the only non-deprecated serde YAML deserializer with solid test coverage. All other crate versions verified against crates.io registry on 2026-03-21.

**Primary recommendation:** Build the workspace with `resolver = "3"` (edition 2024 default), define `AgentDef` and `AgentConfig` types with `#[serde(deny_unknown_fields)]`, implement directory scanning with walkdir, and embed default agent templates via `include_str!`. Keep the binary crate thin -- just clap parsing and dispatching to library functions.

<user_constraints>
## User Constraints (from CONTEXT.md)

### Locked Decisions
- **D-01:** Cargo workspace with two crates in `crates/` directory: `rightclaw` (library: types, discovery, config parsing) and `rightclaw-cli` (binary: clap commands, main)
- **D-02:** Root `Cargo.toml` defines workspace, contains no code (per CLAUDE.rust.md convention)
- **D-03:** Edition 2024 for all crates
- **D-04:** RightClaw is a system-level tool, NOT project-scoped. No `<project-path>` argument on `rightclaw up`
- **D-05:** All agents live at `~/.rightclaw/agents/` (default `RIGHTCLAW_HOME=~/.rightclaw`)
- **D-06:** `RIGHTCLAW_HOME` customizable via `--home` flag or `RIGHTCLAW_HOME` env var (for testing and customization)
- **D-07:** `~/.rightclaw/` root may contain common/core settings (TBD in later phases)
- **D-08:** `rightclaw up` reads `$RIGHTCLAW_HOME/agents/` and starts discovered agents. `--agents` flag filters which ones
- **D-09:** Each agent is a subdirectory of `$RIGHTCLAW_HOME/agents/` (e.g., `agents/right/`, `agents/watchdog/`)
- **D-10:** Required files: `IDENTITY.md` + `policy.yaml` -- both must exist for a valid agent
- **D-11:** Optional files (OpenClaw conventions): SOUL.md, USER.md, MEMORY.md, AGENTS.md, TOOLS.md, BOOTSTRAP.md, HEARTBEAT.md
- **D-12:** Optional config files: `agent.yaml` (restart policy, backoff, start prompt), `.mcp.json` (MCP servers)
- **D-13:** Optional directories: `skills/`, `crons/`, `hooks/`
- **D-14:** `rightclaw init` creates `~/.rightclaw/` structure + default "Right" agent
- **D-15:** Agent templates embedded in the binary at compile time (templates/ dir in repo, `include_str!` or similar)
- **D-16:** Future: `rightclaw new-agent <name>` creates blank agent from minimal template (not in Phase 1 scope)
- **D-17:** Fail fast -- if ANY agent has invalid config (bad YAML, missing required files), refuse to start ALL agents
- **D-18:** `agent.yaml` must be valid YAML with known fields -- unknown fields are errors, not silently ignored
- **D-19:** Clear error messages with file path and line number (miette diagnostics)
- **D-20:** devenv.nix includes Rust stable toolchain + process-compose
- **D-21:** OpenShell NOT in devenv (too new for nix) -- require it installed separately
- **D-22:** Include clippy, rustfmt in devenv toolchain

### Claude's Discretion
- Module organization within each crate (how to split types, discovery, config parsing into modules)
- Exact clap command structure and flag naming
- Test organization and fixtures

### Deferred Ideas (OUT OF SCOPE)
None -- discussion stayed within phase scope
</user_constraints>

<phase_requirements>
## Phase Requirements

| ID | Description | Research Support |
|----|-------------|------------------|
| PROJ-01 | Rust project with edition 2024, devenv configuration for Rust toolchain | Cargo workspace with `resolver = "3"`, devenv `languages.rust.enable = true` with `channel = "stable"` |
| PROJ-02 | CI-ready project structure with tests | Workspace layout in `crates/`, assert_cmd + tempfile for integration tests, `cargo test --workspace` |
| WORK-01 | Agent directory structure follows OpenClaw conventions | `AgentDef` type with fields for all convention files, walkdir scanning |
| WORK-02 | Each agent can have optional `agent.yaml` for restart policy, backoff, start prompt | `AgentConfig` struct with serde defaults, `#[serde(deny_unknown_fields)]` |
| WORK-03 | Each agent can have `.mcp.json` for per-agent MCP server configuration | Detect `.mcp.json` presence, store as `Option<PathBuf>` -- no parsing needed in Phase 1 |
| WORK-04 | Each agent must contain `policy.yaml` -- passed to OpenShell, not parsed by RightClaw | Existence validation only, store as `PathBuf` |
| WORK-05 | Agent directory with IDENTITY.md is auto-detected as valid agent | Discovery logic: scan subdirs of `$RIGHTCLAW_HOME/agents/`, check for `IDENTITY.md` + `policy.yaml` |
</phase_requirements>

## Standard Stack

### Core (Phase 1 subset)

| Library | Version | Purpose | Why Standard |
|---------|---------|---------|--------------|
| clap | 4.6.0 | CLI argument parsing (derive API) | Industry standard, subcommand model fits `rightclaw init/up/--help` |
| serde | 1.0.228 | Serialization framework | Non-negotiable for structured data |
| serde-saphyr | 0.0.22 | YAML deserialization | Only non-deprecated serde YAML lib; panic-free, no unsafe, 1000+ tests |
| thiserror | 2.0.18 | Structured error enums | Library-quality error types with derive macro |
| miette | 7.6.0 | User-facing diagnostic errors | Compiler-style errors with source snippets for config validation |
| walkdir | 2.5.0 | Directory traversal | Scan `agents/` to discover subdirectories |
| dirs | 6.0.0 | Platform directories | Resolve `~/.rightclaw` default home |
| tracing | 0.1.44 | Structured logging | Ecosystem standard |
| tracing-subscriber | 0.3.23 | Log output formatting | `EnvFilter` for `RUST_LOG` support |

### Testing

| Library | Version | Purpose | When to Use |
|---------|---------|---------|-------------|
| assert_cmd | 2.2.0 | CLI integration testing | Test `rightclaw --help`, `rightclaw init` |
| predicates | 3.1.4 | Assertion helpers | Assert on stdout/stderr content |
| tempfile | 3.27.0 | Temp directories | Isolated agent directory structures per test |

### NOT needed in Phase 1

| Library | Why Deferred |
|---------|-------------|
| tokio | No async operations in Phase 1 (no HTTP, no process spawning) |
| reqwest | No HTTP calls needed yet |
| minijinja | No process-compose.yaml generation yet |
| ctrlc | No signal handling needed yet |
| which | No external dependency checks yet |

### Alternatives Considered

| Instead of | Could Use | Tradeoff |
|------------|-----------|----------|
| serde-saphyr | serde_yml 0.0.12 | Lower test coverage, less active development |
| miette | anyhow | anyhow is simpler but gives no source-location diagnostics for config errors |
| walkdir | std::fs::read_dir | walkdir handles symlinks, errors, and depth limiting cleanly |

**Installation (Phase 1 subset):**
```bash
# In crates/rightclaw/
cargo add clap --features derive
cargo add serde --features derive
cargo add serde-saphyr
cargo add thiserror
cargo add miette --features fancy
cargo add walkdir
cargo add dirs
cargo add tracing
cargo add tracing-subscriber --features env-filter

# In crates/rightclaw-cli/
cargo add clap --features derive

# Dev dependencies (in rightclaw-cli)
cargo add --dev assert_cmd predicates tempfile
```

## Architecture Patterns

### Recommended Project Structure

```
rightclaw/
├── Cargo.toml              # Workspace root, no code
├── CLAUDE.md               # Project instructions (references CLAUDE.rust.md)
├── CLAUDE.rust.md           # Rust conventions (copied from ~/dev/tpt/)
├── devenv.nix              # Rust stable + process-compose
├── templates/              # Embedded agent templates
│   └── right/              # Default "Right" agent files
│       ├── IDENTITY.md
│       ├── SOUL.md
│       ├── AGENTS.md
│       └── policy.yaml
├── crates/
│   ├── rightclaw/          # Library crate
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs      # Re-exports
│   │       ├── error.rs    # Error types (thiserror + miette)
│   │       ├── config.rs   # RightclawHome resolution (--home / env / default)
│   │       ├── agent/
│   │       │   ├── mod.rs       # Re-exports
│   │       │   ├── types.rs     # AgentDef, AgentConfig, RestartPolicy
│   │       │   └── discovery.rs # scan_agents(), parse_agent_yaml()
│   │       └── init.rs     # rightclaw init logic (create dirs, write templates)
│   └── rightclaw-cli/      # Binary crate
│       ├── Cargo.toml
│       └── src/
│           └── main.rs     # Clap commands, dispatch to rightclaw lib
```

### Pattern 1: Home Directory Resolution

**What:** Resolve RIGHTCLAW_HOME through a priority chain: `--home` flag > `RIGHTCLAW_HOME` env var > `~/.rightclaw` default.

**When to use:** Every CLI command that touches the agents directory.

**Example:**
```rust
use std::path::PathBuf;

pub fn resolve_home(cli_home: Option<&str>) -> miette::Result<PathBuf> {
    if let Some(home) = cli_home {
        return Ok(PathBuf::from(home));
    }
    if let Ok(env_home) = std::env::var("RIGHTCLAW_HOME") {
        return Ok(PathBuf::from(env_home));
    }
    let home = dirs::home_dir()
        .ok_or_else(|| miette::miette!("Could not determine home directory"))?;
    Ok(home.join(".rightclaw"))
}
```

### Pattern 2: Strict YAML Deserialization

**What:** Use `#[serde(deny_unknown_fields)]` on all config structs so typos in `agent.yaml` are caught immediately.

**When to use:** All user-facing config types (`AgentConfig`).

**Caveat:** `deny_unknown_fields` is incompatible with `#[serde(flatten)]` -- do not combine them. Also incompatible with `#[serde(skip)]`. These are serde-level limitations, not serde-saphyr-specific.

**Example:**
```rust
use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentConfig {
    #[serde(default = "default_restart")]
    pub restart: RestartPolicy,
    #[serde(default = "default_max_restarts")]
    pub max_restarts: u32,
    #[serde(default = "default_backoff_seconds")]
    pub backoff_seconds: u32,
    pub start_prompt: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RestartPolicy {
    Never,
    #[default]
    OnFailure,
    Always,
}
```

### Pattern 3: Discovery via Filesystem Scan

**What:** Walk `$RIGHTCLAW_HOME/agents/`, check each subdirectory for required files (`IDENTITY.md` + `policy.yaml`), parse optional configs.

**When to use:** Core agent discovery logic.

**Example:**
```rust
pub fn discover_agents(agents_dir: &Path) -> miette::Result<Vec<AgentDef>> {
    let mut agents = Vec::new();
    for entry in std::fs::read_dir(agents_dir)
        .into_diagnostic()
        .wrap_err_with(|| format!("Failed to read agents directory: {}", agents_dir.display()))?
    {
        let entry = entry.into_diagnostic()?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let identity = path.join("IDENTITY.md");
        let policy = path.join("policy.yaml");
        if !identity.exists() {
            tracing::warn!("Skipping {}: no IDENTITY.md", path.display());
            continue;
        }
        if !policy.exists() {
            return Err(miette::miette!(
                "Agent '{}' has IDENTITY.md but no policy.yaml -- policy.yaml is required",
                path.file_name().unwrap().to_string_lossy()
            ));
        }
        // Parse optional agent.yaml...
        let config = parse_agent_config(&path)?;
        agents.push(AgentDef { /* ... */ });
    }
    Ok(agents)
}
```

### Pattern 4: Embedded Templates with include_str!

**What:** Embed default agent template files in the binary at compile time for `rightclaw init`.

**When to use:** For `rightclaw init` to create `~/.rightclaw/agents/right/` with default files.

**Example:**
```rust
const DEFAULT_IDENTITY: &str = include_str!("../../../templates/right/IDENTITY.md");
const DEFAULT_SOUL: &str = include_str!("../../../templates/right/SOUL.md");
const DEFAULT_POLICY: &str = include_str!("../../../templates/right/policy.yaml");

pub fn init_rightclaw_home(home: &Path) -> miette::Result<()> {
    let agents_dir = home.join("agents").join("right");
    std::fs::create_dir_all(&agents_dir).into_diagnostic()?;
    std::fs::write(agents_dir.join("IDENTITY.md"), DEFAULT_IDENTITY).into_diagnostic()?;
    std::fs::write(agents_dir.join("SOUL.md"), DEFAULT_SOUL).into_diagnostic()?;
    std::fs::write(agents_dir.join("policy.yaml"), DEFAULT_POLICY).into_diagnostic()?;
    Ok(())
}
```

### Anti-Patterns to Avoid

- **Parsing policy.yaml content:** RightClaw validates existence only. OpenShell validates content. Do not parse policy YAML.
- **Using `std::env::set_var()` in tests:** Pollutes global environment. Pass `RIGHTCLAW_HOME` as a parameter instead.
- **Putting business logic in main.rs:** Keep the binary crate thin -- just clap dispatch. All logic lives in the library crate.
- **Using `unwrap()` in library code:** Use `?` with miette/thiserror. `unwrap()` only acceptable in tests.
- **Using `Default` trait that reads environment:** Per CLAUDE.rust.md, use explicit factory methods like `from_cli_args()`.

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| YAML parsing | Custom parser | serde-saphyr | Edge cases in YAML spec are endless |
| Home dir resolution | Manual `$HOME` concat | `dirs` crate | Cross-platform (Linux + macOS), XDG compliance |
| Directory scanning | Manual `read_dir` recursion | `walkdir` (or `read_dir` for depth-1) | Symlink handling, error recovery |
| CLI arg parsing | Manual `std::env::args` | `clap` derive | Subcommands, help text, shell completions |
| Error diagnostics | Custom error formatting | `miette` | Source snippets, labels, suggestions |

**Key insight:** Phase 1 is pure I/O + parsing. Every component has a well-tested library solution. The only custom code is the glue: discovery logic, validation rules, and the init command.

## Common Pitfalls

### Pitfall 1: serde-saphyr deny_unknown_fields + flatten incompatibility
**What goes wrong:** Combining `#[serde(deny_unknown_fields)]` with `#[serde(flatten)]` causes all deserialization to fail.
**Why it happens:** Known serde limitation -- flatten uses an internal `Content` deserializer that conflicts with deny_unknown_fields.
**How to avoid:** Never use `flatten` on structs that also use `deny_unknown_fields`. If you need extensibility, use explicit fields.
**Warning signs:** All YAML files fail to parse with "unknown field" errors even when fields are correct.

### Pitfall 2: Edition 2024 resolver
**What goes wrong:** Workspace doesn't compile or dependencies resolve incorrectly.
**Why it happens:** Edition 2024 defaults to `resolver = "3"`. If workspace Cargo.toml specifies edition but not resolver, it should auto-resolve, but explicit is better.
**How to avoid:** Set `resolver = "3"` explicitly in workspace `Cargo.toml`.
**Warning signs:** Feature resolution warnings or unexpected dependency versions.

### Pitfall 3: Relative paths in include_str!
**What goes wrong:** `include_str!` paths are relative to the source file, not the crate root.
**Why it happens:** Macro resolution happens at compile time relative to the file containing the macro invocation.
**How to avoid:** Use paths relative to the file: `include_str!("../../../templates/right/IDENTITY.md")` from `crates/rightclaw/src/init.rs`. Or use `CARGO_MANIFEST_DIR` in a build script.
**Warning signs:** Compile errors about file not found.

### Pitfall 4: Agent directory name as identifier
**What goes wrong:** Agent names with special characters (spaces, dots, unicode) cause issues in shell scripts and process-compose configs.
**Why it happens:** Directory name is used as the agent name, which later becomes a process name.
**How to avoid:** Validate agent directory names: alphanumeric + hyphens + underscores only. Reject invalid names at discovery time with clear error message.
**Warning signs:** Process-compose YAML generation failures in Phase 2.

### Pitfall 5: Testing with real home directory
**What goes wrong:** Tests that use `~/.rightclaw` modify the user's actual agent setup.
**Why it happens:** Not isolating test environment from production environment.
**How to avoid:** Every test must use `tempfile::tempdir()` as RIGHTCLAW_HOME. Pass home path as parameter, never read env directly in library code.
**Warning signs:** `~/.rightclaw/` appears after running tests.

### Pitfall 6: miette + thiserror interop
**What goes wrong:** `thiserror` errors don't display miette diagnostics.
**Why it happens:** `miette::Report` wraps errors for display, but `thiserror` errors need to implement `miette::Diagnostic` trait for full diagnostic output.
**How to avoid:** Derive both `thiserror::Error` and `miette::Diagnostic` on error enums. Use `#[diagnostic(code(...))]` for error codes.
**Warning signs:** Error messages show raw text instead of fancy diagnostics.

## Code Examples

### Workspace Cargo.toml

```toml
# Root Cargo.toml
[workspace]
members = ["crates/rightclaw", "crates/rightclaw-cli"]
resolver = "3"

[workspace.package]
version = "0.1.0"
edition = "2024"

[workspace.dependencies]
clap = { version = "4.6", features = ["derive"] }
serde = { version = "1.0", features = ["derive"] }
serde-saphyr = "0.0.22"
thiserror = "2.0"
miette = { version = "7.6", features = ["fancy"] }
walkdir = "2.5"
dirs = "6.0"
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
```

```toml
# crates/rightclaw/Cargo.toml
[package]
name = "rightclaw"
version.workspace = true
edition.workspace = true

[dependencies]
serde = { workspace = true }
serde-saphyr = { workspace = true }
thiserror = { workspace = true }
miette = { workspace = true }
walkdir = { workspace = true }
dirs = { workspace = true }
tracing = { workspace = true }
```

```toml
# crates/rightclaw-cli/Cargo.toml
[package]
name = "rightclaw-cli"
version.workspace = true
edition.workspace = true

[[bin]]
name = "rightclaw"
path = "src/main.rs"

[dependencies]
rightclaw = { path = "../rightclaw" }
clap = { workspace = true }
miette = { workspace = true }
tracing-subscriber = { workspace = true }

[dev-dependencies]
assert_cmd = "2.2"
predicates = "3.1"
tempfile = "3.27"
```

### Clap CLI Structure (Recommended)

```rust
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "rightclaw", version, about = "Multi-agent runtime for Claude Code")]
pub struct Cli {
    /// Path to RightClaw home directory
    #[arg(long, env = "RIGHTCLAW_HOME")]
    pub home: Option<String>,

    /// Enable verbose logging
    #[arg(short, long)]
    pub verbose: bool,

    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Initialize RightClaw home directory with default agent
    Init,
    /// List discovered agents and their status
    List,
    // Phase 2+: Up, Down, Status, Attach, Restart
}
```

### Error Type Pattern

```rust
use miette::Diagnostic;
use thiserror::Error;

#[derive(Debug, Error, Diagnostic)]
pub enum AgentError {
    #[error("Agent '{name}' is missing required file: {file}")]
    #[diagnostic(code(rightclaw::agent::missing_file))]
    MissingRequiredFile { name: String, file: String },

    #[error("Failed to parse agent.yaml for '{name}'")]
    #[diagnostic(code(rightclaw::agent::invalid_config))]
    InvalidConfig {
        name: String,
        #[source]
        source: serde_saphyr::Error,
    },

    #[error("Invalid agent directory name '{name}': must be alphanumeric, hyphens, or underscores")]
    #[diagnostic(code(rightclaw::agent::invalid_name))]
    InvalidName { name: String },
}
```

### devenv.nix Configuration

```nix
{ pkgs, lib, config, inputs, ... }:

{
  packages = [
    pkgs.git
    pkgs.process-compose
  ];

  languages.rust = {
    enable = true;
    channel = "stable";
    components = [ "rustc" "cargo" "clippy" "rustfmt" "rust-analyzer" ];
  };

  enterShell = ''
    echo "RightClaw dev environment"
    rustc --version
    cargo --version
  '';

  enterTest = ''
    cargo test --workspace
    cargo clippy --workspace -- -D warnings
  '';
}
```

### Integration Test Example

```rust
// crates/rightclaw-cli/tests/cli_help.rs
use assert_cmd::Command;
use predicates::prelude::*;

#[test]
fn test_help_output() {
    Command::cargo_bin("rightclaw")
        .unwrap()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("Multi-agent runtime"));
}

#[test]
fn test_init_creates_structure() {
    let tmp = tempfile::tempdir().unwrap();
    Command::cargo_bin("rightclaw")
        .unwrap()
        .args(["--home", tmp.path().to_str().unwrap(), "init"])
        .assert()
        .success();

    assert!(tmp.path().join("agents/right/IDENTITY.md").exists());
    assert!(tmp.path().join("agents/right/policy.yaml").exists());
}
```

## State of the Art

| Old Approach | Current Approach | When Changed | Impact |
|--------------|------------------|--------------|--------|
| serde_yaml | serde-saphyr | March 2024 (serde_yaml archived) | Must use serde-saphyr or serde_yml |
| edition = "2021" | edition = "2024" | Rust 1.85.0 (Feb 2025) | resolver = "3" default, new lint defaults |
| color-eyre | miette | Aug 2024 (color-eyre archived) | Use miette for diagnostic errors |
| anyhow everywhere | thiserror (lib) + miette (display) | Best practice 2025+ | Structured errors in libraries |

**Deprecated/outdated:**
- `serde_yaml`: Archived March 2024. Do not use.
- `color-eyre`: Archived August 2024. Do not use.
- `resolver = "2"`: Still works but `"3"` is the 2024 edition default.

## Open Questions

1. **process-compose in nixpkgs is v1.94.0, STACK.md recommends v1.100.0+**
   - What we know: nixpkgs has 1.94.0, project wants 1.100.0+
   - What's unclear: Whether v1.94.0 suffices for Phase 1 features (not needed until Phase 2 anyway)
   - Recommendation: Use nixpkgs version for devenv for now, revisit when process-compose integration is implemented in Phase 2. Can overlay or fetchurl if needed.

2. **IDENTITY.md presence without policy.yaml**
   - What we know: D-10 says both IDENTITY.md AND policy.yaml are required. Success criteria #3 says "IDENTITY.md" dirs are recognized as valid agents. Success criteria #4 says policy.yaml existence is validated.
   - What's unclear: Should a directory with IDENTITY.md but no policy.yaml be an error or a warning?
   - Recommendation: Per D-17 (fail fast), treat missing policy.yaml as a hard error. A directory with IDENTITY.md signals intent to be an agent -- missing policy.yaml is likely a user mistake.

3. **`.mcp.json` parsing depth**
   - What we know: Phase 1 needs to detect `.mcp.json` exists (WORK-03). The actual content is MCP server configuration passed to Claude Code.
   - What's unclear: Whether to validate it as valid JSON or just check existence.
   - Recommendation: Check existence only. Store as `Option<PathBuf>`. JSON validation adds complexity with no Phase 1 benefit -- Claude Code will validate the format when it consumes it.

## Canonical References

Per CONTEXT.md, downstream agents MUST read these:
- `~/dev/tpt/CLAUDE.rust.md` -- Rust project standards (MUST be copied to project and referenced in CLAUDE.md)
- `.planning/research/STACK.md` -- Full crate recommendations
- `.planning/research/ARCHITECTURE.md` -- Component boundaries and data flow

## Sources

### Primary (HIGH confidence)
- crates.io registry -- verified all crate versions via `cargo search` on 2026-03-21
- [serde container attributes](https://serde.rs/container-attrs.html) -- deny_unknown_fields behavior
- [devenv Rust language support](https://devenv.sh/languages/rust/) -- devenv.nix Rust configuration
- [CLAUDE.rust.md](/home/wb/dev/tpt/CLAUDE.rust.md) -- Project Rust conventions

### Secondary (MEDIUM confidence)
- [Rust 2024 edition](https://doc.rust-lang.org/edition-guide/rust-2024/) -- resolver = "3" as default
- [serde-saphyr GitHub](https://github.com/bourumir-wyngs/serde-saphyr) -- library characteristics and test coverage claims

### Tertiary (LOW confidence)
- process-compose nixpkgs version (1.94.0) -- may be updated in newer nixpkgs revisions

## Metadata

**Confidence breakdown:**
- Standard stack: HIGH -- all crate versions verified against registry, well-established libraries
- Architecture: HIGH -- workspace pattern is standard Rust, per CLAUDE.rust.md conventions
- Pitfalls: HIGH -- serde deny_unknown_fields gotchas documented from official serde issues
- devenv config: MEDIUM -- devenv 2.0 recently released, Rust module interface stable but details may shift

**Research date:** 2026-03-21
**Valid until:** 2026-04-20 (stable domain, 30-day validity)
