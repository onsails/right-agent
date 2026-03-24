# Project Research Summary

**Project:** RightClaw v2.1 -- Headless Agent Isolation
**Domain:** Multi-agent CLI runtime -- dropping --dangerously-skip-permissions, per-agent HOME isolation, managed settings
**Researched:** 2026-03-24
**Confidence:** HIGH

## Executive Summary

RightClaw v2.1 eliminates `--dangerously-skip-permissions` and replaces it with CC's `permissions.allow` rules + sandbox auto-approval. The bypass permissions mechanism is actively broken across multiple CC versions (v2.1.42+, v2.1.71+, v2.1.77+) with cascading regressions that Anthropic is not fixing because they are moving toward explicit permission grants as the intended model. The replacement approach uses `permissions.allow: ["Bash", "Read", "Edit", "Write", "WebFetch", "Agent(*)"]` in per-agent project settings combined with `sandbox.autoAllowBashIfSandboxed: true` and `allowUnsandboxedCommands: false`. This gives identical headless behavior (no prompts) with actual security enforcement (deny rules respected, sandbox boundaries enforced). The wrapper template drops the `--dangerously-skip-permissions` flag entirely -- no bypass mode, no bypass warning dialog, no `skipDangerousModePermissionPrompt` workaround needed.

Per-agent HOME isolation (`HOME=<agent_dir>`) is the correct isolation primitive. The STACK researcher recommended `CLAUDE_CONFIG_DIR` as primary, but the PITFALLS research conclusively settles this: the `.claude.json` race condition (confirmed 8+ times on CC's issue tracker, 14 corruptions in 11 hours with concurrent sessions) makes shared `~/.claude.json` untenable for multi-agent. HOME override gives each agent its own `.claude.json`, `.claude/settings.json`, sessions, and memory. The cost -- broken git, SSH, and Telegram paths -- is solved with explicit env var forwarding (`GIT_CONFIG_GLOBAL`, `SSH_AUTH_SOCK`, `GIT_SSH_COMMAND`) and pre-populating `<agent_home>/.claude/channels/telegram/` with copied config. `CLAUDE_CONFIG_DIR` is set as belt-and-suspenders alongside HOME, not as a replacement.

The top risk is the protected directory write prompt (CC issue #35718, regression since v2.1.78): writes to `.claude/` paths outside the exempted subdirs (commands/, agents/, skills/) trigger an interactive prompt that blocks headless operation. Neither `--dangerously-skip-permissions` nor `permissions.allow` suppress this gate. The mitigation is pre-populating ALL `.claude/` files at `rightclaw up` time so CC never needs to create or modify them. For `allowManagedDomainsOnly` (silent domain blocking without prompts), the pragmatic decision is: do NOT use managed settings for v2.1 launch. Instead, use comprehensive `allowedDomains` in project-level settings with sandbox enforcement. Non-allowed domains are blocked at the OS level by bwrap/Seatbelt -- the sandbox blocks them before CC's WebFetch prompt fires. Managed settings (`/etc/claude-code/managed-settings.json`) are machine-wide, require sudo, and affect all CC sessions. Offer `rightclaw config managed-settings install` as an opt-in for users who want stricter enforcement.

## Key Findings

### Recommended Stack

No new Rust dependencies for v2.1. The changes are to codegen output (settings.json content, shell wrapper template) and a new `codegen/home_scaffold.rs` module. The permission and sandbox settings use `serde_json` (already in workspace). The shell wrapper template changes are in the existing `minijinja` template.

**Core technologies (v2.1 changes only):**
- **`permissions.allow` in settings.json**: Replaces `--dangerously-skip-permissions`. Explicit tool whitelist with deny rule support.
- **`sandbox.autoAllowBashIfSandboxed: true`**: Auto-approves Bash within sandbox without prompts. Combined with `allowUnsandboxedCommands: false`, all Bash goes through sandbox.
- **`HOME` override + `CLAUDE_CONFIG_DIR`**: Per-agent isolation of all CC config, trust, sessions, memory. HOME is primary, CLAUDE_CONFIG_DIR is belt-and-suspenders.
- **`ANTHROPIC_API_KEY` env var**: Required for headless agents. OAuth is broken under HOME override on Linux.

**Key env vars for shell wrapper:**
| Variable | Value | Purpose |
|----------|-------|---------|
| `HOME` | `<agent_home>` | Per-agent CC config isolation |
| `CLAUDE_CONFIG_DIR` | `<agent_home>/.claude` | Explicit CC config redirection |
| `GIT_CONFIG_GLOBAL` | `<real_home>/.gitconfig` | Git identity under HOME override |
| `GIT_SSH_COMMAND` | `ssh -F <real_home>/.ssh/config -o UserKnownHostsFile=<real_home>/.ssh/known_hosts` | SSH under HOME override |
| `SSH_AUTH_SOCK` | forwarded from parent | SSH agent socket |
| `ANTHROPIC_API_KEY` | forwarded from parent | API authentication |
| `CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC` | `1` | Disable telemetry, feature flags |
| `DISABLE_AUTOUPDATER` | `1` | Prevent mid-session CC updates |

### Expected Features

**Must have (v2.1 core -- all P1):**
- `permissions.allow` in generated settings.json replacing `--dangerously-skip-permissions`
- Per-agent HOME override in shell wrapper
- Agent-scoped `.claude.json` with trust + hooks trust entries
- Pre-populate ALL `.claude/` files before launch (settings.json, settings.local.json, skills/)
- Telegram channel files copied to agent HOME
- Git/SSH env forwarding in wrapper
- `enableAllProjectMcpServers: true` in settings
- Absolute paths in all sandbox filesystem rules (no `~/` under HOME override)
- `ANTHROPIC_API_KEY` passthrough (document as required for headless)
- `.git/` directory in every agent dir (prevents trust dialog)

**Should have (v2.1.x -- P2):**
- `rightclaw config managed-settings install` for opt-in strict domain blocking
- Agent startup health check (pre-flight validation before PC launch)
- CC version compatibility check in `rightclaw doctor`
- Managed settings conflict detection in `rightclaw doctor`

**Defer (v2.2+):**
- `dontAsk` mode support (SDK-only, not available via CLI/settings)
- Per-agent managed settings (CC does not support this)
- Protected directory prompt fix (depends on CC fixing #35718)

**Anti-features (do NOT build):**
- OAuth token copying between agents (stale tokens, race conditions)
- Symlinking host `.claude/` into agent dir (defeats isolation)
- `defaultMode: "bypassPermissions"` in settings (same regressions as CLI flag)

### Architecture Approach

Three new subsystems integrate with the existing codegen pipeline: (1) HOME scaffold that creates agent-specific home directories with trust, user-scope settings, and Telegram config; (2) permissions-based authorization that replaces bypass mode with explicit allow/deny rules; (3) optionally, managed settings generation for system-wide domain blocking. The settings hierarchy is deliberately split: managed scope (machine-wide domains), user scope (agent HOME `~/.claude/settings.json` for UI prefs), and project scope (agent cwd `.claude/settings.json` for permissions, sandbox, plugins).

**Major components:**
1. **`codegen/home_scaffold.rs`** (NEW) -- Creates `~/.rightclaw/homes/{name}/` with `.claude.json` trust, `.claude/settings.json` user-scope, Telegram channel files
2. **`codegen/settings.rs`** (MODIFIED) -- Add `permissions.allow`, remove `skipDangerousModePermissionPrompt`, absolute paths in all sandbox rules, remove `allowedDomains` if managed settings used
3. **`codegen/shell_wrapper.rs` + template** (MODIFIED) -- Remove `--dangerously-skip-permissions`, add HOME/CLAUDE_CONFIG_DIR/GIT_CONFIG_GLOBAL/SSH_AUTH_SOCK exports
4. **`codegen/managed_settings.rs`** (NEW, opt-in) -- Generates `/etc/claude-code/managed-settings.json` with merged domain allowlist and `allowManagedDomainsOnly: true`
5. **`init.rs`** (MODIFIED) -- Use `scaffold_agent_home()` instead of `pre_trust_directory()`, stop mutating host config

### Critical Pitfalls

1. **`.claude.json` race condition corrupts multi-agent state** -- CC uses non-atomic writes with no file locking. Per-agent HOME override is the fix, not a nice-to-have. Without it, 3+ agents corrupt `.claude.json` within minutes.

2. **Protected directory write prompt blocks headless agents** -- Even in bypass mode, CC prompts for `.claude/` writes (except commands/, agents/, skills/). Neither `permissions.allow` nor `--dangerously-skip-permissions` suppresses this. Pre-populate ALL `.claude/` files before launch.

3. **`~/` paths in sandbox settings resolve to agent HOME, not real HOME** -- `denyRead: ["~/.ssh"]` becomes `<agent_dir>/.ssh` (nonexistent). Real SSH keys remain accessible. All generated sandbox paths must be absolute.

4. **Bypass permissions is broken across CC v2.1.42+** -- The `skipDangerousModePermissionPrompt` flag, the `--dangerously-skip-permissions` CLI flag, and `defaultMode: "bypassPermissions"` in settings all have active regressions. Do not rely on any bypass mechanism.

5. **Workspace trust requires `.git/` in agent dir** -- Without `.git/`, CC shows trust dialog even with `hasTrustDialogAccepted: true` in `.claude.json`. Ensure `git init` runs in every agent directory.

## Key Tension Resolutions

### HOME Override vs CLAUDE_CONFIG_DIR

**Decision: HOME override as primary, CLAUDE_CONFIG_DIR as belt-and-suspenders.**

The STACK researcher recommended `CLAUDE_CONFIG_DIR` alone (preserves git/SSH, more targeted). The ARCHITECTURE researcher recommended HOME + CLAUDE_CONFIG_DIR together. The PITFALLS researcher confirmed that `.claude.json` lives at `$HOME/.claude.json` regardless of `CLAUDE_CONFIG_DIR` -- meaning CLAUDE_CONFIG_DIR alone does NOT prevent the `.claude.json` race condition (Anti-Pattern 5 in Architecture). HOME override moves everything, including `.claude.json`, to the agent dir. The git/SSH/Telegram breakage is a known cost, fully solved by env var forwarding.

### permissions.allow vs --dangerously-skip-permissions

**Decision: Drop bypass mode entirely. Use `permissions.allow` + sandbox auto-approval.**

The FEATURES researcher noted that `permissions.allow` cannot fully replace bypass mode for `.claude/` writes (Prompt 3). The PITFALLS researcher confirmed bypass mode itself is broken (Pitfalls 2, 3). The resolution: use `permissions.allow` for tool authorization + pre-populate `.claude/` files to avoid the protected directory gate. If an edge case surfaces where CC still prompts for `.claude/` writes despite pre-population, keep `--dangerously-skip-permissions` + `skipDangerousModePermissionPrompt` as a documented fallback flag (`--legacy-bypass`).

### Managed Settings

**Decision: Do NOT use managed settings for v2.1 launch. Offer as opt-in.**

The ARCHITECTURE researcher designed a managed settings subsystem with `allowManagedDomainsOnly: true`. The PITFALLS researcher flagged that this is machine-wide (affects all CC sessions), requires sudo, and cannot be customized per-agent. The STACK researcher recommended project-level settings only. Resolution: use per-agent `sandbox.network.allowedDomains` in project settings with sandbox enforcement. The sandbox blocks non-allowed domains at the OS level. For domains accessed via WebFetch (not Bash), add `WebFetch(domain:example.com)` to `permissions.allow`. Offer `rightclaw config managed-settings install` as an opt-in for users who want silent domain blocking via managed settings.

### Pre-populating .claude/ Files

**Decision: Generate all .claude/ files at `rightclaw up` time, not just at `rightclaw init`.**

All researchers agree that CC's protected directory write prompt (v2.1.78+) is a blocker. The solution is to ensure CC never needs to write to `.claude/` -- RightClaw pre-creates: `settings.json`, `settings.local.json` (empty `{}`), `skills/` directory, and any plugin-specific files. The `.claude/commands/`, `.claude/agents/`, and `.claude/skills/` subdirectories are exempted by CC and can be written to freely.

## Implications for Roadmap

### Phase 1: HOME Isolation and Permission Foundation

**Rationale:** Every other change depends on HOME isolation working correctly. The `.claude.json` race condition (Pitfall 1) makes this the foundational piece. Permission strategy must be decided here because it changes the shell wrapper template and settings output simultaneously.
**Delivers:** Per-agent HOME at `~/.rightclaw/homes/{name}/`, `.claude.json` with trust entries, user-scope settings in agent HOME, `permissions.allow` in project settings, absolute paths in all sandbox rules, `.git/` init in agent dirs.
**Addresses:** HOME override (P1), trust dialog elimination (P1), permission mode replacement (P1), sandbox path resolution (P1), credential strategy (P1)
**Avoids:** Pitfall 1 (.claude.json race), Pitfall 2 (bypass mode broken), Pitfall 4 (workspace trust), Pitfall 6 (tilde path resolution), Pitfall 7 (OAuth credentials), Pitfall 10 (Git/SSH identity), Pitfall 12 (secret leakage in PC env)

### Phase 2: Shell Wrapper and Codegen Update

**Rationale:** With the HOME scaffold and permission model defined, the wrapper template and settings generator must be updated to produce the new output. This is the "make it work" phase.
**Delivers:** Updated `agent-wrapper.sh.j2` without `--dangerously-skip-permissions` and with HOME/GIT/SSH exports. Updated `generate_settings()` with `permissions.allow`, no `skipDangerousModePermissionPrompt`, no `~/` paths. Pre-population of all `.claude/` files including `settings.local.json`.
**Uses:** HOME scaffold from Phase 1, permission rules defined in Phase 1
**Implements:** Subsystem 2 (HOME override) and Subsystem 3 (permissions-based auth) from Architecture

### Phase 3: Telegram, MCP, and Agent Environment Completeness

**Rationale:** After core headless operation works, handle the per-feature environment needs. Telegram requires config files in agent HOME. MCP servers need `enableAllProjectMcpServers: true`. Hooks need `disableAllHooks: true` (overridable).
**Delivers:** Telegram `.env` and `access.json` copied to agent HOME's `.claude/channels/telegram/`. MCP auto-approval in settings. Hook disabling in settings. Pre-flight health check before PC launch.
**Avoids:** Pitfall 11 (Telegram path resolution), Pitfall 3 (permissions.allow gaps for MCP)

### Phase 4: Doctor Enhancements and Managed Settings Opt-in

**Rationale:** Diagnostic tooling and the optional managed settings system are not launch blockers but prevent support headaches. CC version skew (Pitfall 8) and managed settings conflicts (Pitfall 9) need detection.
**Delivers:** CC version compatibility check in `rightclaw doctor`. Managed settings conflict detection. `rightclaw config managed-settings install` command. Agent isolation validation (`rightclaw doctor --agent <name>`).
**Addresses:** CC version skew (P2), managed settings (opt-in P2), agent health diagnostics (P2)
**Avoids:** Pitfall 8 (CC version skew), Pitfall 9 (managed settings conflict), Pitfall 13 (proxy port conflicts)

### Phase Ordering Rationale

- **Foundation first:** Phase 1 establishes the isolation model and permission strategy. Everything else builds on these decisions.
- **Codegen second:** Phase 2 implements the decisions in code. Must compile and pass tests before moving to feature-specific work.
- **Features third:** Phase 3 handles per-feature environment (Telegram, MCP, hooks). These are independent of each other but depend on HOME isolation working.
- **Diagnostics last:** Phase 4 is about operability, not functionality. Ship working agents before shipping better error messages.
- **Managed settings opt-in:** Deliberately excluded from core phases. Users who want machine-wide domain blocking can opt in; the default path works without sudo.

### Research Flags

Phases needing deeper research during planning:
- **Phase 1:** Empirical test needed: does `permissions.allow` with `Edit(.claude/**)` suppress the protected directory prompt? If yes, pre-population becomes optional for some paths. If no, pre-population is mandatory.
- **Phase 3:** Telegram plugin path resolution under HOME override is unverified. Need to test that the plugin reads from `$HOME/.claude/channels/telegram/` and not a hardcoded path.

Phases with standard patterns (skip research-phase):
- **Phase 2:** Shell wrapper and settings generation are well-understood codegen. The output format is documented in CC's official settings reference.
- **Phase 4:** Doctor checks and CLI commands are standard patterns. No research needed.

## Confidence Assessment

| Area | Confidence | Notes |
|------|------------|-------|
| Stack | HIGH | No new deps. Permission and sandbox settings verified against code.claude.com official docs. Env var behavior documented. |
| Features | HIGH | Complete prompt catalog with 8 interactive prompts identified. Each has verified trigger, mitigation, and CC issue reference. |
| Architecture | HIGH | Three subsystems cleanly separated. Data flow diagrams show settings scope distribution. Build order is dependency-driven. |
| Pitfalls | HIGH | 14 pitfalls identified (5 critical, 6 moderate, 3 minor). `.claude.json` race confirmed by 8+ CC issue reports. Bypass mode regressions confirmed across 4 CC versions. |

**Overall confidence:** HIGH

### Gaps to Address

- **Protected directory prompt behavior with `permissions.allow`:** Whether `Edit(.claude/**)` in allow rules suppresses the protected directory gate is untested. CC's code evaluates protected directory logic independently of permission rules. Mitigation: pre-populate all `.claude/` files regardless, and test empirically during Phase 1.

- **`CLAUDE_CONFIG_DIR` + HOME interaction:** When both are set, which takes precedence for `.claude.json` trust resolution? The STACK researcher says CLAUDE_CONFIG_DIR redirects `.claude.json`; the PITFALLS researcher (Anti-Pattern 5) says it does not. Mitigation: set both, test empirically, document the result.

- **`dontAsk` mode availability:** The FEATURES researcher identified `dontAsk` as SDK-only. The STACK researcher recommends it via `--permission-mode dontAsk` CLI flag. If the CLI flag exists, it would be ideal (auto-deny unapproved tools). Mitigation: test `--permission-mode dontAsk` on the CLI. If it works, use it. If not, broad `permissions.allow` achieves the same effect.

- **CC auto-update breaking agents:** No mechanism to pin CC version via settings. `DISABLE_AUTOUPDATER=1` may not be respected by all update channels. Mitigation: set `DISABLE_AUTOUPDATER=1` in wrapper, recommend `autoUpdatesChannel: "stable"` in settings, add CC version check to doctor.

## Sources

### Primary (HIGH confidence)
- [Claude Code Settings Reference](https://code.claude.com/docs/en/settings) -- complete settings hierarchy, managed settings, sandbox schema
- [Claude Code Permissions Documentation](https://code.claude.com/docs/en/permissions) -- permission modes, rule syntax, managed-only flags
- [Claude Code Sandboxing Documentation](https://code.claude.com/docs/en/sandboxing) -- sandbox architecture, autoAllowBashIfSandboxed, domain isolation
- [Claude Code Environment Variables](https://code.claude.com/docs/en/env-vars) -- CLAUDE_CONFIG_DIR, ANTHROPIC_API_KEY, all relevant env vars
- [Claude Code Headless Mode](https://code.claude.com/docs/en/headless) -- non-interactive usage, --bare, --allowedTools

### Secondary (MEDIUM confidence)
- [GitHub #28922: .claude.json race condition](https://github.com/anthropics/claude-code/issues/28922) -- concurrent write corruption
- [GitHub #18998: 14 corruptions in 11 hours](https://github.com/anthropics/claude-code/issues/18998) -- severity confirmation
- [GitHub #35718: bypass mode .claude/ write prompt](https://github.com/anthropics/claude-code/issues/35718) -- protected directory regression
- [GitHub #36168: bypass permissions broken v2.1.77+](https://github.com/anthropics/claude-code/issues/36168) -- complete regression
- [GitHub #25503: bypass dialog regression](https://github.com/anthropics/claude-code/issues/25503) -- VS Code silent fallback
- [GitHub #3833: CLAUDE_CONFIG_DIR behavior](https://github.com/anthropics/claude-code/issues/3833) -- unclear semantics
- [Trail of Bits claude-code-config](https://github.com/trailofbits/claude-code-config) -- community security patterns
- [CVE-2026-33068](https://advisories.gitlab.com/pkg/npm/@anthropic-ai/claude-code/CVE-2026-33068/) -- workspace trust bypass via repo settings

### Tertiary (LOW confidence)
- [GitHub #5572: hasTrustDialogHooksAccepted](https://github.com/anthropics/claude-code/issues/5572) -- hooks trust flag behavior
- [GitHub #26233: skipDangerousModePermissionPrompt undocumented](https://github.com/anthropics/claude-code/issues/26233) -- setting location ambiguity
- `dontAsk` mode CLI availability -- conflicting information between STACK and FEATURES research, needs empirical validation

---
*Research completed: 2026-03-24*
*Ready for roadmap: yes*
