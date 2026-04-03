# Phase 33: Auth Detection — Research

**Researched:** 2026-04-03
**Domain:** MCP OAuth credential inspection, CLI subcommand wiring, `rightclaw up` pre-launch warn
**Confidence:** HIGH

<user_constraints>
## User Constraints (from CONTEXT.md)

### Locked Decisions

1. **Expiry states: 3 only** — `present` / `missing` / `expired`. No "expiring soon".
   - `missing` — key absent from `~/.claude/.credentials.json`
   - `expired` — key present, `expires_at > 0`, and `expires_at < now_unix`
   - `present` — key present and not expired (`expires_at == 0` OR `expires_at >= now_unix`)

2. **Table layout: grouped by agent** — output groups servers under agent name. `--agent <name>` filters to single agent. No flat/columnar layout. Skip agents with no HTTP/SSE servers silently.

3. **Warning in `rightclaw up`: specific enumeration** — one Warn log line naming each agent+server pair:
   ```
   [WARN] MCP auth required: right/notion (missing), scout/notion (expired)
   ```
   No generic redirect. List is bounded.

4. **Server filter: `url` field presence = OAuth candidate** — server entry has `url` field = HTTP/SSE = OAuth candidate. Stdio servers (command+args only) skipped silently. No hardcoded name blocklist.

### Claude's Discretion

None specified.

### Deferred Ideas (OUT OF SCOPE)

None surfaced during discussion.
</user_constraints>

<phase_requirements>
## Phase Requirements

| ID | Description | Research Support |
|----|-------------|------------------|
| DETECT-01 | Operator can run `rightclaw mcp status [--agent <name>]` and see a table of MCP servers with auth state per agent (present / missing / expired) | `read_credential` + timestamp comparison + grouped println — no new deps. `McpCommands` enum + `Commands::Mcp` variant mirroring `Memory` pattern. |
| DETECT-02 | Operator sees a non-fatal Warn during `rightclaw up` when any agent has MCP servers with missing or expired OAuth tokens | After agent discovery loop, before process-compose launch — collect `(agent, server, state)` tuples where state != present, emit single `tracing::warn!` if non-empty. |
</phase_requirements>

## Summary

Phase 33 wires two surfaces: a new `rightclaw mcp status` subcommand and a pre-launch warn in `rightclaw up`. Both surfaces read `.mcp.json` to identify OAuth candidates (entries with `url` field), then call `read_credential` from Phase 32 to determine auth state per server, then compare `expires_at` against current Unix time.

No new library dependencies are needed. The detect logic is pure: read JSON, call existing function, compare timestamp. The CLI wiring mirrors the existing `Memory`/`MemoryCommands` pattern exactly. The `up` warn is injected after the agent discovery loop and before the process-compose launch, following the existing non-fatal warn pattern already used for missing `rg`.

The implementation splits cleanly into two tasks: (1) a `mcp::detect` module in `crates/rightclaw/` with the core `AuthState` enum and `mcp_auth_status` function, and (2) CLI wiring in `crates/rightclaw-cli/src/main.rs` plus the `cmd_up` warn injection.

**Primary recommendation:** Add `crates/rightclaw/src/mcp/detect.rs` for the reusable detect logic; wire CLI and `cmd_up` warn as a second task.

## Standard Stack

### Core (all existing — no new deps)

| Library | Version | Purpose | Why Standard |
|---------|---------|---------|--------------|
| `rightclaw::mcp::credentials` | Phase 32 | `read_credential`, `mcp_oauth_key`, `CredentialToken` | Built in Phase 32; `read_credential` returns `Ok(None)` for missing key, token has `expires_at: u64` |
| `serde_json` | workspace | Parse `.mcp.json` | Already in workspace |
| `std::time::SystemTime` | stdlib | Current Unix timestamp for expiry comparison | No external dep needed |
| `tracing` | workspace | Non-fatal warn in `cmd_up` | Already used throughout |
| `miette` | workspace | Error propagation in CLI | Already used throughout |
| `dirs` | workspace | Resolve `~/.claude/.credentials.json` | Already used in `cmd_up` for `host_home` |

