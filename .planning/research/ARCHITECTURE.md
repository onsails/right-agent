# Architecture Research: v2.1 Headless Agent Isolation

**Domain:** Multi-agent CLI runtime -- managed settings, HOME override, dropping bypass mode
**Researched:** 2026-03-24
**Confidence:** HIGH
**Focus:** How managed-settings.json, HOME override, and permissions.allow integrate with existing codegen architecture

## System Overview (v2.1 Target)

```
                            User
                             |
                      rightclaw CLI
                             |
              +--------------+--------------+
              |              |              |
        Agent Discovery  Config Gen    PC Lifecycle
        (scan agents/)   (codegen)     (spawn/attach/stop)
              |              |              |
              v              v              v
        +----------+  +-----------+  +----------------+
        | AgentDef |->| PCConfig  |->| process-compose|
        | (parsed) |  | Generator |  | (child proc)   |
        +----------+  +-----------+  +----------------+
              |              |              |
              v              v         per-agent process
        +-----------+  +-----------+  +---------------------+
        | Settings  |  | Combined  |  | Shell Wrapper       |
  NEW-->| Generator |  | Prompt    |  | (generated script)  |
        | (.claude/ |  | Generator |  +--------|------------+
        | settings) |  +-----------+           |
        +-----------+       |            exec $CLAUDE_BIN
              |             |       NEW: no --dangerously-skip-permissions
              v             v       NEW: HOME=$AGENT_HOME
        +-----------+  +-----------+
   NEW->| Managed   |  | Per-agent |
        | Settings  |  | .claude/  |
        | Generator |  | settings  |
        | (/etc/cc/)|  | .json     |
        +-----------+  +-----------+
              |
              v
        +---------------------+
   NEW->| Home Scaffold       |
        | (agent HOME dir     |
        |  with trust, auth,  |
        |  git/SSH forwarding)|
        +---------------------+
```

## Current Architecture (v2.0 Baseline)

### Codegen Pipeline

`cmd_up()` in main.rs drives the pipeline for each agent:

1. **`generate_combined_prompt(agent)`** -- produces `{name}-prompt.md` in `run/`
2. **`generate_wrapper(agent, prompt_path, debug_log)`** -- produces `{name}.sh` in `run/`
3. **`generate_settings(agent, no_sandbox)`** -- writes `.claude/settings.json` into `agent.path`
4. **`generate_process_compose(agents, run_dir)`** -- produces `process-compose.yaml` in `run/`

### Current Files and Responsibilities

| File | Responsibility | Changes Needed for v2.1 |
|------|---------------|-------------------------|
| `codegen/settings.rs` | Generates per-agent `.claude/settings.json` with sandbox config | Add `permissions.allow` + `defaultMode`, remove `skipDangerousModePermissionPrompt` |
| `codegen/shell_wrapper.rs` | Generates bash wrapper calling `claude` | Remove `--dangerously-skip-permissions`, add `HOME` override |
| `templates/agent-wrapper.sh.j2` | Jinja2 template for wrapper script | Add `export HOME=`, remove `--dangerously-skip-permissions` |
| `init.rs` | Creates `~/.rightclaw/agents/right/` + pre-trust | Scaffold HOME dir, update trust path, new `managed-settings.json` |
| `agent/types.rs` | `AgentDef`, `AgentConfig`, `SandboxOverrides` | Add `permissions` section to `AgentConfig` |
| `codegen/mod.rs` | Re-exports codegen functions | Add `generate_managed_settings`, `scaffold_agent_home` |
| `codegen/process_compose.rs` | Generates process-compose.yaml | No changes expected |
| `codegen/system_prompt.rs` | Generates combined prompt content | No changes expected |
| `config.rs` | Resolves `RIGHTCLAW_HOME` | No changes expected |
| `doctor.rs` | Validates dependencies and agent structure | Add HOME scaffold validation |

## Integration Architecture: Three New Subsystems

