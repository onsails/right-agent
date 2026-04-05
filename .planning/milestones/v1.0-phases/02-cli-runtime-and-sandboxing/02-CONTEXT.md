# Phase 2: CLI Runtime and Sandboxing - Context

**Gathered:** 2026-03-22
**Status:** Ready for planning

<domain>
## Phase Boundary

Full CLI lifecycle commands (`up`, `down`, `status`, `restart`, `attach`) with each agent running inside an OpenShell sandbox via process-compose. Code generation of shell wrappers and process-compose.yaml. No agent content (default agent, skills, crons) — that's Phase 3+.

Note: `rightclaw up` has NO `<project-path>` argument (decided Phase 1, D-04). It reads agents from `$RIGHTCLAW_HOME/agents/`.

</domain>

<decisions>
## Implementation Decisions

### Codegen Strategy
- **D-01:** Generated files (process-compose.yaml, shell wrappers) live at `$RIGHTCLAW_HOME/run/`
- **D-02:** Files are persistent and inspectable — overwritten on each `rightclaw up`, NOT cleaned on `down`
- **D-03:** Each agent gets a shell wrapper script at `$RIGHTCLAW_HOME/run/<agent-name>.sh`
- **D-04:** process-compose.yaml generated at `$RIGHTCLAW_HOME/run/process-compose.yaml`

### Agent Launch Pattern
- **D-05:** Agent's working directory = its agent folder (`$RIGHTCLAW_HOME/agents/<name>/`), NOT a project path
- **D-06:** Claude Code reads SOUL.md, AGENTS.md, MEMORY.md etc. naturally from cwd — no concatenation needed
- **D-07:** Only IDENTITY.md passed via `--append-system-prompt-file` flag to Claude Code
- **D-08:** Always use `--dangerously-skip-permissions` — OpenShell is the security layer, CC prompts add friction

### OpenShell Wrapping
- **D-09:** Shell wrapper invokes `openshell sandbox create --policy <agent>/policy.yaml -- claude --append-system-prompt-file <agent>/IDENTITY.md --dangerously-skip-permissions`
- **D-10:** Fail by default if OpenShell not installed. `--no-sandbox` flag allows running without OpenShell (for development/testing)
- **D-11:** When `--no-sandbox`, wrapper runs `claude` directly without `openshell sandbox create`

### Shutdown & Cleanup
- **D-12:** `rightclaw down` explicitly destroys each OpenShell sandbox (via `openshell sandbox destroy` or equivalent)
- **D-13:** `rightclaw down` stops process-compose but keeps `$RIGHTCLAW_HOME/run/` files for debugging
- **D-14:** run/ files overwritten on next `rightclaw up` — no manual cleanup needed

### process-compose Integration
- **D-15:** Use process-compose REST API (reqwest via Unix socket) for `status` and `restart` — NOT CLI shelling out
- **D-16:** `rightclaw attach` uses `exec process-compose attach` — replaces our process with PC's TUI
- **D-17:** `rightclaw up` spawns process-compose, `rightclaw up -d` uses `--detached-with-tui`
- **D-18:** Unix socket path stored in a known location (`$RIGHTCLAW_HOME/run/pc.sock` or similar) for other commands to find

### Claude's Discretion
- Exact process-compose REST API endpoints and error handling
- How to track sandbox names for cleanup
- Shell wrapper template format (bash vs sh)
- Process-compose version compatibility handling

</decisions>

<canonical_refs>
## Canonical References

**Downstream agents MUST read these before planning or implementing.**

### Rust conventions
- `CLAUDE.rust.md` — Rust project standards: workspace, error handling (FAIL FAST), dependency versioning

### Phase 1 code (build on top of this)
- `crates/rightclaw/src/agent/discovery.rs` — `discover_agents()` returns `Vec<AgentDef>`
- `crates/rightclaw/src/agent/types.rs` — `AgentDef`, `AgentConfig`, `RestartPolicy`
- `crates/rightclaw/src/config.rs` — `resolve_home()` with cli > env > default priority
- `crates/rightclaw/src/error.rs` — `AgentError` with miette diagnostics
- `crates/rightclaw-cli/src/main.rs` — Clap CLI with `--home` flag, existing init/list subcommands

### External tool docs
- `seed.md` — Original vision doc with process-compose.yaml generation example and agent launch pattern
- `.planning/research/ARCHITECTURE.md` — Component boundaries, data flow, shell wrapper pattern
- `.planning/research/PITFALLS.md` — Signal propagation, OAuth race condition, OpenShell alpha issues

### process-compose
- Research: process-compose REST API, Unix socket support, `--detached-with-tui` flag

### OpenShell
- Research: `openshell sandbox create --policy <path> -- <command>`, policy YAML format, `sandbox destroy`

</canonical_refs>

<code_context>
## Existing Code Insights

### Reusable Assets
- `discover_agents()` in `discovery.rs` — returns all valid agents, already validates IDENTITY.md + policy.yaml
- `resolve_home()` in `config.rs` — handles cli > env > default home directory resolution
- `AgentDef` has `policy_path`, `config` (parsed agent.yaml), `mcp_config_path` — all needed for codegen
- `start.sh` at repo root — existing prototype of the shell wrapper pattern (concatenation approach, to be replaced)

### Established Patterns
- Clap with `Commands` enum for subcommands (main.rs)
- miette for error diagnostics (error.rs)
- `serde-saphyr` for YAML parsing (types.rs)
- `resolve_home(cli: Option<&str>) -> Result<PathBuf>` for home directory resolution

### Integration Points
- `main.rs` needs new subcommands: `up`, `down`, `status`, `restart`, `attach`
- `lib.rs` needs new modules: `codegen` (wrapper/yaml generation), `runtime` (PC interaction)
- `AgentDef` provides all data needed to generate shell wrappers and process-compose entries

</code_context>

<specifics>
## Specific Ideas

- The shell wrapper pattern from `start.sh` is the prototype — but replace concatenation with `--append-system-prompt-file` pointing only to IDENTITY.md
- process-compose.yaml should be generated, not hand-written — the seed.md has a good example of the target format

</specifics>

<deferred>
## Deferred Ideas

None — discussion stayed within phase scope

</deferred>

---

*Phase: 02-cli-runtime-and-sandboxing*
*Context gathered: 2026-03-22*
