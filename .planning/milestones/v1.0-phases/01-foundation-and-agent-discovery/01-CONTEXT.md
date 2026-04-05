# Phase 1: Foundation and Agent Discovery - Context

**Gathered:** 2026-03-21
**Status:** Ready for planning

<domain>
## Phase Boundary

Rust project scaffold with Cargo workspace, core types for representing agents, and directory scanning logic that discovers and parses agent definitions from `~/.rightclaw/agents/`. No runtime behavior — just types, parsing, and validation.

</domain>

<decisions>
## Implementation Decisions

### Project Structure
- **D-01:** Cargo workspace with two crates in `crates/` directory: `rightclaw` (library: types, discovery, config parsing) and `rightclaw-cli` (binary: clap commands, main)
- **D-02:** Root `Cargo.toml` defines workspace, contains no code (per CLAUDE.rust.md convention)
- **D-03:** Edition 2024 for all crates

### Agent Home Directory
- **D-04:** RightClaw is a system-level tool, NOT project-scoped. No `<project-path>` argument on `rightclaw up`
- **D-05:** All agents live at `~/.rightclaw/agents/` (default `RIGHTCLAW_HOME=~/.rightclaw`)
- **D-06:** `RIGHTCLAW_HOME` customizable via `--home` flag or `RIGHTCLAW_HOME` env var (for testing and customization)
- **D-07:** `~/.rightclaw/` root may contain common/core settings (TBD in later phases)
- **D-08:** `rightclaw up` reads `$RIGHTCLAW_HOME/agents/` and starts discovered agents. `--agents` flag filters which ones

### Agent Directory Layout
- **D-09:** Each agent is a subdirectory of `$RIGHTCLAW_HOME/agents/` (e.g., `agents/right/`, `agents/watchdog/`)
- **D-10:** Required files: `IDENTITY.md` + `policy.yaml` — both must exist for a valid agent
- **D-11:** Optional files (OpenClaw conventions): SOUL.md, USER.md, MEMORY.md, AGENTS.md, TOOLS.md, BOOTSTRAP.md, HEARTBEAT.md
- **D-12:** Optional config files: `agent.yaml` (restart policy, backoff, start prompt), `.mcp.json` (MCP servers)
- **D-13:** Optional directories: `skills/`, `crons/`, `hooks/`

### Default Agent Shipping
- **D-14:** `rightclaw init` creates `~/.rightclaw/` structure + default "Right" agent
- **D-15:** Agent templates embedded in the binary at compile time (templates/ dir in repo, `include_str!` or similar)
- **D-16:** Future: `rightclaw new-agent <name>` creates blank agent from minimal template (not in Phase 1 scope)

### Validation Strictness
- **D-17:** Fail fast — if ANY agent has invalid config (bad YAML, missing required files), refuse to start ALL agents
- **D-18:** `agent.yaml` must be valid YAML with known fields — unknown fields are errors, not silently ignored
- **D-19:** Clear error messages with file path and line number (miette diagnostics)

### Devenv Setup
- **D-20:** devenv.nix includes Rust stable toolchain + process-compose
- **D-21:** OpenShell NOT in devenv (too new for nix) — require it installed separately
- **D-22:** Include clippy, rustfmt in devenv toolchain

### Claude's Discretion
- Module organization within each crate (how to split types, discovery, config parsing into modules)
- Exact clap command structure and flag naming
- Test organization and fixtures

</decisions>

<canonical_refs>
## Canonical References

**Downstream agents MUST read these before planning or implementing.**

### Rust conventions
- `~/dev/tpt/CLAUDE.rust.md` — Rust project standards: workspace architecture, error handling (FAIL FAST), dependency versioning, testing rules, edition 2024. MUST be referenced in project CLAUDE.md.

### Project context
- `.planning/PROJECT.md` — Project vision, core value, constraints
- `.planning/REQUIREMENTS.md` — Full requirements list with phase mapping
- `.planning/research/STACK.md` — Recommended crates and versions
- `.planning/research/ARCHITECTURE.md` — Component boundaries and build order

### Existing code
- `identity/` — Current proto-agent files (IDENTITY.md, SOUL.md, AGENTS.md) — becomes the basis for agents/right/
- `start.sh` — Current launch script showing the Claude Code invocation pattern
- `skills/clawhub/SKILL.md` — Existing ClawHub skill
- `devenv.nix` — Current devenv scaffold (Rust commented out)
- `seed.md` — Original product vision document

</canonical_refs>

<code_context>
## Existing Code Insights

### Reusable Assets
- `identity/IDENTITY.md`, `identity/SOUL.md`, `identity/AGENTS.md` — These become the default "Right" agent template files
- `skills/clawhub/SKILL.md` — Existing ClawHub skill, will live at `$RIGHTCLAW_HOME/agents/right/skills/clawhub/`
- `start.sh` — Shows the Claude Code launch pattern (`claude --append-system-prompt --dangerously-skip-permissions -p`)

### Established Patterns
- Agent identity is split across multiple markdown files (IDENTITY.md for core, SOUL.md for personality, AGENTS.md for capabilities)
- Shell script wrapping claude CLI with identity concatenation

### Integration Points
- `devenv.nix` needs Rust toolchain + process-compose added
- `CLAUDE.md` needs reference to CLAUDE.rust.md conventions

</code_context>

<specifics>
## Specific Ideas

- Copy CLAUDE.rust.md from ~/dev/tpt/ and reference it in project CLAUDE.md
- The existing `identity/` directory content should migrate to become the default "Right" agent template

</specifics>

<deferred>
## Deferred Ideas

None — discussion stayed within phase scope

</deferred>

---

*Phase: 01-foundation-and-agent-discovery*
*Context gathered: 2026-03-21*
