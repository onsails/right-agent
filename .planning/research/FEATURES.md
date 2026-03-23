# Feature Research: CC Native Sandbox & Per-Agent HOME Isolation

**Domain:** Multi-agent runtime sandbox migration (OpenShell -> Claude Code native sandbox)
**Researched:** 2026-03-23
**Confidence:** HIGH (CC native sandbox is well-documented, settings.json schema is official)

## Feature Landscape

### Table Stakes (Users Expect These)

Features that are non-negotiable for the v2.0 sandbox migration. Without these, the migration is incomplete or agents fail to launch.

| Feature | Why Expected | Complexity | Notes |
|---------|--------------|------------|-------|
| Per-agent `settings.json` generation with `sandbox.enabled: true` | Core of the migration -- replaces OpenShell policy.yaml entirely | MEDIUM | Generate `.claude/settings.json` inside each agent dir. Sandbox settings live under `"sandbox"` key. Must include `autoAllowBashIfSandboxed: true` for autonomous operation. |
| Per-agent `$HOME` override via environment variable | Agents must have isolated `.claude/` dirs, settings, memory, permissions | MEDIUM | Set `HOME=<agent-dir>` in the shell wrapper. CC reads `~/.claude/settings.json` relative to `$HOME`. Also set `CLAUDE_CONFIG_DIR=<agent-dir>/.claude` as belt-and-suspenders since CC sometimes still uses the real home. |
| Filesystem write isolation to agent working directory | Default sandbox behavior -- agents can only write to their own cwd + subdirs | LOW | This is the sandbox default. No config needed. Agent dir IS the working dir, so writes are naturally scoped. |
| Network domain allowlists per agent | Agents need Anthropic API, GitHub, possibly Telegram, skills.sh | MEDIUM | `sandbox.network.allowedDomains` array. Must include at minimum: `api.anthropic.com`, `claude.ai`, `statsig.anthropic.com`, `sentry.io`. Telegram agents add `api.telegram.org`. Skills.sh agents add the registry domain. |
| Filesystem `allowWrite` for `/tmp` and agent-specific paths | CC subprocess tools (npm, git, etc.) need temp file access | LOW | `sandbox.filesystem.allowWrite: ["/tmp"]` is the common minimum. Also need write access to `~/.claude` (resolves to `<agent-dir>/.claude` with HOME override). |
| Filesystem `denyRead` for sensitive host paths | Prevent agents from reading SSH keys, cloud credentials, etc. | LOW | `sandbox.filesystem.denyRead: ["~/.ssh", "~/.aws", "~/.gnupg", "~/.docker"]`. Since HOME is overridden, `~` resolves to agent dir -- so must use absolute paths to deny real host paths (e.g., `/home/<user>/.ssh`). |
| Remove OpenShell code paths entirely | v1 -> v2 migration removes all `openshell` binary invocations, policy.yaml handling, sandbox create/destroy lifecycle | MEDIUM | Affects: `sandbox.rs` (delete `destroy_sandboxes`, `AgentState.sandbox_name`), `shell_wrapper.rs` (remove openshell exec branch), `init.rs` (stop generating `policy.yaml`), `doctor.rs` (remove openshell binary check), `install.sh` (remove openshell step). |
| Replace `policy.yaml` requirement with `settings.json` generation | Agent discovery currently requires `policy.yaml` -- must change to optional or generate settings.json instead | MEDIUM | `AgentDef.policy_path` becomes optional. `init.rs` generates `.claude/settings.json` with sandbox config instead of `policy.yaml`. Discovery validates `.claude/settings.json` exists (or generates it). |
| `rightclaw doctor` checks for `bubblewrap` + `socat` (Linux) | Users need clear feedback on missing sandbox dependencies | LOW | On Linux: check `bwrap` and `socat` in PATH. On macOS: no check needed (Seatbelt is built-in). Replace the `openshell` check. |
| `install.sh` updated for new dependencies | Installer must install bubblewrap + socat on Linux, drop OpenShell | LOW | Replace `install_openshell()` with `install_sandbox_deps()`. Use `apt install bubblewrap socat` or `dnf install bubblewrap socat`. |
| `excludedCommands` for incompatible tools | Docker, watchman, etc. cannot run inside CC sandbox | LOW | `sandbox.excludedCommands: ["docker"]`. Include in generated settings.json. Let users extend via `agent.yaml`. |
| Pre-trust agent directory in `~/.claude.json` | Without this, CC shows trust dialog blocking non-interactive launch | LOW | Already implemented in `init.rs::pre_trust_directory()`. Must work with HOME override -- trust entry must be in the HOST's `~/.claude.json`, not the agent's overridden HOME. |