### Subsystem 1: Managed Settings Generation

**What:** Write `/etc/claude-code/managed-settings.json` (Linux) with `allowManagedDomainsOnly: true` and the merged domain allowlist.

**Why managed settings, not project settings:** The `allowManagedDomainsOnly` flag is a managed-settings-only feature. It cannot be set in user, project, or local settings. Without it, non-allowed domains prompt the user (blocking headless operation). With it, non-allowed domains are silently blocked.

**Where in code:** New file `codegen/managed_settings.rs`

**Data flow:**
```
AgentDef[] (all agents)
    |
    v
Collect all allowed_domains from:
  - DEFAULT_ALLOWED_DOMAINS (settings.rs constant)
  - Per-agent SandboxOverrides.allowed_domains
    |
    v
Merge, deduplicate
    |
    v
Write /etc/claude-code/managed-settings.json:
{
  "sandbox": {
    "network": {
      "allowedDomains": [...merged...],
      "allowManagedDomainsOnly": true
    }
  },
  "permissions": {
    "disableBypassPermissionsMode": "disable"
  }
}
```

**Domain merging tension:** Managed settings arrays merge with project/user settings. But `allowManagedDomainsOnly: true` means ONLY managed `allowedDomains` are respected -- domains from project/user/local settings are ignored. This is exactly what we want: per-agent `.claude/settings.json` domains become decorative. The managed file is the single source of truth.

**Consequence for per-agent overrides:** Per-agent `sandbox.allowed_domains` in `agent.yaml` must be merged INTO the managed settings, not into the per-agent project settings. This is a design inversion from v2.0 where they merged into `.claude/settings.json`. The managed settings file must contain the union of all agents' domain needs.

**Alternative considered: Per-agent managed settings via HOME override.** If each agent has its own HOME, each could have its own `/etc/claude-code/managed-settings.json` equivalent. But CC reads managed settings from a SYSTEM path (`/etc/claude-code/`), not relative to HOME. So this doesn't work -- there's one managed-settings.json for the entire machine. All agents share it.

**Privilege concern:** Writing to `/etc/claude-code/` requires root. Options:
1. **Require sudo for `rightclaw up`** -- bad UX, breaks non-root workflows
2. **Pre-create with install.sh** -- `install.sh` already runs with root, could set up the dir with correct perms
3. **Use a different managed settings delivery** -- MDM/plist on macOS, but overkill
4. **Recommendation:** `install.sh` creates `/etc/claude-code/` owned by the user (or group-writable). Then `rightclaw up` writes managed-settings.json without sudo. Doctor checks this.

### Subsystem 2: HOME Override (Agent Isolation)

**What:** Each agent runs with `HOME` pointing to a dedicated directory, isolating it from host `~/.claude.json`, `~/.claude/settings.json`, and other user config.

**Where HOME lives:**
```
~/.rightclaw/
  homes/
    right/           <-- HOME for agent "right"
      .claude.json   <-- trust entries (pre-generated)
      .claude/
        settings.json  <-- user-scope settings (pre-generated)
    scout/
      .claude.json
      .claude/
        settings.json
```

**Why separate from agent dir:** Agent dir (`~/.rightclaw/agents/right/`) is the cwd (working directory) where CC reads project-scoped config (`.claude/settings.json`, CLAUDE.md, skills). HOME is where CC reads user-scoped config (`~/.claude.json`, `~/.claude/settings.json`). These are different scopes in CC's hierarchy:

| Scope | Reads from | Our control |
|-------|-----------|-------------|
| Managed | `/etc/claude-code/managed-settings.json` | `generate_managed_settings()` |
| User | `$HOME/.claude/settings.json` | HOME override + `scaffold_agent_home()` |
| Project | `<cwd>/.claude/settings.json` | `generate_settings()` (existing) |
| Local | `<cwd>/.claude/settings.local.json` | Not used |

**Shell wrapper changes:**

