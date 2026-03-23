# Session Report: RightClaw v1.0

**Date:** 2026-03-21 to 2026-03-23
**Duration:** ~3 days (marathon session)
**Model:** Claude Opus 4.6 (1M context)

## Summary

Built RightClaw from scratch — a multi-agent runtime for Claude Code with OpenShell sandboxing, process-compose orchestration, Telegram channels, and declarative skill management. Went from seed.md idea to a working product with manual verification.

## Work Performed

### Phases Completed (4 planned + 2 inserted)

| Phase | Plans | What was built |
|-------|-------|----------------|
| 1. Foundation & Agent Discovery | 2 | Cargo workspace, core types, agent parsing, devenv |
| 2. CLI Runtime & Sandboxing | 3 | Codegen, process-compose integration, all CLI subcommands |
| 3. Default Agent & Installation | 4 | BOOTSTRAP.md, policy.yaml, install.sh, doctor, Telegram |
| 3.1 CC Settings & Plugin Config | 1 | .claude/settings.json with enabledPlugins |
| 3.2 Interactive Setup & Pairing | 1 | rightclaw pair command |
| 4. Skills & Automation | 2 | /skills (skills.sh), /rightcron (cron reconciler) |

### Manual Testing & Bug Fixes (post-phase)

| Fix | Issue |
|-----|-------|
| claude-bun detection | Nix installs as claude-bun, not claude |
| `-p` flag = print mode | Was being used as project dir flag |
| `--append-system-prompt` conflict | CC doesn't allow mixing with `--append-system-prompt-file` |
| Pre-trust agent directory | CC trust dialog not bypassed by --dangerously-skip-permissions |
| Skip bypass mode warning | `skipDangerousModePermissionPrompt` in settings.json |
| Telegram auto-pairing | Pre-write access.json with user ID |
| `DISABLE_NONESSENTIAL_TRAFFIC` | ANY value (even "0") blocks feature flags via `!!` |
| `--channels` flag | Required for Telegram but channels gated by `tengu_harbor` flag |
| OpenShell `~` in paths | Must expand to absolute paths |
| OpenShell base image | Has Claude Code installed, not OpenClaw |
| OpenShell OAuth | Sandbox containers can't access host OAuth tokens |
| Skills path | Must be `.claude/skills/` not `skills/` |
| `is_tty: true` needed | Without it CC enters print mode under PC |
| PC restart crashes | REST API restart kills PC with is_tty processes |
| PC detached mode | `--detached-with-tui` fails, use `--detached` |
| PC Unix socket | `--use-uds` crashes TUI, use TCP `--port` |
| SessionStart prompt hooks | Don't work (ToolUseContext bug) |
| Positional prompt for bootstrap | Use `--` separator to prevent --channels consuming it |

### Seeds Planted (6)

1. **SEED-001:** Skill-policy compatibility audit with env var management
2. **SEED-002:** Fix BOOTSTRAP.md onboarding flow (doesn't trigger via Telegram)
3. **SEED-003:** Claude-native sandboxing as alternative to OpenShell
4. **SEED-004:** Per-agent HOME isolation from host settings
5. **SEED-005:** Support skills.sh instead of ClawHub
6. **SEED-006:** Rename clawhub to rightskills, add env/secrets support

## Metrics

| Metric | Value |
|--------|-------|
| Commits | 130 |
| Files changed | 101 |
| Lines added | ~17,770 |
| Lines removed | ~125 |
| Automated tests | 120 (100 unit + 20 integration) |
| Requirements defined | 41 |
| Requirements validated | 41 |
| Phases planned | 6 |
| Phases completed | 6 |
| Plans executed | 13 |
| Subagents spawned | ~50+ (researchers, planners, checkers, executors, verifiers) |

## Test Results (Final)

| Test | Status |
|------|--------|
| rightclaw --help | ✓ |
| rightclaw init | ✓ |
| rightclaw list | ✓ |
| rightclaw doctor | ✓ |
| rightclaw up --no-sandbox | ✓ |
| Telegram messaging | ✓ |
| Telegram auto-pairing | ✓ |
| Model selection (sonnet) | ✓ |
| rightclaw status | ✓ |
| rightclaw down | ✓ |
| rightclaw up -d + attach | ✓ |
| /skills search + install | ✓ |
| /rightcron create + bootstrap | ✓ |
| BOOTSTRAP.md onboarding | ✗ (SEED-002) |
| OpenShell sandbox | ✗ (SEED-003 — needs API key) |
| rightclaw restart | ✗ (PC bug — use TUI Ctrl+R) |

## Architecture Delivered

```
rightclaw init → creates ~/.rightclaw/agents/right/ with:
  IDENTITY.md, SOUL.md, AGENTS.md, BOOTSTRAP.md
  policy.yaml (OpenShell, expanded absolute paths)
  agent.yaml (model: sonnet, restart policy)
  .claude/settings.json (skipPermissions, spinnerTips, plugins)
  .claude/skills/clawhub/SKILL.md (skills.sh manager)
  .claude/skills/rightcron/SKILL.md (cron reconciler)
  .mcp.json (Telegram marker)

rightclaw up → generates ~/.rightclaw/run/ with:
  right-prompt.md (combined identity + startup instructions)
  right.sh (shell wrapper with claude binary resolution)
  process-compose.yaml (is_tty, restart policy)
  state.json (sandbox tracking)
  → launches process-compose on TCP port 18927
  → positional prompt triggers /rightcron bootstrap
  → agent communicates via Telegram channel

rightclaw down → stops PC + destroys sandboxes
rightclaw status → queries PC REST API
rightclaw attach → exec into PC TUI
rightclaw doctor → validates deps + agent structure
```

## Key Learnings

1. **Claude Code is not designed for daemon mode.** Every aspect (trust dialogs, permission prompts, TTY detection, channels feature flags) assumes an interactive terminal user. Making it work as a daemon requires extensive workarounds.

2. **OpenShell is alpha and container-only.** No host file access means OAuth doesn't work. The provider system only supports API keys. This limits the target audience.

3. **process-compose has quirks.** `is_tty` fakes enough for CC but breaks restart. Unix socket mode crashes TUI. Detached-with-TUI needs /dev/tty. TCP mode works.

4. **The agentskills.io ecosystem is mature.** 500k+ skills, multiple registries, standard format. RightClaw should lean into this rather than build its own.

5. **Feature flags gate functionality.** `CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC` (a telemetry setting) blocks channels entirely because it prevents GrowthBook flag fetches.
