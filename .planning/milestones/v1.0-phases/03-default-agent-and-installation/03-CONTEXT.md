# Phase 3: Default Agent and Installation - Context

**Gathered:** 2026-03-22
**Status:** Ready for planning

<domain>
## Phase Boundary

First-run experience: install script, doctor command, default "Right" agent with BOOTSTRAP.md onboarding, production-quality OpenShell policy with comprehensive comments, Telegram channel setup. No new CLI subcommands beyond `doctor`. No skill management or cron (Phase 4).

</domain>

<decisions>
## Implementation Decisions

### Onboarding Flow (BOOTSTRAP.md)
- **D-01:** Match OpenClaw's onboarding — 4 questions: name, creature type, vibe, emoji
- **D-02:** BOOTSTRAP.md is conversational (CC reads it as system prompt, agent drives the conversation)
- **D-03:** On completion, writes IDENTITY.md (updated with user's choices), USER.md (user name, preferences), SOUL.md (personality based on vibe)
- **D-04:** After writing files, BOOTSTRAP.md self-deletes (agent removes the file)
- **D-05:** Telegram setup is NOT in BOOTSTRAP.md — it happens in `rightclaw init` before agent launches (MCP servers load at session start, can't be added mid-session)

### Telegram Channel Setup
- **D-06:** `rightclaw init` prompts for Telegram bot token during initialization
- **D-07:** Both flag (`--telegram-token <token>`) and interactive prompt supported — flag takes priority, interactive fallback
- **D-08:** If token provided: writes `.mcp.json` with Claude Code Telegram plugin config, updates policy.yaml network allowlist to include `api.telegram.org`
- **D-09:** If token skipped: no `.mcp.json` created, policy.yaml has commented-out Telegram network rule
- **D-10:** Token validation: basic format check (numeric:alphanumeric), not API verification

### OpenShell Policy (policy.yaml)
- **D-11:** Strict least-privilege default: agent dir read-write only, everything else read-only (/usr, /lib)
- **D-12:** Network: only api.github.com and api.telegram.org (if configured)
- **D-13:** `hard_requirement` for Landlock — no silent degradation on older kernels
- **D-14:** Comprehensive comments throughout policy.yaml showing how to expand permissions:
  - How to allow all outbound network hosts
  - How to add read-only access to a specific directory
  - How to add read-write access to a specific directory
  - How to give broad filesystem access (with security warning)
  - How to add additional network endpoints
- **D-15:** The policy.yaml serves as self-documenting reference — users learn OpenShell policy format from it

### Install Script (install.sh)
- **D-16:** Downloads pre-built binaries from GitHub Releases (platform detection: linux-x86_64, darwin-arm64)
- **D-17:** Checks for existing installations of process-compose and OpenShell — only installs missing ones
- **D-18:** Calls each tool's official install mechanism (curl-based installers)
- **D-19:** After installing, runs `rightclaw init` to create ~/.rightclaw/ + default agent (includes Telegram token prompt)
- **D-20:** Runs `rightclaw doctor` at the end to verify everything works

### Doctor Command
- **D-21:** `rightclaw doctor` checks: rightclaw binary, process-compose, openshell, claude CLI — all in PATH
- **D-22:** Validates ~/.rightclaw/agents/ structure — at least one valid agent (IDENTITY.md + policy.yaml)
- **D-23:** Reports pass/fail per check with clear fix instructions

### Claude's Discretion
- Exact BOOTSTRAP.md conversation design (question phrasing, follow-up handling)
- doctor command output formatting
- install.sh error handling and platform edge cases
- .mcp.json exact structure (follow Claude Code Telegram plugin docs)

</decisions>

<canonical_refs>
## Canonical References

**Downstream agents MUST read these before planning or implementing.**

### Rust conventions
- `CLAUDE.rust.md` — Rust project standards, error handling, testing

### Existing templates (to be expanded)
- `templates/right/IDENTITY.md` — Current default identity (needs BOOTSTRAP.md additions)
- `templates/right/SOUL.md` — Current default soul
- `templates/right/AGENTS.md` — Current default capabilities
- `templates/right/policy.yaml` — Current placeholder (needs full OpenShell policy)

### Phase 1/2 code (build on top of)
- `crates/rightclaw/src/init.rs` — `init_rightclaw_home()` with include_str! templates. Needs Telegram token prompt added.
- `crates/rightclaw-cli/src/main.rs` — CLI with existing init command. Needs doctor command + telegram flag on init.
- `crates/rightclaw/src/runtime/deps.rs` — `verify_dependencies()` — reusable for doctor command

### OpenShell policy format
- `.planning/research/PITFALLS.md` — Landlock hard_requirement, OpenShell alpha notes
- Research from project init — OpenShell policy YAML schema (version, filesystem_policy, network_policies, process, landlock)

### Claude Code Telegram plugin
- Official plugin: `anthropics/claude-plugins-official/external_plugins/telegram/`
- MCP config format for .mcp.json

</canonical_refs>

<code_context>
## Existing Code Insights

### Reusable Assets
- `init_rightclaw_home()` in `init.rs` — already creates ~/.rightclaw/ and copies templates. Needs extension for Telegram setup.
- `verify_dependencies()` in `deps.rs` — checks process-compose, claude, openshell in PATH. Doctor command can reuse this.
- `templates/right/` — existing template files. Need BOOTSTRAP.md added and policy.yaml fleshed out.
- `start.sh` at repo root — prototype launch script, reference for install.sh patterns

### Established Patterns
- `include_str!` for embedding templates at compile time
- Clap CLI with Commands enum for subcommands
- miette for error diagnostics
- `resolve_home()` for home directory resolution

### Integration Points
- `main.rs` needs `Doctor` subcommand added to Commands enum
- `init.rs` needs `--telegram-token` flag handling and .mcp.json generation
- `templates/right/` needs BOOTSTRAP.md and expanded policy.yaml
- New `install.sh` at repo root (bash script, not Rust)

</code_context>

<specifics>
## Specific Ideas

- Policy.yaml should be the best OpenShell policy documentation a user has ever seen — every section commented with "how to change this" examples
- BOOTSTRAP.md onboarding should feel like OpenClaw's — playful, suggests options if user feels stuck
- install.sh should be a single curl-pipeable line: `curl -LsSf https://... | sh`

</specifics>

<deferred>
## Deferred Ideas

None — discussion stayed within phase scope

</deferred>

---

*Phase: 03-default-agent-and-installation*
*Context gathered: 2026-03-22*
