# Phase 9: Agent Environment Setup - Context

**Gathered:** 2026-03-24
**Status:** Ready for planning

<domain>
## Phase Boundary

Pre-populate each agent's HOME directory on `rightclaw up` so CC starts headlessly with no interactive
prompts: git workspace recognition, Telegram channel config, built-in skills, and settings.local.json.

New capabilities out of scope: `rightclaw agent init` subcommand (future), secretspec integration (future),
managed settings or doctor (Phase 10).

</domain>

<decisions>
## Implementation Decisions

### git init (AENV-01)

- **D-01:** Run `git init` (regular, not bare) in each agent directory during `rightclaw up`. A regular
  git init creates `.git/` which CC checks for workspace trust. Bare repos have no `.git/` subdirectory
  and would not be recognized.

- **D-02:** Skip git init if `.git/` already exists in the agent directory (idempotent). Re-init a live
  repo with existing commits would disrupt branch tracking. Check `agent.path.join(".git").exists()`
  before running.

- **D-03:** Use `std::process::Command::new("git").arg("init").current_dir(&agent.path)` — no git2
  crate dependency needed for a one-shot init. Fail fast if git binary is missing (add to
  `verify_dependencies()` check in doctor).

### Telegram Channel Config (AENV-02)

- **D-04:** Telegram config is **per-agent**, not shared from host. Each agent has its own bot.
  Config comes from `agent.yaml` — no copying from host `~/.claude/channels/telegram/`.

- **D-05:** `AgentConfig` gains two new optional fields:
  - `telegram_token_file: Option<String>` — path to file containing the bot token (relative to agent dir).
    **This is the default approach.** `rightclaw up` reads the file and writes its content to
    `$AGENT_DIR/.claude/channels/telegram/.env`.
  - `telegram_token: Option<String>` — inline bot token as fallback (if no file path provided).
  - `telegram_user_id: Option<String>` — numeric Telegram user ID for access.json pre-pairing.
    If absent, access.json is not written (user must pair interactively).

  **Precedence:** `telegram_token_file` takes priority over `telegram_token` if both are set.

- **D-06:** Default token file path convention: `.telegram.env` in the agent directory. When
  `rightclaw init` prompts the user for a Telegram token, it writes the token to
  `$AGENT_DIR/.telegram.env` (not inline in agent.yaml), then sets `telegram_token_file: .telegram.env`
  in agent.yaml, and appends `.telegram.env` to `$AGENT_DIR/.gitignore`.

- **D-07:** On `rightclaw up`, for each agent with Telegram config:
  1. Determine token: read `telegram_token_file` (relative to agent dir) or use inline `telegram_token`
  2. Write `TELEGRAM_BOT_TOKEN=<token>` to `$AGENT_DIR/.claude/channels/telegram/.env`
  3. If `telegram_user_id` is set, write `access.json` with `dmPolicy: allowlist, allowFrom: [user_id]`
  4. Ensure `.mcp.json` exists in agent dir (marker for shell wrapper `--channels` flag)
  5. Skip silently if no Telegram config in agent.yaml

- **D-08:** Overwrite `.env` and `access.json` on every `up` (not idempotent). Keeps agent in sync
  with agent.yaml if token changes. Both files are regenerated from agent.yaml values each time.

### Skills Propagation (AENV-03)

- **D-09:** On every `rightclaw up`, reinstall built-in skills from embedded SKILL.md constants into
  each agent's `.claude/skills/`. This is the same operation `init.rs` already does for the default
  agent. Always overwrite — ensures all agents always get the latest built-in skills after upgrades.
  Skills installed: `clawhub/SKILL.md`, `rightcron/SKILL.md` (and `installed.json`).

- **D-10:** User-created skills in `$AGENT_DIR/.claude/skills/` (non-built-in dirs) are NOT touched.
  Only the specific built-in skill directories are overwritten. Achieved by writing to named paths
  rather than wiping the entire skills directory.

### settings.local.json (AENV-03)

- **D-11:** Write empty `{}` to `$AGENT_DIR/.claude/settings.local.json` only if the file does not
  already exist. Agents and CC may write to settings.local.json at runtime — overwriting on every
  `up` would destroy those writes.

### Ordering in cmd_up() per-agent loop

- **D-12:** New operations in the per-agent loop (after Phase 8 additions):
  1. Generate combined prompt (existing)
  2. Generate shell wrapper (existing)
  3. Generate settings.json (existing)
  4. Generate .claude.json (Phase 8)
  5. Create credential symlink (Phase 8)
  6. **git init if .git/ missing** (Phase 9, D-01)
  7. **Telegram channel config** (Phase 9, D-07)
  8. **Reinstall built-in skills** (Phase 9, D-09)
  9. **Write settings.local.json if missing** (Phase 9, D-11)

