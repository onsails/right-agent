# Phase 2: CLI Runtime and Sandboxing - Research

**Researched:** 2026-03-22
**Domain:** Rust CLI lifecycle commands with process-compose orchestration and OpenShell sandboxing
**Confidence:** HIGH

## Summary

Phase 2 builds the runtime layer on top of Phase 1's agent discovery. The CLI needs five new subcommands (`up`, `down`, `status`, `restart`, `attach`) that generate shell wrappers and a process-compose.yaml, then delegate process lifecycle to process-compose. Each agent runs inside an OpenShell sandbox via a generated shell wrapper script.

The key integration points are well-understood: process-compose has a documented REST API accessible via Unix domain socket, reqwest 0.13 has built-in UDS support, and OpenShell's CLI follows a straightforward `sandbox create --policy <path> --name <name> -- <command>` pattern. The `rightclaw attach` command should use `std::os::unix::process::CommandExt` to replace the process with `process-compose attach`.

**Primary recommendation:** Generate all artifacts to `$RIGHTCLAW_HOME/run/`, use reqwest over Unix socket for status/restart API calls, use stdlib CommandExt for attach, and track sandbox names in a state file for explicit cleanup on `down`.

<user_constraints>
## User Constraints (from CONTEXT.md)

### Locked Decisions
- **D-01:** Generated files (process-compose.yaml, shell wrappers) live at `$RIGHTCLAW_HOME/run/`
- **D-02:** Files are persistent and inspectable -- overwritten on each `rightclaw up`, NOT cleaned on `down`
- **D-03:** Each agent gets a shell wrapper script at `$RIGHTCLAW_HOME/run/<agent-name>.sh`
- **D-04:** process-compose.yaml generated at `$RIGHTCLAW_HOME/run/process-compose.yaml`
- **D-05:** Agent's working directory = its agent folder (`$RIGHTCLAW_HOME/agents/<name>/`), NOT a project path
- **D-06:** Claude Code reads SOUL.md, AGENTS.md, MEMORY.md etc. naturally from cwd -- no concatenation needed
- **D-07:** Only IDENTITY.md passed via `--append-system-prompt-file` flag to Claude Code
- **D-08:** Always use `--dangerously-skip-permissions` -- OpenShell is the security layer
- **D-09:** Shell wrapper invokes `openshell sandbox create --policy <agent>/policy.yaml -- claude --append-system-prompt-file <agent>/IDENTITY.md --dangerously-skip-permissions`
- **D-10:** Fail by default if OpenShell not installed. `--no-sandbox` flag allows running without OpenShell
- **D-11:** When `--no-sandbox`, wrapper runs `claude` directly without `openshell sandbox create`
- **D-12:** `rightclaw down` explicitly destroys each OpenShell sandbox (via `openshell sandbox delete`)
- **D-13:** `rightclaw down` stops process-compose but keeps `$RIGHTCLAW_HOME/run/` files for debugging
- **D-14:** run/ files overwritten on next `rightclaw up` -- no manual cleanup needed
- **D-15:** Use process-compose REST API (reqwest via Unix socket) for `status` and `restart` -- NOT CLI shelling out
- **D-16:** `rightclaw attach` uses CommandExt to replace the process with process-compose attach TUI
- **D-17:** `rightclaw up` spawns process-compose, `rightclaw up -d` uses `--detached-with-tui`
- **D-18:** Unix socket path stored at `$RIGHTCLAW_HOME/run/pc.sock`

### Claude's Discretion
- Exact process-compose REST API endpoints and error handling
- How to track sandbox names for cleanup
- Shell wrapper template format (bash vs sh)
- Process-compose version compatibility handling

### Deferred Ideas (OUT OF SCOPE)
None -- discussion stayed within phase scope
</user_constraints>

<phase_requirements>
## Phase Requirements

