# Stack Research: v2.0 Native Sandbox & Agent Isolation

**Domain:** Claude Code native sandboxing integration, per-agent HOME isolation
**Researched:** 2026-03-23
**Confidence:** HIGH (official docs verified)

This research covers ONLY what's new for v2.0. The existing stack (clap, tokio, reqwest, serde, serde-saphyr, minijinja, miette+thiserror, tracing, process-compose) is validated and unchanged.

## What Changes in v2.0

### Removed: OpenShell

All OpenShell code paths are removed. No more `openshell` binary dependency, no more `sandbox create/delete`, no more `policy.yaml` parsing.

**Affected codebase:**
- `runtime/sandbox.rs` — `destroy_sandboxes()` calls `openshell sandbox delete` (remove entirely)
- `runtime/deps.rs` — `verify_dependencies()` checks for `openshell` (replace with `bwrap`/`socat`)
- `doctor.rs` — `run_doctor()` checks `openshell` binary (replace with `bwrap`/`socat`)
- `codegen/shell_wrapper.rs` — generates `openshell sandbox create` command (replace with `HOME=` + `CLAUDE_CONFIG_DIR=`)
- `templates/agent-wrapper.sh.j2` — template references `openshell` (rewrite)
- `agent/types.rs` — `AgentDef.policy_path` (remove, replace with settings.json generation)
- `agent/discovery.rs` — validates `policy.yaml` exists (remove requirement)
- `runtime/sandbox.rs` — `RuntimeState.no_sandbox`, `AgentState.sandbox_name` (simplify)

### Added: CC Native Sandbox via `settings.json`

Claude Code has built-in OS-level sandboxing since mid-2025. On Linux it uses bubblewrap for filesystem isolation and socat for network proxy communication. On macOS it uses Seatbelt (works out of the box, no deps).

RightClaw generates a per-agent `.claude/settings.json` inside each agent's HOME directory to configure the sandbox.

### Added: Per-Agent HOME Isolation via `CLAUDE_CONFIG_DIR`

Claude Code officially supports `CLAUDE_CONFIG_DIR` (documented in env vars page). This redirects where CC stores its config and data files. Combined with setting the cwd to the agent dir, this gives full isolation.

**Strategy:**
```
CLAUDE_CONFIG_DIR=~/.rightclaw/agents/<name>/.claude claude --cwd <agent-dir>
```

This is BETTER than `HOME=<agent-dir>` because:
1. `CLAUDE_CONFIG_DIR` only redirects CC's config, not all home-relative paths
2. Shell tools like `git`, `ssh`, `cargo` still find `~/.gitconfig`, `~/.ssh/`, etc.
3. No risk of breaking tools that depend on `$HOME`

However, `CLAUDE_CONFIG_DIR` has known bugs (creates local `.claude/` dirs, IDE integration issues). Fallback plan: `HOME=<agent-dir>` works universally but is a blunt instrument.

**Recommendation: Use `CLAUDE_CONFIG_DIR` as primary, with `HOME` override as `--legacy-isolation` flag.**

## CC Sandbox `settings.json` Schema