### Differentiators (Competitive Advantage)

Features that set RightClaw apart from raw CC sandbox usage or OpenClaw.

| Feature | Value Proposition | Complexity | Notes |
|---------|-------------------|------------|-------|
| Per-agent sandbox config in `agent.yaml` | Users define sandbox overrides (extra domains, extra write paths, excluded commands) declaratively in their agent config, RightClaw generates the settings.json | MEDIUM | Extend `agent.yaml` with optional `sandbox:` section. Fields: `allowed_domains`, `allow_write`, `deny_read`, `excluded_commands`. RightClaw merges base defaults + agent overrides into generated settings.json. |
| Automatic domain detection from agent features | Telegram agent gets `api.telegram.org` auto-allowed. Skills.sh agent gets registry domain auto-allowed. No manual network config. | LOW | Already partially done via `.mcp.json` detection for `--channels`. Extend to sandbox domain lists. |
| `allowUnsandboxedCommands: false` enforcement | Closes the CC escape hatch where Claude can retry commands outside sandbox. RightClaw enforces strict sandboxing. | LOW | Single setting in generated settings.json. Aligns with RightClaw's "security-first" positioning. |
| `CLAUDE_CONFIG_DIR` + `HOME` dual override | Belt-and-suspenders isolation. HOME override gives natural path resolution. CLAUDE_CONFIG_DIR ensures CC's config reader uses the right directory even when HOME handling has edge cases. | LOW | Set both env vars in wrapper script. `HOME=<agent-dir>` and `CLAUDE_CONFIG_DIR=<agent-dir>/.claude`. |
| Agent-to-agent filesystem isolation via `denyRead` | Each agent's sandbox denies reading other agents' directories. Agent "right" cannot read `~/.rightclaw/agents/scout/`. | MEDIUM | Generate `denyRead` entries for all sibling agent directories. Requires knowing the full agent list at settings.json generation time. |
| Sandbox config validation in `rightclaw doctor` | Validate that each agent's `.claude/settings.json` has valid sandbox config, required domains are present, filesystem paths resolve correctly. | LOW | Parse the generated settings.json, check sandbox.enabled is true, check allowedDomains includes Anthropic API. |
| Sandbox config diff on `rightclaw up` | Show what changed in sandbox config since last launch. Helps users understand security posture. | LOW | Compare generated settings.json with existing one before overwriting. Show diff summary. |

### Anti-Features (Commonly Requested, Often Problematic)

