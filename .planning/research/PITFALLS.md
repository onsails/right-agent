# Pitfalls Research

**Domain:** Replacing OpenShell sandbox with CC native sandbox + per-agent HOME isolation
**Researched:** 2026-03-23
**Confidence:** HIGH (official docs verified, codebase audited, known issues confirmed)

## Critical Pitfalls

### Pitfall 1: `pre_trust_directory()` Writes to Real HOME, Not Agent HOME

**What goes wrong:**
The existing `init.rs::pre_trust_directory()` writes trust state to `~/.claude.json` and `~/.claude/settings.json` using `dirs::home_dir()`. In v2.0, each agent's `$HOME` is `~/.rightclaw/agents/<name>/`. But Claude Code reads trust state from the `$HOME` it sees at runtime. If `$HOME` points to the agent dir, CC will look for `.claude.json` at `~/.rightclaw/agents/right/.claude.json` -- but `pre_trust_directory()` wrote it to the real `~/.claude.json`. The agent hits the workspace trust dialog and blocks, waiting for interactive input that never comes.

**Why it happens:**
The v1.0 code uses `dirs::home_dir()` (which reads the real HOME from `/etc/passwd`) to locate CC config files. In OpenShell, the sandbox ran with the real HOME, so this worked. With per-agent HOME override, the runtime HOME and the init-time HOME diverge.

**How to avoid:**
- `pre_trust_directory()` must write `.claude.json` relative to the agent's directory (the agent's runtime HOME), not the real user HOME.
- Write `hasTrustDialogAccepted: true` into `<agent_dir>/.claude.json`, since that is where CC will look when `HOME=<agent_dir>`.
- The `skipDangerousModePermissionPrompt` in `<agent_dir>/.claude/settings.json` already exists -- verify CC reads it from the overridden HOME.
- Test: launch `HOME=/tmp/test-agent claude --version` and check which `.claude.json` it reads.

**Warning signs:**
- Agent hangs immediately on launch with "Quick safety check" prompt.
- `hasTrustDialogAccepted` is set in the real `~/.claude.json` but agent still prompts.
- Works fine with `--no-sandbox` (because HOME is not overridden).

**Phase to address:** Phase 1 (HOME isolation implementation). This is the first thing that will break.

---

### Pitfall 2: OAuth Credentials Inaccessible Under Overridden HOME

**What goes wrong:**
Claude Code stores OAuth credentials in the macOS Keychain (keyed to the app, not to HOME) or in `~/.claude/.credentials.json` on Linux. When HOME is overridden to the agent dir, CC on Linux looks for credentials at `<agent_dir>/.claude/.credentials.json` -- which does not exist. The agent cannot authenticate.

On macOS, Keychain access works regardless of HOME (it is keyed by app bundle identifier). But on Linux, credential discovery is HOME-dependent.

**Why it happens:**
CC's credential resolution on Linux uses `$HOME/.claude/.credentials.json`. The v1.0 OpenShell approach ran with real HOME and allowed read access to `~/.claude` in the policy. With HOME override, the credentials file simply is not at the expected path.

**How to avoid:**
- **API keys (recommended):** Pass `ANTHROPIC_API_KEY` env var per agent. API keys are not HOME-dependent. This also solves the OAuth token race condition from v1.0 (Pitfall 3 in old research).
- **Credential symlink:** Symlink `<agent_dir>/.claude/.credentials.json` -> `~/.claude/.credentials.json`. But this re-introduces the multi-agent OAuth race condition.
- **CLAUDE_CONFIG_DIR:** Set `CLAUDE_CONFIG_DIR` env var to point at the real `~/.claude/` for credential resolution while using `HOME` override for agent isolation. Verify this actually works -- GitHub issue #3833 reports unclear behavior.
- **Test:** Which files does CC actually read from `$HOME/.claude/` vs `$CLAUDE_CONFIG_DIR`?

**Warning signs:**
- Agent starts but immediately errors with "ANTHROPIC_API_KEY not set" or "Please run /login."
- Works on macOS (Keychain) but fails on Linux.
- Works with `--no-sandbox` (real HOME used).

**Phase to address:** Phase 1 (HOME isolation). Authentication is a hard blocker -- nothing works without it.

---

### Pitfall 3: Bubblewrap Fails on Ubuntu 24.04+ Due to AppArmor User Namespace Restrictions

