# Pitfalls Research: Headless Agent Isolation (v2.1)

**Domain:** HOME override, managed settings, dropping `--dangerously-skip-permissions`, headless autonomous agents
**Researched:** 2026-03-24
**Confidence:** HIGH (official docs verified, CC issue tracker audited, codebase audited, race conditions confirmed by multiple reporters)

---

## Critical Pitfalls

Mistakes here cause agents to hang indefinitely, fail to authenticate, or run without the security controls you think are enforced.

### Pitfall 1: `.claude.json` Race Condition Corrupts Trust State Across Agents

**What goes wrong:**
Claude Code stores per-project trust state, OAuth session, MCP configs, and preferences in a single `~/.claude.json` file. With per-agent HOME override (`HOME=<agent_dir>`), each agent gets its OWN `.claude.json` at `<agent_dir>/.claude.json` -- this isolation is actually correct and avoids the race. BUT if any code path still writes to the REAL `~/.claude.json` (like `pre_trust_directory()` currently does, or if `rightclaw init` writes trust there), the old race condition applies.

The bigger risk: if v2.1 does NOT set `HOME` per-agent and instead uses `CLAUDE_CONFIG_DIR` or some other mechanism that still shares `~/.claude.json`, then ALL agents write trust state, allowed tools, and caches to the same file. Claude Code uses non-atomic writes (truncate + write, no file locking). With 3+ concurrent agents, `.claude.json` corrupts within minutes. This is confirmed as a known CC bug reported 8+ times since June 2025, all closed without resolution (GitHub #28922). One user with 30+ sessions saw 14 corruptions in 11 hours (GitHub #18998).

**Why it happens:**
Claude Code was designed for single-instance use. The `.claude.json` file is a monolithic state bag (trust, OAuth, MCP, caches) with no file locking or atomic writes. RightClaw runs multiple concurrent CC instances by design.

**Consequences:**
- Corrupted `.claude.json` causes all agents to crash on startup
- Trust state lost -- agents re-prompt for workspace trust (hangs headless)
- OAuth tokens corrupted -- agents lose authentication
- Allowed tools list corrupted -- permissions reset unpredictably

**Prevention:**
- Per-agent `HOME` override is the correct solution -- each agent gets its own `.claude.json`
- NEVER share a single `~/.claude.json` across agents
- `pre_trust_directory()` must write to `<agent_dir>/.claude.json`, not the real `~/.claude.json`
- Validate at `rightclaw up` time that no two agents share the same HOME

**Detection:**
- JSON parse errors in agent startup logs
- Agents that worked moments ago suddenly fail with "corrupted config"
- `~/.claude.json` contains truncated or invalid JSON

**Phase to address:** Phase 1 (HOME isolation implementation). This is the fundamental reason per-agent HOME exists.

**Confidence:** HIGH -- confirmed by multiple CC issue reports (#28922, #18998) and CC official docs acknowledging backup mechanism

---

### Pitfall 2: `skipDangerousModePermissionPrompt` Is Broken in Multiple CC Versions

**What goes wrong:**
The v2.0 codebase sets `skipDangerousModePermissionPrompt: true` in `settings.json` and passes `--dangerously-skip-permissions` on the CLI. The v2.1 milestone wants to drop the CLI flag and use `permissions.allow` + sandbox instead. But `skipDangerousModePermissionPrompt` itself is broken:

1. **Setting location ambiguity:** Must be a TOP-LEVEL key in `settings.json`, NOT nested under `permissions` (GitHub #26233). The setting is undocumented.
2. **VS Code regression (v2.1.42+):** The dialog doesn't render in VS Code, so the session silently falls back to default mode -- every command prompts (GitHub #25503, blog post from testinginproduction.co).
3. **Still prompts for Bash in v2.1.71:** Even with the setting present, certain commands still trigger prompts (GitHub #32466).
4. **Blocks `~/.claude/` writes (v2.1.77+):** A new permission gate blocks writes to `~/.claude/` paths even with `--dangerously-skip-permissions`, breaking skill memory writes (GitHub #35718).
5. **Complete bypass regression (v2.1.77+):** Bypass permissions is "broken in all Claude Code versions newer than v2.1.77" (GitHub #36168).

**Why it matters for v2.1:**
If v2.1 drops `--dangerously-skip-permissions` and relies on `permissions.allow` + `defaultMode: "bypassPermissions"`, the agent still gets the bypass warning dialog unless `skipDangerousModePermissionPrompt` is set correctly. And given the regressions, even that may not work on the user's CC version. The whole bypass permissions mechanism is unstable.

**Prevention:**
- Do NOT rely on `bypassPermissions` mode OR `--dangerously-skip-permissions` for v2.1
- Instead, use `permissions.allow` with explicit tool patterns + `defaultMode: "default"` or `"acceptEdits"` + sandbox
- Test against the user's actual CC version at `rightclaw up` time
- Add `rightclaw doctor` check: verify CC version is compatible with the permission strategy

**Detection:**
- Agent hangs on launch with "WARNING: Claude Code running in Bypass Permissions mode" dialog
- Agent runs but prompts for every Bash command (fell back to default mode)
- Skill writes to `.claude/` fail with permission denied

**Phase to address:** Phase 1 (permission strategy). This is the core design decision for v2.1 -- the approach to replace `--dangerously-skip-permissions`.

**Confidence:** HIGH -- multiple CC GitHub issues confirm the instability, blog post documents the VS Code regression

---

### Pitfall 3: `permissions.allow` Cannot Fully Replace `--dangerously-skip-permissions`

**What goes wrong:**
The v2.1 plan is to replace `--dangerously-skip-permissions` with explicit `permissions.allow` rules in `settings.json`. But there is a gap:

1. **`.claude/` directory writes still prompt:** Even in `bypassPermissions` mode, writes to `.git`, `.claude`, `.vscode`, and `.idea` directories trigger confirmation prompts. Exception: `.claude/commands`, `.claude/agents`, and `.claude/skills` are exempt. But `.claude/settings.json`, `.claude/CLAUDE.md`, memory files, and other `.claude/` paths still prompt. This means skill memory writes, installed.json updates, and any agent self-modification trigger interactive prompts that block headless operation.

2. **Permission pattern fragility:** Bash permission patterns like `Bash(curl *)` are fragile -- options before the URL, different protocols, redirects, variables, and extra spaces all defeat the pattern. The docs explicitly warn about this.

3. **Read/Edit deny rules don't apply to Bash:** A `Read(./.env)` deny rule blocks the Read tool but does NOT prevent `cat .env` in Bash. Sandbox enforcement is needed for defense-in-depth.

4. **`dontAsk` mode is SDK-only:** The `dontAsk` mode (auto-deny unapproved tools) is only available in the TypeScript SDK, not the CLI.

**Why it matters:**
If the agent needs to write to its own `.claude/` directory (skill installs, memory, etc.), `permissions.allow` alone cannot prevent the interactive prompt. The sandbox provides OS-level enforcement but doesn't suppress permission prompts for `.claude/` writes.

**Prevention:**
- Accept that some `.claude/` writes will still prompt in default mode
- Pre-create all `.claude/` files at `rightclaw up` time (installed.json, memory files, skill directories)
- Use `autoAllowBashIfSandboxed: true` with sandbox to auto-approve Bash commands within sandbox boundaries
- For `.claude/` writes that skills perform: add `sandbox.filesystem.allowWrite` for the agent's `.claude/` directory
- Consider keeping `--dangerously-skip-permissions` as a fallback with sandbox enforcement as the primary safety layer (this is the Trail of Bits recommended approach)

**Detection:**
- Agent hangs waiting for "authorize Claude to modify its config files" prompt
- Skill installs silently fail
- Agent modifies `.claude/` state but gets stuck on next attempt

**Phase to address:** Phase 1 (permission strategy). Fundamental design question: do we fully drop bypass mode or keep it with sandbox as the safety layer?

**Confidence:** HIGH -- official CC docs explicitly document the `.claude/` write protection in bypass mode

---

### Pitfall 4: Workspace Trust Requires `.git` Directory in Agent Dir

**What goes wrong:**
Claude Code's workspace trust check has a subtle dependency: it only shows the "Do you trust the files in this folder?" dialog in directories WITHOUT a `.git` directory. If the directory IS a git repo, the trust prompt does not appear. This is confirmed behavior (GitHub #28506).

With per-agent HOME override, `HOME=<agent_dir>` means `<agent_dir>` IS the working directory. If `<agent_dir>` has no `.git/`, the trust dialog appears on every launch. Even with `hasTrustDialogAccepted: true` in `.claude.json`, some CC versions still show the prompt (GitHub #9113, #12227).

Additionally, there's `hasTrustDialogHooksAccepted` -- a separate flag that can't even be set via `claude config set` (GitHub #5572). Without it, hooks are skipped with "workspace trust not accepted" debug message.

Some evidence suggests trust state is stored server-side (tied to account + workspace path), not just locally. If true, local config changes are futile for some trust prompts.

**Why it happens:**
CC's trust mechanism evolved organically -- trust dialog, hooks trust dialog, per-project tool allowlists, server-side state -- all with different persistence and different bypass behaviors.

**Prevention:**
- Ensure every agent dir is a git repo: `git init <agent_dir>` at `rightclaw init` or `rightclaw up` time
- Write BOTH `hasTrustDialogAccepted: true` AND `hasTrustDialogHooksAccepted: true` to `<agent_dir>/.claude.json`
- Set these via `claude config set` from within the agent dir (if CC supports it under HOME override)
- Test: verify agent starts without any interactive prompt from a fresh state

**Detection:**
- Agent hangs immediately on first launch with "Quick safety check" prompt
- Works after manual acceptance but blocks again after clean state
- Debug logs show "workspace trust not accepted"

**Phase to address:** Phase 1 (HOME isolation). Must be solved before any agent can launch headlessly.

**Confidence:** HIGH -- confirmed by CC issues #28506, #9113, #5572; the `.git` dependency is documented in issue discussions

---

### Pitfall 5: `allowManagedDomainsOnly` Requires Managed Settings Scope -- Cannot Be Set Per-Agent

**What goes wrong:**
The v2.1 plan uses `allowManagedDomainsOnly: true` for silent domain blocking (no prompts for unapproved domains). But this setting is a "managed-only" setting -- it can ONLY be set in `/etc/claude-code/managed-settings.json` (Linux) or `/Library/Application Support/ClaudeCode/managed-settings.json` (macOS). It CANNOT be set in user settings (`~/.claude/settings.json`) or project settings (`.claude/settings.json`).

This means:
1. It applies to ALL Claude Code instances on the machine, not just RightClaw agents
2. It requires `sudo` to write to `/etc/claude-code/`
3. It blocks domains for the user's regular Claude Code usage too
4. It cannot be customized per-agent

Without `allowManagedDomainsOnly`, non-allowed domains trigger an interactive prompt ("Allow this domain?") instead of being silently blocked. This hangs headless agents.

**Why it happens:**
Managed settings are designed for enterprise IT departments deploying organization-wide policies, not for per-agent configuration. The managed-only restriction is intentional to prevent users from accidentally locking themselves out.

**Prevention:**
- Use `sandbox.network.allowedDomains` in per-agent settings (this IS settable per-agent) + `sandbox.enabled: true` + `allowUnsandboxedCommands: false`
- With sandbox enabled and `autoAllowBashIfSandboxed: true`, network access to allowed domains works without prompts
- For domains NOT in the allow list: the sandbox blocks them at the OS level (no prompt), but WebFetch tool still prompts. To handle WebFetch: add `WebFetch(domain:api.anthropic.com)` etc. to `permissions.allow`
- If the user is willing to install managed settings system-wide, provide `rightclaw config managed-settings install` helper
- Alternatively: file a feature request for `allowManagedDomainsOnly` to work at user/project scope

**Detection:**
- Agent hangs with "Allow access to domain X?" prompt
- Agent reports "network access blocked" for domains not in allow list
- Setting `allowManagedDomainsOnly` in `settings.json` has no effect

**Phase to address:** Phase 2 (network isolation). Not a startup blocker if sandbox is used, but blocks full headless operation for non-sandboxed network tools.

**Confidence:** HIGH -- official CC docs explicitly list `allowManagedDomainsOnly` as managed-only

---

## Moderate Pitfalls

### Pitfall 6: `~/` Path Resolution Under HOME Override Produces Wrong Paths

**What goes wrong:**
CC resolves `~` in `sandbox.filesystem.allowWrite`, `denyRead`, etc. to `$HOME`. With `HOME=<agent_dir>`, `~/.ssh` resolves to `<agent_dir>/.ssh`, not the real user's `~/.ssh`. The existing v2.0 code generates:

```json
"denyRead": ["~/.ssh", "~/.aws", "~/.gnupg"]
```

With HOME override, this denies read access to `<agent_dir>/.ssh` (which doesn't exist) and ALLOWS read access to the real `~/.ssh` (which contains private keys). The security intent is completely inverted.

**Prevention:**
- Already identified in v2.0 PITFALLS.md but NOT yet implemented
- Replace ALL `~/` paths in generated `settings.json` with absolute paths expanded at generation time
- In `generate_settings()`: resolve `dirs::home_dir()` and use absolute paths like `/home/wb/.ssh`
- For `allowWrite` of the agent dir: already uses `agent.path.display()` (absolute) -- this is correct
- For `denyRead`: use `format!("{}", real_home.join(".ssh").display())`

**Detection:**
- Agent can read real `~/.ssh` despite `denyRead: ["~/.ssh"]`
- Agent is blocked from its own `<agent_dir>/.ssh` (which doesn't exist)
- Security audit reveals sandbox allows access to unintended paths

**Phase to address:** Phase 1 (settings generation update). Security-critical.

**Confidence:** HIGH -- this is simple path resolution logic documented in CC sandbox docs

---

### Pitfall 7: OAuth Credential Symlink/Copy Creates Security and Race Condition

**What goes wrong:**
With `HOME=<agent_dir>`, CC on Linux looks for `.claude/.credentials.json` at `<agent_dir>/.claude/.credentials.json`. If using OAuth (not API key), the credentials file doesn't exist in the agent dir. Common "fix" attempts:

1. **Symlink:** `<agent_dir>/.claude/.credentials.json -> ~/.claude/.credentials.json` -- all agents share one file. OAuth token refresh from any agent corrupts the shared file for others (same non-atomic write problem as `.claude.json`).
2. **Copy:** Copy `.credentials.json` into each agent dir at `rightclaw up`. Tokens expire and need refresh. If one agent refreshes, other agents still have stale tokens.
3. **`CLAUDE_CONFIG_DIR`:** Unclear behavior (GitHub #3833, #25762). May or may not split credential resolution from HOME resolution. Not officially supported for this use case.

**Prevention:**
- **API keys are the only reliable option.** Use `ANTHROPIC_API_KEY` env var per agent. API keys don't expire, don't need refresh, and don't depend on HOME.
- If API key is not possible: use `apiKeyHelper` in `settings.json` -- a script that outputs a valid key/token. The script runs outside the sandbox, has access to the real HOME, and returns a fresh token.
- Document clearly: "RightClaw requires `ANTHROPIC_API_KEY` or `apiKeyHelper` -- OAuth is not supported with HOME isolation"

**Detection:**
- Agent errors with "Please run /login" or "ANTHROPIC_API_KEY not set"
- Works on macOS (Keychain doesn't depend on HOME) but fails on Linux
- Multiple agents with symlinked credentials fail intermittently

**Phase to address:** Phase 1 (HOME isolation). Authentication is a hard blocker.

**Confidence:** HIGH -- confirmed by CC docs on credential locations and `.claude.json` race condition reports

---

### Pitfall 8: CC Version Skew Breaks Permission Strategy

**What goes wrong:**
Claude Code's permission system is actively evolving with regressions across versions:
- v2.1.42: `skipDangerousModePermissionPrompt` regression in VS Code
- v2.1.71: `--dangerously-skip-permissions` still prompts for some commands
- v2.1.77: Bypass permissions fully broken
- v2.1.79: Edit tool prompts despite bypassPermissions

RightClaw generates `settings.json` with permission rules, but the user's CC version may not honor them correctly. A settings key that works in CC v2.1.60 may be broken in v2.1.77 and fixed in v2.1.80.

**Prevention:**
- Add CC version detection to `rightclaw doctor` and `rightclaw up`
- Parse `claude --version` output
- Maintain a compatibility matrix: which RightClaw version works with which CC version range
- Pin CC version recommendation in docs and doctor output
- Consider CC `autoUpdatesChannel: "stable"` in generated settings (uses ~1-week-old versions, skips regression releases)

**Detection:**
- Agent works on developer's machine, fails on user's machine with different CC version
- Permissions worked yesterday, broken after CC auto-update
- Different agents behave differently despite identical settings

**Phase to address:** Phase 2 (doctor improvements). Not a startup blocker but prevents support headaches.

**Confidence:** MEDIUM -- based on pattern of CC regressions in issue tracker; specific version numbers may shift

---

### Pitfall 9: Managed Settings Conflict With Per-Agent Settings

**What goes wrong:**
If the user (or their organization) has `/etc/claude-code/managed-settings.json` deployed, it takes precedence over ALL other settings including RightClaw's generated per-agent `settings.json`. This can:

1. **Override permission rules:** If managed settings set `allowManagedPermissionRulesOnly: true`, RightClaw's per-agent `permissions.allow` rules are IGNORED. Only managed rules apply.
2. **Override sandbox config:** If managed settings set `allowManagedReadPathsOnly: true`, per-agent `allowRead` entries are ignored.
3. **Block sandbox domains:** If managed settings don't include domains the agent needs (e.g., `api.telegram.org`), those domains are blocked with no override possible.
4. **Disable bypass mode:** If managed settings set `disableBypassPermissionsMode: "disable"`, agents cannot use `--dangerously-skip-permissions` at all.

The user won't know why their agents are broken because the managed settings are invisible to them (deployed by IT).

**Prevention:**
- Add `rightclaw doctor` check: detect presence of `/etc/claude-code/managed-settings.json` and warn about potential conflicts
- Parse the managed settings file (if readable) and report conflicts with generated per-agent settings
- Document: "If your organization uses managed settings, verify they don't conflict with RightClaw's generated settings"
- Support `rightclaw up --show-effective-settings` to dump the merged settings CC will actually use (run `claude --print-config` or `/status`)

**Detection:**
- Agent ignores permission rules set in `.claude/settings.json`
- Sandbox configuration doesn't match what RightClaw generated
- `rightclaw doctor` shows settings.json is correct but agent behaves differently

**Phase to address:** Phase 2 (doctor + diagnostics). Edge case but devastating when hit.

**Confidence:** HIGH -- official CC docs document managed settings precedence

---

### Pitfall 10: Git/SSH Identity Loss Under HOME Override

**What goes wrong:**
Already identified in v2.0 PITFALLS.md but NOT yet implemented. With `HOME=<agent_dir>`:

- SSH reads keys from `$HOME/.ssh/` -- nonexistent under agent dir
- Git reads global config from `$HOME/.gitconfig` -- nonexistent under agent dir
- GPG reads keyring from `$HOME/.gnupg/` -- nonexistent under agent dir
- npm reads `$HOME/.npmrc` -- nonexistent
- Known hosts file at `$HOME/.ssh/known_hosts` missing -- SSH prompts "Are you sure you want to continue connecting?" (hangs headless)

**Prevention:**
- In shell wrapper, set env vars BEFORE `exec claude`:
  ```bash
  export GIT_CONFIG_GLOBAL="/home/wb/.gitconfig"
  export GIT_AUTHOR_NAME="..."
  export GIT_AUTHOR_EMAIL="..."
  export GIT_COMMITTER_NAME="..."
  export GIT_COMMITTER_EMAIL="..."
  export SSH_AUTH_SOCK="${SSH_AUTH_SOCK}"  # forward from parent
  export GIT_SSH_COMMAND="ssh -F /home/wb/.ssh/config -o UserKnownHostsFile=/home/wb/.ssh/known_hosts"
  ```
- Add `sandbox.network.allowUnixSockets` for SSH agent socket path
- For sandbox: add real `~/.ssh/` to `sandbox.filesystem.allowRead` (absolute path) so sandbox doesn't block SSH reads
- Do NOT symlink `~/.ssh/` into agent dir with write access -- security risk (agent/skill could modify SSH keys)

**Detection:**
- `git push` fails with "Permission denied (publickey)"
- Commits appear with wrong author identity
- SSH prompts "Are you sure you want to continue connecting?" (hangs headless)
- `git config --global user.name` returns empty inside agent

**Phase to address:** Phase 1 (shell wrapper enhancement). Blocks any git workflow.

**Confidence:** HIGH -- straightforward env var behavior, already identified in v2.0

---

### Pitfall 11: Telegram Plugin Path Resolution Under HOME Override

**What goes wrong:**
The Telegram plugin reads its bot token from `~/.claude/channels/telegram/.env` and access control from `~/.claude/channels/telegram/access.json`. With `HOME=<agent_dir>`, these resolve to `<agent_dir>/.claude/channels/telegram/` -- but `init_rightclaw_home()` writes the token to the real `~/.claude/channels/telegram/` by default (when `telegram_env_dir` is None).

Currently, `init_rightclaw_home()` has a `telegram_env_dir` parameter but it's only used in tests. The production code path in `cmd_init()` passes `None`, causing Telegram config to be written to the real HOME.

**Prevention:**
- When HOME override is active, `rightclaw up` must copy/generate Telegram config into `<agent_dir>/.claude/channels/telegram/`
- Modify `cmd_init()` to pass the agent dir as `telegram_env_dir` when the Telegram token is provided
- At `rightclaw up` time: if Telegram is configured but `.env` is in real HOME, copy it to agent dir
- Alternatively: generate Telegram env at `rightclaw up` time (not just `init`), ensuring it goes to the right place

**Detection:**
- Telegram channel connected in `rightclaw pair` mode but silent under `rightclaw up` with HOME override
- Bot token "not found" errors in Claude debug logs
- Plugin loads but ignores messages (missing `access.json`)

**Phase to address:** Phase 2 (Telegram integration update). Not a startup blocker but blocks Telegram functionality.

**Confidence:** HIGH -- codebase audit confirms the path issue in `init.rs:120`

---

## Minor Pitfalls

### Pitfall 12: Process-Compose Environment Section Leaks Secrets

**What goes wrong:**
If API keys or tokens are set in the process-compose YAML `environment:` section, they become visible via `process-compose process list` and the REST API. The TUI also shows env vars for each process.

**Prevention:**
- Set secrets in the shell wrapper (`export ANTHROPIC_API_KEY=...`) or use CC's `apiKeyHelper` in settings.json
- Never put `ANTHROPIC_API_KEY` in process-compose YAML
- Consider reading API key from a file at wrapper startup: `export ANTHROPIC_API_KEY=$(cat /path/to/key)`

**Phase to address:** Phase 1 (wrapper generation). Security concern.

**Confidence:** HIGH -- process-compose REST API exposes env vars by design

---

### Pitfall 13: Sandbox Proxy Socket Conflicts With Custom Ports

**What goes wrong:**
CC's sandbox creates proxy sockets (HTTP/SOCKS) to enforce network isolation. If `sandbox.network.httpProxyPort` or `socksProxyPort` is set in settings and multiple agents use the same port, only the first agent can bind.

By default, CC auto-assigns ports, which should avoid conflicts. But if RightClaw sets explicit proxy ports (or the user does via agent.yaml overrides), conflicts arise.

**Prevention:**
- Do NOT set `httpProxyPort` or `socksProxyPort` in generated settings -- let CC auto-assign
- If overrides exist in agent.yaml, validate uniqueness across all agents at `rightclaw up` time
- Add a validation step: if any `SandboxOverrides` includes proxy ports, ensure no duplicates

**Phase to address:** Phase 2 (multi-agent validation). Low probability but easy to prevent.

**Confidence:** MEDIUM -- inferred from CC docs; proxy auto-assignment likely works but not explicitly confirmed for multi-instance

---

### Pitfall 14: `autoMemoryDirectory` Setting Redirects Memory Writes

**What goes wrong:**
CC has an `autoMemoryDirectory` setting that controls where auto-memory files are stored. If this setting exists in user settings (`~/.claude/settings.json`), it applies to all projects. With HOME override, user settings are at `<agent_dir>/.claude/settings.json` (which is also the project settings). If `autoMemoryDirectory` is set to a path with `~/`, it resolves under the agent HOME. If set to an absolute path, all agents may write memory to the same directory (race condition).

**Prevention:**
- Do NOT set `autoMemoryDirectory` in generated settings
- If agents need separate memory: their agent dirs already isolate `.claude/` directories
- Document: do not set `autoMemoryDirectory` in agent.yaml overrides

**Phase to address:** Phase 3 (documentation). Low priority.

**Confidence:** MEDIUM -- theoretical based on docs; CC notes this setting is "not accepted in project settings" to prevent repo-controlled redirection, but user settings behavior under HOME override is untested

---

## Technical Debt Patterns

Shortcuts that seem reasonable but create problems.

| Shortcut | Immediate Benefit | Long-term Cost | When Acceptable |
|----------|-------------------|----------------|-----------------|
| Keep `--dangerously-skip-permissions` with sandbox | Works immediately, no permission gaps | Bypass warning dialog on every version, CC regressions break it regularly, Trail of Bits security concern (users copy the pattern without sandbox) | Only if sandbox enforcement is verified before launch |
| Use `defaultMode: "bypassPermissions"` in settings.json instead of CLI flag | No CLI flag needed | Same regressions as CLI flag, VS Code fallback bug, `.claude/` writes still prompt | Only in sandboxed+containerized environments |
| Symlink real `~/.claude/` into agent dir | Quick auth fix | Defeats entire isolation purpose, shared state = race conditions, security leak | Never for production |
| Skip `.git` init in agent dir | Simpler init | Trust dialog blocks every headless launch | Never |
| Use `CLAUDE_CONFIG_DIR` instead of HOME override | More targeted, doesn't affect SSH/Git | Undocumented, unclear behavior (CC issues #3833, #25762), may not work | Only after verification with specific CC version |
| Set managed settings at `/etc/claude-code/` | `allowManagedDomainsOnly` works | Affects ALL CC instances on machine, requires sudo | Only if user understands system-wide impact |
| Use `permissions.allow: ["Bash"]` to auto-allow all Bash | No need for bypass mode | Doesn't auto-allow Edit/Write/WebFetch, need separate rules for each tool | Only with additional tool-specific allow rules |

## Integration Gotchas

| Integration | Common Mistake | Correct Approach |
|-------------|----------------|------------------|
| CC permission modes | Using `bypassPermissions` and expecting no prompts | Use sandbox + `autoAllowBashIfSandboxed` + explicit `permissions.allow` for tools |
| CC managed settings | Setting `allowManagedDomainsOnly` in per-agent settings | Must be in `/etc/claude-code/managed-settings.json` (or use sandbox domains instead) |
| CC workspace trust | Writing trust to real `~/.claude.json` | Write to `<agent_dir>/.claude.json` AND ensure `.git/` exists in agent dir |
| CC credentials | Assuming OAuth works with HOME override | Use `ANTHROPIC_API_KEY` env var or `apiKeyHelper` in settings.json |
| CC `.claude/` writes | Expecting `permissions.allow` to suppress all prompts | Pre-create all `.claude/` files at `rightclaw up` time |
| CC version compat | Assuming current CC version behavior is stable | Pin CC version recommendation, add version check to doctor |
| Sandbox path resolution | Using `~/.ssh` in denyRead with HOME override | Use absolute paths: `/home/user/.ssh` |
| Shell wrapper secrets | Putting API key in process-compose YAML env section | Set in wrapper script or use `apiKeyHelper` |
| SSH under sandbox | Expecting SSH to work without explicit socket allowance | Add `sandbox.network.allowUnixSockets` for SSH agent socket |
| Git under HOME override | Expecting `~/.gitconfig` to be found | Set `GIT_CONFIG_GLOBAL` env var in wrapper |

## Security Mistakes

| Mistake | Risk | Prevention |
|---------|------|------------|
| Using `bypassPermissions` without sandbox on bare metal | Agent has unrestricted access to entire filesystem and network | Always pair with sandbox; Anthropic engineers only use bypass mode inside containers |
| Symlinking `~/.ssh/` with write access into agent dir | Agent/malicious skill can modify SSH keys, add authorized_keys entries | Read-only access only via `sandbox.filesystem.allowRead` with absolute path |
| Setting `enableWeakerNestedSandbox` on bare metal | Disables user namespace isolation, substantially weakens sandbox | Only when running inside Docker/container with additional isolation |
| Allowing `sandbox.filesystem.allowWrite` to HOME or PATH directories | Agent can modify shell config (`.bashrc`), plant executables | Never allow write to PATH dirs or shell configs |
| Putting `ANTHROPIC_API_KEY` in generated files on disk | Key visible in wrapper scripts if file permissions are lax | Use `apiKeyHelper` script or read key from secure storage at runtime; set wrapper permissions to 700 |
| Not denying read access to credential files | Agent can read SSH keys, AWS credentials, GPG keyring | Explicit `sandbox.filesystem.denyRead` with absolute paths for sensitive directories |
| Setting `allowAllUnixSockets: true` | Exposes Docker socket, D-Bus, and other system sockets | Explicitly list only needed sockets (SSH agent) |
| Sharing `~/.claude.json` across agents | Race condition corrupts auth, trust, and all CC state | Per-agent HOME isolation |

## UX Pitfalls

| Pitfall | User Impact | Better Approach |
|---------|-------------|-----------------|
| Silent fallback to default mode (CC VS Code regression) | Agent appears to work but prompts for everything | Add startup self-test: verify permission mode is active |
| Managed settings silently overriding per-agent config | User's agent config has no effect, no indication why | `rightclaw doctor` detects managed settings and warns |
| CC version auto-update breaking permissions | Agent worked yesterday, broken today | Recommend `autoUpdatesChannel: "stable"` in generated settings |
| SSH "Are you sure?" prompt hanging headless agent | Agent blocks indefinitely on first SSH connection | Pre-populate `known_hosts` or set `StrictHostKeyChecking=accept-new` in wrapper |
| Trust dialog re-appearing after CC update | Agent stops working after CC update | Write trust state AND ensure `.git/` exists AND set `hasTrustDialogHooksAccepted` |
| Opaque "Permission denied" errors from sandbox | User doesn't know which sandbox rule blocked | Add `rightclaw doctor --agent <name>` to validate effective settings |

## "Looks Done But Isn't" Checklist

- [ ] **Permissions strategy:** Often assumes `permissions.allow: ["Bash"]` covers everything -- verify Edit, Write, WebFetch, MCP tools are also allowed
- [ ] **HOME override:** Often missing `.git/` directory in agent dir -- verify trust dialog doesn't appear
- [ ] **HOME override:** Often missing `hasTrustDialogHooksAccepted` -- verify hooks fire on SessionStart
- [ ] **Credentials:** Often assumes OAuth works -- verify `ANTHROPIC_API_KEY` or `apiKeyHelper` is configured
- [ ] **Path resolution:** Often uses `~/` in sandbox settings -- verify all paths are absolute under HOME override
- [ ] **Git identity:** Often missing `GIT_COMMITTER_*` -- verify both author and committer are set
- [ ] **SSH known_hosts:** Often forgotten -- verify SSH doesn't prompt on first connection
- [ ] **Managed settings:** Often ignored -- verify `/etc/claude-code/managed-settings.json` doesn't conflict
- [ ] **Multi-agent proxy:** Often untested -- verify 3+ agents can all access network simultaneously
- [ ] **Telegram path:** Often written to real HOME -- verify bot responds under overridden HOME
- [ ] **API key security:** Often visible in process-compose or wrapper -- verify key is not in logs or REST API

## Recovery Strategies

| Pitfall | Recovery Cost | Recovery Steps |
|---------|---------------|----------------|
| `.claude.json` corruption | LOW | Restore from backup (CC keeps 5 timestamped backups), or regenerate with `claude config set` |
| Trust dialog blocks agent | LOW | Create `.git/` in agent dir, write trust state to `<agent_dir>/.claude.json`, restart |
| Permission mode regression | MEDIUM | Downgrade CC to last known working version, or switch permission strategy |
| OAuth credentials not found | LOW | Set `ANTHROPIC_API_KEY` env var, restart |
| Sandbox paths wrong | LOW | Regenerate `settings.json` with absolute paths via `rightclaw up` |
| SSH/Git identity lost | LOW | Set env vars in wrapper, restart |
| Managed settings conflict | MEDIUM | Contact IT to understand managed settings, adjust RightClaw config to work within constraints |
| Telegram in wrong HOME | LOW | Copy `.env` and `access.json` to agent's `.claude/channels/telegram/`, restart |
| CC version incompatibility | MEDIUM | Pin CC version: `npm install -g @anthropic-ai/claude-code@<version>` |

## Pitfall-to-Phase Mapping

| Pitfall | Prevention Phase | Verification |
|---------|------------------|--------------|
| #1 `.claude.json` race condition | Phase 1: HOME isolation | Each agent has isolated `.claude.json`, no shared state |
| #2 `skipDangerousModePermissionPrompt` broken | Phase 1: permission strategy | Agent starts without bypass warning dialog |
| #3 `permissions.allow` gaps | Phase 1: permission strategy | Agent runs headlessly with all needed tool access |
| #4 Workspace trust + `.git` | Phase 1: HOME isolation | Agent starts without trust dialog from fresh state |
| #5 `allowManagedDomainsOnly` scope | Phase 2: network isolation | Non-allowed domains blocked silently without prompt |
| #6 `~/` path resolution | Phase 1: settings generation | denyRead paths resolve to real user directories |
| #7 OAuth credentials | Phase 1: authentication | Agent authenticates with API key under HOME override |
| #8 CC version skew | Phase 2: doctor | `rightclaw doctor` reports CC version compatibility |
| #9 Managed settings conflict | Phase 2: doctor | `rightclaw doctor` detects and warns about managed settings |
| #10 Git/SSH identity | Phase 1: wrapper enhancement | `git push` works from inside agent with HOME override |
| #11 Telegram paths | Phase 2: Telegram update | Bot responds under overridden HOME |
| #12 Secret leakage | Phase 1: wrapper + security | API key not visible in PC REST API or logs |
| #13 Proxy port conflicts | Phase 2: multi-agent validation | 3+ agents run with network access concurrently |
| #14 autoMemoryDirectory | Phase 3: documentation | Documented as unsupported with per-agent HOME |

## Sources

### Official Documentation
- [Claude Code Settings Reference](https://code.claude.com/docs/en/settings) -- complete settings hierarchy, managed settings locations, merge behavior
- [Claude Code Permissions Docs](https://code.claude.com/docs/en/permissions) -- permissions.allow syntax, defaultMode options, managed-only settings list
- [Claude Code Sandboxing Docs](https://code.claude.com/docs/en/sandboxing) -- allowManagedDomainsOnly, sandbox path resolution, security limitations

### Claude Code Issue Tracker
- [#28922: .claude.json race condition reported 8 times](https://github.com/anthropics/claude-code/issues/28922) -- concurrent write corruption
- [#18998: Severe .claude.json corruption with 30+ sessions](https://github.com/anthropics/claude-code/issues/18998) -- 14 corruptions in 11 hours
- [#25503: --dangerously-skip-permissions should bypass dialog](https://github.com/anthropics/claude-code/issues/25503) -- skipDangerousModePermissionPrompt regression
- [#35718: bypass mode doesn't bypass ~/.claude/ writes](https://github.com/anthropics/claude-code/issues/35718) -- skill memory blocked
- [#36168: bypass permissions broken in v2.1.77+](https://github.com/anthropics/claude-code/issues/36168) -- complete regression
- [#32466: bypass mode still prompts for Bash commands](https://github.com/anthropics/claude-code/issues/32466) -- v2.1.71 regression
- [#28506: bypass doesn't bypass workspace trust](https://github.com/anthropics/claude-code/issues/28506) -- .git dependency discovered
- [#9113: workspace trust not respecting pre-config](https://github.com/anthropics/claude-code/issues/9113) -- trust dialog bug
- [#5572: hasTrustDialogHooksAccepted can't be set via config](https://github.com/anthropics/claude-code/issues/5572)
- [#26233: skipDangerousModePermissionPrompt undocumented](https://github.com/anthropics/claude-code/issues/26233)
- [#3833: CLAUDE_CONFIG_DIR behavior unclear](https://github.com/anthropics/claude-code/issues/3833)
- [#29026: Desktop app ignores settings.json permissions](https://github.com/anthropics/claude-code/issues/29026)
- [#18160: Bash permission patterns not matching](https://github.com/anthropics/claude-code/issues/18160)
- [CVE-2026-33068: Workspace trust dialog bypass via repo settings](https://github.com/anthropics/claude-code/security/advisories/GHSA-mmgp-wc2j-qcv7)

### Blog Posts and Analysis
- [Debugging Claude Code's Bypass Permissions Regression](https://www.testinginproduction.co/blog/debugging-claude-code-bypass-permissions) -- VS Code silent fallback analysis
- [Trail of Bits claude-code-config](https://github.com/trailofbits/claude-code-config) -- opinionated sandbox config reference
- [managed-settings.com](https://managed-settings.com/) -- managed settings configuration guide

### Existing Codebase
- `init.rs` (`pre_trust_directory()`) -- writes to real `~/.claude.json`, needs HOME-aware update
- `codegen/settings.rs` (`generate_settings()`) -- uses `~/.ssh` in denyRead, needs absolute paths
- `templates/agent-wrapper.sh.j2` -- hardcodes `--dangerously-skip-permissions`, needs replacement
- `agent/types.rs` (`SandboxOverrides`) -- may need new fields for permission rules
- `main.rs` (`cmd_up`) -- generates settings per-agent, needs HOME override and trust setup

---
*Pitfalls research for: Headless agent isolation -- HOME override + managed settings + dropping bypass mode (v2.1)*
*Researched: 2026-03-24*
