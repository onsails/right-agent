# OAuth Refresh Token Persistence Fix

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix OAuth refresh token loss on token refresh — `db_update_oauth_token` doesn't persist the new `refresh_token` returned by providers that rotate refresh tokens (e.g. Composio), causing permanent auth failure after restart.

**Architecture:** Add `refresh_token` parameter to `db_update_oauth_token`. Update both callsites (scheduler timer-fire, reconnect task). Revert debug hack from investigation.

**Tech Stack:** Rust, rusqlite, tokio

---

### Task 1: Fix `db_update_oauth_token` to persist refresh_token

**Files:**
- Modify: `crates/rightclaw/src/mcp/credentials.rs:409-425` (function signature + SQL)
- Modify: `crates/rightclaw/src/mcp/credentials.rs:651-672` (existing test)

- [ ] **Step 1: Update the existing test to expect refresh_token persistence**

The existing `db_update_oauth_token_test` sets `refresh_token` to `"rt"` via `db_set_oauth_state`, then calls `db_update_oauth_token` but never checks that refresh_token is preserved or updated. Add a new refresh_token parameter and assert it's persisted.

```rust
#[test]
fn db_update_oauth_token_test() {
    let conn = setup_db();
    db_add_server(&conn, "notion", "https://mcp.notion.com/mcp").unwrap();
    db_set_oauth_state(
        &conn,
        "notion",
        "old",
        Some("rt"),
        "https://ex.com/token",
        "c",
        None,
        "2026-04-13T12:00:00Z",
    )
    .unwrap();
    db_update_oauth_token(&conn, "notion", "new-tok", Some("rt2"), "2026-04-13T13:00:00Z").unwrap();
    let servers = db_list_servers(&conn).unwrap();
    assert_eq!(servers[0].auth_token.as_deref(), Some("new-tok"));
    assert_eq!(servers[0].refresh_token.as_deref(), Some("rt2"));
    assert_eq!(
        servers[0].expires_at.as_deref(),
        Some("2026-04-13T13:00:00Z")
    );
}
```

- [ ] **Step 2: Add test for None refresh_token (provider didn't rotate)**

```rust
#[test]
fn db_update_oauth_token_keeps_old_refresh_when_none() {
    let conn = setup_db();
    db_add_server(&conn, "notion", "https://mcp.notion.com/mcp").unwrap();
    db_set_oauth_state(
        &conn,
        "notion",
        "old",
        Some("rt-original"),
        "https://ex.com/token",
        "c",
        None,
        "2026-04-13T12:00:00Z",
    )
    .unwrap();
    // Pass None — should keep "rt-original"
    db_update_oauth_token(&conn, "notion", "new-tok", None, "2026-04-13T13:00:00Z").unwrap();
    let servers = db_list_servers(&conn).unwrap();
    assert_eq!(servers[0].refresh_token.as_deref(), Some("rt-original"));
}
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `devenv shell -- cargo test -p rightclaw db_update_oauth_token`
Expected: compilation error — `db_update_oauth_token` doesn't accept refresh_token parameter yet.

- [ ] **Step 4: Update `db_update_oauth_token` signature and SQL**

```rust
pub fn db_update_oauth_token(
    conn: &Connection,
    name: &str,
    access_token: &str,
    refresh_token: Option<&str>,
    expires_at: &str,
) -> Result<(), CredentialError> {
    let changed = if let Some(rt) = refresh_token {
        conn.execute(
            "UPDATE mcp_servers SET auth_token = ?1, refresh_token = ?2, expires_at = ?3 WHERE name = ?4",
            rusqlite::params![access_token, rt, expires_at, name],
        )
    } else {
        conn.execute(
            "UPDATE mcp_servers SET auth_token = ?1, expires_at = ?2 WHERE name = ?3",
            rusqlite::params![access_token, expires_at, name],
        )
    }
    .map_err(map_db_err)?;
    if changed == 0 {
        return Err(CredentialError::ServerNotFound(name.to_string()));
    }
    Ok(())
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `devenv shell -- cargo test -p rightclaw db_update_oauth_token`
Expected: PASS (both tests)

- [ ] **Step 6: Commit**

```bash
git add crates/rightclaw/src/mcp/credentials.rs
git commit -m "fix: persist refresh_token in db_update_oauth_token

Providers that rotate refresh tokens (e.g. Composio) return a new
refresh_token on each refresh. The old function only persisted
access_token + expires_at, so the new refresh_token was lost.
After restart, the stale refresh_token was loaded from SQLite,
causing permanent 'invalid refresh token' errors."
```

### Task 2: Update callers to pass refresh_token

**Files:**
- Modify: `crates/rightclaw/src/mcp/refresh.rs:196` (scheduler timer-fire path)
- Modify: `crates/rightclaw-cli/src/main.rs:613` (reconnect task)

- [ ] **Step 1: Fix the scheduler timer-fire callsite in refresh.rs**

At line ~196, change `db_update_oauth_token` call to pass the new refresh_token from `new_entry`:

```rust
if let Err(e) = crate::mcp::credentials::db_update_oauth_token(
    &conn,
    &name,
    &access_token,
    new_entry.refresh_token.as_deref(),
    &expires_at,
) {
```

- [ ] **Step 2: Fix the reconnect task callsite in main.rs**

At line ~613, change `db_update_oauth_token` call to pass the new refresh_token from `new_state`:

```rust
if let Err(e) = rightclaw::mcp::credentials::db_update_oauth_token(
    &conn,
    &sn,
    &access_token,
    new_state.refresh_token.as_deref(),
    &new_state.expires_at.to_rfc3339(),
) {
```

- [ ] **Step 3: Build workspace to verify compilation**

Run: `devenv shell -- cargo build --workspace`
Expected: success, no errors

- [ ] **Step 4: Run all tests**

Run: `devenv shell -- cargo test --workspace`
Expected: all pass

- [ ] **Step 5: Commit**

```bash
git add crates/rightclaw/src/mcp/refresh.rs crates/rightclaw-cli/src/main.rs
git commit -m "fix: pass refresh_token to db_update_oauth_token at both callsites"
```

### Task 3: Revert debug hack and clean up

**Files:**
- Modify: `crates/rightclaw-cli/src/main.rs:556-569`

- [ ] **Step 1: Revert the forced immediate refresh hack**

Replace the debug block back to original logic (remove TODO comment, `immediate_state`, fake `expires_at`):

```rust
                // Send NewEntry for non-expired OAuth servers. Expired tokens
                // are handled by the reconnect task which sends NewEntry after refresh.
                for (name, (state, token_arc)) in &oauth_map {
                    if state.refresh_token.is_some() {
                        let due_in = rightclaw::mcp::refresh::refresh_due_in(state);
                        if due_in > std::time::Duration::ZERO {
                            let msg = rightclaw::mcp::refresh::RefreshMessage::NewEntry {
                                server_name: name.clone(),
                                state: state.clone(),
                                token: token_arc.clone(),
                            };
```

- [ ] **Step 2: Build to verify**

Run: `devenv shell -- cargo build --workspace`
Expected: success

- [ ] **Step 3: Commit**

```bash
git add crates/rightclaw-cli/src/main.rs
git commit -m "chore: revert debug hack for immediate OAuth refresh on startup"
```
