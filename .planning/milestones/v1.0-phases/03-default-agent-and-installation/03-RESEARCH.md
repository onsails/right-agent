# Phase 3: Default Agent and Installation - Research

**Researched:** 2026-03-22
**Domain:** First-run experience -- install script, doctor command, default agent onboarding, OpenShell policy, Telegram channel
**Confidence:** HIGH

## Summary

Phase 3 transforms RightClaw from a developer tool into a user-installable product. It spans five distinct domains: (1) a production-quality OpenShell policy.yaml with comprehensive comments, (2) a BOOTSTRAP.md onboarding conversation modeled on OpenClaw, (3) Telegram channel setup via `rightclaw init --telegram-token`, (4) a `rightclaw doctor` diagnostic command, and (5) an install.sh script that downloads all three binaries. The existing codebase already has `init_rightclaw_home()`, `verify_dependencies()`, template embedding via `include_str!`, and the Clap CLI with Commands enum -- this phase extends all of these.

**Critical discovery:** Claude Code Channels (Telegram plugin) require the `--channels plugin:telegram@claude-plugins-official` flag on the `claude` launch command AND the plugin must be pre-installed. Being in `.mcp.json` alone does NOT activate channel message delivery. This means the shell wrapper template (`agent-wrapper.sh.j2`) must conditionally include the `--channels` flag, and the init flow should run `/plugin install telegram@claude-plugins-official` or document that the user must do it. The `.mcp.json` file configures the MCP server (tools), but channel event delivery needs `--channels`.

**Primary recommendation:** Build outward from existing patterns. Extend `init_rightclaw_home()` with Telegram token handling and `.mcp.json` generation. Add `Doctor` variant to Commands enum reusing `verify_dependencies()`. Write policy.yaml and BOOTSTRAP.md as template files embedded with `include_str!`. Shell script (install.sh) is pure bash, not Rust.

<user_constraints>
## User Constraints (from CONTEXT.md)

