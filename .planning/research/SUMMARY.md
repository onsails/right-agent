# Project Research Summary

**Project:** RightClaw v2.0
**Domain:** Multi-agent CLI runtime — sandbox migration (OpenShell -> CC native sandbox + per-agent HOME isolation)
**Researched:** 2026-03-23
**Confidence:** HIGH

## Executive Summary

RightClaw v2.0 replaces NVIDIA OpenShell with Claude Code's built-in sandbox (bubblewrap/Seatbelt) and introduces per-agent HOME isolation. The migration removes a heavy, alpha-quality external dependency (OpenShell required an ANTHROPIC_API_KEY, Docker/K3s, and `policy.yaml` authoring) and replaces it with CC's native `settings.json`-driven sandbox that works out of the box on macOS (Seatbelt) and requires only `bubblewrap` + `socat` on Linux. No new Rust crates are needed -- the entire migration is code changes to existing modules plus a new `codegen/settings.rs` module that generates per-agent `.claude/settings.json` files.

The recommended isolation approach is `HOME=$AGENT_DIR` as the primary mechanism, with absolute paths in all generated `settings.json` files. Setting HOME to the agent directory makes CC naturally scope all its config, memory, skills, and session state per-agent without any custom code. This is a Unix primitive that gives clean isolation. However, HOME override breaks `~` expansion in sandbox paths, SSH/git config resolution, credential discovery on Linux, and trust file location. Every generated `settings.json` must use fully-expanded absolute paths, never `~/` prefixes. `CLAUDE_CONFIG_DIR` is documented by CC but has known bugs (GitHub issues #3833, #25762) -- it should NOT be relied on as the primary mechanism, but can serve as a fallback flag (`--legacy-isolation`). The critical insight across all four research files: trust files (`.claude.json`) must be written to the agent's HOME directory (where CC will look at runtime), not the real user HOME. Getting this wrong causes agents to hang on the workspace trust dialog.

The top risks are: (1) Ubuntu 24.04+ AppArmor blocks unprivileged bubblewrap -- confirmed open issue on Anthropic's sandbox-runtime repo, affects the most popular developer desktop; (2) CC's Write/Edit tools bypass the bwrap sandbox in `bypassPermissions` mode -- the sandbox only constrains Bash tool commands, not file operations through CC's native tools; (3) OAuth credentials on Linux are HOME-dependent, so `ANTHROPIC_API_KEY` env var is required for headless multi-agent operation. All three are addressable: AppArmor via doctor detection and documented fix, Write/Edit bypass via accepting the constraint (bwrap still isolates all subprocess execution), and OAuth via API keys.

## Key Findings

### Recommended Stack

No new Rust crate dependencies. The v2.0 changes use `serde_json` (already in workspace) for generating `settings.json`, `minijinja` (already in workspace) for the rewritten shell wrapper template, and `which` (already in workspace) for dependency detection. The external dependency profile simplifies: OpenShell is removed entirely, replaced by `bubblewrap` + `socat` on Linux (both widely packaged) and zero new deps on macOS.

**Core technologies (changes only):**
- **CC native sandbox via `settings.json`**: Replaces OpenShell policy.yaml. Generated per-agent with `sandbox.enabled: true`, domain allowlists, filesystem write/deny rules. CC translates this to bwrap (Linux) or Seatbelt (macOS) profiles internally.
- **bubblewrap 0.4+ / socat 1.7+** (Linux only): Low-level sandboxing. CC invokes bwrap for filesystem/network namespace isolation and socat for Unix socket relay to network proxy. macOS needs nothing (Seatbelt is built-in).
- **`HOME` override**: Unix primitive for per-agent isolation. Each agent process runs with `HOME=$AGENT_DIR`, making all CC config paths agent-scoped.

**Minimum versions:**
| Component | Version | Rationale |
|-----------|---------|-----------|
| Claude Code | v2.1+ | Sandbox features, settings.json schema |
| Node.js | 22+ | Required by CC for sandbox features |
| bubblewrap | 0.4+ | Any distro-packaged version works |
| socat | 1.7+ | Any distro-packaged version works |
| Linux kernel | 4.18+ | User namespace support for bubblewrap |

### Expected Features

**Must have (table stakes for v2.0):**
- Per-agent `.claude/settings.json` generation with `sandbox.enabled: true`
- Per-agent HOME override (`HOME=$AGENT_DIR`) in shell wrapper
- Remove ALL OpenShell code paths (sandbox.rs, shell_wrapper.rs, discovery.rs, doctor.rs, deps.rs, init.rs, install.sh)
- Filesystem denyRead for sensitive host paths (SSH, AWS, GPG) using absolute paths
- `allowUnsandboxedCommands: false` -- closes CC's escape hatch
- `rightclaw doctor` checks for bubblewrap + socat on Linux
- Trust file (`.claude.json`) written to agent HOME, not real user HOME
- Installer updated for bubblewrap + socat (Linux), OpenShell removed

**Should have (differentiators, v2.x):**
- Sandbox config overrides in `agent.yaml` (`sandbox:` section with allowed_domains, allow_write, deny_read, excluded_commands)
- Agent-to-agent filesystem isolation (auto-generate denyRead for sibling agent dirs)
- Automatic domain detection from agent features (Telegram agents get `api.telegram.org`)
- Sandbox config diff on `rightclaw up` (show what changed before overwriting)
- AppArmor detection and fix guidance in `rightclaw doctor` for Ubuntu 24.04+

**Defer (v3+):**
- Custom proxy integration (`sandbox.network.httpProxyPort`)
- Managed settings deployment for enterprise
- MCP server isolation via `allowedMcpServers`
- Dynamic sandbox policy reload (CC reads settings at launch only -- restart required)
- Custom bwrap/Seatbelt profiles (anti-feature -- CC generates its own)

### Architecture Approach

The architecture change is a simplification. The v1.0 pipeline (discover -> resolve policy -> generate openshell wrapper -> spawn PC) becomes: discover -> generate settings.json -> generate HOME-override wrapper -> spawn PC. The sandbox lifecycle vanishes entirely -- bubblewrap is ephemeral per-process, no create/destroy needed. `RuntimeState` drops `sandbox_name` and `no_sandbox` fields. `cmd_down` becomes trivial (just shut down PC, no sandbox cleanup). A new `codegen/settings.rs` module generates per-agent `.claude/settings.json` that configures CC's native sandbox.

**Components to modify:**
1. **`codegen/shell_wrapper.rs`** -- Remove OpenShell invocation, add `HOME=$AGENT_DIR` export, single code path
2. **`agent/types.rs`** -- Remove `policy_path` from `AgentDef`, add optional sandbox config fields to `AgentConfig`
3. **`agent/discovery.rs`** -- Remove `policy.yaml` requirement
4. **`runtime/sandbox.rs`** -- Remove `AgentState`, `destroy_sandboxes()`, `sandbox_name_for()`. Simplify `RuntimeState`
5. **`runtime/deps.rs`** -- Replace `openshell` check with `bwrap` + `socat` (Linux only)
6. **`init.rs`** -- Stop generating `policy.yaml`, generate `.claude/settings.json` with sandbox config, fix `pre_trust_directory()` for HOME override
7. **`doctor.rs`** -- Replace `openshell` check with platform-conditional `bwrap` + `socat`

**Component to add:**
1. **`codegen/settings.rs`** (NEW) -- Generates per-agent `.claude/settings.json` with sandbox config, permissions, and feature-specific domains

### Critical Pitfalls

1. **Trust file written to wrong HOME** -- `pre_trust_directory()` uses `dirs::home_dir()` which reads from `/etc/passwd`, ignoring `$HOME` env. Under HOME override, CC reads `.claude.json` from the agent dir. Write trust to `$AGENT_DIR/.claude.json`, not the real user home. Getting this wrong = agent hangs forever on workspace trust dialog.

2. **Ubuntu 24.04+ AppArmor blocks bubblewrap** -- `kernel.apparmor_restrict_unprivileged_userns=1` blocks bwrap's `--unshare-net`. Confirmed on Anthropic's sandbox-runtime issue #74. `rightclaw doctor` must detect this and print the fix (create AppArmor profile for bwrap). Cannot ship v2.0 without handling this -- Ubuntu 24.04 LTS is the most common developer desktop.

3. **OAuth credentials inaccessible on Linux under HOME override** -- CC on Linux stores credentials in `$HOME/.claude/.credentials.json`. Agent HOME has no credentials. Require `ANTHROPIC_API_KEY` env var for multi-agent use. macOS Keychain works regardless of HOME.

4. **`~` paths in sandbox settings resolve to agent dir, not real HOME** -- `sandbox.filesystem.denyRead: ["~/.ssh"]` becomes `<agent_dir>/.ssh` (nonexistent). Real SSH keys at `/home/user/.ssh` remain accessible. Generate all sandbox paths as absolute, expanded at generation time.

5. **Write/Edit tools bypass bwrap sandbox** -- In `bypassPermissions` mode, CC's Write and Edit tools operate outside bwrap. Only Bash tool commands are sandboxed. This is a known CC limitation (issue #29048). Accept this -- bwrap still constrains all subprocess execution, which is where most damage potential lies.

## Implications for Roadmap

Based on research, the migration should follow a remove-then-add pattern. The Architecture research explicitly recommends this build order: "Remove first, add second. The compiler catches all leftover references."

### Phase 1: Remove OpenShell and Add HOME Isolation Foundation

**Rationale:** Everything downstream depends on the new isolation model. OpenShell removal and HOME override are tightly coupled -- the shell wrapper template changes, the agent types change, the runtime state changes. Do it atomically so the compiler catches all dangling references.
**Delivers:** Compilable codebase with OpenShell fully removed, HOME override in shell wrapper, simplified RuntimeState, policy.yaml no longer required, dependency checks updated for bwrap/socat.
**Addresses:** Remove OpenShell code paths (P1), shell wrapper rewrite (P1), agent discovery update (P1), dependency checks (P1)
**Avoids:** Pitfall #8 (OpenShell migration breakage) -- handle v1.0 state file deserialization gracefully with `#[serde(default)]`. Pitfall #9 (env var propagation) -- set all env vars in shell wrapper, not process-compose YAML.

### Phase 2: Settings Generation and Sandbox Configuration

**Rationale:** With OpenShell gone and HOME override in place, the sandbox config must exist before CC launches. The `codegen/settings.rs` module generates per-agent `.claude/settings.json`. This is the security boundary -- must be correct before any agents run.
**Delivers:** Per-agent `.claude/settings.json` with `sandbox.enabled: true`, filesystem deny rules (absolute paths), network domain allowlists, `allowUnsandboxedCommands: false`. Trust file (`.claude.json`) written to agent HOME.
**Addresses:** settings.json generation (P1), base denyRead for sensitive paths (P1), pre-trust fix for HOME override (P1), `allowUnsandboxedCommands: false` (P1)
**Avoids:** Pitfall #1 (trust file location) -- write to agent dir. Pitfall #4 (tilde expansion) -- generate absolute paths only. Pitfall #2 (OAuth credentials) -- document ANTHROPIC_API_KEY requirement, add to wrapper env.

### Phase 3: Doctor, Installer, and Platform Compatibility

**Rationale:** Users need clear feedback on missing dependencies and platform-specific issues before launching agents. The Ubuntu 24.04 AppArmor issue (Pitfall #3) is a hard blocker for the most popular Linux distro.
**Delivers:** Updated `rightclaw doctor` with bwrap/socat checks, AppArmor detection on Ubuntu 24.04+, updated `install.sh` with bubblewrap+socat installation, macOS zero-dep confirmation.
**Addresses:** Doctor update (P1), installer update (P1)
**Avoids:** Pitfall #3 (AppArmor blocks bwrap) -- detect and provide fix script. Pitfall #7 (macOS Seatbelt deprecation) -- no action needed, just awareness.

### Phase 4: Telegram, Git, and Agent Environment Setup

**Rationale:** After core sandbox works, agent-specific environment needs attention. Telegram plugin reads `.env` from HOME-relative path. Git/SSH config is lost under HOME override. These are blockers for real-world agent workflows.
**Delivers:** Telegram `.env` and `access.json` written to agent HOME, `GIT_CONFIG_GLOBAL` and `SSH_AUTH_SOCK` env vars in wrapper, git author identity env vars, multi-agent concurrent network verification.
**Addresses:** Telegram channel support under HOME override, git/SSH functionality in sandboxed agents
**Avoids:** Pitfall #5 (SSH/Git identity lost) -- explicit env vars. Pitfall #6 (Telegram .env path) -- write to agent dir. Pitfall #10 (proxy socket conflicts) -- let CC auto-assign, verify concurrently.

### Phase 5: Sandbox Config Extensibility (v2.x)

**Rationale:** Once the base migration is stable, add user-facing configurability. Sandbox overrides in `agent.yaml`, agent-to-agent isolation, automatic domain detection.
**Delivers:** `agent.yaml` `sandbox:` section, per-agent domain auto-detection, agent-to-agent filesystem isolation via denyRead, config diff on `rightclaw up`.
**Addresses:** Sandbox overrides in agent.yaml (P2), agent-to-agent isolation (P2), auto domain detection (P2), sandbox config diff (P3)

### Phase Ordering Rationale

- **Remove before add:** Phase 1 strips OpenShell. Phase 2 adds the replacement. This prevents any code path where both systems coexist.
- **Security before environment:** Phases 2-3 establish the sandbox boundary. Phase 4 adds convenience (git, Telegram). Security comes first.
- **Core before extensions:** Phases 1-4 achieve feature parity with v1.0 (minus OpenShell). Phase 5 adds new capabilities.
- **Dependency chain:** settings.json must exist before wrapper launches CC. Doctor must detect bwrap before user runs `rightclaw up`. Telegram needs HOME override working before `.env` paths can be validated.

### Research Flags

Phases likely needing deeper research during planning:
- **Phase 2 (Settings Generation):** CC's exact behavior when user-scope and project-scope `settings.json` collapse (both resolve to same file under HOME override). Needs empirical verification.
- **Phase 4 (Telegram/Git):** Telegram plugin's actual path resolution under HOME override is unverified. SSH agent forwarding through bwrap namespace boundary needs testing.

Phases with standard patterns (skip research-phase):
- **Phase 1 (Remove OpenShell):** Pure code deletion + struct simplification. Compiler-guided.
- **Phase 3 (Doctor/Installer):** Standard dependency detection pattern. Shell scripting.
- **Phase 5 (Extensibility):** Standard config merging. Well-understood patterns.

## Confidence Assessment

| Area | Confidence | Notes |
|------|------------|-------|
| Stack | HIGH | No new crates. All deps verified on crates.io. bwrap/socat are mature system packages. |
| Features | HIGH | CC sandbox settings.json schema verified against official docs (code.claude.com). Feature matrix is clear. |
| Architecture | HIGH | Component changes are well-scoped. New `codegen/settings.rs` is straightforward JSON generation. Build order is dependency-driven. |
| Pitfalls | HIGH | 5 critical pitfalls identified with concrete prevention strategies. Ubuntu AppArmor issue confirmed by Anthropic's own repo (sandbox-runtime #74). |

**Overall confidence:** HIGH

### Gaps to Address

- **HOME override + trust dialog interaction:** Whether CC implicitly trusts its own HOME directory (making `pre_trust_directory()` unnecessary) is unverified. Needs empirical test: `HOME=/tmp/test-agent claude --version` and check for trust prompt.
- **CLAUDE_CONFIG_DIR reliability:** GitHub issues #3833 and #25762 report unclear behavior. The Stack researcher recommends it as primary; Architecture researcher recommends HOME override. Recommendation: use HOME override as primary, keep CLAUDE_CONFIG_DIR as potential belt-and-suspenders addition if HOME alone proves insufficient.
- **Write/Edit tool sandbox bypass:** CC issue #29048 confirms Write/Edit tools operate outside bwrap in bypassPermissions mode. This means an agent can write to any path its Unix user can access, regardless of `sandbox.filesystem.allowWrite`. This is a fundamental CC limitation -- RightClaw cannot fix it. Document it clearly. Future mitigation: Anthropic may address this in CC, or RightClaw could use managed settings scope (but that requires server-managed delivery).
- **Multi-agent proxy socket contention:** CC likely uses PID-based socket naming, but this needs verification with 3+ concurrent agents.
- **AppArmor fix persistence:** The bwrap AppArmor profile must survive system updates. Verify that `/etc/apparmor.d/local-bwrap` persists across `apt upgrade`.

## Key Tension Resolutions

### HOME Override vs CLAUDE_CONFIG_DIR

The Stack researcher recommends `CLAUDE_CONFIG_DIR` as primary (cleaner, does not break git/ssh). The Architecture researcher recommends `HOME` override (more comprehensive, Unix primitive). The Features researcher says use both.

**Resolution: HOME override as primary.** Rationale:
1. HOME override gives complete isolation -- CC's entire config tree moves to the agent dir automatically.
2. `CLAUDE_CONFIG_DIR` has known bugs and unclear scope (what exactly does it redirect?).
3. Git/SSH breakage under HOME override is solvable with explicit env vars (`GIT_CONFIG_GLOBAL`, `SSH_AUTH_SOCK`).
4. The "blunt instrument" nature of HOME override is actually a feature for multi-agent isolation -- no leakage by default.

**Tradeoff documented:** Tools that read `$HOME` (git, ssh, npm, cargo) need explicit env var overrides in the shell wrapper. This is a one-time setup cost per tool.

### Trust File Location

Trust (`.claude.json` with `hasTrustDialogAccepted`) must be written to the agent's runtime HOME, which IS the agent dir when HOME is overridden. The existing `pre_trust_directory()` using `dirs::home_dir()` writes to the real user HOME -- this is wrong under v2.0 semantics. Fix: write to `$AGENT_DIR/.claude.json`.

### Write/Edit Sandbox Bypass

CC's Write and Edit tools are JavaScript-level operations that do not go through bwrap. In `bypassPermissions` mode, they can write anywhere the Unix user can. This is a CC design choice, not a RightClaw bug. The bwrap sandbox constrains Bash tool subprocess execution, which is where most dangerous operations (installing packages, running scripts, network access) happen. Accept this limitation and document it.

### Ubuntu 24.04 AppArmor

This is a hard blocker for the majority of Linux users. `rightclaw doctor` must detect the condition (try `bwrap --unshare-net echo ok`) and provide the exact fix commands. The installer should attempt to create the AppArmor profile automatically (with sudo).

## Sources

### Primary (HIGH confidence)
- [Claude Code Sandboxing Docs](https://code.claude.com/docs/en/sandboxing) -- sandbox architecture, settings.json schema, platform behavior
- [Claude Code Settings Reference](https://code.claude.com/docs/en/settings) -- complete settings.json schema including all sandbox fields
- [Claude Code Environment Variables](https://code.claude.com/docs/en/env-vars) -- CLAUDE_CONFIG_DIR, CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC
- [Claude Code Authentication Docs](https://code.claude.com/docs/en/authentication) -- credential storage locations per platform
- [bubblewrap GitHub](https://github.com/containers/bubblewrap) -- user namespace requirements, version info
- [sandbox-runtime issue #74](https://github.com/anthropic-experimental/sandbox-runtime/issues/74) -- Ubuntu 24.04 AppArmor conflict confirmation

### Secondary (MEDIUM confidence)
- [Anthropic Engineering: Claude Code Sandboxing](https://www.anthropic.com/engineering/claude-code-sandboxing) -- high-level architecture overview
- [Trail of Bits claude-code-config](https://github.com/trailofbits/claude-code-config) -- community security config patterns
- [GitHub Issue #25762: CLAUDE_CONFIG_DIR](https://github.com/anthropics/claude-code/issues/25762) -- CLAUDE_CONFIG_DIR status
- [GitHub Issue #3833: CLAUDE_CONFIG_DIR behavior](https://github.com/anthropics/claude-code/issues/3833) -- edge cases
- [GitHub Issue #29048: Write/Edit bypass sandbox](https://github.com/anthropics/claude-code/issues/29048) -- sandbox limitation
- [Ubuntu: Restricted unprivileged user namespaces](https://ubuntu.com/blog/ubuntu-23-10-restricted-unprivileged-user-namespaces) -- AppArmor userns restriction

### Tertiary (LOW confidence)
- [Ona: How Claude Code escapes its own sandbox](https://ona.com/stories/how-claude-code-escapes-its-own-denylist-and-sandbox) -- sandbox escape research, needs validation
- [sandbox-exec deprecation (OpenAI Codex issue #215)](https://github.com/openai/codex/issues/215) -- macOS Seatbelt long-term status

---
*Research completed: 2026-03-23*
*Ready for roadmap: yes*
