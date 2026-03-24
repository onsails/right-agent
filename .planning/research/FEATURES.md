# Feature Research: Headless Agent Isolation (v2.1)

**Domain:** Multi-agent runtime -- eliminating interactive prompts for fully autonomous headless operation
**Researched:** 2026-03-24
**Confidence:** HIGH (CC official docs verified, GitHub issues cross-referenced, existing codebase analyzed)

## Interactive Prompt Catalog

Every interactive prompt/dialog Claude Code can show that blocks a headless agent. This is the definitive list based on official docs, GitHub issues, and empirical testing.

### Prompt 1: Bypass Permissions Warning Dialog

**Trigger:** Launching CC with `--dangerously-skip-permissions` (or `--permission-mode bypassPermissions`)
**What it shows:** "WARNING: Claude Code running in Bypass Permissions mode. Yes, I accept / No, exit"
**Blocking?:** YES -- hangs until user clicks "Yes, I accept"
**Current mitigation:** `skipDangerousModePermissionPrompt: true` in `~/.claude/settings.json`
**Status:** CC bug #25503 (OPEN) -- the flag itself should be sufficient, but CC requires the persisted setting
**v2.0 state:** RightClaw writes `skipDangerousModePermissionPrompt: true` to both host `~/.claude/settings.json` and agent `.claude/settings.json` via `pre_trust_directory()`
**v2.1 approach:** Eliminate `--dangerously-skip-permissions` entirely. Use `permissions.allow` + sandbox instead. No bypass = no bypass warning.
**Confidence:** HIGH (official docs + issue #25503)

### Prompt 2: Workspace Trust Dialog

**Trigger:** Launching CC from a directory not yet trusted in `~/.claude.json`
**What it shows:** "Quick safety check: Is this a project you created or one you trust? 1. Yes, I trust this folder / 2. No, exit"
**Blocking?:** YES -- hangs until user selects option
**When it appears:** Only in directories WITHOUT a `.git` folder, OR directories not in `~/.claude.json` projects with `hasTrustDialogAccepted: true`
**Current mitigation:** `pre_trust_directory()` in `init.rs` writes `hasTrustDialogAccepted: true` to `~/.claude.json`
**Critical edge case with HOME override:** CC reads trust from `~/.claude.json`. With `HOME=<agent-dir>`, `~/.claude.json` resolves to `<agent-dir>/.claude.json`, not the host's `~/.claude.json`. Trust entries must exist in BOTH locations.
**v2.1 approach:** With HOME override, write `.claude.json` with trust entry into agent dir. Also keep host-level trust as fallback. Alternatively, `CLAUDE_CONFIG_DIR` may redirect where CC reads `.claude.json` -- needs verification.
**Security note:** CVE-2026-33068 (fixed in v2.1.53) -- trust dialog could be bypassed via repo-controlled `.claude/settings.json` setting `defaultMode: bypassPermissions`. Fixed by evaluating trust dialog before loading repo settings.
**Confidence:** HIGH (official docs + issue #28506 + CVE-2026-33068)

### Prompt 3: Protected Directory Write Prompt

**Trigger:** CC attempts to write to `.claude/`, `.git/`, `.vscode/`, or `.idea/` directories
**What it shows:** "Authorize Claude to modify its config files for this session?"
**Blocking?:** YES -- even in `bypassPermissions` mode (regression since v2.1.78)
**Exception:** Writes to `.claude/commands/`, `.claude/agents/`, `.claude/skills/` are EXEMPT (CC routinely writes there)
**Current mitigation:** None effective -- `--dangerously-skip-permissions` does NOT bypass this (issue #35718)
**v2.1 impact:** If agents need to write to their own `.claude/` (e.g., skill memory, settings), this prompt blocks headless operation. The `.claude/skills/` exemption covers most cases, but `.claude/settings.json` writes would be blocked.
**v2.1 approach:** Use `permissions.allow` with `Edit(.claude/**)` to explicitly allow. This MAY not help because the protected-directory logic is evaluated independently. Alternative: ensure RightClaw generates all `.claude/` files BEFORE launch so CC never needs to create/modify them. Pre-populate `.claude/settings.json`, `.claude/settings.local.json`, `.claude/skills/installed.json` at generation time.
**Confidence:** HIGH (issue #35718, confirmed regression from v2.1.78)

### Prompt 4: Tool Permission Prompts (Bash, Edit, Write)

**Trigger:** CC wants to use a tool not yet approved
**What it shows:** "Allow Claude to run [command]? Yes / Yes, don't ask again / No"
**Blocking?:** YES -- hangs for each unapproved tool use
**Current mitigation:** `--dangerously-skip-permissions` auto-approves everything
**v2.1 approach:** Replace `--dangerously-skip-permissions` with explicit `permissions.allow` rules:
```json
{
  "permissions": {
    "allow": ["Bash", "Read", "Edit", "Write", "WebFetch", "Agent(*)"],
    "deny": ["Read(./.env)", "Read(./.env.*)"]
  }
}
```
Combined with `sandbox.autoAllowBashIfSandboxed: true`, this auto-approves Bash commands inside the sandbox without prompts. `Edit` and `Write` tools need explicit allow rules.
**Key difference from bypass mode:** `permissions.allow` respects deny rules. Bypass mode ignores allow/deny (except protected directories). Allow rules are SAFER because they can be scoped.
**Confidence:** HIGH (official permissions docs)

### Prompt 5: New Domain Network Access Prompt

**Trigger:** Sandbox-enabled CC tries to access a domain not in `allowedDomains`
**What it shows:** "Allow network access to [domain]? Allow once / Allow always / Deny"
**Blocking?:** YES for 5 minutes (curl timeout), then fails silently
**Current mitigation:** Comprehensive `allowedDomains` list in sandbox settings
**v2.1 approach:** Two options:
  1. **Comprehensive allowedDomains** in project-level settings (current approach) -- works but user can add domains interactively. Missing a domain = 5-minute hang.
  2. **`allowManagedDomainsOnly: true`** in managed settings -- silently blocks non-allowed domains (no prompt). Requires managed-settings.json at system level. Machine-wide impact.
**Recommendation:** Use option 2 (`allowManagedDomainsOnly: true`) via `rightclaw init --strict-sandbox`. Document machine-wide impact. For users who don't want machine-wide, option 1 with generous default domain list + per-agent overrides.
**Confidence:** HIGH (official sandbox docs + SEED-008 UAT testing)

### Prompt 6: MCP Server Authorization Prompt

**Trigger:** CC discovers unapproved MCP servers in `.mcp.json` or `~/.claude.json`
**What it shows:** "1 MCP server needs auth: [server name]. Approve / Deny"
**Blocking?:** YES -- hangs until approved
**Current mitigation:** `--dangerously-skip-permissions` auto-approves. Also: setting `enableAllProjectMcpServers: true` in settings.json auto-approves project-level MCP servers.
**v2.1 approach:** Set `enableAllProjectMcpServers: true` in agent `.claude/settings.json`. With HOME override, host MCP servers (Canva, Gmail, etc.) are no longer visible -- only agent-specific MCP servers load.
**Confidence:** HIGH (official settings docs)

### Prompt 7: ANTHROPIC_API_KEY Approval Prompt

**Trigger:** `ANTHROPIC_API_KEY` env var is set in interactive mode
**What it shows:** "API key detected. Use this key instead of your subscription? Yes / No"
**Blocking?:** YES in interactive mode only. In `-p` mode (non-interactive), API key is auto-used.
**Current mitigation:** RightClaw agents run in interactive mode (not `-p`), so this prompt appears on first launch.
**v2.1 approach:** Two options:
  1. Pre-approve via `~/.claude.json` key approval state (undocumented, fragile)
  2. Don't use `ANTHROPIC_API_KEY` -- use OAuth via subscription
  3. Accept one-time prompt on first agent launch
**Recommendation:** Document that OAuth-based agents avoid this entirely. For API key users, the one-time approval is acceptable (CC persists the decision in `.claude.json`). With HOME override, each agent needs separate approval -- use OAuth instead.
**Confidence:** MEDIUM (observed behavior, not fully documented)

### Prompt 8: Plugin/Marketplace Trust Prompt

**Trigger:** CC discovers new plugins from `extraKnownMarketplaces` in project settings
**What it shows:** "Install marketplace [name]? / Install plugin [name]?"
**Blocking?:** YES -- hangs until approved
**Current mitigation:** `--dangerously-skip-permissions` does NOT bypass plugin trust
**v2.1 approach:** Pre-install all plugins via settings.json `enabledPlugins` at generation time. With HOME override, only agent-specific plugins are visible. If using Telegram plugin, pre-enable it in generated settings:
```json
{ "enabledPlugins": { "telegram@claude-plugins-official": true } }
```
Already implemented in `generate_settings()`.
**Confidence:** MEDIUM (observed behavior, partially documented)

## Permissions.allow vs --dangerously-skip-permissions

### Coverage Comparison

| Capability | `--dangerously-skip-permissions` | `permissions.allow` + sandbox |
|-----------|------|------|
| Auto-approve Bash commands | YES | YES (via `"Bash"` allow + `autoAllowBashIfSandboxed`) |
| Auto-approve file edits | YES | YES (via `"Edit"` allow) |
| Auto-approve file writes | YES | YES (via `"Write"` allow) |
| Auto-approve file reads | YES (all reads auto-approved anyway) | YES (reads never prompt by default) |
| Auto-approve WebFetch | YES | YES (via `"WebFetch"` allow) |
| Auto-approve MCP tools | YES | YES (via `mcp__*` allow rules) |
| Auto-approve subagents | YES | YES (via `"Agent(*)"` allow) |
| Skip bypass warning dialog | Requires `skipDangerousModePermissionPrompt` | N/A (no bypass = no warning) |
| Protected directory writes (.claude/, .git/) | NO (still prompts since v2.1.78) | UNTESTED -- `Edit(.claude/**)` may work but protected-dir logic may override |
| Deny rules respected | NO (bypass ignores allow/deny except protected dirs) | YES (deny rules always evaluated first) |
| Sandbox filesystem enforcement on Edit/Write tools | NO (Write tool bypasses bwrap in bypass mode) | YES (sandbox enforced for Bash; Edit/Write use permission rules) |
| Network domain prompts | YES (auto-allows all domains) | Requires `allowedDomains` list or `allowManagedDomainsOnly` |

### Key Insight: The Permission Evaluation Order

CC evaluates permissions in this order:
1. **Deny rules** (from `disallowed_tools` and `settings.json`) -- if match, BLOCKED even in bypass mode
2. **Permission mode** -- `bypassPermissions` approves everything reaching this step; `dontAsk` denies unlisted tools
3. **Allow rules** (from `allowed_tools` and `settings.json`) -- if match, APPROVED
4. **canUseTool callback** (SDK only) -- in `dontAsk` mode, this step is skipped and tool is DENIED

**For RightClaw v2.1:** Use `defaultMode: "default"` (or no mode) with broad `permissions.allow` rules. This gives the same auto-approval as bypass mode but RESPECTS deny rules and doesn't trigger the bypass warning dialog.

### Recommended Settings for Headless Agents

```json
{
  "skipDangerousModePermissionPrompt": true,
  "spinnerTipsEnabled": false,
  "prefersReducedMotion": true,
  "permissions": {
    "allow": [
      "Bash",
      "Read",
      "Edit",
      "Write",
      "WebFetch",
      "Agent(*)"
    ],
    "deny": [
      "Read(./.env)",
      "Read(./.env.*)"
    ]
  },
  "sandbox": {
    "enabled": true,
    "autoAllowBashIfSandboxed": true,
    "allowUnsandboxedCommands": false,
    "filesystem": {
      "allowWrite": ["/tmp"],
      "denyRead": ["/home/user/.ssh", "/home/user/.aws", "/home/user/.gnupg"]
    },
    "network": {
      "allowedDomains": ["api.anthropic.com", "github.com", "npmjs.org", "crates.io", "agentskills.io"]
    }
  },
  "enableAllProjectMcpServers": true,
  "disableAllHooks": true
}
```

**Why `disableAllHooks: true`:** With HOME override, host hooks (GSD workflows, personal automations) don't load from host `~/.claude/settings.json`. But if hooks exist in the agent's `.claude/settings.json`, they would fire. Disabling hooks prevents unexpected behavior. Agent-specific hooks can be explicitly enabled per agent.

### Protected Directory Problem -- The Hard One

The biggest obstacle to dropping `--dangerously-skip-permissions`: **protected directory writes**.

Current behavior (v2.1.78+):
- Even in `bypassPermissions` mode, writes to `.claude/`, `.git/`, `.vscode/`, `.idea/` prompt for confirmation
- Exception: `.claude/commands/`, `.claude/agents/`, `.claude/skills/` are auto-approved
- `permissions.allow` with `Edit(.claude/**)` has NOT been tested against the protected-directory gate

**Risk:** If `permissions.allow` ALSO doesn't bypass the protected-directory gate, then agents that need to modify `.claude/settings.json`, `.claude/CLAUDE.md`, or any `.claude/` files outside the exempted subdirs will be blocked.

**Mitigation strategy:**
1. Generate ALL `.claude/` files before launch (settings.json, skills/, installed.json)
2. Agent skills that write to `.claude/skills/*/memory/` are in the exempted path
3. If CC writes to `.claude/settings.local.json` automatically (for persisting approved permissions), this may trigger the prompt -- test empirically
4. Fallback: keep `--dangerously-skip-permissions` with `skipDangerousModePermissionPrompt: true` until CC fixes #35718

## HOME Override Behavior Catalog

### What `HOME=<agent-dir>` Does

Sets `$HOME` to the agent directory (e.g., `~/.rightclaw/agents/right`). CC resolves `~` via `$HOME`, so all user-level config paths redirect to the agent directory.

### What `CLAUDE_CONFIG_DIR=<agent-dir>/.claude` Does

Redirects CC's config directory (where `settings.json`, `sessions/`, `skills/`, `memory/`, etc. live). This is a belt-and-suspenders complement to HOME override. Known issues: partially respected (9+ open bugs).

### Resolution Table

| Path | Normal Resolution | With HOME Override | Correct? |
|------|------------------|-------------------|----------|
| `~/.claude/settings.json` | `/home/user/.claude/settings.json` | `<agent-dir>/.claude/settings.json` | YES -- per-agent settings |
| `~/.claude/skills/` | `/home/user/.claude/skills/` | `<agent-dir>/.claude/skills/` | YES -- per-agent skills |
| `~/.claude/agents/` | `/home/user/.claude/agents/` | `<agent-dir>/.claude/agents/` | YES -- per-agent subagents (won't load host's) |
| `~/.claude.json` | `/home/user/.claude.json` | `<agent-dir>/.claude.json` | NEEDS FIX -- trust entries must be here |
| `~/.claude/CLAUDE.md` | `/home/user/.claude/CLAUDE.md` | `<agent-dir>/.claude/CLAUDE.md` | YES -- host CLAUDE.md not loaded |
| `~/.claude/channels/telegram/.env` | `/home/user/.claude/channels/telegram/.env` | `<agent-dir>/.claude/channels/telegram/.env` | NEEDS FIX -- must copy/create here |
| `~/.claude/channels/telegram/access.json` | `/home/user/.claude/channels/telegram/access.json` | `<agent-dir>/.claude/channels/telegram/access.json` | NEEDS FIX -- must copy/create here |
| Host hooks in `~/.claude/settings.json` | Loaded | NOT loaded | BENEFIT -- no host hook interference |
| Host MCP servers in `~/.claude.json` | Loaded | NOT loaded | BENEFIT -- no unrelated MCP servers |
| OAuth tokens in `~/.claude.json` | Available | NOT available | ISSUE -- agents need ANTHROPIC_API_KEY or copied OAuth |
| `sandbox.filesystem.denyRead: ["~/.ssh"]` | Denies `/home/user/.ssh` | Denies `<agent-dir>/.ssh` (doesn't exist) | BROKEN -- must use absolute paths |
| `pre_trust_directory()` target | Host `~/.claude.json` | `<agent-dir>/.claude.json` | NEEDS FIX -- trust must be in agent-scoped location |
| `/etc/claude-code/managed-settings.json` | Loaded | Still loaded (absolute path) | OK -- system-level settings unaffected by HOME |

### What CC Auto-Creates in `~/.claude/`

CC auto-creates these directories/files as needed:
- `~/.claude/` (directory itself)
- `~/.claude/settings.json` (on first permission decision or `/config`)
- `~/.claude/settings.local.json` (for local permission persistence, also `.gitignore`s it)
- `~/.claude/sessions/` (conversation history)
- `~/.claude/memory/` (auto-memory files)
- `~/.claude/plans/` (plan files)
- `~/.claude/ide/` (IDE integration lock files)
- `~/.claude/local/` (local installation binary)

With HOME override, these auto-create inside the agent dir. This is CORRECT behavior for isolation.

### Authentication with HOME Override

| Auth Method | Works with HOME Override? | Notes |
|------------|--------------------------|-------|
| `ANTHROPIC_API_KEY` env var | YES | Env vars are independent of HOME. Pass via process-compose env. |
| OAuth via `~/.claude.json` | NO (without migration) | OAuth tokens stored in host `.claude.json`. Agent's `.claude.json` is empty. Must copy or symlink. |
| `apiKeyHelper` in settings.json | YES | Helper script runs relative to agent context. |
| Bedrock/Vertex/Foundry | YES | Use env vars (`CLAUDE_CODE_USE_BEDROCK`, etc.) |

**Recommendation:** Require `ANTHROPIC_API_KEY` for headless agents. OAuth is designed for interactive use with browser-based login flows. API keys are the intended headless auth mechanism.

### Git/SSH Access with HOME Override

| Resource | Default Location | With HOME Override | Solution |
|----------|-----------------|-------------------|----------|
| `~/.gitconfig` | `/home/user/.gitconfig` | `<agent-dir>/.gitconfig` (doesn't exist) | Set `GIT_CONFIG_GLOBAL=/home/user/.gitconfig` env var |
| `~/.ssh/` | `/home/user/.ssh/` | `<agent-dir>/.ssh` (doesn't exist) | Set `SSH_AUTH_SOCK` (if using agent), or use `GIT_SSH_COMMAND` with explicit key path |
| `~/.gitignore_global` | `/home/user/.gitignore_global` | `<agent-dir>/.gitignore_global` | Set via `GIT_CONFIG_GLOBAL` or `core.excludesFile` in agent `.gitconfig` |
| `~/.npmrc` | `/home/user/.npmrc` | `<agent-dir>/.npmrc` | Copy or set `NPM_CONFIG_USERCONFIG` |

**Env vars to forward in shell wrapper:**
```bash
export HOME="<agent-dir>"
export GIT_CONFIG_GLOBAL="/home/user/.gitconfig"
# SSH agent forwarding -- only if SSH_AUTH_SOCK exists
if [ -n "$SSH_AUTH_SOCK" ]; then
  export SSH_AUTH_SOCK="$SSH_AUTH_SOCK"
fi
```

## Feature Landscape

### Table Stakes (Agents Must Work Without Prompts)

| Feature | Why Expected | Complexity | Notes |
|---------|--------------|------------|-------|
| Drop `--dangerously-skip-permissions` for `permissions.allow` | Bypass mode shows warning dialog, doesn't respect deny rules, has protected-dir regression | MEDIUM | Use `"Bash", "Read", "Edit", "Write", "WebFetch"` in allow list. Sandbox handles Bash auto-approval. |
| Per-agent HOME override | Agents must not see host config, hooks, MCP servers, or other agents' state | MEDIUM | `HOME=<agent-dir>` in wrapper. CLAUDE_CONFIG_DIR as backup. Handle trust, Telegram, git/SSH edge cases. |
| Workspace trust for HOME-overridden agents | Trust dialog blocks headless launch | LOW | Write `.claude.json` with trust entry into agent dir during `rightclaw up` codegen, not just `init`. |
| Pre-populate all `.claude/` files before launch | Protected-dir prompt blocks any CC writes to `.claude/` (v2.1.78+ regression) | LOW | Already mostly done (settings.json, skills/, installed.json). Add: settings.local.json (empty), channels/ dir structure. |
| `allowManagedDomainsOnly` support | Network domain prompts hang headless agents for 5 minutes | MEDIUM | `rightclaw init --strict-sandbox` writes managed-settings.json. Document machine-wide impact. |
| `enableAllProjectMcpServers: true` in settings | MCP auth prompts block headless agents | LOW | Add to generated settings.json. |
| `disableAllHooks: true` in settings | Prevent host/stale hooks from firing in agent context | LOW | Add to generated settings.json. Can be overridden per-agent. |
| `ANTHROPIC_API_KEY` passthrough | OAuth doesn't work cleanly with HOME override | LOW | Document as recommended auth for headless agents. Forward from process-compose env. |
| Git/SSH env forwarding | Agents need git access but HOME override breaks gitconfig/SSH | LOW | Forward `GIT_CONFIG_GLOBAL`, `SSH_AUTH_SOCK` in wrapper. |
| Telegram channel files in agent HOME | HOME override breaks Telegram `.env` and `access.json` paths | MEDIUM | Copy/create Telegram channel files in `<agent-dir>/.claude/channels/telegram/` during codegen. |

### Differentiators (Competitive Advantage)

| Feature | Value Proposition | Complexity | Notes |
|---------|-------------------|------------|-------|
| Zero-prompt headless launch | No RightClaw agent ever shows an interactive prompt. Complete "fire and forget" | MEDIUM | Combination of all table stakes above. Unique vs OpenClaw which has no isolation. |
| Managed settings deployment via CLI | `rightclaw init --strict-sandbox` sets up machine-wide silent domain blocking | LOW | Writes `/etc/claude-code/managed-settings.json`. Requires sudo. Clear messaging. |
| Permission deny rules respected | Unlike bypass mode, agents respect deny rules for sensitive files | LOW | `permissions.deny` prevents reading `.env`, secrets, etc. even with broad allow. |
| Per-agent environment isolation report | `rightclaw doctor` reports each agent's isolation status: HOME override, trust, Telegram, permissions | MEDIUM | Extend doctor to validate agent-level config completeness. |
| Agent startup health check | Detect and report missing trust entries, missing Telegram files, missing API key before launch | MEDIUM | Pre-flight check in `rightclaw up` before spawning process-compose. |

### Anti-Features (Do NOT Build)

| Feature | Why Requested | Why Problematic | Alternative |
|---------|---------------|-----------------|-------------|
| OAuth token copying/sharing between agents | "I don't want to set API keys" | OAuth tokens are session-bound, expire, require browser refresh. Copying creates stale token issues. CC may detect token mismatch and force re-auth. | Use `ANTHROPIC_API_KEY` for headless agents. OAuth is for interactive human use. |
| `--dangerously-skip-permissions` with workarounds | "Just fix the bypass warning" | Bypass mode doesn't respect deny rules, has active regressions (#35718), and Anthropic is moving away from it. Building on a foundation they're deprecating is risky. | `permissions.allow` + sandbox is the intended replacement. Aligns with CC's security direction. |
| `dontAsk` mode as default | "Auto-deny everything not in allow list" | `dontAsk` is SDK-only (TypeScript), not available via CLI flag or settings.json. Attempting to set it via settings.json is undocumented and may not work. | Use `permissions.allow` with explicit rules. Unlisted tools prompt (won't happen if all tools are allowed). |
| Per-agent managed-settings.json | "Each agent should have its own managed settings" | Managed settings are machine-wide by design. Per-agent managed settings would require CC to support multiple managed-settings paths, which it doesn't. | Use project-level settings for per-agent config. Managed settings only for machine-wide policies (domain blocking). |
| Symlinking host `.claude/` into agent dir | "Share plugins/settings across agents" | Defeats isolation. Changes in one agent affect others. Symlink races with concurrent agents. | Each agent gets own `.claude/` populated at codegen time. Shared resources via `agent.yaml` overrides. |

## Feature Dependencies

```
[Drop --dangerously-skip-permissions]
    |
    +--requires--> [permissions.allow in settings.json]
    |                  +--generates--> [Broad allow rules: Bash, Edit, Write, Read, WebFetch]
    |                  +--generates--> [Targeted deny rules: .env, secrets]
    |
    +--requires--> [Pre-populate .claude/ files]
    |                  +--avoids--> [Protected directory write prompt]
    |
    +--requires--> [allowManagedDomainsOnly OR comprehensive allowedDomains]
                       +--avoids--> [Network domain prompt]

[Per-agent HOME override]
    |
    +--requires--> [Trust entry in agent-scoped .claude.json]
    +--requires--> [Telegram files in agent-scoped .claude/channels/]
    +--requires--> [Git/SSH env forwarding]
    +--requires--> [ANTHROPIC_API_KEY for auth]
    |
    +--enables---> [Host config isolation (no host hooks, MCP, settings)]
    +--enables---> [Per-agent auto-memory isolation]
    +--enables---> [Per-agent session isolation]

[allowManagedDomainsOnly]
    +--requires--> [rightclaw init --strict-sandbox]
    +--conflicts--> [Per-agent allowedDomains in project settings]
    (managed allowedDomains override project-level ones)
```

### Dependency Notes

- **Dropping --dangerously-skip-permissions requires permissions.allow:** Without explicit allow rules, every tool use prompts. The allow rules replace the blanket bypass.
- **Pre-populate .claude/ avoids protected directory prompt:** CC's protected-directory logic (v2.1.78+) prompts on ANY write to `.claude/` except exempted subdirs. Generating all files before launch prevents CC from needing to write there.
- **allowManagedDomainsOnly conflicts with per-agent allowedDomains:** When `allowManagedDomainsOnly: true` is set in managed settings, ONLY managed-level `allowedDomains` are respected. Project-level domains are ignored. This means agent.yaml `sandbox.allowed_domains` overrides stop working for network. Workaround: put the superset of all agent domains in managed settings, or don't use `allowManagedDomainsOnly`.
- **HOME override requires trust in agent-scoped location:** CC reads trust from `~/.claude.json`. With HOME override, that's `<agent-dir>/.claude.json`. Trust entry must be there.

## MVP Definition

### Launch With (v2.1 Core)

- [ ] **`permissions.allow` in generated settings.json** -- `["Bash", "Read", "Edit", "Write", "WebFetch"]` replaces `--dangerously-skip-permissions`
- [ ] **Remove `--dangerously-skip-permissions` from wrapper template** -- no more bypass mode
- [ ] **Per-agent HOME override in wrapper** -- `HOME=<agent-dir>` + `CLAUDE_CONFIG_DIR=<agent-dir>/.claude`
- [ ] **Agent-scoped `.claude.json` with trust entry** -- generate during `rightclaw up`, not just `init`
- [ ] **Telegram files in agent HOME** -- copy `.env` and `access.json` to `<agent-dir>/.claude/channels/telegram/`
- [ ] **Git/SSH env forwarding** -- `GIT_CONFIG_GLOBAL`, `SSH_AUTH_SOCK` in wrapper
- [ ] **`enableAllProjectMcpServers: true`** in generated settings
- [ ] **`disableAllHooks: true`** in generated settings (overridable per agent)
- [ ] **Pre-populate `.claude/settings.local.json`** (empty JSON `{}`) to prevent CC auto-creation prompt
- [ ] **ANTHROPIC_API_KEY documentation** -- recommend for headless agents, document OAuth limitations

### Add After Validation (v2.1.x)

- [ ] **`rightclaw init --strict-sandbox`** -- writes managed-settings.json with `allowManagedDomainsOnly: true` + domain superset
- [ ] **Agent startup health check** -- pre-flight validation before process-compose launch
- [ ] **`rightclaw doctor` agent isolation report** -- per-agent trust, Telegram, permissions status

### Future Consideration (v2.2+)

- [ ] **Protected directory prompt fix** -- if/when CC fixes #35718, simplify .claude/ pre-population
- [ ] **`dontAsk` mode support** -- if CC adds CLI/settings support for dontAsk, use it as alternative to broad allow rules
- [ ] **Per-agent managed settings** -- if CC adds per-directory managed settings, migrate from machine-wide

## Feature Prioritization Matrix

| Feature | User Value | Implementation Cost | Priority |
|---------|------------|---------------------|----------|
| `permissions.allow` in settings.json | HIGH | LOW | P1 |
| Remove `--dangerously-skip-permissions` from wrapper | HIGH | LOW | P1 |
| Per-agent HOME override | HIGH | MEDIUM | P1 |
| Agent-scoped `.claude.json` trust | HIGH | LOW | P1 |
| Telegram files in agent HOME | HIGH (Telegram users) | LOW | P1 |
| Git/SSH env forwarding | HIGH | LOW | P1 |
| `enableAllProjectMcpServers` | MEDIUM | LOW | P1 |
| `disableAllHooks` | MEDIUM | LOW | P1 |
| Pre-populate `settings.local.json` | MEDIUM | LOW | P1 |
| ANTHROPIC_API_KEY docs | MEDIUM | LOW | P1 |
| `rightclaw init --strict-sandbox` | MEDIUM | MEDIUM | P2 |
| Agent startup health check | MEDIUM | MEDIUM | P2 |
| Doctor isolation report | LOW | MEDIUM | P3 |

## Sources

- [Claude Code Permissions Documentation](https://code.claude.com/docs/en/permissions) -- permission modes, rule syntax, managed-only settings, HIGH confidence
- [Claude Code Sandboxing Documentation](https://code.claude.com/docs/en/sandboxing) -- sandbox modes, autoAllowBashIfSandboxed, domain prompts, HIGH confidence
- [Claude Code Settings Reference](https://code.claude.com/docs/en/settings) -- complete settings.json schema, scopes, precedence, HIGH confidence
- [Claude Code Environment Variables](https://code.claude.com/docs/en/env-vars) -- CLAUDE_CONFIG_DIR, ANTHROPIC_API_KEY, all env vars, HIGH confidence
- [Claude Code Headless Mode](https://code.claude.com/docs/en/headless) -- -p flag, --bare mode, --allowedTools, HIGH confidence
- [GitHub Issue #25503: Bypass permissions dialog regression](https://github.com/anthropics/claude-code/issues/25503) -- OPEN, confirmed regression v2.1.42+, HIGH confidence
- [GitHub Issue #28506: Workspace trust prompt not bypassed](https://github.com/anthropics/claude-code/issues/28506) -- CLOSED (trust only in non-git dirs), HIGH confidence
- [GitHub Issue #35718: Protected directory write prompt](https://github.com/anthropics/claude-code/issues/35718) -- CLOSED, regression v2.1.78+, confirmed by multiple users, HIGH confidence
- [GitHub Issue #3833: CLAUDE_CONFIG_DIR behavior unclear](https://github.com/anthropics/claude-code/issues/3833) -- OPEN, hybrid behavior documented, MEDIUM confidence
- [CVE-2026-33068: Workspace trust dialog bypass via repo settings](https://advisories.gitlab.com/pkg/npm/@anthropic-ai/claude-code/CVE-2026-33068/) -- fixed in v2.1.53, HIGH confidence
- [SEED-004: Per-agent HOME isolation](SEED-004-per-agent-home-isolation.md) -- internal project seed, HIGH confidence
- [SEED-008: Managed settings for strict sandbox](SEED-008-managed-settings-strict-sandbox.md) -- internal project seed, HIGH confidence
- [SmartScope Claude Code Auto Approve Guide](https://smartscope.blog/en/generative-ai/claude/claude-code-auto-permission-guide/) -- community guide on permission modes, MEDIUM confidence
- [morphllm: 5 Modes, Only 1 Nuclear](https://www.morphllm.com/claude-code-dangerously-skip-permissions) -- comprehensive bypass mode analysis, MEDIUM confidence

---
*Feature research for: Headless Agent Isolation (v2.1)*
*Researched: 2026-03-24*
