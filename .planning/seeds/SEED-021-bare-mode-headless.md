---
id: SEED-021
status: dormant
planted: 2026-04-02
planted_during: v3.0 Teloxide Bot Runtime (phase 28.2 UAT)
trigger_when: Next milestone — any work on agent startup, CC invocation, or performance
scope: medium
---

# SEED-021: Switch to --bare mode for claude -p invocations

## Why This Matters

`--bare` skips auto-discovery of hooks, skills, plugins, MCP servers, auto memory, and
CLAUDE.md on startup. This directly cuts the ~2s Node.js init overhead by removing all
filesystem scanning at startup. Anthropic docs note:

> `--bare` is the recommended mode for scripted and SDK calls, and will become the default
> for `-p` in a future release.

Currently `claude -p` without `--bare` scans the working directory and `~/.claude` for
everything. With `--bare`, we pass everything explicitly — which is exactly what rightclaw
should be doing anyway (declarative, reproducible, no surprises).

## What --bare Changes

| Feature | Without --bare | With --bare |
|---|---|---|
| CLAUDE.md loading | Auto-discovered | Skipped (use `--append-system-prompt-file`) |
| Skills | Auto-loaded from `.claude/skills/` | Skipped |
| Hooks | Auto-loaded from settings | Skipped |
| MCP servers | Auto-loaded from `.mcp.json` + global | Skipped (use `--mcp-config`) |
| Auto memory | Enabled | Disabled |
| OAuth/keychain | Used | **Skipped** — needs `ANTHROPIC_API_KEY` |

## Critical Gotcha: API Key

**`--bare` skips OAuth and keychain reads.** Currently agents inherit the user's API key
via keychain. In bare mode, `ANTHROPIC_API_KEY` must be injected explicitly.

Options:
1. `ANTHROPIC_API_KEY` env var in process-compose.yaml (read from keychain at `rightclaw up` time, inject as env var — similar to `RC_TELEGRAM_TOKEN` pattern)
2. `--settings '{"apiKey":"..."}'` — but this embeds key in process args (visible in `ps`)
3. `--settings /path/to/settings.json` with `apiKeyHelper` field — cleanest option

The `apiKeyHelper` approach: generate a per-agent `settings.json` that includes
`"apiKeyHelper": "cat /run/secrets/anthropic_key"` or reads from a file path. rightclaw
already generates `settings.json` per agent — just add the key source there.

## What to Pass Explicitly

```bash
claude --bare -p "$PROMPT" \
  --mcp-config /home/wb/.rightclaw/agents/right/.mcp.json \
  --settings /home/wb/.rightclaw/agents/right/.claude/settings.json \
  --append-system-prompt-file /home/wb/.rightclaw/agents/right/SOUL.md \
  --allowedTools "Bash,Read,Edit,Write,Skill,StructuredOutput,mcp__rightclaw__*"
```

## When to Surface

**Trigger:** Next milestone touching agent startup, `bot.rs` CC invocation, or performance.

This seed should be presented during `/gsd:new-milestone` when the milestone scope matches:
- Agent startup performance
- CC invocation refactoring
- Any work on `crates/bot/src/telegram/worker.rs` (invoke_cc function)

## Scope Estimate

**Medium** — One phase:
1. Solve API key injection (keychain → env var at `rightclaw up` time, or `apiKeyHelper`)
2. Update `invoke_cc` in `worker.rs` to add `--bare` + explicit `--mcp-config` + `--settings`
3. Update `generate_settings` to include `apiKeyHelper` if bare mode enabled
4. Test: verify agent still works, skills still load (passed via `--append-system-prompt-file` SOUL.md)
5. Handle SOUL.md/IDENTITY.md — currently auto-loaded via CLAUDE.md; with `--bare` need explicit passing

## Breadcrumbs

- `crates/bot/src/telegram/worker.rs` — `invoke_cc` function — where `claude -p` is built
- `crates/rightclaw/src/codegen/settings.rs` — `generate_settings` — add `apiKeyHelper` field
- `crates/rightclaw-cli/src/main.rs` — `rightclaw up` — where API key could be read from keychain
- Docs: https://code.claude.com/docs/en/headless (bare mode section)

## Notes

Skills in `--bare` mode: the skills system (`.claude/skills/`) is skipped. But skills work
via the `Skill` tool which reads skill files at runtime — so they still work as long as the
agent has `Read` access to `.claude/skills/`. No issue here.

CLAUDE.md files: with `--bare`, CLAUDE.md is NOT auto-loaded. SOUL.md + IDENTITY.md must
be passed via `--append-system-prompt-file`. rightclaw already controls this — straightforward.