| ID | Description | Research Support |
|----|-------------|------------------|
| CLI-01 | `rightclaw up` scans agents, generates config, launches all | Codegen module (minijinja templates), process-compose spawn via tokio::process |
| CLI-02 | `rightclaw up --agents a,b` launches only named agents | Filter `discover_agents()` result by name before codegen |
| CLI-03 | `rightclaw up -d` launches in background with TUI server | process-compose `--detached-with-tui` flag with `--unix-socket` |
| CLI-04 | `rightclaw attach` connects to running TUI | `CommandExt` replacing process with `process-compose attach` |
| CLI-05 | `rightclaw status` shows agent states | process-compose REST API `GET /processes` via reqwest Unix socket |
| CLI-06 | `rightclaw restart <agent>` restarts single agent | process-compose REST API `POST /process/restart/{name}` |
| CLI-07 | `rightclaw down` stops all agents, destroys sandboxes | REST API `POST /project/stop`, then `openshell sandbox delete` per agent |
| SAND-01 | Each agent launches inside OpenShell sandbox with YAML policy | Shell wrapper generates `openshell sandbox create --policy <path> --name <name> -- claude ...` |
| SAND-02 | Shell wrapper reads policy from agent dir, invokes openshell | Generated bash script at `$RIGHTCLAW_HOME/run/<name>.sh` |
| SAND-03 | `rightclaw down` explicitly destroys sandboxes | State file tracks sandbox names, `down` iterates and calls `openshell sandbox delete` |
</phase_requirements>

## Standard Stack

### Core (already in workspace)
| Library | Version | Purpose | Why Standard |
|---------|---------|---------|--------------|
| clap | 4.6 | CLI parsing with new subcommands | Already in workspace, derive API |
| tokio | 1.50 | Async runtime for HTTP + process spawning | Required by reqwest, needed for async PC API calls |
| serde | 1.0 | Serialization | Already in workspace |
| serde-saphyr | 0.0 | YAML parsing | Already in workspace |
| minijinja | 2.18 | Template engine for codegen | Already in workspace deps plan (STACK.md) |
| miette | 7.6 | Diagnostic errors | Already in workspace |
| thiserror | 2.0 | Error types | Already in workspace |
| tracing | 0.1 | Structured logging | Already in workspace |

### New Dependencies for Phase 2
| Library | Version | Purpose | When to Use |
|---------|---------|---------|-------------|
| reqwest | 0.13 | HTTP client with Unix socket support | `status` and `restart` commands via PC REST API |
| tokio | 1.50 (features: full) | Async process spawn + signal handling | Spawning process-compose, Ctrl+C handling |
| serde_json | 1.0 | JSON parsing for PC API responses | Parsing process-compose REST API JSON responses |
| which | 7.0 | Find executables in PATH | Verify `process-compose`, `openshell`, `claude` exist before `up` |

### No Additional Crates Needed
| Problem | Why No Extra Crate |
|---------|-------------------|
| Process replacement for attach | `std::os::unix::process::CommandExt` is stdlib |
| Signal handling (Ctrl+C) | `tokio::signal::ctrl_c()` with tokio "signal" feature |
| File permissions (chmod +x) | `std::os::unix::fs::PermissionsExt` is stdlib |
| Unix socket path | Just a `PathBuf`, no special handling needed |

### Alternatives Considered
| Instead of | Could Use | Tradeoff |
|------------|-----------|----------|
| reqwest (UDS) | hyperlocal / hyper-client-sockets | Lower level, more boilerplate. reqwest 0.13 has built-in `unix_socket()` on ClientBuilder |
| reqwest (UDS) | Shell out to `process-compose process list` | Decision D-15 explicitly forbids CLI shelling out for status/restart |
| ctrlc crate for signals | tokio::signal | tokio already in deps, `tokio::signal::ctrl_c()` is simpler than adding another crate |
| nix crate (execvp) | std::os::unix::process::CommandExt | stdlib sufficient, no need for nix dependency |

**Installation (new workspace deps):**
```toml
# In workspace Cargo.toml [workspace.dependencies]
reqwest = { version = "0.13", default-features = false, features = ["json", "rustls-tls"] }
tokio = { version = "1.50", features = ["full"] }
serde_json = "1.0"
which = "7.0"
minijinja = "2.18"
```

