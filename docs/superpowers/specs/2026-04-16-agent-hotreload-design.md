# Agent Hot-Reload: DB Migration + Aggregator Registration

## Problem

When a new agent is added via `rightclaw agent init` + `rightclaw reload` while rightclaw is running, two things break:

1. **Missing tables in `data.db`**: Bot opens per-agent `data.db` with `migrate: false`. The aggregator was supposed to migrate it, but it only migrates databases for agents known at startup. New agents get an empty DB → `no such table: mcp_servers`, `no such table: cron_specs`.

2. **Aggregator doesn't know the new agent**: The aggregator loads `agent-tokens.json` into memory at startup and never re-reads it. New agent's MCP requests get 401 Unauthorized because the token isn't in the in-memory map and no `BackendRegistry` exists in the dispatcher.

## Design

### Fix 1: Bot migrates its own DB

Change `open_connection(&agent_dir, false)` to `open_connection(&agent_dir, true)` in `crates/bot/src/lib.rs:173`.

Each agent's `data.db` is owned by its bot process. The bot is the primary long-lived consumer. Having it run migrations is the natural ownership model.

The aggregator's startup migration for per-agent DBs can remain (idempotent) — it serves existing agents that start before their bots. But the bot no longer depends on it.

Update documentation:
- `open_connection()` doc-comment: remove "Only the MCP aggregator should pass `migrate: true`"
- ARCHITECTURE.md: update Migration Ownership section

### Fix 2: `/reload` endpoint on internal API

#### Request/Response

```
POST /reload
Content-Type: application/json

{}

Response 200:
{
  "added": ["him"],
  "total": 3
}
```

Empty JSON body (or `{}`). Response lists newly registered agents.

#### Handler logic (`internal_api.rs`)

1. Read `agent-tokens.json` from `token_map_path` on disk
2. Diff against current `token_map` (in-memory `Arc<RwLock<HashMap>>`)
3. For each new agent:
   - Resolve `agents_dir.join(agent_name)` as `agent_dir`
   - Create `RightBackend` for the agent
   - Parse `agent_config` to determine sandbox mode (for mTLS dir) and hindsight
   - Create `BackendRegistry` with empty proxies
   - Insert into `dispatcher.agents`
   - Add token → AgentInfo mapping to `token_map`
4. Return list of added agent names

Removed agents are NOT cleaned up (safe default — running bots may still need them).

#### State changes

`InternalState` gains three fields:
- `token_map: AgentTokenMap` — the shared in-memory token map
- `token_map_path: PathBuf` — path to `agent-tokens.json` on disk
- `agents_dir: PathBuf` — path to `~/.rightclaw/agents/`

These are threaded from `run_aggregator_http()` through `internal_router()`.

#### InternalClient (`crates/rightclaw/src/mcp/internal_client.rs`)

New method:
```rust
pub async fn reload(&self) -> Result<ReloadResponse, InternalClientError> {
    self.post("/reload", &serde_json::json!({})).await
}
```

With `ReloadResponse { added: Vec<String>, total: usize }` defined in core (shared between client and server).

#### CLI integration (`cmd_reload` in `main.rs`)

After `client.reload_configuration().await?` (process-compose), add:

```rust
let socket_path = home.join("run/internal.sock");
let internal = rightclaw::mcp::InternalClient::new(&socket_path);
match internal.reload().await {
    Ok(resp) => {
        if !resp.added.is_empty() {
            println!("Registered {} new agent(s) in aggregator: {}", resp.added.len(), resp.added.join(", "));
        }
    }
    Err(e) => {
        eprintln!("warning: failed to reload aggregator: {e:#}");
    }
}
```

Aggregator reload failure is a warning, not a hard error — process-compose reload already succeeded, agents will start. MCP will work after next full restart.

## Files changed

| File | Change |
|------|--------|
| `crates/bot/src/lib.rs` | `migrate: false` → `true` (line 173) |
| `crates/rightclaw/src/mcp/internal_client.rs` | Add `reload()` method + `ReloadResponse` type |
| `crates/rightclaw-cli/src/internal_api.rs` | Add `token_map`, `token_map_path`, `agents_dir` to `InternalState`; add `/reload` endpoint + handler |
| `crates/rightclaw-cli/src/aggregator.rs` | Thread `token_map`, `token_map_path`, `agents_dir` through to `internal_router()` |
| `crates/rightclaw-cli/src/main.rs` | `cmd_reload`: call `/reload` via `InternalClient` after process-compose reload |
| `crates/rightclaw/src/memory/mod.rs` | Update `open_connection` doc-comment |
| `ARCHITECTURE.md` | Update Migration Ownership section |

## Not in scope

- Removing agents from aggregator on hot-reload (add later if needed)
- Restoring proxy backends for new agents (they start with no external MCP servers — `/mcp add` works normally after boot)
- File watching / automatic reload (explicit `rightclaw reload` is the trigger)