**What goes wrong:**
Ubuntu 24.04 LTS ships with `kernel.apparmor_restrict_unprivileged_userns=1` by default. Bubblewrap uses `--unshare-net` to create an isolated network namespace, which requires creating a user namespace first. AppArmor blocks this unless `bwrap` has an explicit AppArmor profile granting the `userns` permission. The sandbox fails with: `bwrap: loopback: Failed RTM_NEWADDR: Operation not permitted`.

This is confirmed as an open issue on Anthropic's own sandbox-runtime repo (issue #74).

**Why it happens:**
Ubuntu's security team restricted unprivileged user namespaces to reduce kernel attack surface (user namespaces have been a rich source of privilege escalation bugs). The restriction is per-application via AppArmor profiles. `bwrap` is deliberately not given a permissive profile because it could be used by any malicious program to obtain capabilities.

Ubuntu 25.04+ further tightens this by enabling `apparmor_restrict_unprivileged_unconfined` by default, closing bypass routes via `aa-exec` and `busybox`.

**How to avoid:**
- `rightclaw doctor` must detect this condition: check `sysctl kernel.apparmor_restrict_unprivileged_userns` and test `bwrap --unshare-net echo ok`.
- Provide a fix script or doctor hint: create `/etc/apparmor.d/local-bwrap` with `userns` permission, then `sudo systemctl reload apparmor`.
- Document the fix prominently -- Ubuntu 24.04 LTS is the most common developer desktop.
- Consider whether `--share-net` (using the proxy without network namespace isolation) is acceptable as a fallback, since CC's own proxy-based filtering still provides domain-level restriction.
- On Debian, the older `kernel.unprivileged_userns_clone=1` sysctl is needed instead.

**Warning signs:**
- `rightclaw up` works on macOS/Fedora but fails on Ubuntu.
- `bwrap` errors mentioning "Operation not permitted" or "Permission denied."
- `rightclaw doctor` shows `bubblewrap` as installed but sandbox still fails.

**Phase to address:** Phase 1 (sandbox setup) and install script update. This is the most common Linux distro -- cannot ship without handling it.

---

### Pitfall 4: CC Sandbox Settings Resolution With Overridden HOME

**What goes wrong:**
CC's sandbox settings follow a multi-scope precedence: Managed > CLI > Local > Project > User. The "User" scope reads from `~/.claude/settings.json`. The "Project" scope reads from `<cwd>/.claude/settings.json`. With HOME override:

1. User-level sandbox settings (`~/.claude/settings.json`) resolve to `<agent_dir>/.claude/settings.json` -- which is the SAME file as the project-level settings (since cwd = agent dir = HOME).
2. This means user-scope and project-scope collapse into a single file. Settings that should only apply globally now apply only to the agent, and vice versa.
3. Managed settings at `/etc/claude-code/managed-settings.json` (Linux) still work correctly since they use absolute paths.

The `~` prefix in `sandbox.filesystem.allowWrite` resolves to `$HOME`, so `~/.kube` becomes `<agent_dir>/.kube`, not the real user's `~/.kube`. If the agent needs to access paths relative to the real user home (SSH keys, git config), the tilde-expanded paths will be wrong.

**Why it happens:**
CC was designed for single-user, single-instance use where HOME, cwd, and user identity are all consistent. Per-agent HOME isolation breaks this assumption.

**How to avoid:**
- Use absolute paths in all sandbox settings, never `~/` prefix. Generate `settings.json` with fully expanded paths.
- Accept that user-scope and project-scope collapse. This is actually fine for per-agent isolation -- each agent IS a "user" with its own settings. Just do not rely on the distinction.
- For paths that must reference the real user HOME (e.g., SSH keys), use absolute paths in `sandbox.filesystem.allowRead` and `sandbox.filesystem.allowWrite`.
- Test: generate a `settings.json` with `~/some-path` and verify what CC resolves it to under overridden HOME.

**Warning signs:**
- Sandbox allows access to wrong directories (agent dir instead of real user home).
- `sandbox.filesystem.denyRead: ["~/"]` blocks the agent's own directory instead of the user's home.
- Settings that work with `--no-sandbox` break with sandbox enabled.

**Phase to address:** Phase 1 (settings generation). Affects every generated `settings.json`.

---

### Pitfall 5: SSH/Git Identity Lost Under Overridden HOME

