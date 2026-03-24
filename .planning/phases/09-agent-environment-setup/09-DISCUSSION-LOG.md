# Phase 9: Agent Environment Setup - Discussion Log

> **Audit trail only.** Do not use as input to planning, research, or execution agents.
> Decisions are captured in CONTEXT.md — this log preserves the alternatives considered.

**Date:** 2026-03-24
**Phase:** 09-agent-environment-setup
**Areas discussed:** git init type, Telegram copy strategy, Skills propagation, settings.local.json

---

## git init type

| Option | Description | Selected |
|--------|-------------|----------|
| Regular git init | Creates .git/ inside agent dir — standard working tree | ✓ |
| Bare git init | Creates git internals directly in dir, no .git/ subdir | |
| Just the .git directory | Manually create minimal .git/ without git binary | |

**User's choice:** Regular git init
**Notes:** ROADMAP said "bare init" but user confirmed regular init is correct. Bare repos have no .git/ subdir so CC would not recognize workspace trust.

**Follow-up — idempotency:**

| Option | Description | Selected |
|--------|-------------|----------|
| Skip if .git/ exists | Only init if absent — preserves existing repo state | ✓ |
| Always re-init | Runs every time — safe but unnecessary | |

**User's choice:** Skip if .git/ already exists

---

## Telegram copy strategy

| Option | Description | Selected |
|--------|-------------|----------|
| Copy from host, overwrite always | Single shared bot, copy on every up | |
| Per-agent in agent.yaml | Different bots per agent | ✓ (user clarified) |
| Symlink channels dir | Auto-sync but concurrent write risk | |

**User's choice:** Per-agent config — user noted each agent has its own Telegram bot, so the original "copy from host" framing was wrong.

**Follow-up — where does the token come from:**

| Option | Description | Selected |
|--------|-------------|----------|
| agent.yaml telegram_token field | Inline + file reference supported | ✓ |
| Pre-placed .env (no automation) | User manages file manually | |
| Env var per agent | AGENT_<NAME>_TELEGRAM_TOKEN | |

**User's choice:** agent.yaml with file reference as default. User wants `.telegram.env` file written by init, gitignored, referenced via `telegram_token_file: .telegram.env`.

**Follow-up — access.json / user ID:**

| Option | Description | Selected |
|--------|-------------|----------|
| Per-agent in agent.yaml | telegram_user_id per agent | ✓ |
| Shared from host access.json | Same trusted users for all agents | |

**User's choice:** Per-agent — different agents may have different trusted Telegram users.

---

## Skills propagation

| Option | Description | Selected |
|--------|-------------|----------|
| Reinstall from embedded files always | Always fresh, upgrade-aware | ✓ |
| Only create dir if missing | Preserves edits, new agents miss skills | |
| Copy from default agent | Couples all agents to default agent's skills | |

**User's choice:** Reinstall from embedded files on every `up`

---

## settings.local.json

| Option | Description | Selected |
|--------|-------------|----------|
| Only if missing | Preserves runtime writes | ✓ |
| Overwrite always | Resets to {} every up | |

**User's choice:** Only write if missing

---

## Claude's Discretion

- Token file format (`.env` format vs raw token) — use same format as init.rs today
- Whether git binary absence is Warn or hard error — suggested Warn/non-fatal

## Deferred Ideas

- `rightclaw agent init` subcommand for adding new agents with guided setup
- secretspec / env var injection as alternative to .env file (needs Telegram plugin validation)
- Agent-level `env:` section in agent.yaml for arbitrary env var forwarding
