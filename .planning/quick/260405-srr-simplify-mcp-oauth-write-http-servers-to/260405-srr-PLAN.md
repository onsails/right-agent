---
phase: quick
plan: 260405-srr
type: execute
wave: 1
depends_on: []
files_modified:
  # Delete
  - crates/rightclaw/src/mcp/oauth.rs
  - crates/rightclaw/src/mcp/refresh.rs
  - crates/bot/src/telegram/oauth_callback.rs
  # Gut/rewrite
  - crates/rightclaw/src/mcp/credentials.rs
  - crates/rightclaw/src/mcp/detect.rs
  - crates/rightclaw/src/mcp/mod.rs
  # Simplify
  - crates/bot/src/telegram/handler.rs
  - crates/bot/src/telegram/dispatch.rs
  - crates/bot/src/telegram/mod.rs
  - crates/bot/src/lib.rs
  - crates/rightclaw/src/doctor.rs
  # Cargo.toml cleanup
  - Cargo.toml
  - crates/rightclaw/Cargo.toml
  - crates/bot/Cargo.toml
  # New: .claude.json MCP helpers
  - crates/rightclaw/src/codegen/claude_json.rs
autonomous: true
must_haves:
  truths:
    - "CC handles OAuth natively for type:http MCP servers in .claude.json"
    - "/mcp add writes HTTP server to .claude.json projects.<agent_path>.mcpServers with type:http"
    - "/mcp remove removes from .claude.json"
    - "/mcp list shows both .claude.json HTTP servers and .mcp.json stdio servers"
    - "/mcp auth is replaced with guidance message"
    - "oauth.rs, refresh.rs, oauth_callback.rs are deleted"
    - "sha2, subtle, rand, base64, axum workspace deps are removed"
    - "cargo build --workspace succeeds with no errors"
  artifacts:
    - path: "crates/rightclaw/src/mcp/credentials.rs"
      provides: ".claude.json read/write helpers + atomic write"
    - path: "crates/rightclaw/src/mcp/detect.rs"
      provides: "MCP server listing from .claude.json + .mcp.json"
  key_links:
    - from: "crates/bot/src/telegram/handler.rs"
      to: "crates/rightclaw/src/mcp/credentials.rs"
      via: "add_http_server_to_claude_json / remove_http_server_from_claude_json"
      pattern: "mcp::credentials::"
---

<objective>
Simplify MCP OAuth by deleting the entire custom OAuth flow (oauth.rs, refresh.rs, oauth_callback.rs) and switching to CC's native OAuth handling via `type: "http"` entries in `.claude.json`.

Purpose: CC handles OAuth natively for HTTP MCP servers when they appear in .claude.json with `"type": "http"`. The 1500+ lines of custom PKCE/token-exchange/refresh/callback-server code are dead weight. This task deletes them and rewires `/mcp add|remove|list|auth` to use .claude.json instead of .mcp.json for HTTP servers.

Output: Leaner codebase, fewer dependencies, CC-native OAuth that just works.
</objective>

<execution_context>
@$HOME/.claude/get-shit-done/workflows/execute-plan.md
@$HOME/.claude/get-shit-done/templates/summary.md
</execution_context>

<context>
@CLAUDE.md
@CLAUDE.rust.md
@crates/rightclaw/src/mcp/mod.rs
@crates/rightclaw/src/mcp/credentials.rs
@crates/rightclaw/src/mcp/detect.rs
@crates/rightclaw/src/codegen/claude_json.rs
@crates/bot/src/telegram/handler.rs
@crates/bot/src/telegram/dispatch.rs
@crates/bot/src/telegram/mod.rs
@crates/bot/src/lib.rs
@crates/bot/Cargo.toml
@crates/rightclaw/Cargo.toml
@Cargo.toml
@crates/rightclaw/src/doctor.rs
</context>

<tasks>