## Architecture Patterns

### Recommended Project Structure (additions to Phase 1)
```
crates/rightclaw/src/
├── agent/          # (Phase 1 -- exists)
├── config.rs       # (Phase 1 -- exists)
├── error.rs        # (Phase 1 -- extend with new variants)
├── init.rs         # (Phase 1 -- exists)
├── codegen/
│   ├── mod.rs              # Re-exports
│   ├── shell_wrapper.rs    # Generate per-agent bash scripts
│   └── process_compose.rs  # Generate process-compose.yaml
├── runtime/
│   ├── mod.rs              # Re-exports
│   ├── pc_client.rs        # reqwest-based process-compose REST API client
│   └── sandbox.rs          # OpenShell sandbox tracking and cleanup
└── lib.rs          # Add pub mod codegen, runtime

crates/rightclaw-cli/src/
└── main.rs         # Add Up, Down, Status, Restart, Attach subcommands
```

### Pattern 1: Codegen with minijinja Templates
**What:** Use minijinja to render shell wrapper and process-compose.yaml from templates embedded via `include_str!`
**When to use:** All file generation in the codegen module
**Example:**
```rust
// codegen/shell_wrapper.rs
use minijinja::{Environment, context};

const WRAPPER_TEMPLATE: &str = include_str!("../../../templates/agent-wrapper.sh.j2");

pub fn generate_wrapper(agent: &AgentDef, no_sandbox: bool) -> miette::Result<String> {
    let mut env = Environment::new();
    env.add_template("wrapper", WRAPPER_TEMPLATE)
        .map_err(|e| miette::miette!("template error: {e:#}"))?;
    let tmpl = env.get_template("wrapper").unwrap();
    tmpl.render(context! {
        agent_name => agent.name,
        identity_path => agent.identity_path.display().to_string(),
        policy_path => agent.policy_path.display().to_string(),
        no_sandbox => no_sandbox,
        start_prompt => agent.config.as_ref()
            .and_then(|c| c.start_prompt.as_deref())
            .unwrap_or("You are starting. Read your MEMORY.md to restore context."),
    }).map_err(|e| miette::miette!("render error: {e:#}"))
}
```

### Pattern 2: process-compose REST API Client
**What:** Typed async client using reqwest with Unix socket transport
**When to use:** `status`, `restart`, `down` commands
**Example:**
```rust
// runtime/pc_client.rs
use reqwest::Client;

pub struct PcClient {
    client: Client,
    base_url: String, // arbitrary when using UDS -- host is ignored
}

impl PcClient {
    pub fn new(socket_path: &std::path::Path) -> miette::Result<Self> {
        let client = Client::builder()
            .unix_socket(socket_path)
            .build()
            .map_err(|e| miette::miette!("failed to create PC client: {e:#}"))?;
        Ok(Self {
            client,
            base_url: "http://localhost".to_string(),
        })
    }

    pub async fn list_processes(&self) -> miette::Result<Vec<ProcessInfo>> {
        let resp = self.client
            .get(format!("{}/processes", self.base_url))
            .send().await
            .map_err(|e| miette::miette!("PC API error: {e:#}"))?;
        let data: ProcessesResponse = resp.json().await
            .map_err(|e| miette::miette!("PC API parse error: {e:#}"))?;
        Ok(data.data)
    }

    pub async fn restart_process(&self, name: &str) -> miette::Result<()> {
        self.client
            .post(format!("{}/process/restart/{name}", self.base_url))
            .send().await
            .map_err(|e| miette::miette!("PC restart error: {e:#}"))?;
        Ok(())
    }

    pub async fn shutdown(&self) -> miette::Result<()> {
        self.client
            .post(format!("{}/project/stop", self.base_url))
            .send().await
            .map_err(|e| miette::miette!("PC shutdown error: {e:#}"))?;
        Ok(())
    }
}
```