### Claude's Discretion

- Exact token file format: `TELEGRAM_BOT_TOKEN=<token>` (plain env file) vs just `<token>` (raw).
  Use the format the Telegram plugin expects from `.env` — same as `init.rs` writes today.
- Whether `git` binary absence should be a `Warn` (skip git init) or hard error in `verify_dependencies`.
  Suggested: `Warn` severity in doctor, non-fatal in `cmd_up` (log warning, continue).

</decisions>

<canonical_refs>
## Canonical References

**Downstream agents MUST read these before planning or implementing.**

### Core implementation files
- `crates/rightclaw-cli/src/main.rs` — `cmd_up()` per-agent loop (lines 301-351, Phase 8 additions)
- `crates/rightclaw/src/init.rs` — existing Telegram token write pattern (lines 119-160) and skills install (lines 57-76); Phase 9 extends this pattern to `cmd_up()`
- `crates/rightclaw/src/agent/types.rs` — `AgentConfig` and `SandboxOverrides` structs (add Telegram fields)
- `crates/rightclaw/src/codegen/mod.rs` — codegen module exports (new telegram channel codegen goes here)

### Requirements
- `.planning/REQUIREMENTS.md` — Phase 9 requirements: AENV-01, AENV-02, AENV-03, PERM-03

### Phase 8 context (decisions that constrain this phase)
- `.planning/phases/08-home-isolation-permission-model/08-CONTEXT.md` — D-06 (agent-local writes only),
  D-07 (host_home resolved before per-agent loop), existing per-agent loop structure

</canonical_refs>

<code_context>
## Existing Code Insights

### Reusable Assets
- `init.rs` lines 119-160: Telegram `.env` + `access.json` write pattern — copy this exact logic into
  a `generate_telegram_channel_config(agent, run_dir)` codegen function
- `init.rs` lines 57-76: Skills install loop (`built_in_skills` + `claude_skills_dir`) — extract into
  a shared `install_builtin_skills(agent_path)` function callable from both `init` and `cmd_up`
- `crates/rightclaw/src/agent/types.rs` `AgentConfig`: add `telegram_token`, `telegram_token_file`,
  `telegram_user_id` as `Option<String>` fields with `#[serde(default)]`

### Established Patterns
- Per-agent generation in `cmd_up()`: write to `agent.path.join(".claude")/...` — same pattern for all
  new files (settings.local.json, skills, telegram channel config)
- `std::process::Command` already used in runtime — use same pattern for `git init`
- `host_home` already resolved before per-agent loop (Phase 8) — no new resolution needed

### Integration Points
- `AgentConfig` in `types.rs`: new Telegram fields read by `cmd_up()` per-agent loop
- `generate_telegram_channel_config()` new codegen function: takes agent + host_home, writes to
  `$AGENT_DIR/.claude/channels/telegram/`
- `install_builtin_skills()` extracted function: takes agent path, writes from embedded constants
- git init via `Command::new("git").arg("init")` in per-agent loop

</code_context>

<specifics>
## Specific Ideas

- Token file default path: `.telegram.env` in agent dir. `rightclaw init` creates this file and sets
  `telegram_token_file: .telegram.env` in agent.yaml, then gitignores it. Keeps secrets out of
  version-controlled YAML by default.
- `rightclaw init` extended to prompt for Telegram token (existing) AND write `.telegram.env` +
  set `telegram_token_file` in agent.yaml (new behavior replacing inline token storage).
- User mentioned "rightclaw agent init" as a future multi-agent setup command — noted as deferred.

</specifics>

<deferred>
## Deferred Ideas

- **`rightclaw agent init` subcommand** — guided setup for adding new agents (Telegram pairing,
  identity, etc.). Separate from `rightclaw init` which sets up the default "right" agent. Future phase.
- **secretspec / env var injection for Telegram token** — if the Telegram plugin supports reading
  `TELEGRAM_BOT_TOKEN` from the process environment (not just `.env` file), we could inject via shell
  wrapper and skip the `.env` write entirely. Needs empirical validation. Future improvement.
- **Agent-level `env:` section in agent.yaml** — arbitrary env var forwarding (GITHUB_TOKEN, NPM_TOKEN,
  etc.) mentioned in Phase 8 deferred list. Still deferred.

</deferred>

---

*Phase: 09-agent-environment-setup*
*Context gathered: 2026-03-24*