<task type="auto" tdd="true">
  <name>Task 1: Delete OAuth files, gut credentials.rs, add .claude.json MCP helpers</name>
  <files>
    crates/rightclaw/src/mcp/oauth.rs (DELETE)
    crates/rightclaw/src/mcp/refresh.rs (DELETE)
    crates/rightclaw/src/mcp/mod.rs
    crates/rightclaw/src/mcp/credentials.rs
    crates/rightclaw/src/mcp/detect.rs
    crates/rightclaw/src/codegen/claude_json.rs
    crates/rightclaw/Cargo.toml
  </files>
  <behavior>
    - Test: add_http_server_to_claude_json creates .claude.json with projects.<path>.mcpServers.<name> = {type: "http", url: "<url>"} when file absent
    - Test: add_http_server_to_claude_json merges into existing .claude.json preserving other projects/fields
    - Test: remove_http_server_from_claude_json removes the named server entry
    - Test: remove_http_server_from_claude_json returns ServerNotFound when name absent
    - Test: list_all_mcp_servers returns HTTP servers from .claude.json AND stdio servers from .mcp.json
    - Test: list_all_mcp_servers returns empty when neither file exists
    - Test: detect.rs mcp_auth_status still works for .mcp.json url-bearing servers (existing tests pass, simplified to not check _rightclaw_oauth expiry)
  </behavior>
  <action>
    1. DELETE `crates/rightclaw/src/mcp/oauth.rs` and `crates/rightclaw/src/mcp/refresh.rs`.

    2. Update `crates/rightclaw/src/mcp/mod.rs`:
       Remove `pub mod oauth;` and `pub mod refresh;`. Keep `pub mod credentials;` and `pub mod detect;`.

    3. Gut `crates/rightclaw/src/mcp/credentials.rs`:
       - Remove `CredentialToken`, `OAuthMetadata` structs and all their impls.
       - Remove `write_bearer_to_mcp_json`, `read_bearer_from_mcp_json`, `write_oauth_metadata`, `read_oauth_metadata` — these are the old .mcp.json OAuth helpers.
       - Keep `CredentialError` (rename variants as needed), `write_json_atomic`, `read_mcp_json` (internal helper).
       - Add new functions for .claude.json MCP server management:
         - `add_http_server_to_claude_json(claude_json_path: &Path, agent_path_key: &str, server_name: &str, url: &str) -> Result<(), CredentialError>`:
           Read-modify-write .claude.json, creating structure if absent. Write to `projects.<agent_path_key>.mcpServers.<name>` = `{"type": "http", "url": url}`. Use `write_json_atomic`.
         - `remove_http_server_from_claude_json(claude_json_path: &Path, agent_path_key: &str, server_name: &str) -> Result<(), CredentialError>`:
           Read-modify-write, remove the named server. Return `CredentialError::ServerNotFound` if absent.
         - `list_http_servers_from_claude_json(claude_json_path: &Path, agent_path_key: &str) -> Result<Vec<(String, String)>, CredentialError>`:
           Return vec of (name, url) for all mcpServers in the project entry.
       - Keep `read_mcp_json` for listing stdio servers from .mcp.json (used by /mcp list).
       - Remove all old tests, write new tests for the new functions.

    4. Simplify `crates/rightclaw/src/mcp/detect.rs`:
       - `AuthState` enum: keep Present and Missing, remove Expired (CC manages auth state now, we can't peek into CC's credential store).
       - `ServerStatus`: keep name, url, add `source: ServerSource` enum (ClaudeJson | McpJson) to distinguish origin.
       - `mcp_auth_status` function: simplify to just list servers. For .mcp.json url-bearing servers, report Present if url exists (we can't check CC auth). For stdio, skip. This is now really just a "list" function.
       - Remove all `_rightclaw_oauth` metadata reading.
       - Update tests accordingly.

    5. In `crates/rightclaw/src/codegen/claude_json.rs`:
       - The existing `generate_agent_claude_json` already writes to `projects.<path>` in .claude.json.
       - No changes needed here — the new credentials.rs functions write to the same file using the same project key pattern.

    6. Update `crates/rightclaw/Cargo.toml`:
       - Remove: `sha2`, `subtle`, `rand`, `base64`.
       - Keep: `tempfile` (used by write_json_atomic).

    Ensure `cargo test -p rightclaw` passes after all changes.
  </action>
  <verify>
    <automated>cargo test -p rightclaw -- --no-fail-fast 2>&1 | tail -30</automated>
  </verify>
  <done>
    oauth.rs and refresh.rs deleted. credentials.rs gutted and replaced with .claude.json helpers. detect.rs simplified. sha2/subtle/rand/base64 removed from rightclaw Cargo.toml. All rightclaw crate tests pass.
  </done>
</task>

<task type="auto">
  <name>Task 2: Rewire bot — delete oauth_callback.rs, simplify handler/dispatch/lib.rs, clean deps</name>
  <files>
    crates/bot/src/telegram/oauth_callback.rs (DELETE)
    crates/bot/src/telegram/mod.rs
    crates/bot/src/telegram/handler.rs
    crates/bot/src/telegram/dispatch.rs
    crates/bot/src/lib.rs
    crates/bot/Cargo.toml
    crates/rightclaw/src/doctor.rs
    Cargo.toml
  </files>
  <action>
    1. DELETE `crates/bot/src/telegram/oauth_callback.rs`.

    2. Update `crates/bot/src/telegram/mod.rs`:
       - Remove `pub mod oauth_callback;` line.

    3. Update `crates/bot/src/telegram/handler.rs`:
       - Remove `use super::oauth_callback::PendingAuthMap;` import.
       - Remove `pending_auth: PendingAuthMap` parameter from `handle_mcp` signature.
       - Remove `home: Arc<RightclawHome>` parameter from `handle_mcp` (no longer needed for tunnel config).
       - **Replace `handle_mcp_auth`** entirely: instead of the 190-line OAuth flow, send a single message:
         ```
         "CC handles OAuth natively for HTTP MCP servers.\n\nTo add an HTTP server:\n  /mcp add <name> <url>\n\nCC will prompt for authentication when the agent connects."
         ```
       - **Rewrite `handle_mcp_add`**: Instead of writing to .mcp.json, call `rightclaw::mcp::credentials::add_http_server_to_claude_json()`. The `agent_path_key` is `agent_dir.canonicalize()?.display().to_string()`. The .claude.json path is `agent_dir.join(".claude.json")`.
       - **Rewrite `handle_mcp_remove`**: Call `rightclaw::mcp::credentials::remove_http_server_from_claude_json()` instead of editing .mcp.json.
       - **Rewrite `handle_mcp_list`**: Call `rightclaw::mcp::credentials::list_http_servers_from_claude_json()` for HTTP servers from .claude.json, and also read .mcp.json for stdio servers (rightmemory etc). Display both sections.
       - Update the subcommand usage strings accordingly. Remove `clientId` from /mcp add (CC handles that).

    4. Update `crates/bot/src/telegram/dispatch.rs`:
       - Remove `use super::oauth_callback::PendingAuthMap;` import.
       - Remove `pending_auth: PendingAuthMap` from `run_telegram` parameters.
       - Remove `pending_auth_arc` from `dptree::deps![]`.
       - Remove `home: PathBuf` parameter from `run_telegram` if only used for pending_auth flow.
       - Update `handle_mcp` endpoint wiring to match new signature (no pending_auth, no home).

    5. Update `crates/bot/src/lib.rs`:
       - Remove the entire OAuth callback server section (lines ~145-204): PendingAuthMap creation, global_config read (if only for OAuth), refresh scheduler spawn, OAuthCallbackState, pending auth cleanup, axum socket/handle, `tokio::select!` for axum+teloxide.
       - Simply call `telegram::run_telegram(token, config.allowed_chat_ids, agent_dir, args.debug).await` directly (no `tokio::select!` needed).
       - Remove the `use std::collections::HashMap; use std::sync::Arc;` block that was only for PendingAuthMap.
       - Remove unused imports of oauth_callback types.

    6. Update `crates/bot/Cargo.toml`:
       - Remove: `axum`, `rand`, `base64`, `subtle`, `reqwest` (bot no longer makes HTTP calls for OAuth — the only reqwest usage was in oauth_callback.rs and refresh scheduler).
       - Keep: `which` (used elsewhere? check — if only in handler.rs for cloudflared check, which is now removed, remove it too).

    7. Update `crates/rightclaw/src/doctor.rs`:
       - **Remove `check_mcp_tokens` and `check_mcp_tokens_impl`**: CC manages auth state, we cannot inspect CC's credential store. The check becomes meaningless.
       - **Remove `mcp_auth_issues` pub fn**: used by cmd_up to warn about unauthenticated servers. Remove callers in rightclaw-cli too (grep for `mcp_auth_issues`).
       - Remove `MCP_ISSUES_PREFIX` const.
       - Remove `check_mcp_tokens(home)` call from `run_doctor`.
       - The `check_tunnel_config` and `check_cloudflared_binary` checks can stay — tunnel is still useful for other things. But update the detail text to not mention "MCP OAuth callbacks" specifically.

    8. Update root `Cargo.toml` workspace dependencies:
       - Remove: `sha2`, `axum`, `subtle`, `rand`, `base64`.
       - Keep: `hex` (check if still used anywhere — if not, remove too).

    9. Search for any remaining references to removed types/functions and fix compilation:
       - `mcp_auth_issues` in rightclaw-cli/src/main.rs
       - `PendingAuth`, `PendingAuthMap` anywhere
       - `run_refresh_scheduler` anywhere
       - `OAuthCallbackState`, `run_oauth_callback_server`, `run_pending_auth_cleanup` anywhere
       - `write_bearer_to_mcp_json`, `read_bearer_from_mcp_json`, `write_oauth_metadata`, `read_oauth_metadata` anywhere
       - `CredentialToken`, `OAuthMetadata` anywhere

    Build workspace: `cargo build --workspace` must succeed. Run all tests: `cargo test --workspace`.
  </action>
  <verify>
    <automated>cargo build --workspace 2>&1 | tail -20 && cargo test --workspace -- --no-fail-fast 2>&1 | tail -30</automated>
  </verify>
  <done>
    oauth_callback.rs deleted. Bot lib.rs no longer starts axum callback server or refresh scheduler. handler.rs /mcp auth replaced with guidance message. /mcp add|remove write to .claude.json. axum/rand/base64/subtle/sha2 removed from workspace deps. `cargo build --workspace` and `cargo test --workspace` pass clean.
  </done>
</task>

</tasks>

<threat_model>
## Trust Boundaries

| Boundary | Description |
|----------|-------------|
| Telegram -> bot handler | User commands cross trust boundary |
| bot -> .claude.json filesystem | Bot writes config consumed by CC |

## STRIDE Threat Register

| Threat ID | Category | Component | Disposition | Mitigation Plan |
|-----------|----------|-----------|-------------|-----------------|
| T-quick-01 | T (Tampering) | .claude.json write | accept | File is in agent dir with agent-level permissions. CC reads it at startup. Same trust model as existing .mcp.json writes. |
| T-quick-02 | I (Info Disclosure) | OAuth removal | mitigate | Removing custom OAuth token storage from .mcp.json reduces credential exposure surface. CC's native credential store is encrypted. |
| T-quick-03 | S (Spoofing) | /mcp add server URL | accept | User provides URL via Telegram; same trust model as before. Bot validates URL format. |
</threat_model>

<verification>
1. `cargo build --workspace` compiles clean
2. `cargo test --workspace` all tests pass
3. `rg "oauth\.rs|refresh\.rs|oauth_callback" crates/` returns zero matches (except comments/docs)
4. `rg "sha2|axum|subtle" Cargo.toml crates/*/Cargo.toml` returns zero matches
5. `/mcp add notion https://mcp.notion.com/mcp` writes to .claude.json (manual test)
6. `/mcp list` shows servers from both .claude.json and .mcp.json
</verification>

<success_criteria>
- Zero custom OAuth code remains (oauth.rs, refresh.rs, oauth_callback.rs deleted)
- .claude.json used for HTTP MCP server config instead of .mcp.json
- 5 workspace deps removed (sha2, axum, subtle, rand, base64)
- Workspace builds and all tests pass
- /mcp add|remove|list work against .claude.json
- /mcp auth returns guidance message instead of OAuth flow
</success_criteria>

<output>
After completion, create `.planning/quick/260405-srr-simplify-mcp-oauth-write-http-servers-to/260405-srr-SUMMARY.md`
</output>
