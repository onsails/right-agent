# Early URL parsing in `handle_mcp_add`

## Context

The `/mcp add` handler in `handler.rs` receives a raw URL string from the user and uses it in multiple places (OAuth discovery, haiku AI classification, internal API registration) before it's ever validated. URL validation (`Url::parse` + scheme/host checks) only happens deep in the internal API handler (`internal_api.rs:168`), meaning:

1. Invalid URLs waste a haiku call and OAuth discovery HTTP requests before being rejected
2. Query string stripping uses naive `find('?')` instead of proper URL parsing
3. The `is_public_url` check parses the URL again redundantly

The fix: parse the URL once at the top of `handle_mcp_add` with `url::Url`, derive `bare_url` and `has_query` from the parsed result, validate early, and pass strings downstream.

## Files

- **Modify:** `crates/bot/src/telegram/handler.rs` — `handle_mcp_add` function (lines 735-894)
- **Read-only:** `crates/rightclaw/src/mcp/credentials.rs` — `validate_server_url`, `is_public_url`
- **Read-only:** `crates/rightclaw-cli/src/internal_api.rs` — `validate_server_url` call (stays as defense-in-depth)

## Changes

### `handle_mcp_add` in `handler.rs`

Replace lines 752-764 (current):
```rust
let original_url = parts[1];
// ...
let bare_url = match original_url.find('?') {
    Some(pos) => &original_url[..pos],
    None => original_url,
};
```

With:
```rust
let original_url = parts[1];

// Parse URL early — reject garbage before any network calls
let parsed = match url::Url::parse(original_url) {
    Ok(u) => u,
    Err(e) => {
        bot.send_message(msg.chat.id, format!("Invalid URL: {e}"))
            .await?;
        return Ok(());
    }
};

// Derive bare URL (without query string) and query presence from parsed URL
let has_query = parsed.query().is_some();
let bare_url = {
    let mut clean = parsed.clone();
    clean.set_query(None);
    clean.to_string()
};
```

Then update downstream references:
- `bare_url` is now `String` not `&str` — use `&bare_url` where needed
- Remove `let has_query = original_url.contains('?');` (line 798) — already derived above
- Replace `is_public_url(bare_url)` (line 799) with `is_public_url(&bare_url)` — no behavior change, still branches haiku vs bearer-default
- `url_to_register` (line 861-865): use `original_url` for query_string, `&bare_url` for others — same logic, just using properly parsed values

### What stays the same

- `validate_server_url` in `internal_api.rs:168` stays — defense-in-depth, also does scheme+host validation that we don't duplicate in handler
- `is_public_url` stays as the haiku gate — it's a semantic check (public vs private), not just URL parsing
- `redact_url` in `aggregator.rs` stays — it operates on stored URLs from DB, not user input

## Verification

1. `devenv shell -- cargo check --workspace` — no compilation errors
2. `devenv shell -- cargo test -p rightclaw --lib` — existing tests pass
3. Manual test scenarios (mental walkthrough):
   - `https://mcp.example.com/mcp` → parsed OK, no query, bare_url same as original
   - `https://mcp.example.com/mcp?key=secret` → parsed OK, has_query=true, bare_url strips `?key=secret`
   - `not-a-url` → `Url::parse` fails, user sees error immediately, no haiku/OAuth wasted
   - `http://mcp.example.com/mcp` → parsed OK, passes handler (scheme not checked here), rejected by `validate_server_url` in internal API