**Confidence: HIGH** — Verified against [official settings docs](https://code.claude.com/docs/en/settings).

The complete sandbox section of `settings.json`:

```json
{
  "sandbox": {
    "enabled": true,
    "autoAllowBashIfSandboxed": true,
    "excludedCommands": ["docker"],
    "allowUnsandboxedCommands": false,
    "enableWeakerNestedSandbox": false,
    "enableWeakerNetworkIsolation": false,
    "filesystem": {
      "allowWrite": ["/tmp/build", "~/.kube"],
      "denyWrite": ["/etc", "/usr/local/bin"],
      "denyRead": ["~/.aws/credentials"],
      "allowRead": ["."],
      "allowManagedReadPathsOnly": false
    },
    "network": {
      "allowedDomains": ["github.com", "*.npmjs.org"],
      "allowUnixSockets": ["/var/run/docker.sock"],
      "allowAllUnixSockets": false,
      "allowLocalBinding": false,
      "allowManagedDomainsOnly": false,
      "httpProxyPort": 8080,
      "socksProxyPort": 8081
    }
  }
}
```

### Field Reference

| Field | Type | Default | Purpose |
|-------|------|---------|---------|
| `sandbox.enabled` | bool | `false` | Enable bash sandboxing (macOS, Linux, WSL2) |
| `sandbox.autoAllowBashIfSandboxed` | bool | `true` | Auto-approve bash commands when sandboxed |
| `sandbox.excludedCommands` | string[] | `[]` | Commands that run OUTSIDE the sandbox |
| `sandbox.allowUnsandboxedCommands` | bool | `true` | Allow `dangerouslyDisableSandbox` escape hatch. Set `false` for strict mode |
| `sandbox.enableWeakerNestedSandbox` | bool | `false` | Weaker sandbox for Docker (Linux/WSL2 only). Reduces security |
| `sandbox.enableWeakerNetworkIsolation` | bool | `false` | macOS only: allow system TLS trust service. Needed for Go tools (gh, terraform) |
| `sandbox.filesystem.allowWrite` | string[] | `[]` | Additional paths for write access (merged across scopes) |
| `sandbox.filesystem.denyWrite` | string[] | `[]` | Paths to deny write access (merged across scopes) |
| `sandbox.filesystem.denyRead` | string[] | `[]` | Paths to deny read access (merged across scopes) |
| `sandbox.filesystem.allowRead` | string[] | `[]` | Re-allow reads within denyRead regions (takes precedence) |
| `sandbox.filesystem.allowManagedReadPathsOnly` | bool | `false` | Managed-only: ignore user/project allowRead |
| `sandbox.network.allowedDomains` | string[] | `[]` | Allowed outbound domains. Supports `*` wildcards |
| `sandbox.network.allowUnixSockets` | string[] | `[]` | Unix socket paths accessible in sandbox |
| `sandbox.network.allowAllUnixSockets` | bool | `false` | Allow all Unix socket connections |
| `sandbox.network.allowLocalBinding` | bool | `false` | Allow binding to localhost ports (macOS only) |
| `sandbox.network.allowManagedDomainsOnly` | bool | `false` | Managed-only: block non-allowed domains without prompting |
| `sandbox.network.httpProxyPort` | int | (auto) | Custom HTTP proxy port (BYO proxy) |
| `sandbox.network.socksProxyPort` | int | (auto) | Custom SOCKS5 proxy port (BYO proxy) |

### Path Prefix Rules

| Prefix | Meaning | Example |
|--------|---------|---------|
| `/` | Absolute path | `/tmp/build` |
| `~/` | Home-relative | `~/.kube` |
| `./` or bare | Relative to project root (project settings) or `~/.claude` (user settings) | `./output` |

Arrays MERGE across all settings scopes (user, project, managed) -- they are concatenated, not replaced.

### RightClaw's Generated `settings.json` per Agent

RightClaw should generate a `settings.json` inside each agent's config directory with:

```json
{
  "permissions": {
    "allow": [
      "Bash(*)",
      "Read(*)",
      "Edit(*)",
      "Write(*)"
    ],
    "defaultMode": "bypassPermissions"
  },
  "sandbox": {
    "enabled": true,
    "autoAllowBashIfSandboxed": true,
    "allowUnsandboxedCommands": false,
    "filesystem": {
      "allowWrite": [
        "~/.rightclaw/agents/<agent-name>/",
        "/tmp"
      ],
      "denyRead": [
        "~/.ssh",
        "~/.aws",
        "~/.gnupg"
      ]
    },
    "network": {
      "allowedDomains": [
        "api.anthropic.com",
        "github.com",
        "*.githubusercontent.com",
        "registry.npmjs.org"
      ]
    }
  }
}
```

**Key decisions:**
1. `allowUnsandboxedCommands: false` -- strict mode, no escape hatch
2. `autoAllowBashIfSandboxed: true` -- agents run autonomously, no prompts
3. `defaultMode: "bypassPermissions"` -- replaces `--dangerously-skip-permissions` flag
4. Deny reads to credential directories by default
5. Users can extend via `agent.yaml` sandbox config

### Integration with `--dangerously-skip-permissions`

`--dangerously-skip-permissions` is equivalent to `--permission-mode bypassPermissions`. The sandbox STILL ENFORCES even in bypass mode -- this is the entire point. Bypass mode + sandbox = autonomous agent with OS-level guardrails.

The `settings.json` can set `defaultMode: "bypassPermissions"` to avoid needing the CLI flag entirely. Combined with `skipDangerousModePermissionPrompt: true` in the CC global config (already handled by RightClaw v1.0 trust setup), this enables fully autonomous startup.

## External Dependencies

### bubblewrap (Linux only)

| Distro | Package | Install Command |
|--------|---------|-----------------|
| Ubuntu/Debian | `bubblewrap` | `sudo apt-get install bubblewrap` |
| Fedora/RHEL | `bubblewrap` | `sudo dnf install bubblewrap` |
| Arch Linux | `bubblewrap` | `sudo pacman -S bubblewrap` |
| Alpine Linux | `bubblewrap` | `sudo apk add bubblewrap` |
| openSUSE | `bubblewrap` | `sudo zypper install bubblewrap` |
| NixOS/nix | `bubblewrap` | Available in nixpkgs |

**Binary name:** `bwrap`
**What it does:** Low-level unprivileged sandboxing. Creates isolated mount/network/PID namespaces. Used by Flatpak. CC's sandbox runtime invokes `bwrap` to create a namespace where the filesystem is read-only except allowed paths, and the network namespace is removed entirely (forcing traffic through socat proxy).
**Kernel requirement:** User namespaces must be enabled (default on modern kernels, NOT available on WSL1).
**Latest version:** 0.11.0 (stable, widely packaged).

### socat (Linux only)

| Distro | Package | Install Command |
|--------|---------|-----------------|
| Ubuntu/Debian | `socat` | `sudo apt-get install socat` |
| Fedora/RHEL | `socat` | `sudo dnf install socat` |
| Arch Linux | `socat` | `sudo pacman -S socat` |
| Alpine Linux | `socat` | `sudo apk add socat` |
| openSUSE | `socat` | `sudo zypper install socat` |
| NixOS/nix | `socat` | Available in nixpkgs |

**Binary name:** `socat`
**What it does:** Multipurpose relay (SOcket CAT). CC uses it to bridge Unix domain sockets between the sandboxed namespace and the host's network proxy. Since bubblewrap removes the network namespace entirely, all traffic must flow through a Unix socket to a proxy running on the host. socat handles this relay.
**Latest version:** 1.8.1.1 (2026-02-12).

### macOS: No Additional Dependencies

Seatbelt is built into macOS. No `brew install` needed. CC sandbox works out of the box on macOS.

## Rust Crate Changes

### No New Crates Needed

The v2.0 changes are primarily about:
1. **Generating JSON files** -- `serde_json` (already in workspace)
2. **Modifying shell wrapper template** -- `minijinja` (already in workspace)
3. **Updating dependency checks** -- `which` (already in workspace)
4. **File I/O** -- `std::fs` (stdlib)

No new Rust crate dependencies are required for v2.0.

### Crate Usage for New Features

| Feature | Crate | Already In Workspace |
|---------|-------|---------------------|
| Generate `settings.json` | `serde_json` + `serde` | Yes |
| Template new shell wrapper | `minijinja` | Yes |
| Check for `bwrap`/`socat` | `which` | Yes |
| Create agent `.claude/` dirs | `std::fs` | stdlib |
| Path manipulation | `std::path` | stdlib |

## Key Environment Variables

| Variable | Purpose | How RightClaw Uses It |
|----------|---------|----------------------|
| `CLAUDE_CONFIG_DIR` | Redirect CC config/data directory | Set to `~/.rightclaw/agents/<name>/.claude` per agent |
| `CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC` | Disable telemetry, autoupdater, etc. | Set in agent wrapper (reduces noise, prevents feature flag issues) |
| `CLAUDE_CODE_TMPDIR` | Override temp directory | Optional: isolate temp files per agent |
| `CLAUDE_CODE_DISABLE_CRON` | Disable CC's built-in cron | NOT set (we use CC cron via CronSync) |

**CRITICAL GOTCHA from v1.0 memory:** `CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC` set to ANY value (including "0" or "false") blocks ALL feature flags including channels. If Telegram channels are needed, do NOT set this variable.

## Shell Wrapper Changes

### v1.0 Wrapper (OpenShell)
```bash
exec openshell sandbox create \
  --no-auto-providers \
  --no-keep \
  --policy "policy.yaml" \
  --name "rightclaw-<agent>" \
  -- claude \
    --append-system-prompt-file "prompt.md" \
    --dangerously-skip-permissions \
    --channels plugin:telegram@claude-plugins-official \
    -- "startup prompt"
```

### v2.0 Wrapper (Native Sandbox)
```bash
export CLAUDE_CONFIG_DIR="$HOME/.rightclaw/agents/<agent>/.claude"
exec "$CLAUDE_BIN" \
  --append-system-prompt-file "prompt.md" \
  --dangerously-skip-permissions \
  --channels plugin:telegram@claude-plugins-official \
  -- "startup prompt"
```

**Differences:**
1. No `openshell` invocation at all
2. `CLAUDE_CONFIG_DIR` set to agent-specific `.claude/` directory
3. `--dangerously-skip-permissions` remains (sandbox enforces OS-level restrictions independently)
4. `settings.json` at `$CLAUDE_CONFIG_DIR/settings.json` configures sandbox
5. No `--no-sandbox` flag needed -- sandbox is controlled via `settings.json` `sandbox.enabled`
6. No cleanup needed on shutdown (no sandbox create/destroy lifecycle)

## Agent Directory Layout (v2.0)

```
~/.rightclaw/agents/<name>/
  IDENTITY.md              # Required (unchanged)
  SOUL.md                  # Optional personality
  USER.md                  # Optional user context
  AGENTS.md                # Optional operational framework
  MEMORY.md                # Optional persistent memory
  BOOTSTRAP.md             # Optional first-run (self-deletes)
  HEARTBEAT.md             # Optional health check
  TOOLS.md                 # Optional tool list
  agent.yaml               # Optional config (restart, model, etc.)
  .mcp.json                # Optional MCP servers
  crons/                   # CronSync specs
  .claude/                 # GENERATED by RightClaw
    settings.json          # Sandbox config + permissions
    settings.local.json    # Optional user overrides
    CLAUDE.md              # Optional agent-scoped instructions
```

**Key change:** `policy.yaml` is REMOVED. Replaced by `.claude/settings.json` (generated). The `.claude/` directory under each agent is where `CLAUDE_CONFIG_DIR` points. CC creates its own state files inside this directory (sessions, memory, etc.).

## What NOT to Use

| Avoid | Why | Use Instead |
|-------|-----|-------------|
| `HOME=<agent-dir>` as primary isolation | Breaks git, ssh, cargo, and any tool that reads `$HOME` | `CLAUDE_CONFIG_DIR` |
| OpenShell sandbox | Removed in v2.0. Alpha instability, API key requirement | CC native sandbox |
| `policy.yaml` | OpenShell format, not used by CC sandbox | `settings.json` with `sandbox.*` fields |
| Custom bwrap invocation | CC manages bwrap internally via sandbox runtime | Let CC handle bwrap via `sandbox.enabled: true` |
| `sandbox.allowUnsandboxedCommands: true` | Allows agents to escape sandbox via `dangerouslyDisableSandbox` param | Set to `false` for strict enforcement |

## Alternatives Considered

| Recommended | Alternative | When to Use Alternative |
|-------------|-------------|------------------------|
| `CLAUDE_CONFIG_DIR` | `HOME` override | If `CLAUDE_CONFIG_DIR` proves buggy for a specific CC version |
| `settings.json` sandbox | Manual bwrap wrapping | Never for RightClaw -- CC sandbox is more comprehensive (includes network proxy) |
| Per-agent `.claude/settings.json` | Global `~/.claude/settings.json` | Never -- agents must have independent sandbox configs |
| `defaultMode: "bypassPermissions"` in settings | `--dangerously-skip-permissions` flag | Flag is fine too, but settings-based is cleaner and avoids the startup permission dialog |

## Version Compatibility

| Component | Minimum Version | Why |
|-----------|-----------------|-----|
| Claude Code | v2.1+ | Sandbox features, `CLAUDE_CONFIG_DIR` support |
| Node.js | 22+ | Required by CC for some sandbox features |
| bubblewrap | 0.4+ | User namespace support (any distro-packaged version works) |
| socat | 1.7+ | Unix socket relay (any distro-packaged version works) |
| Linux kernel | 4.18+ | User namespace support for bubblewrap |

## Sources

- [Claude Code Sandboxing Docs](https://code.claude.com/docs/en/sandboxing) -- Complete sandbox reference, verified 2026-03-23 (HIGH confidence)
- [Claude Code Settings Reference](https://code.claude.com/docs/en/settings) -- Full settings.json schema including sandbox fields, verified 2026-03-23 (HIGH confidence)
- [Claude Code Environment Variables](https://code.claude.com/docs/en/env-vars) -- `CLAUDE_CONFIG_DIR` and all env vars, verified 2026-03-23 (HIGH confidence)
- [Anthropic Engineering: Claude Code Sandboxing](https://www.anthropic.com/engineering/claude-code-sandboxing) -- Architecture overview (MEDIUM confidence -- high-level, no implementation details)
- [sandbox-runtime npm package](https://github.com/anthropic-experimental/sandbox-runtime) -- Open source sandbox runtime, `@anthropic-ai/sandbox-runtime` (MEDIUM confidence)
- [bubblewrap GitHub](https://github.com/containers/bubblewrap) -- bubblewrap source and docs (HIGH confidence)
- [pkgs.org/download/bubblewrap](https://pkgs.org/download/bubblewrap) -- Package availability across distros (HIGH confidence)
- [pkgs.org/download/socat](https://pkgs.org/download/socat) -- socat package availability (HIGH confidence)
- [CLAUDE_CONFIG_DIR feature request](https://github.com/anthropics/claude-code/issues/25762) -- Community confirmation of CLAUDE_CONFIG_DIR working (MEDIUM confidence)
- [Trail of Bits claude-code-config](https://github.com/trailofbits/claude-code-config) -- Industry sandbox configuration patterns (MEDIUM confidence)

---
*Stack research for: RightClaw v2.0 CC Native Sandbox & Agent Isolation*
*Researched: 2026-03-23*
