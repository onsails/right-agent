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

**`--bare` skips OAuth and keychain reads.** This is a known bug (anthropics/claude-code#38022,
OPEN, no official fix). Currently agents use OAuth (claude.ai login) — no `ANTHROPIC_API_KEY`.

**Known workaround (macOS):** read OAuth token from keychain, pass via `apiKeyHelper`:
```bash
TOKEN=$(security find-generic-password -s "Claude Code-credentials" -w \
  | python3 -c "import sys,json; print(json.loads(sys.stdin.read())['claudeAiOauth']['accessToken'])")
echo "{\"apiKeyHelper\": \"echo $TOKEN\"}" > /tmp/agent-settings.json
claude --bare --settings /tmp/agent-settings.json -p "..."
```

**Linux equivalent:** same approach but read from `~/.rightclaw/agents/right/.claude.json`
(OAuth token may be stored there) or via `secret-tool` if using GNOME Keyring.

`rightclaw up` could generate the `apiKeyHelper` command into the per-agent `settings.json`
at launch time (reads real keychain once, embeds the echo command). Tokens rotate — needs
refresh logic or re-run `rightclaw up` to refresh.

## Simpler Alternative: --strict-mcp-config (no bare needed)

From CLI reference: `--strict-mcp-config` — Only use MCP servers from `--mcp-config`,
ignoring **all other MCP configurations** (including cloud/account MCP servers).

Combined with `ENABLE_CLAUDEAI_MCP_SERVERS=false` (already shipped), this gives near-bare
MCP isolation WITHOUT breaking OAuth:

```bash
claude -p "$PROMPT" \
  --strict-mcp-config \
  --mcp-config /path/to/agent/.mcp.json
```

This may already be sufficient — evaluate before attempting full `--bare` migration.

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