**Installation:** no new packages — all deps already in workspace.

## Architecture Patterns

### Recommended Module Structure

New file: `crates/rightclaw/src/mcp/detect.rs`
Modify: `crates/rightclaw/src/mcp/mod.rs` (add `pub mod detect`)
Modify: `crates/rightclaw/src/lib.rs` — no change needed (`pub mod mcp` already exported)
Modify: `crates/rightclaw-cli/src/main.rs` — `McpCommands` enum, `Commands::Mcp`, `cmd_mcp_status`, `cmd_up` warn

### Pattern 1: AuthState enum + mcp_auth_status function

Place in `crates/rightclaw/src/mcp/detect.rs`.

```rust
// crates/rightclaw/src/mcp/detect.rs
use std::path::Path;
use crate::mcp::credentials::{read_credential, CredentialError};

#[derive(Debug, Clone, PartialEq)]
pub enum AuthState {
    Present,
    Missing,
    Expired,
}

impl std::fmt::Display for AuthState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AuthState::Present => write!(f, "present"),
            AuthState::Missing => write!(f, "missing"),
            AuthState::Expired => write!(f, "expired"),
        }
    }
}

/// One row in the mcp status table.
pub struct ServerStatus {
    pub name: String,
    pub url: String,
    pub state: AuthState,
}

/// Return auth status for all OAuth-candidate servers in an agent's .mcp.json.
/// OAuth candidates = server entries that have a `url` field.
/// Reads credentials from `credentials_path` (host ~/.claude/.credentials.json).
pub fn mcp_auth_status(
    agent_mcp_path: &Path,
    credentials_path: &Path,
) -> Result<Vec<ServerStatus>, CredentialError> { ... }
```

The function:
1. Returns `Ok(vec![])` if `.mcp.json` does not exist (no servers configured).
2. Parses `mcpServers` object; skips entries without `url` field.
3. For each entry with `url`: calls `read_credential(credentials_path, name, url)`.
4. Determines state: `None` → `Missing`; `Some(token)` where `expires_at > 0 && expires_at < now_unix` → `Expired`; else → `Present`.
5. Returns `Vec<ServerStatus>` sorted by server name (deterministic output).

Getting current Unix time (stdlib, no chrono needed):
```rust
let now_unix = std::time::SystemTime::now()
    .duration_since(std::time::UNIX_EPOCH)
    .unwrap_or_default()
    .as_secs();
```

### Pattern 2: McpCommands enum — mirrors MemoryCommands exactly

In `crates/rightclaw-cli/src/main.rs`:

```rust
/// Subcommands for `rightclaw mcp`.
#[derive(Subcommand)]
pub enum McpCommands {
    /// Show MCP server auth state per agent (present / missing / expired)
    Status {
        /// Filter to a single agent by name
        #[arg(long)]
        agent: Option<String>,
    },
}
```

Add to `Commands` enum:
```rust
/// Inspect and manage MCP server OAuth credentials
Mcp {
    #[command(subcommand)]
    command: McpCommands,
},
```

Dispatch in `main()` match arm:
```rust
Commands::Mcp { command } => match command {
    McpCommands::Status { agent } => cmd_mcp_status(&home, agent.as_deref()),
},
```

### Pattern 3: cmd_mcp_status output — grouped by agent

```rust
fn cmd_mcp_status(home: &Path, agent_filter: Option<&str>) -> miette::Result<()> {
    // discover agents (same as cmd_list / cmd_up)
    // for each agent (or filtered single agent):
    //   call mcp_auth_status(agent.path.join(".mcp.json"), credentials_path)
    //   if servers is empty: skip (no HTTP/SSE servers)
    //   else: print "agentname:" then "  servername    state" per server
    // if no output produced: print "No MCP OAuth servers found."
}
```