### Pattern 3: stdlib CommandExt for attach
**What:** Replace current process with `process-compose attach` using the Unix execvp syscall
**When to use:** `rightclaw attach` command only
**Example:**
```rust
// In main.rs, Commands::Attach handler
use std::os::unix::process::CommandExt;

let socket_path = run_dir.join("pc.sock");
if !socket_path.exists() {
    return Err(miette::miette!("No running instance found. Is rightclaw up?"));
}

let err = std::process::Command::new("process-compose")
    .arg("attach")
    .arg("--unix-socket")
    .arg(&socket_path)
    .exec(); // replaces current process, only returns on error
return Err(miette::miette!("Failed to attach: {err}"));
```

### Pattern 4: Sandbox Name Tracking via State File
**What:** Write a JSON state file mapping agent names to sandbox names for cleanup
**When to use:** `up` writes state, `down` reads it for cleanup

```rust
// runtime/sandbox.rs
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
pub struct RuntimeState {
    pub agents: Vec<AgentState>,
    pub socket_path: String,
    pub started_at: String,
}

#[derive(Serialize, Deserialize)]
pub struct AgentState {
    pub name: String,
    pub sandbox_name: String,  // "rightclaw-<agent-name>"
}
```

The sandbox name convention: `rightclaw-<agent-name>` (e.g., `rightclaw-watchdog`). This is deterministic, so even without a state file, `down` can reconstruct expected sandbox names from discovered agents.

### Anti-Patterns to Avoid
- **Shell out for status/restart:** D-15 locks this -- use reqwest via Unix socket, not `process-compose process list` subprocess
- **Build a process manager:** process-compose handles restarts, health, logs. Do not reimplement.
- **Dynamic policy assembly:** Policies are static files in agent dirs. Do not merge or generate them at runtime in Phase 2.
- **Hardcode process-compose YAML:** Use minijinja templates, not string concatenation

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| Process lifecycle (restart, health) | Custom restart loop | process-compose availability config | Mature, battle-tested, has TUI |
| Process monitoring TUI | Custom terminal UI | process-compose TUI via attach | Feature-complete, handles logs, colors, keybindings |
| HTTP over Unix socket | Raw hyper + tokio UnixStream | reqwest 0.13 `ClientBuilder::unix_socket()` | Built-in, ergonomic, handles serialization |
| Template rendering | String format!() for YAML | minijinja | Proper escaping, conditionals, loops |
| Executable path lookup | Manual PATH splitting | `which` crate | Cross-platform, handles edge cases |
| Signal handling | Raw libc signal handlers | `tokio::signal::ctrl_c()` | Safe, async-aware, cross-platform |

**Key insight:** This phase is a thin orchestration layer. Every hard problem (process lifecycle, TUI, sandboxing) is solved by an external tool. RightClaw's job is codegen + glue.

## process-compose REST API Reference

Endpoints needed for this phase (verified from Go source code at `src/api/pc_api.go`):

| Operation | Method | Path | Use Case |
|-----------|--------|------|----------|
| List all processes | GET | `/processes` | `rightclaw status` |
| Get single process | GET | `/process/{name}` | Detailed status |
| Restart process | POST | `/process/restart/{name}` | `rightclaw restart <agent>` |
| Stop process | PATCH | `/process/stop/{name}` | Individual agent stop |
| Stop all (shutdown) | POST | `/project/stop` | `rightclaw down` |
| Health check | GET | `/live` | Verify PC is running before API calls |

**Authentication:** Optional via `PC_API_TOKEN` header (`X-PC-Token-Key`). Not needed for local UDS connections.

**process-compose up flags for rightclaw:**
```bash
# Foreground (rightclaw up)
process-compose up -f $RIGHTCLAW_HOME/run/process-compose.yaml \
  --unix-socket $RIGHTCLAW_HOME/run/pc.sock

# Background (rightclaw up -d)
process-compose up -f $RIGHTCLAW_HOME/run/process-compose.yaml \
  --unix-socket $RIGHTCLAW_HOME/run/pc.sock \
  --detached-with-tui
```