Current template:
```bash
exec "$CLAUDE_BIN" \
  --append-system-prompt-file "{{ combined_prompt_path }}" \
  --dangerously-skip-permissions \
  ...
```

New template:
```bash
export HOME="{{ agent_home }}"
export CLAUDE_CONFIG_DIR="{{ agent_home }}/.claude"
# Forward SSH agent from real home
{% if ssh_auth_sock %}export SSH_AUTH_SOCK="{{ ssh_auth_sock }}"{% endif %}
# Forward git config
export GIT_CONFIG_GLOBAL="{{ real_home }}/.gitconfig"

exec "$CLAUDE_BIN" \
  --append-system-prompt-file "{{ combined_prompt_path }}" \
  ...
```

Key changes:
- **Remove `--dangerously-skip-permissions`** -- replaced by `permissions.allow` in settings
- **Add `HOME` export** -- points to agent-specific home
- **Add `CLAUDE_CONFIG_DIR` export** -- explicitly point CC to the right config dir (belt-and-suspenders with HOME)
- **Forward `SSH_AUTH_SOCK`** -- agents still need git push/pull via SSH
- **Forward `GIT_CONFIG_GLOBAL`** -- agents inherit user's git identity (name, email, signing)
- **ANTHROPIC_API_KEY** -- must be passed through (env var, not file-based)

**What goes in the agent HOME:**

| File | Purpose | Generated by |
|------|---------|-------------|
| `.claude.json` | Trust entry for agent's cwd | `scaffold_agent_home()` |
| `.claude/settings.json` | User-scope settings (spinnerTips, prefersReducedMotion, skipDangerousModePermissionPrompt) | `scaffold_agent_home()` |
| `.claude/channels/telegram/.env` | Telegram bot token (if configured) | `scaffold_agent_home()` |
| `.claude/channels/telegram/access.json` | Telegram pairing (if configured) | `scaffold_agent_home()` |

