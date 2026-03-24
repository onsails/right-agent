# Stack Research: Headless Agent Isolation

**Domain:** Claude Code settings/permissions/config for headless multi-agent isolation
**Researched:** 2026-03-24
**Confidence:** HIGH (verified against official Claude Code docs at code.claude.com)

## Executive Summary

v2.1 drops `--dangerously-skip-permissions` and replaces it with explicit `permissions.allow` rules + sandbox + `dontAsk` mode. This research documents the exact CC settings schema, managed settings mechanism, `CLAUDE_CONFIG_DIR` behavior, and HOME override implications needed to implement full headless agent isolation.

---

## 1. Replacing `--dangerously-skip-permissions`

### The Replacement Stack

Three CC mechanisms combine to achieve prompt-free operation without bypass mode:

| Mechanism | What It Does | Where Configured |
|-----------|-------------|-----------------|
| `permissions.allow` | Pre-approves specific tools/commands | `.claude/settings.json` (project) |
| `sandbox.enabled` + `autoAllowBashIfSandboxed` | Auto-approves all bash within sandbox boundaries | `.claude/settings.json` (project) |
| `defaultMode: "dontAsk"` | Auto-denies anything not pre-approved (no prompts) | `.claude/settings.json` or CLI flag |

### Why This Is Better

`--dangerously-skip-permissions` maps to `defaultMode: "bypassPermissions"` -- it approves EVERYTHING. The replacement approves only what's whitelisted and denies everything else silently. No prompts, no bypasses.

### Permission Rule Syntax (Verified)

Rules follow format `Tool` or `Tool(specifier)`. Evaluation order: **deny -> ask -> allow**. First match wins.

```json
{
  "permissions": {
    "allow": [
      "Bash",
      "Read",
      "Edit",
      "Write",
      "Glob",
      "Grep",
      "WebFetch",
      "WebSearch",
      "Agent(Explore)"
    ],
    "deny": [
      "Read(./.env)",
      "Read(~/.ssh/**)",
      "Read(~/.aws/**)",
      "Read(~/.gnupg/**)"
    ]
  }
}
```

**Tool names for permission rules:**
- `Bash` / `Bash(npm run *)` / `Bash(git commit *)` -- glob patterns with `*`
- `Read` / `Read(./.env)` -- gitignore-spec patterns
- `Edit` / `Edit(/src/**/*.ts)` -- applies to all file-editing tools
- `Write` -- same as Edit for permissions
- `WebFetch` / `WebFetch(domain:example.com)`
- `WebSearch`
- `Glob`, `Grep` -- read-only, no approval needed by default
- `Agent(name)` -- subagent control
- `mcp__servername__toolname` -- MCP tool control

**Path pattern prefixes for Read/Edit rules:**

| Prefix | Meaning | Example |
|--------|---------|---------|
| `//path` | Absolute from filesystem root | `Read(//Users/alice/secrets/**)` |
| `~/path` | Relative to home directory | `Read(~/Documents/*.pdf)` |
| `/path` | Relative to project root | `Edit(/src/**/*.ts)` |
| `path` or `./path` | Relative to current directory | `Read(*.env)` |

**Confidence:** HIGH -- verified from official docs at code.claude.com/docs/en/permissions

### `dontAsk` Mode Details

- Denies everything not pre-approved via `permissions.allow` rules
- No prompts shown -- tools auto-denied silently
- Available as `defaultMode: "dontAsk"` in settings or `--permission-mode dontAsk` CLI flag
- **Known issue:** Subagents spawned via Task tool also run in `dontAsk` mode and get auto-denied if parent didn't pre-approve their tools. Must pre-approve `Agent(Explore)` etc. in `permissions.allow`.

**Confidence:** HIGH -- documented behavior, confirmed by multiple GitHub issues

### RightClaw Implementation Plan

Current wrapper template:
```bash
exec "$CLAUDE_BIN" \
  --dangerously-skip-permissions \
  ...
```

Replace with:
```bash
exec "$CLAUDE_BIN" \
  --permission-mode dontAsk \
  ...
```