## OpenShell CLI Reference

Commands needed for this phase:

```bash
# Create sandbox with policy and named for tracking
openshell sandbox create \
  --policy /path/to/agents/<name>/policy.yaml \
  --name rightclaw-<agent-name> \
  -- claude --append-system-prompt-file /path/to/agents/<name>/IDENTITY.md \
     --dangerously-skip-permissions

# List sandboxes (for orphan detection)
openshell sandbox list

# Delete sandbox (explicit cleanup)
openshell sandbox delete rightclaw-<agent-name>
```

**Important:** `openshell sandbox delete` is the correct command (not `destroy`). It stops all processes, releases resources, and purges injected credentials.

## Shell Wrapper Template

Recommendation: **Use bash** (not sh). Bash is universally available on Linux/macOS, and we need `set -euo pipefail` for safety.

```bash
#!/usr/bin/env bash
# Generated by rightclaw -- do not edit
# Agent: {{ agent_name }}
set -euo pipefail

{% if not no_sandbox %}
exec openshell sandbox create \
  --policy "{{ policy_path }}" \
  --name "rightclaw-{{ agent_name }}" \
  -- claude \
    --append-system-prompt-file "{{ identity_path }}" \
    --dangerously-skip-permissions \
    --prompt "{{ start_prompt }}"
{% else %}
# WARNING: Running without sandbox (--no-sandbox mode)
exec claude \
  --append-system-prompt-file "{{ identity_path }}" \
  --dangerously-skip-permissions \
  --prompt "{{ start_prompt }}"
{% endif %}
```

Note: `exec` replaces the shell process with the actual command, keeping the process tree clean for process-compose signal delivery.

## process-compose.yaml Template

```yaml
# Generated by rightclaw -- do not edit
version: "0.5"
is_strict: true

processes:
{% for agent in agents %}
  {{ agent.name }}:
    command: "{{ agent.wrapper_path }}"
    working_dir: "{{ agent.working_dir }}"
    availability:
      restart: "{{ agent.restart_policy }}"
      backoff_seconds: {{ agent.backoff_seconds }}
      max_restarts: {{ agent.max_restarts }}
    shutdown:
      signal: 15
      timeout_seconds: 30
{% endfor %}
```

## Common Pitfalls

### Pitfall 1: Signal Propagation Through Container Boundaries
**What goes wrong:** SIGTERM from process-compose reaches the shell wrapper but not the Claude process inside the OpenShell sandbox (different PID namespace via K3s/Docker).
**Why it happens:** OpenShell sandboxes run in containers. Signals do not cross container boundaries.
**How to avoid:** `rightclaw down` must explicitly call `openshell sandbox delete` for each agent. Do NOT rely on process-compose signals alone. The `exec` in the shell wrapper helps -- it makes the openshell process the direct child of process-compose, but the claude process inside the sandbox is still isolated.
**Warning signs:** After `rightclaw down`, `openshell sandbox list` still shows active sandboxes.

### Pitfall 2: Stale Socket File
**What goes wrong:** If process-compose crashes, `$RIGHTCLAW_HOME/run/pc.sock` remains on disk. Next `rightclaw up` may fail or connect to a dead socket.
**Why it happens:** Unix sockets leave files on disk that are not automatically cleaned up on crash.
**How to avoid:** Before spawning process-compose, check if `pc.sock` exists. If it does, try a health check (`GET /live`). If unreachable, delete the stale socket file, then proceed.
**Warning signs:** `rightclaw status` returns connection refused despite socket file existing.

### Pitfall 3: process-compose Not Found
**What goes wrong:** User runs `rightclaw up` without process-compose installed.
**Why it happens:** process-compose is an external dependency, not bundled.
**How to avoid:** Use `which::which("process-compose")` before spawn. Give actionable error with install URL.
**Warning signs:** Cryptic "No such file or directory" from tokio::process.