**What goes wrong:**
SSH reads keys and config from `$HOME/.ssh/`. Git reads global config from `$HOME/.gitconfig`. When HOME is the agent directory, SSH cannot find keys and git loses the user's name/email/signing config. Agents that need to push to git repos or access private repos via SSH will fail silently or with cryptic auth errors.

**Why it happens:**
SSH and git both resolve `~` and `$HOME` to find their configuration. SSH is particularly strict: it checks file permissions on `~/.ssh/` and refuses to use keys if permissions are wrong. Even symlinks can trigger permission check failures on some SSH versions.

Additionally, `gpg` (for commit signing), `npm` (`~/.npmrc`), `pip` (`~/.config/pip/`), and many other tools use HOME-relative config paths.

**How to avoid:**
- Set `GIT_CONFIG_GLOBAL` env var to point at the real user's `~/.gitconfig`. This overrides HOME-based git config resolution.
- Set `SSH_AUTH_SOCK` to forward the user's SSH agent into the agent process. The SSH agent does not depend on HOME -- it uses a socket.
- For direct SSH key access (no agent), symlink or copy `~/.ssh/` into the agent dir with correct permissions (700 for dir, 600 for keys). But this leaks SSH keys into the agent's sandbox -- security tradeoff.
- Alternatively, set `GIT_SSH_COMMAND="ssh -F /real/home/.ssh/config -i /real/home/.ssh/id_ed25519"` to explicitly point SSH at the real key location.
- For git author identity, set `GIT_AUTHOR_NAME`, `GIT_AUTHOR_EMAIL`, `GIT_COMMITTER_NAME`, `GIT_COMMITTER_EMAIL` env vars in the process-compose environment.
- For sandbox filesystem access: add the real `~/.ssh/` to `sandbox.filesystem.allowRead` (read-only!) so the sandbox does not block SSH key reads.

**Warning signs:**
- `git push` fails with "Permission denied (publickey)."
- Commits appear with wrong author identity.
- `git config --global user.name` returns empty inside agent.
- SSH prompts "Are you sure you want to continue connecting?" (missing `known_hosts`).

**Phase to address:** Phase 2 (agent environment setup). Not a blocker for basic functionality but a blocker for any git workflow.

---

## Moderate Pitfalls

### Pitfall 6: Telegram Channel .env and Plugin State Under Overridden HOME

**What goes wrong:**
The Telegram plugin reads its bot token from `~/.claude/channels/telegram/.env` and access control from `~/.claude/channels/telegram/access.json`. With HOME override, these paths resolve to `<agent_dir>/.claude/channels/telegram/` -- but `init.rs` writes the token to the real user's `~/.claude/channels/telegram/.env` by default.

Additionally, the Telegram plugin requires feature flags from GrowthBook, and `CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC` (which blocks ALL feature flags including `tengu_harbor`) must not be set.

**How to avoid:**
- `init_rightclaw_home()` already has a `telegram_env_dir` parameter for overriding the .env path. Use it to write into the agent dir: `<agent_dir>/.claude/channels/telegram/`.
- Verify that the Telegram plugin reads `.env` relative to `$HOME` or relative to the Claude config dir.
- If Telegram state is HOME-relative, write all Telegram config into the agent dir during init.
- Test: launch agent with HOME override, verify Telegram plugin finds the bot token.

**Warning signs:**
- Telegram channel connected with `--no-sandbox` but silent under sandbox.
- Bot token "not found" errors in Claude debug logs.
- Plugin loads but ignores messages (missing `access.json`).

**Phase to address:** Phase 2 (Telegram integration update).

---

### Pitfall 7: macOS Seatbelt Deprecation and Undocumented Profile Language

**What goes wrong:**
macOS uses `sandbox-exec` (Seatbelt) for CC's sandbox enforcement. Apple deprecated `sandbox-exec` years ago. The man page says "DEPRECATED." The Sandbox Profile Language (SBPL) is undocumented -- a Scheme-like DSL with no official reference. macOS updates can change SBPL semantics or break existing profiles without warning.

**Why it happens:**
Apple wants developers to use the App Sandbox (requires `.app` bundles with entitlements), which is unsuitable for CLI tools. There is no documented, supported alternative to `sandbox-exec` for CLI sandboxing on macOS. Apple, Anthropic, OpenAI, and Google all use `sandbox-exec` despite the deprecation.