Credentials path: `host_home.join(".claude").join(".credentials.json")`
where `host_home = dirs::home_dir().ok_or_else(...)?`

Output format (exact per CONTEXT.md decision 2):
```
right:
  notion    missing
  linear    present

scout:
  notion    expired
```
Two-space indent, single tab-stop gap between server name and state (align with padding).

### Pattern 4: cmd_up warn injection

Insert after the per-agent loop (after all `.mcp.json` files are written/updated), before process-compose launch:

```rust
// Collect MCP auth issues across all agents (DETECT-02).
let credentials_path = host_home.join(".claude").join(".credentials.json");
let mut auth_issues: Vec<String> = Vec::new();
for agent in &agents {
    let mcp_path = agent.path.join(".mcp.json");
    match rightclaw::mcp::detect::mcp_auth_status(&mcp_path, &credentials_path) {
        Ok(servers) => {
            for s in servers {
                if s.state != rightclaw::mcp::detect::AuthState::Present {
                    auth_issues.push(format!("{}/{} ({})", agent.name, s.name, s.state));
                }
            }
        }
        Err(e) => {
            tracing::warn!(agent = %agent.name, "failed to read MCP auth status: {e:#}");
        }
    }
}
if !auth_issues.is_empty() {
    tracing::warn!("MCP auth required: {}", auth_issues.join(", "));
}
```

The warn is non-fatal: errors reading credentials are also non-fatal (logged as warn, not propagated).

### Anti-Patterns to Avoid

- **Don't re-implement credential parsing** — `read_credential` from Phase 32 is the single source of truth. Don't parse `.credentials.json` directly in detect.rs.
- **Don't hardcode server names** — the url-field filter is the only discriminator. No name blocklist.
- **Don't use chrono** — `std::time::SystemTime` is sufficient for Unix timestamp comparison.
- **Don't abort `cmd_up` on detect errors** — credential read failures are warnings, not blockers.
- **Don't print warn if no issues** — only emit the warn line when `auth_issues` is non-empty.

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| Credential key derivation | Custom hash | `mcp_oauth_key` from `credentials.rs` | Formula is subtle (field order matters, manual JSON construction) — already correct and tested |
| Token read | Direct JSON parse | `read_credential` | Handles missing file, missing key, deserialize — all tested in Phase 32 |
| Unix timestamp | chrono dep | `std::time::SystemTime::UNIX_EPOCH` | No external dep; `as_secs()` is u64 matching `expires_at: u64` |

**Key insight:** Phase 32 already handles all credential I/O complexity. Phase 33 adds only the state classification logic on top.

## Common Pitfalls