### Pitfall 4: OpenShell Gateway Not Running
**What goes wrong:** `openshell sandbox create` fails because no gateway is bootstrapped.
**Why it happens:** OpenShell auto-bootstraps a gateway on first use, but Docker must be running.
**How to avoid:** The shell wrapper will fail with an openshell error. process-compose will show this in logs and apply restart policy. Document Docker requirement clearly.
**Warning signs:** Agent keeps restarting with openshell errors in process-compose logs.

### Pitfall 5: Concurrent `rightclaw up` Instances
**What goes wrong:** User runs `rightclaw up` twice, overwriting the generated files and creating a second process-compose instance.
**Why it happens:** No lock mechanism preventing concurrent launches.
**How to avoid:** Check for existing `pc.sock` and try health check before launching. If already running, error with "rightclaw is already running. Use `rightclaw down` first or `rightclaw attach` to connect."

## Code Examples

### Spawning process-compose (foreground)
```rust
// runtime/mod.rs
use tokio::process::Command;

pub async fn spawn_foreground(
    config_path: &std::path::Path,
    socket_path: &std::path::Path,
) -> miette::Result<()> {
    let status = Command::new("process-compose")
        .arg("up")
        .arg("-f").arg(config_path)
        .arg("--unix-socket").arg(socket_path)
        .status().await
        .map_err(|e| miette::miette!("failed to spawn process-compose: {e:#}"))?;
    if !status.success() {
        return Err(miette::miette!("process-compose exited with {status}"));
    }
    Ok(())
}
```

### Spawning process-compose (detached)
```rust
pub async fn spawn_detached(
    config_path: &std::path::Path,
    socket_path: &std::path::Path,
) -> miette::Result<()> {
    let _child = Command::new("process-compose")
        .arg("up")
        .arg("-f").arg(config_path)
        .arg("--unix-socket").arg(socket_path)
        .arg("--detached-with-tui")
        .spawn()
        .map_err(|e| miette::miette!("failed to spawn process-compose: {e:#}"))?;

    // Wait briefly then verify via health check
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    let client = PcClient::new(socket_path)?;
    client.health_check().await?;
    println!("Agents launched in background. Use `rightclaw attach` to connect.");
    Ok(())
}
```

### Sandbox Cleanup on Down
```rust
// runtime/sandbox.rs
pub fn destroy_sandboxes(agents: &[String]) -> miette::Result<()> {
    for agent_name in agents {
        let sandbox_name = format!("rightclaw-{agent_name}");
        tracing::info!(sandbox = %sandbox_name, "Destroying sandbox");
        let output = std::process::Command::new("openshell")
            .args(["sandbox", "delete", &sandbox_name])
            .output()
            .map_err(|e| miette::miette!("failed to run openshell: {e:#}"))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            // Don't fail hard -- sandbox may already be gone
            tracing::warn!(sandbox = %sandbox_name, "Sandbox delete failed: {stderr}");
        }
    }
    Ok(())
}
```

### Dependency Verification
```rust
pub fn verify_dependencies(no_sandbox: bool) -> miette::Result<()> {
    which::which("process-compose").map_err(|_| {
        miette::miette!(
            help = "Install: https://f1bonacc1.github.io/process-compose/installation/",
            "process-compose not found in PATH"
        )
    })?;
    which::which("claude").map_err(|_| {
        miette::miette!(
            help = "Install Claude Code CLI: https://docs.anthropic.com/en/docs/claude-code",
            "claude not found in PATH"
        )
    })?;
    if !no_sandbox {
        which::which("openshell").map_err(|_| {
            miette::miette!(
                help = "Install: https://github.com/NVIDIA/OpenShell\n\
                        Or use --no-sandbox to skip (development only)",
                "openshell not found in PATH"
            )
        })?;
    }
    Ok(())
}
```

## State of the Art