The upcoming macOS 26 "Containers" feature may eventually replace Seatbelt for CLI isolation, but details are sparse.

**How to avoid:**
- Accept the deprecation as "deprecated but not going anywhere." Apple's own system software uses Seatbelt internally.
- Do not write custom SBPL profiles. Rely on CC's built-in Seatbelt profile generation from `settings.json`. This way, Anthropic handles any SBPL changes.
- RightClaw only needs to generate the correct `settings.json` -- CC handles the Seatbelt translation.
- Pin to a tested CC version. When macOS updates ship, test sandbox functionality before upgrading.
- No `bubblewrap` or `socat` needed on macOS -- one fewer dependency to manage.

**Warning signs:**
- Sandbox works on macOS 15 but breaks after macOS update.
- `sandbox-exec` errors in system log.
- CC falls back to unsandboxed mode silently on macOS.

**Phase to address:** Phase 1 (cross-platform testing). Low risk for now -- just ensure awareness.

---

### Pitfall 8: Removing OpenShell Code Paths Without Breaking Existing Installs

**What goes wrong:**
v1.0 users have `policy.yaml` in every agent dir, `openshell` checks in doctor and deps, OpenShell-specific sandbox state in runtime JSON, and shell wrappers that call `openshell sandbox create`. A v2.0 upgrade must cleanly migrate without breaking running agents or leaving orphaned OpenShell sandboxes.

**Why it happens:**
Schema evolution. `RuntimeState` struct has `no_sandbox: bool` and `AgentState` has `sandbox_name: String` -- both OpenShell-specific. The `AgentDef` struct requires `policy_path` (OpenShell policy). Doctor checks for `openshell` binary. Shell wrapper template contains OpenShell conditional blocks.

**How to avoid:**
- **Migration path:** `rightclaw up` in v2.0 should detect v1.0 runtime state (presence of `sandbox_name` in state file) and run `openshell sandbox destroy` for any active sandboxes before starting with the new sandbox backend.
- **AgentDef evolution:** Make `policy_path` optional (no longer required). Add a new `settings_path` for the CC sandbox settings.json.
- **Doctor update:** Remove `openshell` check, add `bubblewrap` + `socat` checks (Linux only). Detect platform and skip bwrap check on macOS.
- **Shell wrapper template:** Replace the OpenShell conditional with HOME override + CC native sandbox.
- **Backward compatibility:** If `policy.yaml` exists in an agent dir, ignore it (do not error). Users may have custom policies they want to keep as documentation.
- **Runtime state:** New `RuntimeState` struct should not include OpenShell fields. Handle deserialization of old state files gracefully (serde `#[serde(default)]` on removed fields).

**Warning signs:**
- `rightclaw up` on v2.0 crashes because state file has unexpected fields.
- Orphaned OpenShell sandboxes after upgrade.
- Doctor reports "openshell not found" as failure on v2.0.

**Phase to address:** Phase 1 (migration). Must be addressed before any v2.0 release.

---

### Pitfall 9: Process-Compose Environment Variables Not Reaching Claude Code

**What goes wrong:**
The v2.0 shell wrapper needs to set multiple environment variables (`HOME`, `GIT_CONFIG_GLOBAL`, `SSH_AUTH_SOCK`, `ANTHROPIC_API_KEY`, etc.) before launching `claude`. These must be passed through process-compose's process environment. But process-compose's YAML `environment` section and the shell wrapper's `export` statements interact differently:

1. If env vars are in the YAML `environment` block, they are set before the wrapper runs -- good.
2. If env vars are in the shell wrapper via `export`, they affect only the wrapper's children -- good.
3. But `is_tty: true` in process-compose may interact with the env differently (known v1.0 issue: `is_tty: true` causes restart crashes).
4. `CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC` set to ANY value (even "0" or "false") disables ALL feature flags, breaking Telegram channels and other features.

**How to avoid:**
- Set all agent-specific env vars in the shell wrapper (`export HOME=...`), not in process-compose YAML. The wrapper is the single source of truth for agent environment.
- Never set `CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC` unless explicitly intended.
- Test env var propagation: add `env` to the wrapper and verify all expected vars are set inside the Claude session.
- For `ANTHROPIC_API_KEY`, use process-compose's `environment` section (from a secure source) or an `apiKeyHelper` script in the agent's `settings.json`.