**What does NOT go in agent HOME:**
- OAuth tokens (agents use `ANTHROPIC_API_KEY` env var)
- Plugins/marketplaces (not needed, skills are in agent cwd)
- Auto-memory (managed via agent cwd)
- Session transcripts (ephemeral, don't need host state)

### Subsystem 3: Permissions-Based Authorization (Replacing --dangerously-skip-permissions)

**What:** Instead of `--dangerously-skip-permissions` (which shows a scary "bypass mode" warning), use explicit `permissions.allow` rules + sandbox to achieve the same effect without the warning.

**How it works:**

In the per-agent project settings (`<cwd>/.claude/settings.json`):
```json
{
  "permissions": {
    "allow": [
      "Bash(*)",
      "Edit(*)",
      "Read(*)",
      "Write(*)",
      "WebFetch(*)",
      "mcp__*"
    ],
    "defaultMode": "bypassPermissions"
  },
  "sandbox": {
    "enabled": true,
    "autoAllowBashIfSandboxed": true,
    "allowUnsandboxedCommands": false,
    ...
  }
}
```

And in the managed settings (`/etc/claude-code/managed-settings.json`):
```json
{
  "permissions": {
    "disableBypassPermissionsMode": "disable"
  }
}
```

Wait -- this is contradictory. `defaultMode: "bypassPermissions"` and `disableBypassPermissionsMode: "disable"` conflict. Let me reconsider.

**Revised approach:**

The goal is: no permission prompts, no bypass warning, sandbox enforced.

Option A: `permissions.allow` with wildcards + `defaultMode: "acceptEdits"` + sandbox auto-allow
- `permissions.allow: ["Bash(*)", "Edit(*)", "Read(*)", "Write(*)", ...]` auto-approves everything
- `sandbox.autoAllowBashIfSandboxed: true` auto-approves sandboxed bash
- `sandbox.allowUnsandboxedCommands: false` forces all commands through sandbox
- No `--dangerously-skip-permissions` needed
- No bypass warning shown
- **Risk:** Bash wildcard `Bash(*)` doesn't match commands with shell operators (`&&`, `||`, `|`, `;`, `>`, `$()`, backticks). Those would still prompt.

Option B: Keep `--dangerously-skip-permissions` but suppress the warning via `skipDangerousModePermissionPrompt`
- Already doing this in v2.0
- Works, but the flag name is alarming for documentation/audits
- CC could remove or change behavior of this flag

Option C: `defaultMode: "bypassPermissions"` in settings (no CLI flag needed)
- Settings-driven bypass mode. Same behavior as `--dangerously-skip-permissions` but configured via settings
- If `disableBypassPermissionsMode` is set in managed settings, this won't work
- But we control managed settings too, so don't set `disableBypassPermissionsMode`

**Recommendation: Option A (permissions.allow wildcards + sandbox auto-allow).**

Rationale:
- Cleanest from a security audit perspective
- No bypass mode at all -- permissions are explicitly granted
- Sandbox provides real OS-level enforcement
- The shell operator limitation of `Bash(*)` is mitigated by `autoAllowBashIfSandboxed: true` -- sandboxed commands bypass the permission check entirely regardless of pattern matching
- `allowUnsandboxedCommands: false` ensures nothing escapes the sandbox

**What changes in settings.rs:**

```rust
// Current settings.json output:
{
  "skipDangerousModePermissionPrompt": true,  // REMOVE
  "sandbox": { ... },
}

// New settings.json output:
{
  "permissions": {
    "allow": [
      "Edit(*)",
      "Write(*)",
      "Read(*)",
      "WebFetch(*)",
      "mcp__*"
    ]
  },
  "sandbox": {
    "enabled": true,  // (or false if no_sandbox)
    "autoAllowBashIfSandboxed": true,
    "allowUnsandboxedCommands": false,
    ...
  },
  "spinnerTipsEnabled": false,
  "prefersReducedMotion": true,
}
```

Note: `Bash(*)` is intentionally NOT in the allow list. Bash commands are auto-approved by `autoAllowBashIfSandboxed: true` when sandbox is enabled. When sandbox is disabled (`--no-sandbox`), bash commands would need `Bash(*)` in the allow list, but `--no-sandbox` is a dev-only flag where prompts are acceptable.

## Code Change Map

### 1. New: `codegen/managed_settings.rs`

```rust
pub fn generate_managed_settings(
    agents: &[AgentDef],
) -> miette::Result<serde_json::Value>
```

- Collects `DEFAULT_ALLOWED_DOMAINS` + all per-agent `sandbox.allowed_domains`
- Produces JSON with `allowManagedDomainsOnly: true`
- Called once per `rightclaw up`, not per-agent

### 2. New: `codegen/home_scaffold.rs`

```rust
pub fn scaffold_agent_home(
    agent: &AgentDef,
    homes_dir: &Path,      // ~/.rightclaw/homes/
    real_home: &Path,       // actual $HOME
    telegram_env: Option<TelegramEnv>,
) -> miette::Result<PathBuf>  // returns agent home path
```

- Creates `~/.rightclaw/homes/{agent_name}/`
- Writes `.claude.json` with `hasTrustDialogAccepted: true` for agent cwd
- Writes `.claude/settings.json` with user-scope settings (skipDangerousModePermissionPrompt, reduced motion, etc.)
- Copies Telegram env/access files if configured
- Returns the HOME path for use in wrapper generation

### 3. Modified: `codegen/settings.rs`

Changes:
- Remove `"skipDangerousModePermissionPrompt": true` from output (moves to user-scope settings in agent HOME)
- Add `"permissions": { "allow": [...] }` section
- Keep sandbox config as-is (it's project-scope, correct location)
- Remove `allowedDomains` from project settings (moved to managed settings)
- Keep `allowWrite`, `denyRead`, `excludedCommands` in project settings (these merge normally)

New signature (unchanged, but output changes):
```rust
pub fn generate_settings(agent: &AgentDef, no_sandbox: bool) -> miette::Result<serde_json::Value>
```

### 4. Modified: `codegen/shell_wrapper.rs`

Changes to `generate_wrapper()`:
- Add `agent_home: &str` parameter
- Add `real_home: &str` parameter
- Add `ssh_auth_sock: Option<&str>` parameter
- Remove reference to `--dangerously-skip-permissions` in context

### 5. Modified: `templates/agent-wrapper.sh.j2`

```bash
#!/usr/bin/env bash
# Generated by rightclaw -- do not edit
# Agent: {{ agent_name }}
set -euo pipefail

# Per-agent HOME isolation
export HOME="{{ agent_home }}"
export CLAUDE_CONFIG_DIR="{{ agent_home }}/.claude"

# Forward host environment
export GIT_CONFIG_GLOBAL="{{ real_home }}/.gitconfig"
{% if ssh_auth_sock %}export SSH_AUTH_SOCK="{{ ssh_auth_sock }}"
{% endif %}

# Resolve claude binary
CLAUDE_BIN=""
for bin in claude claude-bun; do
  if command -v "$bin" &>/dev/null; then
    CLAUDE_BIN="$bin"
    break
  fi
done
if [ -z "$CLAUDE_BIN" ]; then
  echo "error: claude CLI not found" >&2
  exit 1
fi

exec "$CLAUDE_BIN" \
  --append-system-prompt-file "{{ combined_prompt_path }}" \
  {% if model %}--model {{ model }} \
  {% endif %}{% if debug %}--debug-file "{{ debug_log_path }}" \
  {% endif %}{% if channels %}--channels {{ channels }} \
  {% endif %}{% if startup_prompt %}-- "{{ startup_prompt }}"
  {% else %}
  {% endif %}
```

Key change: `--dangerously-skip-permissions` is GONE. Replaced by `permissions.allow` in settings + sandbox auto-allow.

### 6. Modified: `init.rs`

Changes:
- `pre_trust_directory()` now writes to agent HOME's `.claude.json` instead of host's `~/.claude.json`
- Remove writes to host's `~/.claude/settings.json` (no longer needed since HOME is overridden)
- Call `scaffold_agent_home()` during init to set up the HOME directory
- Telegram env files go to agent HOME's `.claude/channels/telegram/` instead of host's

### 7. Modified: `agent/types.rs`

Add optional `permissions` section to `AgentConfig`:
```rust
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PermissionOverrides {
    #[serde(default)]
    pub allow: Vec<String>,
    #[serde(default)]
    pub deny: Vec<String>,
}
```

### 8. Modified: `doctor.rs`

Add checks:
- `/etc/claude-code/` directory exists and is writable
- Agent HOME directories exist
- `.claude.json` trust entries are valid

### 9. Modified: `main.rs` (`cmd_up`)

New steps in the pipeline:
```
1. discover agents
2. for each agent:
   a. scaffold_agent_home()        <-- NEW
   b. generate_combined_prompt()
   c. generate_wrapper()           <-- MODIFIED (new params)
   d. generate_settings()          <-- MODIFIED (new output)
3. generate_managed_settings()     <-- NEW (once, not per-agent)
4. write managed-settings.json to /etc/claude-code/
5. generate_process_compose()
6. launch
```

## Data Flow Diagrams

### Domain Allowlist Flow (NEW)

```
agent.yaml (per-agent)          DEFAULT_ALLOWED_DOMAINS
  sandbox:                          (settings.rs const)
    allowed_domains:                    |
      - custom.example.com             |
              |                        |
              v                        v
        +-----------------------------+
        | generate_managed_settings() |
        | (merge + deduplicate)       |
        +-----------------------------+
                    |
                    v
        /etc/claude-code/managed-settings.json
        {
          "sandbox": {
            "network": {
              "allowedDomains": [
                "api.anthropic.com",  // from defaults
                "github.com",
                "custom.example.com", // from agent override
                ...
              ],
              "allowManagedDomainsOnly": true
            }
          }
        }
                    |
                    v
        Claude Code reads managed settings
        (highest priority, overrides all)
                    |
                    v
        Non-listed domains: SILENTLY BLOCKED
        (no user prompt -- headless safe)
```

### Settings Scope Distribution (NEW)

```
+------------------------------------------------------------------+
| MANAGED (/etc/claude-code/managed-settings.json)                  |
| - allowedDomains (union of all agents)                            |
| - allowManagedDomainsOnly: true                                   |
| - (optionally) disableBypassPermissionsMode: "disable"            |
| Written by: generate_managed_settings()                           |
| Scope: Machine-wide, all agents share this                        |
+------------------------------------------------------------------+

+------------------------------------------------------------------+
| USER ($AGENT_HOME/.claude/settings.json)                          |
| - skipDangerousModePermissionPrompt: true                         |
| - spinnerTipsEnabled: false                                       |
| - prefersReducedMotion: true                                      |
| Written by: scaffold_agent_home()                                 |
| Scope: Per-agent (via HOME override)                              |
+------------------------------------------------------------------+

+------------------------------------------------------------------+
| PROJECT (<agent_cwd>/.claude/settings.json)                       |
| - permissions.allow: [Edit(*), Write(*), Read(*), ...]            |
| - sandbox.enabled: true                                           |
| - sandbox.autoAllowBashIfSandboxed: true                          |
| - sandbox.allowUnsandboxedCommands: false                         |
| - sandbox.filesystem.allowWrite: [agent_path, ...]                |
| - sandbox.filesystem.denyRead: [~/.ssh, ...]                      |
| - sandbox.excludedCommands: [...]                                 |
| - enabledPlugins (if telegram)                                    |
| Written by: generate_settings()                                   |
| Scope: Per-agent (written to agent cwd)                           |
+------------------------------------------------------------------+

+------------------------------------------------------------------+
| TRUST ($AGENT_HOME/.claude.json)                                  |
| - projects.<agent_cwd>.hasTrustDialogAccepted: true               |
| Written by: scaffold_agent_home()                                 |
| Scope: Per-agent (via HOME override)                              |
+------------------------------------------------------------------+
```

### HOME Override Flow

```
Real $HOME                    Agent HOME
(/home/user/)                 (~/.rightclaw/homes/right/)
     |                              |
     |-- .gitconfig  <---------+    |-- .claude.json (trust)
     |-- .ssh/agent.sock  <-+  |    |-- .claude/
     |-- .claude.json       |  |    |     |-- settings.json (user scope)
     |-- .claude/            |  |    |     |-- channels/telegram/.env
     |     |-- settings.json |  |    |
     |     |-- ...           |  |    (no .ssh, no .gnupg, no .aws)
     |                       |  |
     |                       |  |
     Forwarded via env vars: |  |
       SSH_AUTH_SOCK --------+  |
       GIT_CONFIG_GLOBAL ------+
```

## Anti-Patterns to Avoid

### Anti-Pattern 1: Per-Agent Managed Settings

**What people might do:** Try to write per-agent managed-settings.json by putting it in the agent's HOME
**Why it's wrong:** Claude Code reads managed settings from a SYSTEM path (`/etc/claude-code/` on Linux, `/Library/Application Support/ClaudeCode/` on macOS). HOME override does not affect this lookup.
**Do this instead:** Write one shared managed-settings.json with the union of all agents' domain needs.

### Anti-Pattern 2: Domains in Both Managed and Project Settings

**What people might do:** Put `allowedDomains` in both managed settings and per-agent `.claude/settings.json`
**Why it's wrong:** When `allowManagedDomainsOnly: true`, CC ignores domains from project/user/local settings. The per-agent domains become dead config that confuses maintainers.
**Do this instead:** Put ALL domains in managed settings only. Remove `allowedDomains` from project settings entirely.

### Anti-Pattern 3: Mixing HOME Override with Host Config Mutation

**What people might do:** Write trust entries to both host `~/.claude.json` and agent HOME's `.claude.json`
**Why it's wrong:** With HOME override, CC reads `$HOME/.claude.json`, not the real home's. Writing to host's file is wasted I/O and creates stale state.
**Do this instead:** Only write to agent HOME's `.claude.json`. Stop touching the host's config.

### Anti-Pattern 4: Copying OAuth Tokens to Agent HOME

**What people might do:** Copy `~/.claude.json`'s OAuth session data to agent HOME
**Why it's wrong:** OAuth tokens are user-specific, may expire, and copying them creates a security surface (token sprawl). Agents should use `ANTHROPIC_API_KEY` env var.
**Do this instead:** Pass `ANTHROPIC_API_KEY` as an environment variable in the shell wrapper or process-compose environment section.

### Anti-Pattern 5: Using CLAUDE_CONFIG_DIR Instead of HOME

**What people might do:** Set only `CLAUDE_CONFIG_DIR` without overriding HOME
**Why it's wrong:** `CLAUDE_CONFIG_DIR` affects where CC stores config data but `.claude.json` (trust state) is read from `$HOME/.claude.json`, not `$CLAUDE_CONFIG_DIR`. Also, `CLAUDE_CONFIG_DIR` has known bugs (still creates local `.claude/` dirs, IDE integration issues).
**Do this instead:** Override `HOME` (which moves everything) AND set `CLAUDE_CONFIG_DIR` as belt-and-suspenders.

## Edge Cases

### Edge Case 1: No ANTHROPIC_API_KEY Set

If the user has no API key and relies on OAuth login, HOME override breaks authentication because the OAuth token is in the real home's `.claude.json`.

**Mitigation:** `rightclaw doctor` checks for `ANTHROPIC_API_KEY` in environment. If not set, warn that headless mode requires an API key. This is already documented in MEMORY.md (SEED-003).

### Edge Case 2: macOS Managed Settings Path Differs

macOS uses `/Library/Application Support/ClaudeCode/managed-settings.json`, not `/etc/claude-code/`.

**Mitigation:** Use conditional path in `generate_managed_settings()`:
```rust
fn managed_settings_dir() -> PathBuf {
    if cfg!(target_os = "macos") {
        PathBuf::from("/Library/Application Support/ClaudeCode")
    } else {
        PathBuf::from("/etc/claude-code")
    }
}
```

### Edge Case 3: Multiple RightClaw Instances

If two users run `rightclaw up` on the same machine, the managed-settings.json is overwritten. Last writer wins.

**Mitigation:** This is acceptable for single-user deployments (RightClaw's target). Document as a known limitation. Future: use file locking or per-user managed settings paths.

### Edge Case 4: Agent Needs to Write Outside Its Directory

An agent that runs `npm install` needs write access to `node_modules/` which may be in a different path. Sandbox `allowWrite` handles this via `SandboxOverrides.allow_write` in agent.yaml.

**No change needed:** This already works in v2.0.

### Edge Case 5: Git Identity in Agent HOME

`git commit` uses the committer identity from `~/.gitconfig`. With HOME override, git won't find the real config.

**Mitigation:** `GIT_CONFIG_GLOBAL` env var explicitly points to the real home's `.gitconfig`. This is forwarded in the shell wrapper.

### Edge Case 6: SSH Agent Forwarding

`git push` over SSH needs the SSH agent socket. With HOME override, `~/.ssh/` doesn't exist in agent HOME.

**Mitigation:** `SSH_AUTH_SOCK` env var is forwarded from the real environment. SSH agent socket is independent of HOME.

## Build Order (Phase Dependency Graph)

```
Phase 1: managed_settings.rs
  - New file, no dependencies on other changes
  - Can be tested in isolation
  - install.sh update for /etc/claude-code/ permissions

Phase 2: home_scaffold.rs
  - New file, depends on AgentDef only
  - Creates HOME directory structure
  - Writes .claude.json trust + .claude/settings.json user-scope

Phase 3: settings.rs changes
  - Remove allowedDomains (moved to managed)
  - Add permissions.allow
  - Remove skipDangerousModePermissionPrompt (moved to user-scope)
  - Depends on: understanding of what goes where (Phases 1+2)

Phase 4: shell_wrapper.rs + template changes
  - Remove --dangerously-skip-permissions
  - Add HOME, CLAUDE_CONFIG_DIR, GIT_CONFIG_GLOBAL, SSH_AUTH_SOCK exports
  - Depends on: Phase 2 (needs agent_home path)

Phase 5: init.rs changes
  - Use scaffold_agent_home() instead of pre_trust_directory()
  - Update Telegram env paths
  - Depends on: Phase 2

Phase 6: agent/types.rs changes
  - Add PermissionOverrides to AgentConfig
  - Independent, can be done anytime

Phase 7: main.rs cmd_up changes
  - Wire everything together
  - Depends on: all previous phases

Phase 8: doctor.rs changes
  - Add new checks
  - Depends on: Phases 1+2 (needs to know what to check)

Phase 9: Integration testing
  - End-to-end wrapper generation + settings validation
  - Depends on: all previous phases
```

## Recommended Phased Implementation

**Phase 1: Managed Settings Foundation**
- `codegen/managed_settings.rs` -- generate managed-settings.json
- `install.sh` update -- create `/etc/claude-code/` with correct perms
- `doctor.rs` -- check `/etc/claude-code/` writability
- Tests: unit tests for domain merging, correct JSON output

**Phase 2: HOME Scaffold**
- `codegen/home_scaffold.rs` -- create agent HOME dirs with trust + user settings
- `agent/types.rs` -- add PermissionOverrides
- Tests: unit tests for HOME directory structure, trust entry format

**Phase 3: Drop Bypass Mode**
- `codegen/settings.rs` -- add permissions.allow, remove stale fields
- `templates/agent-wrapper.sh.j2` -- remove `--dangerously-skip-permissions`, add HOME export
- `codegen/shell_wrapper.rs` -- new parameters for HOME, SSH, git
- `init.rs` -- use scaffold_agent_home, stop mutating host config
- `main.rs` -- wire managed settings + HOME scaffold into cmd_up pipeline
- Tests: integration tests for full wrapper + settings output

## Sources

- [Claude Code Settings Reference](https://code.claude.com/docs/en/settings) -- settings hierarchy, merging, managed settings paths (verified 2026-03-24, HIGH confidence)
- [Claude Code Sandboxing](https://code.claude.com/docs/en/sandboxing) -- allowManagedDomainsOnly behavior, sandbox config (verified 2026-03-24, HIGH confidence)
- [Claude Code Environment Variables](https://code.claude.com/docs/en/env-vars) -- CLAUDE_CONFIG_DIR, ANTHROPIC_API_KEY (verified 2026-03-24, HIGH confidence)
- [Claude Code Permissions](https://code.claude.com/docs/en/permissions) -- permissions.allow syntax, disableBypassPermissionsMode (verified 2026-03-24, HIGH confidence)
- [CLAUDE_CONFIG_DIR Bug Report](https://github.com/anthropics/claude-code/issues/3833) -- still creates local .claude/ dirs (MEDIUM confidence)
- [CLAUDE_CONFIG_DIR Feature Request](https://github.com/anthropics/claude-code/issues/28808) -- documented limitations
- [Claude Code hasTrustDialogAccepted behavior](https://github.com/anthropics/claude-code/issues/9113) -- trust not persisting in some cases (MEDIUM confidence)
- [--dangerously-skip-permissions .claude/ write bug](https://github.com/anthropics/claude-code/issues/35718) -- bypass doesn't fully bypass (MEDIUM confidence)

---
*Architecture research for: v2.1 Headless Agent Isolation*
*Researched: 2026-03-24*