Combined with project-level `.claude/settings.json`:
```json
{
  "permissions": {
    "defaultMode": "dontAsk",
    "allow": [
      "Bash",
      "Read",
      "Edit",
      "Write",
      "Glob",
      "Grep",
      "WebFetch",
      "WebSearch",
      "Agent(Explore)"
    ],
    "deny": [
      "Read(~/.ssh/**)",
      "Read(~/.aws/**)",
      "Read(~/.gnupg/**)"
    ]
  },
  "sandbox": {
    "enabled": true,
    "autoAllowBashIfSandboxed": true,
    "allowUnsandboxedCommands": false,
    "filesystem": {
      "allowWrite": ["<agent_dir>"],
      "denyRead": ["~/.ssh", "~/.aws", "~/.gnupg"]
    },
    "network": {
      "allowedDomains": [
        "api.anthropic.com",
        "github.com",
        "*.npmjs.org",
        "crates.io",
        "agentskills.io",
        "api.telegram.org"
      ]
    }
  }
}
```

### What Can Be Removed

- `skipDangerousModePermissionPrompt: true` -- no longer needed (we don't use bypass mode)
- `--dangerously-skip-permissions` CLI flag -- replaced by `--permission-mode dontAsk`
- The `pre_trust_directory()` write to `~/.claude/settings.json` setting `skipDangerousModePermissionPrompt` -- no longer relevant

---

## 2. Managed Settings (`managed-settings.json`)

### File Locations (Verified)

| Platform | Path |
|----------|------|
| Linux/WSL | `/etc/claude-code/managed-settings.json` |
| macOS | `/Library/Application Support/ClaudeCode/managed-settings.json` |
| Windows | `C:\Program Files\ClaudeCode\managed-settings.json` |

**Confidence:** HIGH -- verified from official docs

### Precedence (Cannot Be Overridden)

Managed settings have HIGHEST precedence. User, project, and local settings cannot override them.

Full hierarchy (highest to lowest):
1. **Managed** (server-managed > MDM/OS-level > `managed-settings.json`)
2. **Command line arguments**
3. **Local project** (`.claude/settings.local.json`)
4. **Shared project** (`.claude/settings.json`)
5. **User** (`~/.claude/settings.json`)

**Array settings merge across scopes** -- they concatenate and deduplicate, not replace.

### Schema (Enterprise-Only Properties)

These properties ONLY take effect in managed settings:

| Setting | Type | Description |
|---------|------|-------------|
| `disableBypassPermissionsMode` | `"disable"` | Prevents `bypassPermissions` mode and `--dangerously-skip-permissions` flag |
| `allowManagedPermissionRulesOnly` | `boolean` | Only rules in managed settings apply; user/project `allow`/`ask`/`deny` ignored |
| `allowManagedHooksOnly` | `boolean` | Only managed hooks and SDK hooks load; user/project/plugin hooks blocked |
| `allowManagedMcpServersOnly` | `boolean` | Only admin-defined MCP server allowlist applies |
| `sandbox.network.allowManagedDomainsOnly` | `boolean` | Only managed `allowedDomains` + managed `WebFetch(domain:...)` rules apply. Non-allowed domains blocked silently (no prompt). Denied domains still merge from all sources. |
| `sandbox.filesystem.allowManagedReadPathsOnly` | `boolean` | Only managed `allowRead` paths respected |
| `strictKnownMarketplaces` | `array` | Allowlist of plugin marketplaces users can add |
| `blockedMarketplaces` | `array` | Blocklist of marketplace sources |
| `channelsEnabled` | `boolean` | Allow channels for Team/Enterprise users |

### RightClaw Usage of Managed Settings

**Critical consideration:** Managed settings are machine-wide. Writing to `/etc/claude-code/` affects ALL Claude Code sessions on the machine, not just RightClaw agents.

- **Option A: Use managed settings** -- Enforces network policy globally. Requires sudo/admin. Affects user's interactive CC sessions too.
- **Option B: Use project-level settings only** -- Per-agent, no sudo needed. But `allowManagedDomainsOnly` only works in managed settings. Domains CAN be extended by user via their own settings.json.
- **Recommendation: Option B for now.** Use project-level `sandbox.network.allowedDomains` without `allowManagedDomainsOnly`. Accept that users can extend the domain list via their user settings -- this is acceptable since users already control the machine. Managed settings are enterprise-grade overkill for a developer tool. Document Option A for users who want stricter enforcement.

**Confidence:** HIGH -- verified schema from official docs

---

## 3. HOME Override and Config Resolution

### How CC Resolves Config Files (Verified)

CC reads from these paths, all relative to `$HOME`:

| File | Resolved Path | Purpose |
|------|--------------|---------|
| `~/.claude.json` | `$HOME/.claude.json` | Preferences, OAuth, MCP, project trust, caches |
| `~/.claude/settings.json` | `$HOME/.claude/settings.json` | User-level settings |
| `~/.claude/.credentials.json` | `$HOME/.claude/.credentials.json` | API credentials (Linux/Windows) |
| `~/.claude/CLAUDE.md` | `$HOME/.claude/CLAUDE.md` | Global instructions |
| `~/.claude/commands/` | `$HOME/.claude/commands/` | Global slash commands |
| `~/.claude/agents/` | `$HOME/.claude/agents/` | User subagents |
| `~/.claude/skills/` | `$HOME/.claude/skills/` | User skills |
| `~/.claude/plugins/` | `$HOME/.claude/plugins/` | Plugin storage |
| `~/.claude/channels/telegram/` | `$HOME/.claude/channels/telegram/` | Telegram channel config |

Project-level files (relative to cwd):

| File | Resolved Path | Purpose |
|------|--------------|---------|
| `.claude/settings.json` | `$CWD/.claude/settings.json` | Project settings |
| `.claude/settings.local.json` | `$CWD/.claude/settings.local.json` | Local project settings |
| `.mcp.json` | `$CWD/.mcp.json` | Project MCP servers |
| `CLAUDE.md` | `$CWD/CLAUDE.md` | Project instructions |

### `CLAUDE_CONFIG_DIR` Environment Variable

| Aspect | Status |
|--------|--------|
| Documented? | Listed in env-vars reference with one-liner: "Customize where Claude Code stores its configuration and data files" |
| What it redirects | `.claude.json`, `.credentials.json`, `projects/`, `shell-snapshots/`, `statsig/`, `todos/`, `settings.json` |
| What it does NOT redirect | Project-level `.claude/settings.local.json` (still created in cwd) |
| Bugs | IDE integration breaks. Behavior was unclear until v2.0.42+. |
| Stability | Partially supported, not fully documented |

**Confidence:** MEDIUM -- env var exists and is listed officially, but behavior details come from GitHub issues, not docs

### What Breaks When `$HOME` is Overridden

Setting `HOME=$AGENT_DIR` causes CC to look for ALL home-relative configs under the agent directory:

| Config | Default | With HOME=$AGENT_DIR | Impact |
|--------|---------|---------------------|--------|
| `~/.claude.json` | `$HOME/.claude.json` | `$AGENT_DIR/.claude.json` | **BREAKS**: Trust entries, OAuth, preferences lost |
| `~/.claude/settings.json` | `$HOME/.claude/settings.json` | `$AGENT_DIR/.claude/settings.json` | **OK**: This is what we want -- agent-local settings |
| `~/.claude/.credentials.json` | `$HOME/.claude/.credentials.json` | `$AGENT_DIR/.claude/.credentials.json` | **BREAKS**: Credentials not found |
| macOS Keychain | System keychain | System keychain | **OK**: Keychain is per-user, not per-HOME |
| `~/.claude/channels/telegram/` | Under real home | Under agent dir | **BREAKS**: Telegram token/access.json not found |
| `~/.claude/plugins/` | Under real home | Under agent dir | **BREAKS**: Plugins not found |
| Git config (`~/.gitconfig`) | Under real home | Under agent dir | **BREAKS**: Git identity/auth gone |
| SSH keys (`~/.ssh/`) | Under real home | Under agent dir | **BREAKS**: SSH auth gone |

### The `CLAUDE_CONFIG_DIR` Approach (Better Than HOME Override)

Instead of overriding `$HOME`, use `CLAUDE_CONFIG_DIR=$AGENT_DIR/.claude-config`:

| Config | Resolved Path | Impact |
|--------|--------------|--------|
| `.claude.json` | `$AGENT_DIR/.claude-config/.claude.json` | Isolated per-agent |
| `settings.json` | `$AGENT_DIR/.claude-config/settings.json` | Isolated per-agent |
| `.credentials.json` | `$AGENT_DIR/.claude-config/.credentials.json` | Needs to be seeded from host |
| Project `.claude/settings.json` | `$AGENT_DIR/.claude/settings.json` | Still works (cwd-relative) |
| Git config | `~/.gitconfig` (real HOME) | Still works |
| SSH keys | `~/.ssh/` (real HOME) | Still works |
| Telegram | `~/.claude/channels/telegram/` (real HOME) | Still works |

**Recommendation: Use `CLAUDE_CONFIG_DIR` NOT `$HOME` override.**

`CLAUDE_CONFIG_DIR` provides the isolation we need (per-agent trust state, settings, credentials) without breaking git, SSH, Telegram, or other host-dependent config.

**Confidence:** MEDIUM -- CLAUDE_CONFIG_DIR redirects the right files based on GitHub issue analysis, but some edge cases may exist due to incomplete documentation

### Trust Dialog Handling Under Isolation

Current code writes `hasTrustDialogAccepted: true` to `~/.claude.json` under a project path key. With `CLAUDE_CONFIG_DIR`:

1. Create `$AGENT_DIR/.claude-config/.claude.json` with:
```json
{
  "projects": {
    "<absolute_agent_dir_path>": {
      "hasTrustDialogAccepted": true,
      "hasTrustDialogHooksAccepted": true
    }
  },
  "hasCompletedOnboarding": true
}
```

2. This pre-populates the agent's isolated config with trust for its own directory.
3. No need to modify the host's `~/.claude.json` at all.

### Credential Forwarding

On Linux, credentials are in `~/.claude/.credentials.json`. With `CLAUDE_CONFIG_DIR`:
- Copy or symlink credentials to `$AGENT_DIR/.claude-config/.credentials.json`
- OR set `ANTHROPIC_API_KEY` env var (simpler, avoids credential file management)
- OR use `apiKeyHelper` in settings.json to fetch credentials dynamically

**Recommendation:** Use `ANTHROPIC_API_KEY` env var. It's simpler, avoids file management, and is the standard approach for headless/CI deployments. The shell wrapper already has access to the env.

On macOS, OAuth tokens are in the system Keychain, which is per-user not per-HOME. This works regardless of `CLAUDE_CONFIG_DIR`.

---

## 4. Environment Variables Reference

### Config Path Control

| Variable | Purpose | Confidence |
|----------|---------|------------|
| `CLAUDE_CONFIG_DIR` | Redirects where CC stores config/data files | MEDIUM |
| `CLAUDE_CODE_TMPDIR` | Override temp directory (CC appends `/claude/`) | HIGH |

### Auth Control

| Variable | Purpose | Confidence |
|----------|---------|------------|
| `ANTHROPIC_API_KEY` | API key (overrides OAuth in non-interactive mode) | HIGH |
| `ANTHROPIC_AUTH_TOKEN` | Custom Authorization header value | HIGH |
| `ANTHROPIC_BASE_URL` | Override API endpoint | HIGH |

### Behavior Control

| Variable | Purpose | Confidence |
|----------|---------|------------|
| `CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC` | Disables auto-updates, feedback, telemetry, error reporting. ANY value (even "0") enables. | HIGH |
| `CLAUDE_CODE_SIMPLE` | Minimal system prompt, only Bash/Read/Edit tools. Same as `--bare`. | HIGH |
| `CLAUDE_CODE_DISABLE_CRON` | Disables scheduled tasks | HIGH |
| `CLAUDECODE` | Set to `1` in CC-spawned shells (detect CC context) | HIGH |
| `DISABLE_AUTOUPDATER` | Skip auto-updates | HIGH |

### Relevant for RightClaw Wrapper

The shell wrapper should set:
```bash
export CLAUDE_CONFIG_DIR="$AGENT_DIR/.claude-config"
export CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC=1
export DISABLE_AUTOUPDATER=1
```

And forward `ANTHROPIC_API_KEY` from the parent environment if set.

---

## 5. Complete Generated Settings Schema for v2.1

### Per-Agent `.claude/settings.json` (Project-Level)

```json
{
  "permissions": {
    "defaultMode": "dontAsk",
    "allow": [
      "Bash",
      "Read",
      "Edit",
      "Write",
      "Glob",
      "Grep",
      "WebFetch",
      "WebSearch",
      "Agent(Explore)"
    ],
    "deny": [
      "Read(~/.ssh/**)",
      "Read(~/.aws/**)",
      "Read(~/.gnupg/**)"
    ]
  },
  "sandbox": {
    "enabled": true,
    "autoAllowBashIfSandboxed": true,
    "allowUnsandboxedCommands": false,
    "excludedCommands": [],
    "filesystem": {
      "allowWrite": ["<agent_dir_absolute_path>"],
      "denyRead": ["~/.ssh", "~/.aws", "~/.gnupg"]
    },
    "network": {
      "allowedDomains": [
        "api.anthropic.com",
        "github.com",
        "*.npmjs.org",
        "crates.io",
        "agentskills.io",
        "api.telegram.org"
      ]
    }
  },
  "spinnerTipsEnabled": false,
  "prefersReducedMotion": true,
  "enabledPlugins": {
    "telegram@claude-plugins-official": true
  }
}
```

### Per-Agent `.claude-config/.claude.json` (Isolated Global Config)

```json
{
  "hasCompletedOnboarding": true,
  "projects": {
    "/home/user/.rightclaw/agents/right": {
      "hasTrustDialogAccepted": true,
      "hasTrustDialogHooksAccepted": true
    }
  }
}
```

### Key Schema Changes from v2.0

| Setting | v2.0 | v2.1 | Reason |
|---------|------|------|--------|
| `skipDangerousModePermissionPrompt` | `true` | **removed** | Not using bypass mode anymore |
| `--dangerously-skip-permissions` | Yes | **removed** | Replaced by `--permission-mode dontAsk` |
| `permissions.allow` | Not present | Array of tool rules | Explicit tool whitelist |
| `permissions.deny` | Not present | Array of deny rules | Explicit denials |
| `permissions.defaultMode` | Not present | `"dontAsk"` | Silent deny of non-allowed tools |
| `CLAUDE_CONFIG_DIR` | Not set | `$AGENT_DIR/.claude-config` | Per-agent config isolation |
| Trust in `~/.claude.json` | Host file | Per-agent `.claude-config/.claude.json` | No host file modification |

---

## 6. Interaction Between Sandbox and Permissions

Key insight from official docs: **Sandbox and permissions are complementary layers.**

- **Permissions** control CC's built-in tools (Read, Edit, Bash, WebFetch, etc.)
- **Sandbox** provides OS-level enforcement for Bash subprocesses only
- Read/Edit deny rules do NOT prevent `cat .env` in Bash -- sandbox does
- `autoAllowBashIfSandboxed: true` auto-approves Bash commands WITHIN sandbox boundaries
- `allowUnsandboxedCommands: false` prevents sandbox escape via `dangerouslyDisableSandbox` parameter
- `excludedCommands` runs those commands outside sandbox (use sparingly)

For fully headless agents:
1. `permissions.allow: ["Bash", "Read", "Edit", ...]` -- pre-approve all needed tools
2. `permissions.defaultMode: "dontAsk"` -- silently deny anything not listed
3. `sandbox.enabled: true` -- OS-level enforcement for Bash
4. `sandbox.autoAllowBashIfSandboxed: true` -- no bash prompts
5. `sandbox.allowUnsandboxedCommands: false` -- no escape hatch

This gives prompt-free operation with actual OS-level security boundaries.

---

## 7. What NOT to Use

| Avoid | Why | Use Instead |
|-------|-----|-------------|
| `--dangerously-skip-permissions` | Approves everything, no boundaries | `--permission-mode dontAsk` + allow rules |
| `defaultMode: "bypassPermissions"` | Same as above, in settings form | `defaultMode: "dontAsk"` |
| `$HOME` override to agent dir | Breaks git, SSH, Telegram, credentials | `CLAUDE_CONFIG_DIR` |
| `/etc/claude-code/managed-settings.json` | Machine-wide, affects all CC sessions, needs sudo | Project-level settings in `.claude/settings.json` |
| `skipDangerousModePermissionPrompt` | Tied to bypass mode which we're dropping | Not needed with `dontAsk` |

---

## 8. Alternatives Considered

### Permission Mode Alternatives

| Mode | Behavior | Why Not for RightClaw |
|------|----------|----------------------|
| `bypassPermissions` | Approve everything | No boundaries, the thing we're replacing |
| `acceptEdits` | Auto-approve file edits only | Still prompts for Bash, not headless-ready |
| `default` | Prompt for everything | Not headless-compatible |
| `plan` | Read-only, no modifications | Agents need to act |
| **`dontAsk`** | **Deny non-allowed, no prompts** | **Correct choice for headless agents** |

### Config Isolation Alternatives

| Approach | Pros | Cons |
|----------|------|------|
| **`CLAUDE_CONFIG_DIR`** | Isolates CC config, preserves git/SSH/etc | Partially documented, some edge cases |
| `$HOME` override | Complete isolation | Breaks everything that uses `$HOME` |
| Symlinks | Works with existing paths | Fragile, race conditions |
| `--bare` mode | No config loading at all | Loses skills, CLAUDE.md, hooks |
| Docker/container | Full isolation | Overkill, CC already has sandbox |

**Recommendation:** `CLAUDE_CONFIG_DIR` is the right balance. If edge cases surface, fall back to seeding a minimal `$AGENT_DIR/.claude-config/` with just the trust file.

---

## 9. Sandbox Settings Complete Reference

All keys under the `"sandbox"` object in settings.json:

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `enabled` | `boolean` | `false` | Enable bash sandboxing |
| `autoAllowBashIfSandboxed` | `boolean` | `true` | Auto-approve bash when sandboxed |
| `excludedCommands` | `string[]` | `[]` | Commands that run outside sandbox |
| `allowUnsandboxedCommands` | `boolean` | `true` | Allow `dangerouslyDisableSandbox` escape hatch |
| `filesystem.allowWrite` | `string[]` | `[]` | Additional writable paths (merges across scopes) |
| `filesystem.denyWrite` | `string[]` | `[]` | Paths blocked from writing (merges) |
| `filesystem.denyRead` | `string[]` | `[]` | Paths blocked from reading (merges) |
| `filesystem.allowRead` | `string[]` | `[]` | Re-allow reading within denyRead regions (takes precedence) |
| `filesystem.allowManagedReadPathsOnly` | `boolean` | `false` | (Managed only) Only managed allowRead paths |
| `network.allowedDomains` | `string[]` | `[]` | Allowed outbound domains (supports `*.example.com` wildcards) |
| `network.allowManagedDomainsOnly` | `boolean` | `false` | (Managed only) Only managed domains, silent block |
| `network.allowUnixSockets` | `string[]` | `[]` | Unix socket paths accessible in sandbox |
| `network.allowAllUnixSockets` | `boolean` | `false` | Allow all Unix socket connections |
| `network.allowLocalBinding` | `boolean` | `false` | Allow binding to localhost ports (macOS only) |
| `network.httpProxyPort` | `number|null` | `null` | Custom HTTP proxy port |
| `network.socksProxyPort` | `number|null` | `null` | Custom SOCKS5 proxy port |
| `enableWeakerNestedSandbox` | `boolean` | `false` | Weaker sandbox for unprivileged Docker (Linux/WSL2) |
| `enableWeakerNetworkIsolation` | `boolean` | `false` | (macOS) Allow TLS trust service access |

**Sandbox path prefixes** (for filesystem.allowWrite, denyWrite, denyRead, allowRead):

| Prefix | Meaning |
|--------|---------|
| `/` | Absolute path from filesystem root |
| `~/` | Relative to home directory |
| `./` or no prefix | Relative to project root (project settings) or `~/.claude` (user settings) |

---

## Sources

- [Claude Code Settings (official)](https://code.claude.com/docs/en/settings) -- complete settings schema, sandbox settings table, managed settings locations
- [Claude Code Permissions (official)](https://code.claude.com/docs/en/permissions) -- permission rule syntax, modes, managed-only settings table
- [Claude Code Sandboxing (official)](https://code.claude.com/docs/en/sandboxing) -- sandbox architecture, filesystem/network isolation, auto-allow mode
- [Claude Code Environment Variables (official)](https://code.claude.com/docs/en/env-vars) -- CLAUDE_CONFIG_DIR, ANTHROPIC_API_KEY, all env vars
- [Claude Code Headless Mode (official)](https://code.claude.com/docs/en/headless) -- --bare mode, --allowedTools, non-interactive usage
- [Secure Deployment Guide (official)](https://platform.claude.com/docs/en/agent-sdk/secure-deployment) -- proxy patterns, credential management, isolation technologies
- [Claude Code Settings Examples (GitHub)](https://github.com/anthropics/claude-code/tree/main/examples/settings) -- settings-lax.json, settings-strict.json, settings-bash-sandbox.json
- [CLAUDE_CONFIG_DIR issue #3833](https://github.com/anthropics/claude-code/issues/3833) -- CLAUDE_CONFIG_DIR behavior analysis
- [hasTrustDialogHooksAccepted issue #5572](https://github.com/anthropics/claude-code/issues/5572) -- trust dialog config fields
- [dontAsk subagent issue #11934](https://github.com/anthropics/claude-code/issues/11934) -- dontAsk mode + subagent interaction

---
*Stack research for: RightClaw v2.1 Headless Agent Isolation*
*Researched: 2026-03-24*