**Warning signs:**
- Agent launches but env vars are not set (check with `/status` inside CC).
- `ANTHROPIC_API_KEY` visible in process-compose logs (security leak).
- Telegram stops working after adding an unrelated env var.

**Phase to address:** Phase 1 (wrapper generation).

---

### Pitfall 10: CC Sandbox Network Proxy + Socat Socket Path Conflicts With Multiple Agents

**What goes wrong:**
CC's sandbox on Linux creates socat bridges using Unix domain sockets in `/tmp/` (e.g., `/tmp/claude-http-*.sock`, `/tmp/claude-socks-*.sock`). With multiple agents, each Claude instance needs its own proxy sockets. If CC uses PID-based or random naming, this works. But if there is any shared state in the proxy setup, concurrent agents may conflict.

Additionally, the socat bridges and proxy servers run OUTSIDE the sandbox (on the host). If multiple agents share the same proxy port configuration, they may collide.

**How to avoid:**
- Trust CC's own socket naming -- it likely uses PID or random suffixes. Verify by launching two CC instances and checking `/tmp/claude-*` sockets.
- If CC exposes `sandbox.network.httpProxyPort` and `sandbox.network.socksProxyPort` settings, ensure RightClaw does NOT set these (let CC auto-assign), or assign unique ports per agent.
- Test: launch 3 agents simultaneously, verify all have network access, check for socket collisions.

**Warning signs:**
- Second agent fails with "address already in use" on proxy port.
- Intermittent network failures in some agents but not others.
- `socat` errors in system log about socket binding.

**Phase to address:** Phase 2 (multi-agent testing).

---

## Technical Debt Patterns

Shortcuts that seem reasonable but create long-term problems.

| Shortcut | Immediate Benefit | Long-term Cost | When Acceptable |
|----------|-------------------|----------------|-----------------|
| Symlinking real `~/.claude/` into agent dirs instead of proper isolation | Quick fix for credential access | All agents share settings, memory, OAuth tokens -- defeats isolation purpose | Never for production; acceptable for prototype validation only |
| Hardcoding absolute paths to user HOME in shell wrapper | Works on the dev machine | Breaks on any other user's machine or CI | Never |
| Using `--no-sandbox` as default for macOS | Avoids Seatbelt complexity | No sandbox on macOS defeats the security proposition | Only for explicit dev mode |
| Skipping bwrap AppArmor fix in install.sh | Simpler install script | Every Ubuntu 24.04 user hits a wall on first run | Never -- must handle at install time |
| Setting `enableWeakerNestedSandbox: true` by default | Works in Docker/CI | Substantially weakens filesystem isolation | Only when additional container isolation exists |

## Integration Gotchas

Common mistakes when connecting to external services during the sandbox transition.

| Integration | Common Mistake | Correct Approach |
|-------------|----------------|------------------|
| CC native sandbox | Generating `settings.json` with `~/` paths under overridden HOME | Use absolute paths everywhere in generated settings |
| CC workspace trust | Writing trust to real `~/.claude.json` while agent HOME is different | Write trust to `<agent_dir>/.claude.json` |
| CC credentials (Linux) | Expecting `~/.claude/.credentials.json` to be found under overridden HOME | Use `ANTHROPIC_API_KEY` env var, or `CLAUDE_CONFIG_DIR` pointing to real config |
| process-compose | Setting sensitive env vars in YAML (visible in `pc status`) | Set env vars in shell wrapper, or use `apiKeyHelper` in settings.json |
| SSH/Git | Expecting `~/.ssh/` and `~/.gitconfig` to exist under agent HOME | Set `GIT_CONFIG_GLOBAL`, `SSH_AUTH_SOCK`, `GIT_SSH_COMMAND` env vars |
| Telegram plugin | Writing .env to real HOME while agent uses overridden HOME | Write .env into `<agent_dir>/.claude/channels/telegram/` |
| bubblewrap (Ubuntu 24.04+) | Assuming `apt install bubblewrap` is sufficient | Must also configure AppArmor profile for bwrap |

## Security Mistakes

Domain-specific security issues for the sandbox migration.

