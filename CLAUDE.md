@CLAUDE.rust.md

This is a Rust project. Follow conventions in CLAUDE.rust.md.

## Project

**Right Agent**

Right Agent is an opinionated, closed-box AI agent platform — peer to OpenClaw and Hermes in category. Every choice is made for you, security is the default, and we polish what ships before adding more. Built on Claude Code running inside NVIDIA OpenShell sandboxes, orchestrated by process-compose. Drop-in compatible with the OpenClaw/ClawHub ecosystem at the file level (same conventions, same skill format, same registry) — but with security-first enforcement instead of "grant all, pray it works."

**Core Value:** One Telegram thread per agent, every agent in its own sandbox, every credential outside it. The box is closed; you just use it.

### Constraints

- **Language**: Rust (edition 2024)
- **Dependencies**: process-compose (external), OpenShell (external), Claude Code CLI (external)
- **Platforms**: Linux and macOS
- **Compatibility**: Drop-in compatible with OpenClaw file conventions and ClawHub SKILL.md format
- **Security**: Agents default to OpenShell sandbox (`sandbox: mode: openshell`). Agents needing host access (computer-use, Chrome) can use `sandbox: mode: none`. Always `--dangerously-skip-permissions` — OpenShell policy is the security layer for sandboxed agents.
- **OpenShell status**: Alpha software — may have breaking changes. Design for resilience.
- **Stack**: `Cargo.toml` is the source of truth for dependencies. Project standards in `CLAUDE.rust.md`.

## Docs

- Always commit `docs/superpowers/` spec and plan files. Never leave them untracked.

## Conventions

- **Bot-first management**: All agent/MCP configuration goes through the Telegram bot (`/mcp add`, `/mcp remove`, `/mcp auth`, etc.). Never create or edit `.mcp.json`, agent configs, or credential files manually — the bot is the control plane.
- **Debuggability over convenience**: Always prefer direct, observable signals over indirect heuristics. If an API provides status, use it — don't infer status from side effects (e.g. SSH connectivity as a proxy for sandbox readiness). Errors must propagate to logs, never be silently swallowed.
- **Domain research before implementation**: Always verify external tool APIs by reading source code or running `--help` before writing integration code. Never rely solely on web documentation — it may be outdated or wrong.
- **PROMPT_SYSTEM.md**: Always keep PROMPT_SYSTEM.md in sync with the actual prompting system. When changing system prompt generation, agent definitions, JSON schemas, or MCP instructions, update PROMPT_SYSTEM.md to match.
- **MCP with_instructions()**: When adding, removing, or renaming MCP tools, always update `with_instructions()` in both `memory_server.rs` and `aggregator.rs` to reflect the current tool set and descriptions.
- **MCP tool names in agent-facing text**: CC prefixes MCP tools as `mcp__{server}__{tool}`. The Right Agent server is `"right"`, so agents see `mcp__right__<tool>`. All skills, templates, prompts, and codegen that reference tool names for agents must use the full prefixed form. When adding, removing, or renaming tools, update references in: `skills/`, `templates/right/`, `crates/right-agent/src/codegen/agent_def.rs`, `PROMPT_SYSTEM.md`.
- **Debugging agent sessions**: In development, bots run with `--debug`. Three log sources: (1) CC debug logs at `~/.claude/logs/` inside sandbox (`/sandbox/.claude/logs/`) or on host for `--no-sandbox`; (2) stream NDJSON logs at `~/.right/logs/streams/<session-uuid>.ndjson` on host; (3) process-compose per-process logs via REST API: `curl -s "http://localhost:18927/process/logs/{process-name}/0/50"` (e.g. `right-mcp-server`, `right-bot`). Bot and aggregator are separate processes — always check both when debugging MCP issues.
- **Self-healing platform**: Never manually fix agent sandboxes, configs, or state. If a platform change breaks an agent, the platform code must detect and recover automatically (re-upload if files are missing, adjust policy, etc.). Manual fixes mask bugs and prevent proper testing.
- **Never delete sandboxes for recovery**: Sandboxes contain agent data (credentials, installed tools, agent-created files). Deleting a sandbox destroys this data. Platform changes must be designed to work with existing sandboxes — never require sandbox recreation as a migration path.
- **Upgrade-friendly design**: Every new feature must be adoptable by already-deployed agents without recreation. New config fields default to the previous behavior (backward-compatible defaults). `agent config` must expose all user-facing settings — if a feature exists but can't be toggled via CLI, it's incomplete. Think in terms of upgrades, not fresh installs.
## Architecture docs split

`ARCHITECTURE.md` is **prescriptive only** — load-bearing rules, contracts,
gotchas, reference tables. It is `@`-imported and loads on every
conversation; every line costs tokens.

Descriptive content (data flows, feature mechanics, walkthroughs) lives in
`docs/architecture/*.md`. Reference these files by **plain path** in
`ARCHITECTURE.md` or here — never `@`-import them. That is the whole point
of the split.

When adding new content to `ARCHITECTURE.md`, ask: "is this a rule the
codebase enforces, or a description of how it works?" Rule →
`ARCHITECTURE.md`. Description → `docs/architecture/`.

**Cite-on-touch (mandatory):** when modifying a subsystem, re-read the
corresponding `docs/architecture/<x>.md` and update it if drifted. These
docs are not auto-loaded, so they will rot silently if not maintained.
Code is authoritative; the satellite doc is a courtesy to readers.

## Architecture

@ARCHITECTURE.md

Always update ARCHITECTURE.md when significant parts of the architecture change (new crates, module reorganization, new integrations, changed data flows).