### Pitfall 1: .mcp.json parsed before `generate_mcp_config` runs
**What goes wrong:** In `cmd_up`, if the auth status check runs before `generate_mcp_config`, the `.mcp.json` may not yet contain the rightmemory entry — but rightmemory is a stdio server (no `url`) so it would be skipped anyway. However, user-added HTTP servers would already be in `.mcp.json` before `rightclaw up` is invoked (they're added manually or by future `mcp add` command). So detection should run after `generate_mcp_config` (after the per-agent loop) to ensure a complete picture.
**How to avoid:** Place auth status collection after the `for agent in &agents { ... }` loop — which is already the natural location before process-compose launch.

### Pitfall 2: Missing .mcp.json is not an error
**What goes wrong:** If `mcp_auth_status` propagates `io::Error` when `.mcp.json` doesn't exist, `cmd_mcp_status` would fail for newly-initialized agents.
**How to avoid:** `mcp_auth_status` returns `Ok(vec![])` when the file is absent. Handled before `serde_json::from_str`.

### Pitfall 3: expires_at == 0 treated as expired
**What goes wrong:** Linear tokens have `expires_at: 0` (non-expiring by convention). The check `expires_at < now_unix` with `now_unix` being ~1.7 billion would incorrectly mark them expired.
**How to avoid:** State logic must be: `expired` only when `expires_at > 0 && expires_at < now_unix`. When `expires_at == 0` → `present`. This is explicit in CONTEXT.md decision 1 and consistent with REQUIREMENTS.md REFRESH-04.

### Pitfall 4: credentials_path resolution differs between cmd_mcp_status and cmd_up
**What goes wrong:** `cmd_mcp_status` doesn't already resolve `host_home` — it would need to add the `dirs::home_dir()` call that `cmd_up` already has.
**How to avoid:** Both `cmd_mcp_status` and `cmd_up` use `dirs::home_dir()` to resolve credentials path. `cmd_up` already has `host_home` in scope. `cmd_mcp_status` needs to resolve it too — follow the same pattern.

### Pitfall 5: Agent filter in cmd_mcp_status errors on unknown agent name
**What goes wrong:** If `--agent foo` is passed and "foo" doesn't exist in agents dir, `discover_agents` returns all agents, and filtering by name finds nothing — producing empty output with no explanation.
**How to avoid:** After filtering, if `agent_filter` was specified but no match found, return a `miette::miette!("agent '{name}' not found")` error. Mirror the `cmd_up` filter pattern exactly.

## Code Examples

### expires_at state classification
```rust
// Source: CONTEXT.md Decision 1
let state = match token_opt {
    None => AuthState::Missing,
    Some(token) => {
        if token.expires_at > 0 && token.expires_at < now_unix {
            AuthState::Expired
        } else {
            AuthState::Present
        }
    }
};
```

### Filtering OAuth candidates from .mcp.json
```rust
// Source: CONTEXT.md Decision 4
// mcpServers entries with "url" field = HTTP/SSE = OAuth candidate
if let Some(url) = server_obj.get("url").and_then(|v| v.as_str()) {
    // This is an OAuth candidate — check credentials
}
// Entries without "url" (rightmemory, stdio) are silently skipped
```

### cmd_up warn format
```rust
// Source: CONTEXT.md Decision 3
// Single warn line, specific enumeration:
// [WARN] MCP auth required: right/notion (missing), scout/notion (expired)
tracing::warn!("MCP auth required: {}", auth_issues.join(", "));
```

## Environment Availability

Step 2.6: SKIPPED (no external dependencies — pure Rust + stdlib + existing workspace crates).

## Sources

### Primary (HIGH confidence)
- `crates/rightclaw/src/mcp/credentials.rs` — `read_credential`, `CredentialToken.expires_at: u64`, `mcp_oauth_key` signature (read directly)
- `crates/rightclaw-cli/src/main.rs` — `MemoryCommands`, `Commands::Memory`, `cmd_memory_list` pattern (read directly)
- `crates/rightclaw/src/agent/discovery.rs` — `discover_agents`, `AgentDef` structure (read directly)
- `crates/rightclaw/src/codegen/mcp_config.rs` — `.mcp.json` schema (`mcpServers` object, `url` field for HTTP/SSE) (read directly)
- `.planning/phases/33-auth-detection/33-CONTEXT.md` — all locked decisions (read directly)
- `.planning/REQUIREMENTS.md` — DETECT-01, DETECT-02 verbatim (read directly)

### Secondary (MEDIUM confidence)
- stdlib `std::time::SystemTime` for Unix timestamp — standard Rust pattern, no verification needed

## Metadata

**Confidence breakdown:**
- Standard stack: HIGH — no new deps; all existing workspace crates
- Architecture: HIGH — mirrors existing patterns directly observed in codebase
- Pitfalls: HIGH — derived from reading actual code; expires_at=0 edge case explicit in REQUIREMENTS.md REFRESH-04

**Research date:** 2026-04-03
**Valid until:** Stable — depends only on Phase 32 outputs (completed) and existing CLI patterns