| Mistake | Risk | Prevention |
|---------|------|------------|
| Symlinking real `~/.ssh/` into agent sandbox with write access | Agent (or malicious skill) can modify SSH keys or add authorized_keys | Read-only access only via `sandbox.filesystem.allowRead`, or use SSH agent socket |
| Setting `allowAllUnixSockets: true` in sandbox settings | Exposes Docker socket, D-Bus, and other system sockets to agent | Explicitly list only needed sockets (e.g., SSH agent) |
| Copying `ANTHROPIC_API_KEY` into generated files on disk | Key visible in wrapper scripts, process-compose YAML | Use `apiKeyHelper` script or env var injection at runtime |
| Using `enableWeakerNestedSandbox` on bare metal | Disables user namespace isolation, significantly weakens sandbox | Only enable when already inside a container |
| Not denying `~/.ssh/` in sandbox read rules | Agent can read SSH private keys and known_hosts | Add `"~/.ssh"` to `sandbox.filesystem.denyRead` unless explicitly needed |
| Allowing `sandbox.filesystem.allowWrite` to PATH directories | Agent can plant malicious executables | Never allow write to `/usr/local/bin`, `/usr/bin`, or any PATH directory |

## UX Pitfalls

Common user experience mistakes during the sandbox migration.

| Pitfall | User Impact | Better Approach |
|---------|-------------|-----------------|
| Silent sandbox failure (agent runs unsandboxed without warning) | False sense of security | Detect sandbox status and show clear indicator in `rightclaw status` |
| bwrap AppArmor error with no actionable fix hint | User stuck on Ubuntu 24.04 | `rightclaw doctor` detects condition and prints exact fix commands |
| Upgrade from v1.0 orphans OpenShell sandboxes | Memory/resource leak, confusion | `rightclaw up` detects and cleans orphaned v1.0 sandboxes |
| SSH/Git failures with cryptic error messages | Agent cannot work with repos | Clear error: "SSH keys not available in sandbox. Run `rightclaw config set git-access`" or similar |
| Agent-specific settings not taking effect | Debugging nightmare | `rightclaw doctor` includes per-agent settings validation |

## "Looks Done But Isn't" Checklist

Things that appear complete but are missing critical pieces.

- [ ] **HOME override:** Often missing `.claude.json` trust file in agent dir -- verify agent does not hang on workspace trust dialog
- [ ] **Sandbox settings:** Often missing `sandbox.enabled: true` -- verify CC actually activates the sandbox, not just running unsandboxed
- [ ] **bwrap install:** Often missing AppArmor profile on Ubuntu 24.04 -- verify `bwrap --unshare-net echo ok` works
- [ ] **Git identity:** Often missing `GIT_CONFIG_GLOBAL` -- verify `git config user.name` returns correct value inside agent
- [ ] **SSH access:** Often missing SSH agent forwarding or key access -- verify `ssh -T git@github.com` works inside agent
- [ ] **Multi-agent proxy:** Often missing concurrent socket test -- verify 3+ agents can all access network simultaneously
- [ ] **OpenShell cleanup:** Often missing migration code -- verify no orphaned v1.0 sandboxes remain after upgrade
- [ ] **Telegram token:** Often written to wrong HOME -- verify bot responds under overridden HOME
- [ ] **Credential access:** Often assumes macOS behavior on Linux -- verify `ANTHROPIC_API_KEY` or credential file is accessible

## Recovery Strategies

When pitfalls occur despite prevention, how to recover.

| Pitfall | Recovery Cost | Recovery Steps |
|---------|---------------|----------------|
| Trust dialog blocks agent | LOW | Write `.claude.json` with `hasTrustDialogAccepted: true` into agent dir, restart |
| OAuth credentials not found | LOW | Set `ANTHROPIC_API_KEY` env var, restart agent |
| bwrap AppArmor failure | MEDIUM | Create AppArmor profile, reload AppArmor, restart rightclaw |
| Wrong paths in sandbox settings | LOW | Regenerate `settings.json` with absolute paths, restart |
| SSH/Git identity lost | LOW | Set `GIT_CONFIG_GLOBAL` and `GIT_SSH_COMMAND` env vars in wrapper, restart |
| Orphaned OpenShell sandboxes | LOW | `openshell sandbox list` then `openshell sandbox delete` for each, or `docker rm` |
| Socat socket conflict | MEDIUM | Kill conflicting processes, let CC auto-assign new socket paths on restart |
| Agent dir permissions wrong | LOW | `chmod 700 <agent_dir>`, `chmod 600 <agent_dir>/.claude/.credentials.json` |