| Feature | Why Requested | Why Problematic | Alternative |
|---------|---------------|-----------------|-------------|
| Shared write access between agents | "Agent A generates code, agent B tests it" | Breaks isolation model entirely. Two agents writing to same dir causes race conditions, permission conflicts. CC sandbox enforces write isolation at OS level. | Use a shared `/tmp/<project>` directory with `allowWrite`, or communicate via MCP memory server (future). |
| Per-agent network proxy customization | "Different agents need different proxy configs" | `sandbox.network.httpProxyPort` / `socksProxyPort` is session-global. Multiple proxies per-host adds complexity for minimal value. | Use `allowedDomains` per agent instead. Different domain lists achieve the same security boundary without proxy complexity. |
| `enableWeakerNestedSandbox` by default | "Some users run in Docker" | Substantially weakens security. Silent degradation is worse than a clear error. | Expose as `agent.yaml` opt-in: `sandbox.weak_nested: true`. Document clearly. Default to `false`. |
| Dynamic sandbox policy reload | "Hot-reload network domains without restart" | CC sandbox settings are read at launch. No hot-reload mechanism exists (unlike OpenShell's dynamic policy). Implementing one would require restarting the CC process. | Restart the agent via `rightclaw restart <name>` to apply new sandbox config. |
| Full filesystem deny-by-default (deny all reads) | "Maximum security: deny everything, allow only agent dir" | Breaks CC itself. CC needs to read system binaries (`/usr/bin`), libraries (`/lib`), `/proc`, `/dev/urandom`. Overly restrictive denyRead causes cryptic failures. | Use targeted denyRead for sensitive paths (SSH, AWS, GPG) rather than blanket deny. Let CC's default read behavior handle the rest. |
| Agent-level `--dangerously-skip-permissions` control | "Some agents should skip permissions, others shouldn't" | Already using `--dangerously-skip-permissions` for all agents (required for autonomous operation). Removing it per-agent would cause permission prompts in non-interactive sessions. | Use sandbox + permissions rules for fine-grained control instead. Sandbox enforces OS-level restrictions even with skip-permissions. |
| Custom Seatbelt/bubblewrap profiles | "Power users want to write their own OS-level sandbox profiles" | CC generates its own bwrap/seatbelt profiles from settings.json. Custom profiles would conflict. No CC API for injecting custom profiles. | Use `agent.yaml` sandbox overrides which map to CC's settings.json schema. CC translates those to OS-level profiles. |

## Feature Dependencies

```
[Per-agent HOME override]
    |
    +--requires--> [settings.json generation]
    |                  |
    |                  +--requires--> [Remove OpenShell code paths]
    |                  |
    |                  +--requires--> [Agent discovery changes (policy.yaml -> settings.json)]
    |
    +--requires--> [Pre-trust with HOST's ~/.claude.json (not agent HOME)]
    |
    +--enables---> [Per-agent filesystem isolation via denyRead]
    +--enables---> [Per-agent network domain allowlists]
    +--enables---> [Per-agent sandbox config in agent.yaml]

[Shell wrapper rewrite]
    |
    +--requires--> [Remove OpenShell code paths]
    +--requires--> [Per-agent HOME override]
    +--requires--> [CLAUDE_CONFIG_DIR env var]

[install.sh update]
    +--requires--> [Remove OpenShell code paths]
    +--requires--> [bubblewrap + socat dependency detection]

[Agent-to-agent isolation]
    +--requires--> [Per-agent HOME override]
    +--requires--> [settings.json generation with full agent list context]

[Sandbox config in agent.yaml]
    +--requires--> [settings.json generation]
    +--enhances--> [Per-agent network domain allowlists]
    +--enhances--> [Per-agent filesystem isolation]
```

### Dependency Notes

- **Per-agent HOME override requires settings.json generation:** HOME override means CC looks for `~/.claude/settings.json` at `<agent-dir>/.claude/settings.json`. If that file doesn't exist with sandbox config, the sandbox won't be enabled.
- **Shell wrapper rewrite requires OpenShell removal:** The current wrapper has two branches -- openshell exec and no-sandbox fallback. v2 replaces both with `HOME=<agent-dir> CLAUDE_CONFIG_DIR=<agent-dir>/.claude claude`.
- **Pre-trust must use HOST's ~/.claude.json:** When HOME is overridden, `~/.claude.json` resolves to `<agent-dir>/.claude.json`. But CC's trust dialog reads from the REAL user's `~/.claude.json`. So `pre_trust_directory()` must explicitly use the host home directory, not the overridden one. This is a subtle but critical dependency.
- **Agent-to-agent isolation requires full agent list:** To generate `denyRead` entries for sibling agents, you need to know all agents at generation time. This means settings.json generation must happen AFTER agent discovery, not during init.

## MVP Definition

### Launch With (v2.0 Core)

Minimum viable sandbox migration -- what's needed for parity with v1.0 OpenShell behavior.

- [ ] **Remove all OpenShell code** -- delete `destroy_sandboxes()`, openshell wrapper branch, policy.yaml template, openshell binary checks, openshell install step
- [ ] **Per-agent `.claude/settings.json` generation** -- generate with `sandbox.enabled: true`, `autoAllowBashIfSandboxed: true`, `allowUnsandboxedCommands: false`, base `allowedDomains`, base `filesystem.allowWrite`
- [ ] **Shell wrapper rewrite** -- `HOME=<agent-dir> CLAUDE_CONFIG_DIR=<agent-dir>/.claude exec claude ...`
- [ ] **Agent discovery update** -- `policy.yaml` no longer required. Validate/generate `.claude/settings.json` instead.
- [ ] **Pre-trust fix for HOME override** -- `pre_trust_directory()` must use host home explicitly via `dirs::home_dir()` (not env HOME)
- [ ] **Doctor update** -- check bubblewrap + socat (Linux), drop openshell check
- [ ] **Installer update** -- install bubblewrap + socat (Linux), drop OpenShell step
- [ ] **Base denyRead for sensitive paths** -- SSH, AWS, GPG, Docker config using absolute host paths

### Add After Validation (v2.x)

Features to add once core sandbox migration is working.

- [ ] **Sandbox config overrides in `agent.yaml`** -- `sandbox:` section with `allowed_domains`, `allow_write`, `deny_read`, `excluded_commands`
- [ ] **Agent-to-agent filesystem isolation** -- auto-generate `denyRead` for sibling agent dirs
- [ ] **Automatic domain detection** -- Telegram/skills.sh agents get relevant domains auto-added
- [ ] **Sandbox config diff on `rightclaw up`** -- show changes before overwriting settings.json

### Future Consideration (v3+)

- [ ] **Custom proxy integration** -- `sandbox.network.httpProxyPort` for orgs with MITM proxies
- [ ] **Managed settings deployment** -- generate `managed-settings.json` for enterprise use
- [ ] **MCP server isolation** -- per-agent MCP server allowlists via `allowedMcpServers`

## Feature Prioritization Matrix

| Feature | User Value | Implementation Cost | Priority |
|---------|------------|---------------------|----------|
| Remove OpenShell code paths | HIGH | MEDIUM | P1 |
| Per-agent settings.json generation | HIGH | MEDIUM | P1 |
| Shell wrapper rewrite (HOME + CLAUDE_CONFIG_DIR) | HIGH | LOW | P1 |
| Agent discovery update (drop policy.yaml) | HIGH | MEDIUM | P1 |
| Pre-trust fix for HOME override | HIGH | LOW | P1 |
| Base denyRead for sensitive paths | HIGH | LOW | P1 |
| Doctor update (bubblewrap/socat) | MEDIUM | LOW | P1 |
| Installer update | MEDIUM | LOW | P1 |
| `allowUnsandboxedCommands: false` | HIGH | LOW | P1 |
| Sandbox overrides in agent.yaml | MEDIUM | MEDIUM | P2 |
| Agent-to-agent isolation (denyRead siblings) | MEDIUM | MEDIUM | P2 |
| Auto domain detection (Telegram/skills) | MEDIUM | LOW | P2 |
| Sandbox config diff on up | LOW | LOW | P3 |

**Priority key:**
- P1: Must have for v2.0 launch
- P2: Should have, add in v2.x
- P3: Nice to have, future consideration

## Competitor Feature Analysis

| Feature | OpenClaw (no sandbox) | RightClaw v1 (OpenShell) | RightClaw v2 (CC native) |
|---------|----------------------|--------------------------|--------------------------|
| Sandbox enforcement | None | OpenShell Landlock + seccomp + network policies | CC bubblewrap/Seatbelt + network proxy |
| Per-agent isolation | None (shared HOME, shared config) | Partial (OpenShell container, shared host HOME) | Full (agent dir IS HOME, own .claude/, own settings) |
| Network restrictions | None | OpenShell network_policies with per-binary rules | CC allowedDomains (simpler, domain-level, no per-binary) |
| Filesystem restrictions | None | OpenShell filesystem_policy with read_only/read_write lists | CC sandbox.filesystem with allowWrite/denyRead/denyWrite/allowRead |
| Setup complexity | Zero (just run claude) | High (API key, OpenShell install, policy.yaml authoring) | Low (bubblewrap + socat on Linux, zero on macOS) |
| Dependency count | 1 (claude) | 3 (claude, process-compose, openshell) | 2 (claude, process-compose) + bubblewrap/socat on Linux |
| Hot-reload policies | N/A | Yes (OpenShell dynamic network sections) | No (restart agent to apply) |
| Auth requirement | Claude subscription | Claude subscription + ANTHROPIC_API_KEY for OpenShell | Claude subscription only |

## What HOME Override Breaks and How to Fix

This section documents specific issues with setting `HOME=<agent-dir>` for Claude Code sessions.

### What Works

| Aspect | Behavior with HOME Override |
|--------|----------------------------|
| `~/.claude/settings.json` | Resolves to `<agent-dir>/.claude/settings.json` -- per-agent settings naturally scoped |
| `~/.claude/skills/` | Resolves to `<agent-dir>/.claude/skills/` -- per-agent skills naturally scoped |
| `.claude/` project dir | CC creates `.claude/` relative to cwd, which IS the agent dir -- works correctly |
| Memory/auto-memory | Stored in `~/.claude/` which becomes agent-scoped -- each agent has own memory |
| OAuth tokens | CC stores OAuth in `~/.claude/` -- each agent gets own token state |
| Sandbox `~` expansion | `~` in sandbox paths resolves to agent dir -- `~/.ssh` becomes `<agent-dir>/.ssh` (doesn't exist, harmless) |

### What Breaks

| Issue | Root Cause | Fix |
|-------|-----------|-----|
| `~/.claude.json` trust entries | CC reads trust from `~/.claude.json` which now resolves to `<agent-dir>/.claude.json`. Host trust entries are not found. | Write trust to BOTH host `~/.claude.json` AND `<agent-dir>/.claude.json`. Use `dirs::home_dir()` (reads from `/etc/passwd`, ignores `$HOME` env) for host path. |
| `denyRead` with `~` prefix | `sandbox.filesystem.denyRead: ["~/.ssh"]` resolves to `<agent-dir>/.ssh` which doesn't exist. Real SSH keys at `/home/user/.ssh` are NOT denied. | Use absolute paths in denyRead: `["/home/<user>/.ssh"]`. Resolve at generation time using host home dir. |
| Telegram channel `.env` | CC's Telegram plugin reads from `~/.claude/channels/telegram/.env`. With HOME override, this becomes `<agent-dir>/.claude/channels/telegram/.env`. | Copy or symlink the Telegram `.env` into each agent's `.claude/channels/telegram/` dir. Already partially handled by init.rs. |
| `CLAUDE_CONFIG_DIR` edge cases | CC v1.0.30+ moved some config to `~/.config/claude/`. HOME override doesn't affect `$XDG_CONFIG_HOME`. | Set `CLAUDE_CONFIG_DIR=<agent-dir>/.claude` explicitly. This overrides all config directory resolution. |
| Bun runtime path for Telegram | `~/.bun` is needed for Telegram plugin. With HOME override, `~/.bun` resolves to `<agent-dir>/.bun` which doesn't exist. | Add `sandbox.filesystem.allowRead` with absolute path to host's Bun installation. Or use system-installed bun. |
| Global npm/node access | Some CC operations need `~/.npm`, `~/.node`. These resolve to agent dir. | Add absolute paths to `sandbox.filesystem.allowWrite` for host npm/node dirs if needed, or rely on system-installed node. |
| `skipDangerousModePermissionPrompt` in user settings | Currently written to host `~/.claude/settings.json`. With HOME override, CC reads from agent dir. | Already handled: `init.rs` writes this to agent's `.claude/settings.json`. The host-level write in `pre_trust_directory()` is redundant with HOME override but harmless. |
| OAuth re-authentication | Each agent has own `~/.claude/` so no shared OAuth session. First launch of each agent requires separate auth. | Copy host's OAuth credentials into each agent's `.claude/` on first launch, or require `ANTHROPIC_API_KEY` env var (simpler for headless/multi-agent). |

### Critical Path Resolution Summary

When `HOME=/home/user/.rightclaw/agents/right`:

| Path Reference | Resolves To | Correct? |
|---------------|-------------|----------|
| `~/.claude/settings.json` | `/home/user/.rightclaw/agents/right/.claude/settings.json` | YES (per-agent settings) |
| `~/.claude.json` | `/home/user/.rightclaw/agents/right/.claude.json` | BROKEN (trust entries) |
| `~/.ssh` | `/home/user/.rightclaw/agents/right/.ssh` | N/A (doesn't exist) |
| `sandbox denyRead: ["~/.ssh"]` | denies `/home/user/.rightclaw/agents/right/.ssh` | BROKEN (must use absolute) |
| `sandbox allowWrite: ["/tmp"]` | `/tmp` | YES (absolute, no HOME resolution) |
| `sandbox allowedDomains` | N/A (not path-based) | YES |
| cwd for CC | Agent dir (set by process-compose working_dir) | YES |

## CC Native Sandbox Settings Reference (for code generation)

Complete `settings.json` sandbox schema for reference during implementation:

```json
{
  "sandbox": {
    "enabled": true,
    "autoAllowBashIfSandboxed": true,
    "allowUnsandboxedCommands": false,
    "excludedCommands": ["docker"],
    "enableWeakerNestedSandbox": false,
    "filesystem": {
      "allowWrite": ["/tmp"],
      "denyWrite": [],
      "denyRead": ["/home/user/.ssh", "/home/user/.aws", "/home/user/.gnupg"],
      "allowRead": []
    },
    "network": {
      "allowedDomains": [
        "api.anthropic.com",
        "claude.ai",
        "statsig.anthropic.com",
        "sentry.io"
      ],
      "allowUnixSockets": [],
      "allowAllUnixSockets": false,
      "allowLocalBinding": false,
      "allowManagedDomainsOnly": false
    }
  },
  "permissions": {
    "allow": [],
    "deny": []
  },
  "skipDangerousModePermissionPrompt": true,
  "spinnerTipsEnabled": false,
  "prefersReducedMotion": true
}
```

## Sources

- [Claude Code Sandboxing Documentation](https://code.claude.com/docs/en/sandboxing) -- Official sandbox docs, HIGH confidence
- [Claude Code Settings Reference](https://code.claude.com/docs/en/settings) -- Complete settings.json schema including all sandbox keys, HIGH confidence
- [Claude Code Environment Variables](https://code.claude.com/docs/en/env-vars) -- CLAUDE_CONFIG_DIR officially documented, HIGH confidence
- [Anthropic Engineering: Claude Code Sandboxing](https://www.anthropic.com/engineering/claude-code-sandboxing) -- Architecture rationale, HIGH confidence
- [Trail of Bits claude-code-config](https://github.com/trailofbits/claude-code-config) -- Community security config reference, MEDIUM confidence
- [GitHub Issue #25762: CLAUDE_CONFIG_DIR](https://github.com/anthropics/claude-code/issues/25762) -- CLAUDE_CONFIG_DIR status, MEDIUM confidence
- [GitHub Issue #3833: CLAUDE_CONFIG_DIR behavior](https://github.com/anthropics/claude-code/issues/3833) -- Known edge cases with CLAUDE_CONFIG_DIR, MEDIUM confidence
- [GitHub Issue #29048: allowWrite not enforced for Write/Edit tools](https://github.com/anthropics/claude-code/issues/29048) -- Sandbox limitation (Write tool bypasses bwrap in bypassPermissions mode), MEDIUM confidence
- [Ona: How Claude Code escapes its own sandbox](https://ona.com/stories/how-claude-code-escapes-its-own-denylist-and-sandbox) -- Sandbox escape research, MEDIUM confidence

---
*Feature research for: CC Native Sandbox & Per-Agent HOME Isolation*
*Researched: 2026-03-23*