| Old Approach | Current Approach | When Changed | Impact |
|--------------|------------------|--------------|--------|
| hyperlocal for UDS | reqwest 0.13 built-in `unix_socket()` | reqwest 0.13 (2025) | No extra crate needed for Unix socket HTTP |
| ctrlc crate for signals | tokio::signal::ctrl_c() | tokio 1.x | One less dependency, async-native |
| serde_yaml | serde-saphyr | March 2024 | serde_yaml archived, serde-saphyr is replacement |
| Custom YAML string building | minijinja templates | Project decision | Proper escaping, maintainable templates |

## Open Questions

1. **process-compose GET /processes response shape**
   - What we know: Returns JSON with process info (name, status, PID, uptime)
   - What's unclear: Exact JSON schema (field names, status enum values)
   - Recommendation: Start a test process-compose instance, hit `/processes`, define Rust types from actual response. Or read Go source types in the process-compose repo.

2. **OpenShell `--name` flag on `sandbox create`**
   - What we know: The manage-sandboxes docs show `--name` flag and sandboxes identified by name
   - What's unclear: Whether `--name` is supported specifically on `sandbox create` (vs only `sandbox connect`)
   - Recommendation: If `--name` is not available on `create`, fall back to deterministic naming convention and use `sandbox list` to find sandboxes. Test at implementation time.

3. **reqwest unix_socket with Host header**
   - What we know: reqwest routes all traffic through the socket, domain in URL is ignored for routing
   - What's unclear: Whether process-compose checks the Host header
   - Recommendation: Use `http://localhost` as base URL. Go HTTP servers typically don't check Host on UDS.

## Sources

### Primary (HIGH confidence)
- [process-compose launcher docs](https://f1bonacc1.github.io/process-compose/launcher/) - YAML format, availability, shutdown
- [process-compose client docs](https://f1bonacc1.github.io/process-compose/client/) - Unix socket, attach, detached mode
- [process-compose up CLI docs](https://f1bonacc1.github.io/process-compose/cli/process-compose_up/) - All flags for `up` command
- [process-compose REST API source (pc_api.go)](https://github.com/F1bonacc1/process-compose/blob/main/src/api/pc_api.go) - All endpoint paths verified from Go source
- [NVIDIA OpenShell manage-sandboxes](https://docs.nvidia.com/openshell/latest/sandboxes/manage-sandboxes.html) - create, delete, list, connect
- [NVIDIA OpenShell policies](https://docs.nvidia.com/openshell/latest/sandboxes/policies.html) - Policy YAML format, --policy flag
- [reqwest ClientBuilder docs](https://docs.rs/reqwest/latest/reqwest/struct.ClientBuilder.html) - Built-in UDS via `unix_socket()` method
- [std::os::unix::process::CommandExt](https://doc.rust-lang.org/std/os/unix/process/trait.CommandExt.html) - Process replacement for attach
- [process-compose configuration docs](https://f1bonacc1.github.io/process-compose/configuration/) - Global settings, env vars, strict mode

### Secondary (MEDIUM confidence)
- [OpenShell quickstart](https://docs.nvidia.com/openshell/latest/get-started/quickstart.html) - Auto-bootstrap, credential injection
- [NVIDIA OpenShell blog](https://developer.nvidia.com/blog/run-autonomous-self-evolving-agents-more-safely-with-nvidia-openshell/) - Additional flags

### Tertiary (LOW confidence)
- OpenShell `--name` flag on `sandbox create` -- not explicitly shown in quickstart, inferred from manage-sandboxes page

## Metadata

**Confidence breakdown:**
- Standard stack: HIGH - all crates verified, versions confirmed from registry
- Architecture: HIGH - builds directly on Phase 1 patterns, all external APIs documented
- process-compose integration: HIGH - REST API verified from Go source code
- OpenShell integration: MEDIUM - alpha software, `--name` flag needs runtime verification
- Pitfalls: HIGH - inherited from PITFALLS.md research + new phase-specific findings

**Research date:** 2026-03-22
**Valid until:** 2026-04-07 (process-compose stable, OpenShell alpha may change faster)
