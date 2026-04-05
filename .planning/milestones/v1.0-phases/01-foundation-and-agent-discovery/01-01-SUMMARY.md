---
phase: 01-foundation-and-agent-discovery
plan: 01
subsystem: infra
tags: [rust, cargo, clap, serde, miette, thiserror, devenv]

requires: []
provides:
  - Cargo workspace with rightclaw (lib) and rightclaw-cli (binary) crates
  - AgentDef, AgentConfig, RestartPolicy core types with serde deserialization
  - AgentError diagnostic error types with miette
  - resolve_home priority chain (cli > env > ~/.rightclaw)
  - CLI skeleton with init/list subcommands
  - devenv.nix Rust stable toolchain
  - CLAUDE.rust.md coding conventions
affects: [01-02, agent-discovery, process-compose-generation, cli-commands]

tech-stack:
  added: [clap 4.6, serde 1.0, serde-saphyr 0.0, thiserror 2.0, miette 7.6, walkdir 2.5, dirs 6.0, tracing 0.1, tracing-subscriber 0.3]
  patterns: [workspace-deps, deny_unknown_fields, serde-defaults, miette-diagnostics, env-as-parameter]

key-files:
  created:
    - Cargo.toml
    - crates/rightclaw/Cargo.toml
    - crates/rightclaw/src/lib.rs
    - crates/rightclaw/src/agent/mod.rs
    - crates/rightclaw/src/agent/types.rs
    - crates/rightclaw/src/error.rs
    - crates/rightclaw/src/config.rs
    - crates/rightclaw-cli/Cargo.toml
    - crates/rightclaw-cli/src/main.rs
    - CLAUDE.rust.md
  modified:
    - devenv.nix
    - CLAUDE.md

key-decisions:
  - "Added clap 'env' feature to support #[arg(env = ...)] for RIGHTCLAW_HOME"
  - "resolve_home takes env_home as parameter, not std::env::var, per CLAUDE.rust.md"
  - "AgentConfig uses deny_unknown_fields for strict YAML validation"
  - "Reinstalled rustup stable toolchain to fix broken nix ld-wrapper"

patterns-established:
  - "env-as-parameter: Library functions receive env values as parameters, CLI reads env"
  - "deny_unknown_fields: All user-facing config structs reject unknown YAML keys"
  - "workspace-deps: All dependency versions centralized in root Cargo.toml"

requirements-completed: [PROJ-01, PROJ-02, WORK-01, WORK-02]

duration: 4min
completed: 2026-03-21
---

# Phase 01 Plan 01: Project Scaffold Summary

**Cargo workspace with two crates (rightclaw lib + CLI), core agent types with serde-saphyr YAML deserialization, miette diagnostics, and clap CLI skeleton printing --help with init/list subcommands**

## Performance

- **Duration:** 4 min
- **Started:** 2026-03-21T23:04:40Z
- **Completed:** 2026-03-21T23:09:06Z
- **Tasks:** 2
- **Files modified:** 13

## Accomplishments

- Cargo workspace compiles with edition 2024, resolver 3
- `rightclaw --help` prints subcommand listing (init, list) with --home flag
- AgentDef, AgentConfig, RestartPolicy types with serde deserialization and defaults
- AgentError with miette diagnostic codes (rightclaw::agent::*)
- resolve_home implements cli > env > ~/.rightclaw priority chain
- 11 unit tests covering deserialization, unknown field rejection, error display, home resolution
- Clippy clean with -D warnings

## Task Commits

1. **Task 1: Scaffold Cargo workspace, devenv, and project conventions** - `6862ede` (feat)
2. **Task 2: Define core types with TDD** - `3c28721` (test)

## Files Created/Modified

- `Cargo.toml` - Workspace root with centralized dependencies
- `crates/rightclaw/Cargo.toml` - Library crate manifest
- `crates/rightclaw/src/lib.rs` - Module re-exports (agent, config, error)
- `crates/rightclaw/src/agent/mod.rs` - Agent module re-exports
- `crates/rightclaw/src/agent/types.rs` - AgentDef, AgentConfig, RestartPolicy with tests
- `crates/rightclaw/src/error.rs` - AgentError with miette diagnostics and tests
- `crates/rightclaw/src/config.rs` - resolve_home with tests
- `crates/rightclaw-cli/Cargo.toml` - Binary crate manifest
- `crates/rightclaw-cli/src/main.rs` - Clap CLI with init/list subcommands
- `devenv.nix` - Rust stable toolchain with clippy, rustfmt, process-compose
- `CLAUDE.rust.md` - Rust coding conventions (edition 2024, fail-fast errors)
- `CLAUDE.md` - Added reference to CLAUDE.rust.md

## Decisions Made

- Added clap `env` feature for `#[arg(env = "RIGHTCLAW_HOME")]` support (not included in plan's dependency list)
- resolve_home takes `env_home` as parameter instead of reading `std::env::var` directly, per CLAUDE.rust.md rule against Default reading environment
- Reinstalled rustup stable toolchain (1.90.0 -> 1.94.0) to fix broken nix ld-wrapper.sh reference in toolchain

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 3 - Blocking] Fixed broken rustup toolchain linker wrapper**
- **Found during:** Task 1 (cargo build verification)
- **Issue:** rustup stable toolchain had ld.lld wrapper referencing missing nix store path
- **Fix:** `rustup toolchain install stable --force` to regenerate wrapper scripts
- **Verification:** `cargo build --workspace` succeeds
- **Committed in:** 6862ede (part of Task 1)

**2. [Rule 3 - Blocking] Added missing clap `env` feature**
- **Found during:** Task 1 (cargo build verification)
- **Issue:** `#[arg(env = ...)]` requires clap's `env` feature which wasn't in plan's dependency spec
- **Fix:** Added `"env"` to clap features in workspace Cargo.toml
- **Verification:** `cargo build --workspace` succeeds, `--help` shows `[env: RIGHTCLAW_HOME=]`
- **Committed in:** 6862ede (part of Task 1)

---

**Total deviations:** 2 auto-fixed (2 blocking)
**Impact on plan:** Both fixes necessary for compilation. No scope creep.

## Issues Encountered

None beyond the auto-fixed items above.

## User Setup Required

None - no external service configuration required.

## Next Phase Readiness

- Workspace foundation complete, all types defined
- Ready for Plan 02: agent discovery (scan_agents, directory validation, agent.yaml parsing)
- CLI subcommand bodies are `todo!()` -- will be wired in Plan 02

## Self-Check: PASSED

All 12 created/modified files verified present. Both task commits (6862ede, 3c28721) verified in git log.

---
*Phase: 01-foundation-and-agent-discovery*
*Completed: 2026-03-21*