### Locked Decisions
- **D-01:** Match OpenClaw's onboarding -- 4 questions: name, creature type, vibe, emoji
- **D-02:** BOOTSTRAP.md is conversational (CC reads it as system prompt, agent drives the conversation)
- **D-03:** On completion, writes IDENTITY.md (updated with user's choices), USER.md (user name, preferences), SOUL.md (personality based on vibe)
- **D-04:** After writing files, BOOTSTRAP.md self-deletes (agent removes the file)
- **D-05:** Telegram setup is NOT in BOOTSTRAP.md -- it happens in `rightclaw init` before agent launches (MCP servers load at session start, can't be added mid-session)
- **D-06:** `rightclaw init` prompts for Telegram bot token during initialization
- **D-07:** Both flag (`--telegram-token <token>`) and interactive prompt supported -- flag takes priority, interactive fallback
- **D-08:** If token provided: writes `.mcp.json` with Claude Code Telegram plugin config, updates policy.yaml network allowlist to include `api.telegram.org`
- **D-09:** If token skipped: no `.mcp.json` created, policy.yaml has commented-out Telegram network rule
- **D-10:** Token validation: basic format check (numeric:alphanumeric), not API verification
- **D-11:** Strict least-privilege default: agent dir read-write only, everything else read-only (/usr, /lib)
- **D-12:** Network: only api.github.com and api.telegram.org (if configured)
- **D-13:** `hard_requirement` for Landlock -- no silent degradation on older kernels
- **D-14:** Comprehensive comments throughout policy.yaml showing how to expand permissions
- **D-15:** The policy.yaml serves as self-documenting reference
- **D-16:** Downloads pre-built binaries from GitHub Releases (platform detection: linux-x86_64, darwin-arm64)
- **D-17:** Checks for existing installations of process-compose and OpenShell -- only installs missing ones
- **D-18:** Calls each tool's official install mechanism (curl-based installers)
- **D-19:** After installing, runs `rightclaw init` to create ~/.rightclaw/ + default agent (includes Telegram token prompt)
- **D-20:** Runs `rightclaw doctor` at the end to verify everything works
- **D-21:** `rightclaw doctor` checks: rightclaw binary, process-compose, openshell, claude CLI -- all in PATH
- **D-22:** Validates ~/.rightclaw/agents/ structure -- at least one valid agent (IDENTITY.md + policy.yaml)
- **D-23:** Reports pass/fail per check with clear fix instructions

### Claude's Discretion
- Exact BOOTSTRAP.md conversation design (question phrasing, follow-up handling)
- doctor command output formatting
- install.sh error handling and platform edge cases
- .mcp.json exact structure (follow Claude Code Telegram plugin docs)

### Deferred Ideas (OUT OF SCOPE)
None -- discussion stayed within phase scope
</user_constraints>

<phase_requirements>
## Phase Requirements

| ID | Description | Research Support |
|----|-------------|------------------|
| DFLT-01 | RightClaw ships a default "Right" agent in `agents/right/` | Existing `init_rightclaw_home()` creates this structure; needs BOOTSTRAP.md added to template set |
| DFLT-02 | "Right" agent has BOOTSTRAP.md that runs on first conversation | OpenClaw BOOTSTRAP.md template researched -- 4-question conversational format confirmed |
| DFLT-03 | BOOTSTRAP.md onboarding writes IDENTITY.md, USER.md, SOUL.md then self-deletes | OpenClaw pattern: agent calls delete after writing files; "Delete this file" instruction at end |
| DFLT-04 | "Right" agent is general-purpose -- no domain-specific skills | Current IDENTITY.md/SOUL.md/AGENTS.md templates already fulfill this |
| SAND-04 | Shipped default policies use `hard_requirement` for Landlock | OpenShell schema confirms `landlock.compatibility` accepts `hard_requirement` |
| SAND-05 | Shipped default policies cover filesystem, network, and process restrictions | Full OpenShell policy schema documented -- all sections researched with examples |
| INST-01 | `install.sh` one-liner installs rightclaw, process-compose, and OpenShell | Official install scripts documented for both process-compose and OpenShell |
| INST-02 | `rightclaw doctor` validates all dependencies present and functional | `verify_dependencies()` in deps.rs already checks 3 of 4 tools; needs `rightclaw` self-check |
| INST-03 | `rightclaw doctor` validates agent directory structure and policy files | Agent discovery code (`discover_agents()`) already validates structure; doctor can reuse |
| CHAN-01 | Per-agent Telegram channel via `.mcp.json` using official Claude Code Telegram plugin | Plugin requires Bun, `.mcp.json` format documented, `--channels` flag required on launch |
| CHAN-02 | Default "Right" agent BOOTSTRAP.md includes Telegram bot setup as part of onboarding | Per D-05: Telegram is NOT in BOOTSTRAP.md; setup happens in `rightclaw init` pre-launch |
| CHAN-03 | OpenShell policy templates include `api.telegram.org` in network allowlist | Network policy schema documented; conditional inclusion based on token presence |
</phase_requirements>

## Standard Stack

### Core (already in Cargo.toml)
| Library | Version | Purpose | Why Standard |
|---------|---------|---------|--------------|
| clap | 4.6 | CLI with subcommands, env vars | Already used; `Doctor` variant added to Commands enum |
| miette | 7.6 | User-facing diagnostics | Already used; doctor output benefits from structured diagnostics |
| serde_json | 1.0 | JSON serialization for .mcp.json | Already in workspace deps |
| minijinja | 2.18 | Template rendering | Already used for shell wrapper and process-compose.yaml |
| which | 7.0 | PATH binary detection | Already used in `verify_dependencies()` |

### No New Crate Dependencies Required

Phase 3 adds no new Rust crate dependencies. All needed functionality exists:
- `serde_json` for `.mcp.json` generation
- `which` for doctor binary checks
- `std::io` for interactive stdin prompts
- `regex` pattern matching for token validation is simple enough with string operations (no regex crate needed -- just check `split(':')` produces two parts: numeric and alphanumeric)

### External Dependencies (Not Rust)
| Tool | Install Method | Version |
|------|---------------|---------|
| process-compose | `curl get-pc.sh \| sh -b ~/.local/bin` | v1.100.0+ |
| OpenShell | `curl install.sh \| OPENSHELL_VERSION=v0.0.13 sh` | v0.0.13 |
| Claude Code CLI | User pre-installs | latest |
| Bun | `curl bun.sh/install \| bash` | latest (required for Telegram plugin) |

## Architecture Patterns

### Files Modified/Created in This Phase

```
templates/
  right/
    IDENTITY.md          # EXISTS -- needs start_prompt for BOOTSTRAP awareness
    SOUL.md              # EXISTS -- minor update for BOOTSTRAP-generated version
    AGENTS.md            # EXISTS -- no changes
    policy.yaml          # EXISTS (placeholder) -- FULL REWRITE with production policy
    BOOTSTRAP.md         # NEW -- onboarding conversation template
    policy-telegram.yaml # NEW -- variant with Telegram network rule uncommented
    mcp.json.j2          # NEW -- minijinja template for .mcp.json generation

crates/rightclaw/src/
    init.rs              # MODIFY -- add telegram_token param, .mcp.json generation, BOOTSTRAP.md
    doctor.rs            # NEW -- doctor command logic

crates/rightclaw-cli/src/
    main.rs              # MODIFY -- add Doctor subcommand, --telegram-token on Init

install.sh               # NEW -- bash install script at repo root
```

### Pattern 1: Extending init_rightclaw_home() with Telegram Token

**What:** Add optional `telegram_token: Option<&str>` parameter to `init_rightclaw_home()`. If token present, generate `.mcp.json` in agent dir and use the Telegram-enabled policy.yaml variant.

**When to use:** Keeps the single-responsibility pattern -- init creates all files atomically.

**Example:**
```rust
// crates/rightclaw/src/init.rs
const DEFAULT_BOOTSTRAP: &str = include_str!("../../../templates/right/BOOTSTRAP.md");
const DEFAULT_POLICY: &str = include_str!("../../../templates/right/policy.yaml");
const DEFAULT_POLICY_TELEGRAM: &str = include_str!("../../../templates/right/policy-telegram.yaml");
const MCP_TEMPLATE: &str = include_str!("../../../templates/right/mcp.json.j2");

pub fn init_rightclaw_home(home: &Path, telegram_token: Option<&str>) -> miette::Result<()> {
    // ... existing dir creation ...

    // Choose policy variant based on telegram token
    let policy = if telegram_token.is_some() {
        DEFAULT_POLICY_TELEGRAM
    } else {
        DEFAULT_POLICY
    };

    // Write .mcp.json if telegram configured
    if let Some(token) = telegram_token {
        let mcp_json = render_mcp_json(token)?;
        let mcp_path = agents_dir.join(".mcp.json");
        std::fs::write(&mcp_path, mcp_json)?;
    }

    // Add BOOTSTRAP.md to file list
    // ...
}
```

### Pattern 2: Two Policy Variants (Not Runtime Templating)

**What:** Ship two static policy.yaml files -- one with Telegram network rule, one with it commented out. Select at init time. Do NOT use minijinja for policy.yaml generation -- the policy file must be human-readable with comments intact, and YAML comments don't survive serialization/deserialization.

**Why:** YAML comments are the primary documentation mechanism (D-14, D-15). Template engines strip comments. Static files preserve them.

**Recommendation:** Use two nearly-identical files. The only difference is whether the `telegram_api` network policy entry is commented or uncommented. Keep them in sync manually -- they're small and critical enough to warrant duplication over template complexity.

**Alternative considered:** Single file with minijinja `{% if %}` blocks. Rejected because the output YAML needs to contain the commented-out version as documentation, which is awkward to template.

### Pattern 3: Doctor Command Reusing verify_dependencies()

**What:** `rightclaw doctor` runs all checks and prints a report, vs `verify_dependencies()` which fails fast on first missing tool. Doctor should run ALL checks and collect results.

```rust
// crates/rightclaw/src/doctor.rs
pub struct DoctorCheck {
    pub name: &'static str,
    pub status: CheckStatus,
    pub detail: String,
    pub fix: Option<String>,
}

pub enum CheckStatus {
    Pass,
    Fail,
    Warn,
}

pub fn run_doctor(home: &Path) -> Vec<DoctorCheck> {
    let mut checks = Vec::new();

    // Binary checks
    checks.push(check_binary("rightclaw", None));
    checks.push(check_binary("process-compose",
        Some("https://f1bonacc1.github.io/process-compose/installation/")));
    checks.push(check_binary("openshell",
        Some("curl -LsSf https://raw.githubusercontent.com/NVIDIA/OpenShell/main/install.sh | sh")));
    checks.push(check_binary("claude",
        Some("https://docs.anthropic.com/en/docs/claude-code")));

    // Agent structure checks
    checks.extend(check_agent_structure(home));

    checks
}
```

### Pattern 4: Interactive Terminal Prompt (No New Dependencies)

**What:** Use `std::io::stdin()` for the interactive Telegram token prompt. No need for dialoguer/inquire crates for a single yes/no + text input.

```rust
fn prompt_telegram_token() -> miette::Result<Option<String>> {
    use std::io::{self, Write};

    print!("Set up Telegram channel? (paste bot token or press Enter to skip): ");
    io::stdout().flush()?;

    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let token = input.trim();

    if token.is_empty() {
        return Ok(None);
    }

    validate_telegram_token(token)?;
    Ok(Some(token.to_string()))
}

fn validate_telegram_token(token: &str) -> miette::Result<()> {
    let parts: Vec<&str> = token.splitn(2, ':').collect();
    if parts.len() != 2
        || !parts[0].chars().all(|c| c.is_ascii_digit())
        || parts[1].is_empty()
    {
        return Err(miette::miette!(
            help = "Token should look like: 123456789:AAHfiqksKZ8WIB...",
            "Invalid Telegram bot token format"
        ));
    }
    Ok(())
}
```

### Pattern 5: Shell Wrapper Channels Flag

**What:** The agent wrapper template must conditionally include `--channels` when `.mcp.json` exists in the agent directory. This requires updating `agent-wrapper.sh.j2` and passing `has_mcp_config` / `mcp_config_path` to the template context.

**Critical detail:** Per Claude Code docs, `--channels plugin:telegram@claude-plugins-official` must be passed at `claude` launch time. Being in `.mcp.json` alone configures the MCP server tools but does NOT activate channel event delivery.

```bash
# agent-wrapper.sh.j2 (updated)
{% if not no_sandbox %}
exec openshell sandbox create \
  --policy "{{ policy_path }}" \
  --name "rightclaw-{{ agent_name }}" \
  -- claude \
    --append-system-prompt-file "{{ identity_path }}" \
    --dangerously-skip-permissions \
    {% if channels %}--channels {{ channels }} \{% endif %}
    --prompt "{{ start_prompt }}"
{% else %}
exec claude \
  --append-system-prompt-file "{{ identity_path }}" \
  --dangerously-skip-permissions \
  {% if channels %}--channels {{ channels }} \{% endif %}
  --prompt "{{ start_prompt }}"
{% endif %}
```

The `channels` variable would be set from the agent's `.mcp.json` detection or from `agent.yaml` configuration. For now, the Telegram channel identifier is `plugin:telegram@claude-plugins-official`.

### Anti-Patterns to Avoid

- **Generating policy.yaml via serde serialization:** Comments are lost. Use static template files.
- **Putting Telegram setup in BOOTSTRAP.md:** MCP servers load at session start. Cannot add mid-session (D-05).
- **Using regex crate for token validation:** Overkill for "digits:alphanumeric" check. String operations suffice.
- **Making doctor fail-fast like verify_dependencies():** Doctor should report ALL issues, not stop at the first.
- **Hard-coding binary install paths in install.sh:** Use `~/.local/bin` default with override, like process-compose's script.

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| process-compose installation | Custom download logic | Official `get-pc.sh` script | Handles platform detection, checksums, versions |
| OpenShell installation | Custom download logic | Official `install.sh` script | Handles platform detection, version pinning |
| Telegram bot token storage | Custom credential management | Write to `~/.claude/channels/telegram/.env` | Standard Claude Code pattern |
| MCP server configuration | Custom config format | Standard `.mcp.json` with `mcpServers` | Claude Code reads this natively |
| Interactive prompts (complex) | Custom TUI framework | `std::io::stdin()` | Single prompt, no need for dialoguer |

**Key insight:** This phase has exactly zero cases requiring third-party Rust crates. All the "don't hand-roll" items are either shell scripts delegating to official installers, or standard file formats (JSON, YAML) that existing deps handle.

## Common Pitfalls

### Pitfall 1: .mcp.json Without --channels Doesn't Deliver Messages
**What goes wrong:** You write a perfect `.mcp.json` with the Telegram MCP server config, but channel messages never arrive.
**Why it happens:** Claude Code requires both `.mcp.json` (configures the MCP server/tools) AND `--channels plugin:telegram@claude-plugins-official` (activates channel event delivery). They are separate systems.
**How to avoid:** Shell wrapper template must include the `--channels` flag when Telegram is configured. Agent discovery should detect `.mcp.json` presence and set the channels flag accordingly.
**Warning signs:** MCP tools (reply, react, edit_message) appear in Claude's toolset but the bot doesn't respond to Telegram messages.

### Pitfall 2: Telegram Plugin Requires Bun Runtime
**What goes wrong:** install.sh installs rightclaw, process-compose, and OpenShell but Telegram channel doesn't work.
**Why it happens:** The official Claude Code Telegram plugin runs on Bun, not Node.js. If Bun isn't installed, the MCP server fails to start.
**How to avoid:** install.sh should check for `bun` if user provides a Telegram token. Doctor command should check for `bun` with appropriate warning.
**Warning signs:** MCP server crash on startup with "command not found: bun".

### Pitfall 3: Telegram Plugin Must Be Pre-Installed Before Session
**What goes wrong:** `.mcp.json` references the Telegram plugin but it hasn't been installed via `/plugin install telegram@claude-plugins-official`.
**Why it happens:** Plugins need to be installed through Claude Code's plugin system before they can be referenced.
**How to avoid:** The BOOTSTRAP.md first-run or install.sh should guide the user to install the plugin. Alternatively, document that `rightclaw init` output includes "Run `/plugin install telegram@claude-plugins-official` in Claude Code before first launch."
**Warning signs:** Claude Code reports "plugin not found" at startup.

### Pitfall 4: Policy YAML Comments Lost Through Serialization
**What goes wrong:** Generating policy.yaml through serde_saphyr/serde_json strips all comments.
**Why it happens:** YAML comments are not part of the data model -- they exist only in the text representation. Any serialize-deserialize round trip destroys them.
**How to avoid:** Ship policy.yaml as static text files, not generated output. Use `include_str!` to embed them. The two variants (with/without Telegram) are nearly identical and can be maintained as separate files.
**Warning signs:** policy.yaml output has no comments despite D-14 requiring comprehensive documentation.

### Pitfall 5: OpenShell Filesystem Paths Must Be Absolute
**What goes wrong:** policy.yaml uses relative paths like `./agents/right/` and OpenShell rejects the policy.
**Why it happens:** OpenShell validation requires every path to start with `/`. No `..` traversal components allowed.
**How to avoid:** Use `include_workdir: true` for the agent's working directory. Explicitly list absolute paths like `/usr`, `/lib`, `/etc` for read-only access.
**Warning signs:** `openshell sandbox create` returns validation error about paths.

### Pitfall 6: install.sh Runs rightclaw init Before Binary is in PATH
**What goes wrong:** install.sh downloads rightclaw binary to `~/.local/bin/` then immediately calls `rightclaw init`, but the shell's PATH hasn't been refreshed.
**Why it happens:** In many shells, modifying `~/.local/bin/` doesn't take effect until the next shell session or `hash -r` is called.
**How to avoid:** In install.sh, call the binary by its full path (`~/.local/bin/rightclaw init`) rather than relying on PATH resolution. Or run `hash -r` before the call.
**Warning signs:** "rightclaw: command not found" immediately after successful download.

## Code Examples

### OpenShell Policy YAML (Production Quality)
Source: [NVIDIA OpenShell Policy Schema Reference](https://docs.nvidia.com/openshell/latest/reference/policy-schema.html)

```yaml
# OpenShell sandbox policy for the Right agent
# =============================================
#
# This file defines what the agent can access. OpenShell enforces these
# restrictions at the kernel level using Landlock LSM and seccomp BPF.
#
# Sections:
#   filesystem_policy  - Which directories the agent can read/write (STATIC)
#   landlock           - Kernel enforcement mode (STATIC)
#   process            - OS-level user/group identity (STATIC)
#   network_policies   - Allowed outbound connections (DYNAMIC - hot-reloadable)
#
# Static sections lock at sandbox creation. To change them, destroy and
# recreate the sandbox. Dynamic sections can be updated with:
#   openshell policy set <sandbox> --policy policy.yaml --wait
#
# Learn more: https://docs.nvidia.com/openshell/latest/sandboxes/policies.html

version: 1

# ── Filesystem ──────────────────────────────────────────────────────
#
# Paths not listed here are completely inaccessible.
# Every path must be absolute (start with /).
#
# HOW TO: Allow read-only access to a directory
#   Add the path to the read_only list:
#     read_only:
#       - /path/to/dir
#
# HOW TO: Allow read-write access to a directory
#   Add the path to the read_write list:
#     read_write:
#       - /path/to/dir
#
# HOW TO: Give broad filesystem access (SECURITY WARNING)
#   read_only: [/]
#   read_write: [/home]
#   This is NOT recommended. Use the narrowest scope possible.
#
filesystem_policy:
  # Automatically includes the agent's working directory as read-write.
  # This is the agent's own directory (e.g., ~/.rightclaw/agents/right/).
  include_workdir: true

  read_only:
    - /usr           # System binaries, libraries
    - /lib           # Shared libraries
    - /lib64         # 64-bit shared libraries (Linux)
    - /etc           # System configuration (read-only)
    - /proc          # Process information
    - /dev/urandom   # Random number generator
    - /dev/null      # Null device

  read_write:
    - /tmp           # Temporary files

# ── Landlock ────────────────────────────────────────────────────────
#
# Landlock is a Linux Security Module that enforces filesystem restrictions
# at the kernel level. Two modes:
#
#   best_effort      - Uses highest Landlock ABI the kernel supports.
#                      On older kernels, some restrictions may not apply.
#                      SILENTLY DEGRADES -- you may think you're sandboxed
#                      when you're not.
#
#   hard_requirement  - Fails if the kernel doesn't support the required
#                       Landlock ABI level. No silent degradation.
#                       Requires Linux kernel 6.4+ for full filesystem +
#                       network enforcement.
#
# RightClaw uses hard_requirement by default. If your kernel is too old,
# you'll get a clear error instead of false security.
#
landlock:
  compatibility: hard_requirement

# ── Process ─────────────────────────────────────────────────────────
#
# OS-level identity for the agent process inside the sandbox.
# Cannot be root (UID 0) -- OpenShell rejects this.
#
process:
  run_as_user: sandbox
  run_as_group: sandbox

# ── Network ─────────────────────────────────────────────────────────
#
# Default-deny: all outbound connections are blocked unless explicitly allowed.
# Each entry names the endpoints and binaries that may connect.
#
# HOW TO: Allow all outbound network access (NOT RECOMMENDED)
#   Replace the entries below with:
#     allow_all:
#       name: allow-all-outbound
#       endpoints:
#         - host: "*"
#           port: 443
#       binaries:
#         - path: "**"
#
# HOW TO: Add a new API endpoint
#   Add a new entry under network_policies:
#     my_api:
#       name: my-api
#       endpoints:
#         - host: api.example.com
#           port: 443
#           protocol: rest
#           tls: terminate
#           enforcement: enforce
#           access: full        # or read-only, read-write
#       binaries:
#         - path: /usr/local/bin/claude
#
# HOW TO: Allow npm/pip package installation
#   npm_registry:
#     name: npm
#     endpoints:
#       - { host: registry.npmjs.org, port: 443 }
#     binaries:
#       - { path: /usr/bin/npm }
#       - { path: /usr/bin/node }
#
network_policies:
  github_api:
    name: github-api
    endpoints:
      - host: api.github.com
        port: 443
        protocol: rest
        tls: terminate
        enforcement: enforce
        access: read-only
    binaries:
      - { path: /usr/local/bin/claude }
      - { path: /usr/bin/curl }
      - { path: /usr/bin/gh }

  # ── Telegram (uncomment if you set up a Telegram bot) ───────────
  # telegram_api:
  #   name: telegram-bot-api
  #   endpoints:
  #     - host: api.telegram.org
  #       port: 443
  #       protocol: rest
  #       tls: terminate
  #       enforcement: enforce
  #       access: full
  #   binaries:
  #     - { path: "**" }  # Telegram plugin runs via Bun
```

### .mcp.json for Telegram Plugin
Source: [Claude Code Telegram Plugin README](https://github.com/anthropics/claude-plugins-official/blob/main/external_plugins/telegram/README.md)

```json
{
  "mcpServers": {
    "telegram": {
      "type": "stdio",
      "command": "bun",
      "args": ["${CLAUDE_PLUGIN_ROOT}/servers/telegram.js"],
      "env": {
        "TELEGRAM_BOT_TOKEN": "<token>"
      }
    }
  }
}
```

**Important clarification:** The standard Telegram channel setup does NOT use a project-level `.mcp.json`. It uses the plugin system (`/plugin install telegram@claude-plugins-official`) which manages its own MCP configuration. The token is stored in `~/.claude/channels/telegram/.env`. The `.mcp.json` approach would be for a custom/non-plugin MCP server.

**Revised approach for RightClaw:** Since the official path is plugin-based, RightClaw should:
1. During `rightclaw init`, store the token to `~/.claude/channels/telegram/.env` (creating dirs as needed)
2. Document that user must run `/plugin install telegram@claude-plugins-official` in Claude Code
3. Shell wrapper includes `--channels plugin:telegram@claude-plugins-official` when Telegram is configured
4. Skip `.mcp.json` generation entirely -- let the plugin system handle MCP config

**Alternative:** Write `.mcp.json` in the agent directory with the `bun` command pointing to the installed plugin location. This is fragile because plugin install paths vary by OS/method. The plugin system exists precisely to solve this.

### BOOTSTRAP.md Template
Source: [OpenClaw BOOTSTRAP.md Template](https://docs.openclaw.ai/reference/templates/BOOTSTRAP)

```markdown
---
summary: "First-run onboarding for Right agent"
---

# Hey. I just came online.

*You just woke up. Time to figure out who you are.*

There's no memory yet. This is a fresh workspace. That's normal -- everything starts somewhere.

## The Conversation

Don't interrogate. Don't be robotic. Just... talk.

Start with something like:

> "Hey! I just came online and I'm a blank slate. Who are you, and who should I be?"

Then figure out together:

1. **Your name** -- What should they call you? (Right is the default, but maybe something else fits better)
2. **Your nature** -- What kind of creature are you? AI assistant is fine, but maybe you're something weirder -- a familiar, a daemon, a ghost in the machine?
3. **Your vibe** -- Formal? Casual? Snarky? Warm? Chaotic? What feels right?
4. **Your emoji** -- Everyone needs a signature. Offer suggestions if they're stuck. Have fun with it.

Offer suggestions if they seem stuck. This is not a form -- it's a conversation.

## After You Know Who You Are

Update these files with what you learned:

- **IDENTITY.md** -- Rewrite it with your name, creature type, vibe, and emoji. Keep the structure but make it yours.
- **USER.md** -- Create this file with: their name, how to address them, timezone if mentioned, any preferences they shared.
- **SOUL.md** -- Rewrite your personality section based on the vibe they chose. Keep the core principles but adapt the tone.

Write it down. Make it real.

## When You're Done

Delete this file. You don't need a bootstrap script anymore -- you're you now.

---

*Good luck out there. Make it count.*
```

### install.sh Platform Detection Pattern
Source: Community patterns from multiple open-source projects

```bash
#!/usr/bin/env bash
set -euo pipefail

# Detect OS
OS="$(uname -s)"
case "$OS" in
  Linux)  PLATFORM="linux" ;;
  Darwin) PLATFORM="darwin" ;;
  *)      echo "Unsupported OS: $OS" >&2; exit 1 ;;
esac

# Detect architecture
ARCH="$(uname -m)"
case "$ARCH" in
  x86_64|amd64)   ARCH="x86_64" ;;
  arm64|aarch64)   ARCH="aarch64" ;;
  *)               echo "Unsupported architecture: $ARCH" >&2; exit 1 ;;
esac

# Construct download URL
RIGHTCLAW_VERSION="${RIGHTCLAW_VERSION:-latest}"
TARBALL="rightclaw-${ARCH}-unknown-${PLATFORM}-musl.tar.gz"
# ...
```

### Doctor Command Output Format
```
rightclaw doctor

  rightclaw    ok   v0.1.0  ~/.local/bin/rightclaw
  process-compose    ok   v1.100.0  ~/.local/bin/process-compose
  openshell    ok   v0.0.13  ~/.local/bin/openshell
  claude       ok   v2.1.80  /usr/local/bin/claude
  bun          warn v1.2.0   ~/.bun/bin/bun (optional, needed for Telegram)

  agents/right/
    IDENTITY.md     ok
    policy.yaml     ok
    BOOTSTRAP.md    ok   (first-run onboarding pending)

  4/4 required tools found
  1 agent valid
```

## State of the Art

| Old Approach | Current Approach | When Changed | Impact |
|--------------|------------------|--------------|--------|
| OpenClaw manages MCP via `.mcp.json` manually | Claude Code plugin system with `/plugin install` | March 2026 | Telegram uses plugin system, not raw `.mcp.json` |
| Telegram MCP via Node.js | Telegram plugin requires Bun runtime | March 2026 | install.sh should warn about Bun if Telegram configured |
| OpenShell `best_effort` Landlock | RightClaw uses `hard_requirement` | Phase 3 decision | Older kernels (<6.4) get clear error instead of silent degradation |
| OpenClaw BOOTSTRAP.md with WhatsApp/Telegram channel setup | RightClaw separates channel setup from onboarding | Phase 3 decision (D-05) | MCP servers can't be added mid-session; init handles it |

## Open Questions

1. **Plugin install path for Telegram**
   - What we know: `/plugin install telegram@claude-plugins-official` installs it. The plugin stores its MCP config internally.
   - What's unclear: Exact filesystem path where the installed plugin lands, and whether `--channels` can reference it when `claude` is launched from inside an OpenShell sandbox (the Bun binary and plugin files must be accessible inside the sandbox).
   - Recommendation: Test this empirically. The sandbox's `read_only` paths may need to include Bun's install directory and Claude Code's plugin directory (`~/.claude/plugins/`). Add these paths to policy.yaml.

2. **Bun binary path inside sandbox**
   - What we know: Bun installs to `~/.bun/bin/bun` by default. The Telegram plugin MCP server is spawned by Claude Code using Bun.
   - What's unclear: Whether OpenShell's filesystem policy needs to explicitly allow access to `~/.bun/` for the Telegram plugin to work inside the sandbox.
   - Recommendation: Add `~/.bun` to `read_only` in policy.yaml. Add `~/.claude` to `read_only` for plugin access.

3. **install.sh: rightclaw binary distribution**
   - What we know: OpenShell and process-compose have official install scripts. RightClaw needs its own binary on GitHub Releases.
   - What's unclear: The repo doesn't have CI/CD for building release binaries yet. install.sh can't download from GitHub Releases if no releases exist.
   - Recommendation: install.sh should support building from source as fallback (`cargo install --path crates/rightclaw-cli`). Release binary pipeline is a separate concern.

## Sources

### Primary (HIGH confidence)
- [NVIDIA OpenShell Policy Schema Reference](https://docs.nvidia.com/openshell/latest/reference/policy-schema.html) -- complete YAML schema, field types, validation rules
- [NVIDIA OpenShell Sandbox Policies](https://docs.nvidia.com/openshell/latest/sandboxes/policies.html) -- practical examples, workflow
- [Claude Code Channels Documentation](https://code.claude.com/docs/en/channels) -- `--channels` flag, plugin flow, security model
- [Claude Code MCP Documentation](https://code.claude.com/docs/en/mcp) -- `.mcp.json` format, scopes
- [Claude Code Telegram Plugin README](https://github.com/anthropics/claude-plugins-official/blob/main/external_plugins/telegram/README.md) -- setup steps, environment variables, tools
- [Process Compose Installation](https://f1bonacc1.github.io/process-compose/installation/) -- official install script
- [OpenShell GitHub Releases](https://github.com/NVIDIA/OpenShell/releases) -- v0.0.13, binary names, install script

### Secondary (MEDIUM confidence)
- [OpenClaw BOOTSTRAP.md Template](https://docs.openclaw.ai/reference/templates/BOOTSTRAP) -- onboarding format, self-delete pattern
- [OpenClaw community BOOTSTRAP.md example](https://github.com/seedprod/openclaw-prompts-and-skills/blob/main/BOOTSTRAP.md) -- actual content verified

### Tertiary (LOW confidence)
- Plugin filesystem paths (varies by OS and install method -- needs empirical testing)
- Bun/plugin accessibility inside OpenShell sandbox (not documented, needs testing)

## Metadata

**Confidence breakdown:**
- Standard stack: HIGH -- no new dependencies, extends existing patterns
- Architecture (init, doctor, policy): HIGH -- OpenShell schema fully documented, existing code patterns clear
- Architecture (Telegram plugin integration): MEDIUM -- plugin system is well-documented but sandbox interaction with Bun/plugins needs empirical testing
- Pitfalls: HIGH -- critical `.mcp.json` vs `--channels` distinction confirmed from official docs
- Install script: HIGH -- standard bash patterns, official installer scripts for dependencies documented

**Research date:** 2026-03-22
**Valid until:** 2026-04-22 (30 days -- OpenShell alpha may introduce breaking changes sooner)