## Pitfall-to-Phase Mapping

How roadmap phases should address these pitfalls.

| Pitfall | Prevention Phase | Verification |
|---------|------------------|--------------|
| #1 Trust file location | Phase 1: HOME isolation | Agent starts without trust dialog prompt |
| #2 OAuth credentials | Phase 1: HOME isolation | Agent authenticates on Linux with overridden HOME |
| #3 bwrap AppArmor | Phase 1: install/doctor | `rightclaw doctor` detects and reports bwrap status on Ubuntu 24.04 |
| #4 Settings path resolution | Phase 1: settings generation | `sandbox.filesystem` paths resolve to intended absolute locations |
| #5 SSH/Git identity | Phase 2: agent env setup | `git push` works from inside sandboxed agent |
| #6 Telegram .env path | Phase 2: Telegram update | Telegram bot responds under overridden HOME |
| #7 macOS Seatbelt | Phase 1: cross-platform test | Sandbox activates on macOS without errors |
| #8 OpenShell migration | Phase 1: migration | v1.0 state file deserializes without crash, orphaned sandboxes cleaned |
| #9 Env var propagation | Phase 1: wrapper generation | All expected env vars present inside CC session |
| #10 Proxy socket conflicts | Phase 2: multi-agent test | 3+ agents run concurrently with network access |

## Sources

- [Claude Code Sandboxing Docs](https://code.claude.com/docs/en/sandboxing) -- official sandbox architecture, settings, platform specifics
- [Claude Code Settings Reference](https://code.claude.com/docs/en/settings) -- sandbox.* settings keys, scopes, precedence
- [Anthropic Engineering: Claude Code Sandboxing](https://www.anthropic.com/engineering/claude-code-sandboxing) -- proxy architecture, socat/bwrap internals
- [sandbox-runtime issue #74: bwrap fails on Ubuntu 24.04+](https://github.com/anthropic-experimental/sandbox-runtime/issues/74) -- confirmed AppArmor conflict
- [Ubuntu: Restricted unprivileged user namespaces](https://ubuntu.com/blog/ubuntu-23-10-restricted-unprivileged-user-namespaces) -- AppArmor userns restriction design
- [bubblewrap GitHub](https://github.com/containers/bubblewrap) -- bwrap user namespace requirements
- [ArchWiki: Bubblewrap](https://wiki.archlinux.org/title/Bubblewrap) -- AppArmor profile workaround
- [Claude Code issue #3833: CLAUDE_CONFIG_DIR behavior unclear](https://github.com/anthropics/claude-code/issues/3833)
- [Claude Code issue #25762: Feature request for CLAUDE_CONFIG_DIR](https://github.com/anthropics/claude-code/issues/25762)
- [Claude Code issue #9113: Workspace trust not respecting pre-config](https://github.com/anthropics/claude-code/issues/9113)
- [CVE-2026-33068: Workspace trust dialog bypass via repo settings](https://github.com/anthropics/claude-code/security/advisories/GHSA-mmgp-wc2j-qcv7)
- [Claude Code Authentication Docs](https://code.claude.com/docs/en/authentication) -- credential storage locations
- [sandbox-exec deprecation (OpenAI Codex issue #215)](https://github.com/openai/codex/issues/215) -- macOS Seatbelt status
- [Hacker News: macOS Seatbelt situation](https://news.ycombinator.com/item?id=44283454) -- community analysis of deprecation
- [Git Environment Variables](https://git-scm.com/book/en/v2/Git-Internals-Environment-Variables) -- GIT_CONFIG_GLOBAL, GIT_SSH_COMMAND
- [ssh_config(5) man page](https://www.man7.org/linux/man-pages/man5/ssh_config.5.html) -- SSH HOME dependency
- [Trail of Bits: claude-code-config](https://github.com/trailofbits/claude-code-config) -- opinionated sandbox configuration reference
- Existing codebase: `init.rs` (pre_trust_directory), `sandbox.rs` (RuntimeState), `shell_wrapper.rs`, `doctor.rs`, `deps.rs`
- Project memory: SEED-003 (OpenShell API key), SEED-004 (host settings leak)

---
*Pitfalls research for: CC native sandbox + per-agent HOME isolation (v2.0 migration)*
*Researched: 2026-03-23*
