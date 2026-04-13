# API Key MCP Support

## Problem

RightClaw only supports OAuth-authenticated MCP servers. Many MCP servers authenticate via static API keys — either in a custom HTTP header or embedded in the URL query string. Users cannot add these servers today.

## Design

### `/mcp add` Flow (revised)

```
User: /mcp add <name> <url>
  │
  ├─ 1. Validate name & URL (existing logic)
  ├─ 2. Strip query string from URL → bare_url
  ├─ 3. Try OAuth AS discovery on bare_url (existing code)
  │     ├─ Found → run OAuth flow (existing), auth_type = oauth
  │     └─ Not found →
  │           ├─ Is URL a public domain?
  │           │   ├─ Yes → dispatch haiku with web search (see below)
  │           │   │         → returns: bearer / {header, name} / query_string
  │           │   └─ No (local/private IP) → assume bearer
  │           │
  │           ├─ query_string (key already in original URL) → proceed directly
  │           ├─ bearer → ask user for token in Telegram, wait for reply
  │           └─ header {name} → ask user for token in Telegram, wait for reply
  │
  ├─ 4. Attempt MCP connection with assembled credentials
  │     ├─ Success → persist to SQLite, register ProxyBackend, cache tools
  │     └─ Failure → reject, notify user, no state change
  │
  └─ 5. Notify user: "Added <name> (N tools)"
```

### Auth Type Detection

Three-tier determination:

1. **OAuth discovery** (Rust, deterministic): existing `discover_as()` tries RFC 9728, RFC 8414, OIDC discovery. If any endpoint found → OAuth.

2. **Haiku web search** (AI, for public domains only): runs inside the agent's sandbox via SSH, same as regular `claude -p` invocations. Command: `claude -p --bare -m haiku` with web search enabled. Receives the bare URL (query stripped). Structured JSON output schema:
   ```json
   // One of:
   {"auth_type": "bearer"}
   {"auth_type": "header", "header_name": "X-Api-Key"}
   {"auth_type": "query_string"}
   ```
   Prompt asks haiku to search for the MCP server's documentation and determine its authentication method. Runs in sandbox to maintain the security model — no host-side Claude invocations.

3. **Fallback** (local/private domains): assume Bearer token auth.

"Public domain" detection: inverse of the existing `validate_server_url()` private IP checks — if the URL would pass SSRF validation (not localhost, not RFC1918, not link-local), it's public.

### Storage: `mcp_servers` Table (extended)

New columns added via SQLite migration (v10):

```sql
ALTER TABLE mcp_servers ADD COLUMN auth_type      TEXT;  -- 'oauth', 'bearer', 'header', 'query_string'
ALTER TABLE mcp_servers ADD COLUMN auth_header    TEXT;  -- header name for 'header' type (e.g. 'X-Api-Key')
ALTER TABLE mcp_servers ADD COLUMN auth_token     TEXT;  -- current access token (oauth, bearer, header)
ALTER TABLE mcp_servers ADD COLUMN refresh_token  TEXT;  -- oauth only
ALTER TABLE mcp_servers ADD COLUMN token_endpoint TEXT;  -- oauth only
ALTER TABLE mcp_servers ADD COLUMN client_id      TEXT;  -- oauth only
ALTER TABLE mcp_servers ADD COLUMN client_secret  TEXT;  -- oauth only
ALTER TABLE mcp_servers ADD COLUMN expires_at     TEXT;  -- oauth only, ISO8601
```

This replaces `oauth-state.json` entirely. All auth state lives in SQLite.

### Migration from `oauth-state.json`

One-time migration in v10:
1. If `oauth-state.json` exists in agent dir, read it
2. For each entry, UPDATE the corresponding `mcp_servers` row with OAuth fields
3. Set `auth_type = 'oauth'` for migrated rows
4. Delete `oauth-state.json`

Existing servers without auth info get `auth_type = NULL` (treated as unknown/legacy).

### `DynamicAuthClient` Changes

Currently always injects `Authorization: Bearer <token>`. Extend with an `AuthMethod` enum:

```rust
enum AuthMethod {
    Bearer,               // Authorization: Bearer <token>
    Header(String),       // <custom_header>: <token>
    QueryString,          // no header — key is in URL
}
```

`ProxyBackend` stores `AuthMethod` alongside the token `Arc<RwLock<Option<String>>>`. `DynamicAuthClient` reads `AuthMethod` to decide how to attach credentials:
- `Bearer` → `Authorization: Bearer <token>` header
- `Header(name)` → `<name>: <token>` header
- `QueryString` → no injection (token embedded in URL)

### Refresh Scheduler Changes

Currently reads `oauth-state.json`. Change to:
- On startup: query `mcp_servers WHERE auth_type = 'oauth' AND refresh_token IS NOT NULL`
- On refresh: UPDATE `auth_token`, `expires_at` in SQLite
- `RefreshMessage::NewEntry` writes to SQLite instead of JSON
- `RefreshMessage::RemoveServer` clears OAuth columns in SQLite
- Remove all `oauth-state.json` read/write code after migration

### `mcp_list` Output (Agent-facing)

Redact secrets. Agent sees:

```
- notion: connected (5 tools) url=https://mcp.notion.com/mcp
- weather: connected (3 tools) url=https://api.weather.com/mcp?<redacted>
```

Rules:
- If URL contains `?`, display `scheme://host/path?<redacted>`
- Never show `auth_type`, `auth_token`, or any credential info
- Auth is transparent to the agent — aggregator handles it

### `/mcp list` (User-facing, Telegram)

Shows auth type (but not token values):

```
- notion: connected (5 tools) [oauth]
- weather: connected (3 tools) [query_string]
- deepseek: connected (2 tools) [bearer]
```

### System Prompt Update

MCP instructions section adds:

> Authentication for all MCP servers is managed transparently by the RightClaw aggregator. You do not need to handle credentials or authentication flows. If a server reports `needs_auth`, tell the user to run `/mcp auth <server>` in Telegram.

Update existing MCP search strategy instructions to mention that both OAuth and API key servers are supported.

### Connection Verification

After credentials are assembled (token received from user, or URL already contains key), the bot attempts a full MCP connection before persisting:

1. Create temporary `ProxyBackend` with credentials
2. Call `connect()` — this initializes the MCP session and fetches tools
3. If connection succeeds → persist to SQLite, register in aggregator
4. If connection fails → discard, notify user with error details

This prevents adding servers with bad credentials.

## Files Changed

| File | Change |
|------|--------|
| `crates/rightclaw/src/memory/sql/v10_mcp_auth.sql` | New migration: add auth columns |
| `crates/rightclaw/src/memory/migrations.rs` | Register v10 migration |
| `crates/rightclaw/src/mcp/credentials.rs` | Extend `McpServerEntry`, add auth column read/write, migration from oauth-state.json |
| `crates/rightclaw/src/mcp/proxy.rs` | `AuthMethod` enum, extend `DynamicAuthClient` to support Bearer/Header/QueryString |
| `crates/rightclaw/src/mcp/refresh.rs` | Read/write SQLite instead of oauth-state.json |
| `crates/bot/src/telegram/handler.rs` | Rewrite `handle_mcp_add()`: OAuth discovery → haiku fallback → token prompt → verify → persist |
| `crates/rightclaw-cli/src/internal_api.rs` | Extend `/mcp-add` request/response with auth fields, extend `/set-token` to write SQLite |
| `crates/rightclaw-cli/src/aggregator.rs` | Redact URLs in `do_mcp_list()`, pass `AuthMethod` when creating `ProxyBackend` |
| `crates/rightclaw/src/mcp/internal_client.rs` | Extend `mcp_add()` request with auth fields |
| `crates/rightclaw/src/codegen/mcp_instructions.rs` | Update system prompt text |
| `crates/rightclaw/src/mcp/oauth.rs` | Extract `is_public_url()` helper (inverse of private IP check) |

## Out of Scope

- MCP servers with mTLS client certificate auth
- API key rotation/refresh for non-OAuth servers
- Per-tool auth (all tools on a server share one credential)
