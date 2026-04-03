# Phase 35: Token Refresh - Research

**Researched:** 2026-04-03
**Domain:** OAuth 2.0 refresh token grant, tokio async scheduling, Rust struct extension
**Confidence:** HIGH

## Summary

Phase 35 adds automatic MCP OAuth token refresh to the bot process. No CLI command is involved — the bot owns a background tokio task (the "refresh scheduler") that runs for the lifetime of the bot process. It wakes on startup, refreshes any expired/near-expiry tokens immediately, then sleeps each token to `expires_at - 10min` and refreshes again. Tokens with `expires_at == 0` are never touched.

The work breaks into four tightly-coupled concerns: (1) extend `CredentialToken` with `client_id`/`client_secret`, (2) backfill Phase 34's OAuth callback to write those fields, (3) implement `mcp::refresh` module with the scheduler and `refresh_token_for_server()`, (4) add a `check_mcp_tokens` doctor check.

The existing codebase provides all the building blocks. `discover_as` is called as-is to resolve `token_endpoint` on each refresh cycle. `read_credential` / `write_credential` are used without modification. The tokio spawn pattern is established in `crates/bot/src/lib.rs` (cron task) and is directly reusable. The doctor check pattern is established in `doctor.rs`.

**Primary recommendation:** Implement in four sequential tasks — struct extension, scheduler module, bot integration, doctor check — treating the scheduler as the core deliverable since REFRESH-03 (doctor) and REFRESH-04 (skip `expires_at=0`) are mechanically straightforward once the scheduler exists.

<user_constraints>
## User Constraints (from CONTEXT.md)

### Locked Decisions

**D-01: No CLI `rightclaw mcp refresh` command**
REFRESH-01 is superseded. The only refresh mechanism is the bot's background task.
Rationale: rightclaw up already warns on expired tokens (DETECT-02 from Phase 33).
The bot handles refresh at startup and on schedule — operator never needs to invoke refresh manually.

**D-02: No proactive refresh in `rightclaw up`**
REFRESH-02 is superseded. `rightclaw up` continues to warn (Warn, non-fatal) when tokens are
expired (existing DETECT-02 behaviour from Phase 33). It does NOT attempt to refresh.
The bot refreshes immediately on startup if it finds expired tokens.

**D-03: Bot smart refresh scheduler**
The bot process runs a background tokio task that implements this algorithm:

On startup:
1. Scan all credentials for this agent from `~/.claude/.credentials.json`
2. For each token where `expires_at > 0`:
   - If already expired -> refresh immediately
   - If expiry is within 10 minutes -> refresh immediately
   - Otherwise -> schedule for `expires_at - 10min`
3. `expires_at == 0` -> skip (non-expiring, REFRESH-04)

Scheduled refresh:
1. `tokio::time::sleep_until(expires_at - 10min)`
2. Attempt refresh (POST to token_endpoint with `refresh_token`)
3. On success -> write new token, reschedule for new `expires_at - 10min`
4. On failure -> log `tracing::warn!`, do NOT crash; reschedule retry in 5 minutes
5. Retry up to 3 times; if all fail -> log error, stop scheduling (operator sees Warn in doctor)

**D-04: Refresh mechanism — re-run AS discovery**
On each refresh cycle, re-run the AS discovery chain (RFC 9728 -> RFC 8414 -> OIDC) to get
`token_endpoint`. The existing `mcp::oauth::discover_as` function is reused.

**D-05: client_id stored in CredentialToken**
`CredentialToken` is extended with:
- `client_id: Option<String>` — stored at OAuth flow completion time
- `client_secret: Option<String>` — stored if DCR returned one; None for public clients

CC ignores unknown fields in credentials.json (serde_json deserialization is lenient).
These fields are written by the bot at the end of the OAuth flow (Phase 34) AND read
by the refresh scheduler (Phase 35). Phase 34's bot code must be updated to write these
when it writes the `CredentialToken`.

Note: The refresh POST for a public client uses `client_id` only (no client_secret).
For confidential clients, both are sent as form parameters.

**D-06: Doctor aggregated MCP token check**
REFRESH-03 adds one new `DoctorCheck` named `"mcp-tokens"` to `doctor::run_doctor`.
- Walks all agents, calls `mcp::detect::mcp_auth_status` per agent
- If any server has `AuthState::Missing` or `AuthState::Expired` -> Warn with message listing `agent/server` pairs
- If all tokens are Present or `expiresAt=0` (non-expiring) -> Pass
- Warn severity (non-fatal), same pattern as existing doctor checks

**D-07: REFRESH-04 — expiresAt=0 is non-expiring**
Tokens with `expires_at == 0` are NEVER scheduled for refresh. The scheduler skips them.
The doctor check also counts them as "ok" (not missing, not expired).

### Claude's Discretion

- Token endpoint URL caching within a single refresh cycle (fine to re-discover each time)
- Exact retry backoff strategy (3 retries x 5 min is acceptable)
- How the scheduler handles multiple tokens for the same agent (one task per token, or one task per agent)
- Error message format for failed refresh in Telegram (brief Warn in chat, or silent log only)

### Deferred Ideas (OUT OF SCOPE)

- `/mcp refresh <server>` Telegram bot command — deferred or eliminated (auto-refresh makes it unnecessary)
- Per-agent credential isolation (SEED-004 territory) — deferred to v2.1
</user_constraints>

<phase_requirements>
## Phase Requirements

| ID | Description | Research Support |
|----|-------------|------------------|
| REFRESH-01 | Operator can run `rightclaw mcp refresh` for on-demand refresh | Superseded by D-01: no CLI command. Bot scheduler is the only mechanism. |
| REFRESH-02 | `rightclaw up` proactively refreshes expired tokens before launch | Superseded by D-02: `rightclaw up` keeps its existing Warn. Bot handles refresh on startup. |
| REFRESH-03 | `rightclaw doctor` reports missing/expired MCP OAuth tokens per agent (Warn severity) | Implemented as `check_mcp_tokens(home)` in `doctor.rs`. Uses existing `mcp_auth_status` per agent. |
| REFRESH-04 | Tokens with `expiresAt=0` skipped by refresh loop (non-expiring) | Handled in scheduler with explicit `if expires_at == 0 { continue }` guard before computing sleep duration. |
</phase_requirements>

## Standard Stack

### Core (already in workspace — no new deps needed)

| Library | Version | Purpose | Why Standard |
|---------|---------|---------|--------------|
| tokio | 1.50.0 | `tokio::spawn`, `tokio::time::sleep_until`, `tokio::time::Instant` | Already in workspace, async runtime for all bot work |
| reqwest | 0.13.2 | HTTP POST for refresh token grant | Already in workspace, used by `exchange_token` and `discover_as` |
| serde / serde_json | 1.0 | Serialize new `CredentialToken` fields | Already in workspace |
| tracing | 0.1 | `tracing::warn!` / `tracing::error!` on refresh failures | Already in workspace |

No new cargo dependencies are required for this phase.

**Version verification:** All packages already present in Cargo.lock. No installs needed.

## Architecture Patterns

### Recommended Project Structure (additions only)

```
crates/rightclaw/src/mcp/
├── credentials.rs    # MODIFY: add client_id/client_secret to CredentialToken
├── detect.rs         # unchanged
├── mod.rs            # MODIFY: pub mod refresh
├── oauth.rs          # unchanged — discover_as and exchange_token reused as-is
└── refresh.rs        # NEW: RefreshError, refresh_token_for_server(), run_refresh_scheduler()

crates/bot/src/
├── lib.rs            # MODIFY: tokio::spawn(run_refresh_scheduler(...))
└── telegram/
    └── oauth_callback.rs  # MODIFY: write client_id/client_secret into CredentialToken

crates/rightclaw/src/
└── doctor.rs         # MODIFY: add check_mcp_tokens(), call in run_doctor()
```

### Pattern 1: CredentialToken struct extension

`CredentialToken` in `credentials.rs` gets two new optional fields. The `#[serde(skip_serializing_if = "Option::is_none")]` annotation ensures existing credentials files written without these fields round-trip cleanly — serde_json's `Option<T>` deserialization defaults to `None` for absent keys.

```rust
// crates/rightclaw/src/mcp/credentials.rs
#[derive(Serialize, Deserialize, Clone)]
pub struct CredentialToken {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub token_type: Option<String>,
    pub scope: Option<String>,
    #[serde(rename = "expiresAt")]
    pub expires_at: u64,
    // NEW in Phase 35 — used by refresh scheduler
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_secret: Option<String>,
}
```

The `Debug` impl redacts `access_token` and `refresh_token`. It should also redact `client_secret`.

### Pattern 2: OAuth refresh token grant (RFC 6749 §6)

The refresh grant is a form-encoded POST to `token_endpoint`. It is structurally similar to `exchange_token` but uses `grant_type=refresh_token`. No PKCE verifier is required.

```rust
// crates/rightclaw/src/mcp/refresh.rs
async fn post_refresh_grant(
    client: &reqwest::Client,
    token_endpoint: &str,
    refresh_token: &str,
    client_id: &str,
    client_secret: Option<&str>,
) -> Result<TokenResponse, RefreshError> {
    let mut params = vec![
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token),
        ("client_id", client_id),
    ];
    let secret_owned;
    if let Some(s) = client_secret {
        secret_owned = s.to_string();
        params.push(("client_secret", &secret_owned));
    }
    let resp = client.post(token_endpoint).form(&params).send().await
        .map_err(|e| RefreshError::HttpFailed(format!("{e:#}")))?;
    // ... parse TokenResponse same as exchange_token
}
```

`TokenResponse` from `oauth.rs` is already defined and can be reused directly (it has `access_token`, `refresh_token`, `expires_in`).

### Pattern 3: tokio::time::sleep_until with Instant arithmetic

The scheduler uses `tokio::time::Instant` (not `std::time::Instant`) to integrate with tokio's timer wheel. Computing the deadline from a Unix timestamp:

```rust
use tokio::time::{sleep_until, Instant};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

fn deadline_from_unix(expires_at_secs: u64, buffer_secs: u64) -> Option<Instant> {
    let now_unix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    // Guard against underflow: if expires_at <= buffer, treat as already expired
    if expires_at_secs <= buffer_secs || expires_at_secs <= now_unix {
        return None; // caller should refresh immediately
    }

    let target_unix = expires_at_secs - buffer_secs;
    let secs_from_now = target_unix.saturating_sub(now_unix);
    Some(Instant::now() + Duration::from_secs(secs_from_now))
}
```

CRITICAL: `expires_at == 0` must be checked BEFORE this function is called. Integer subtraction `0 - buffer` wraps to `u64::MAX` and causes a multi-year sleep. The guard in D-07 prevents this.

### Pattern 4: Per-token refresh loop with retry

Each token gets its own tokio task (simpler than one task per agent; avoids coupling independent token lifetimes). The loop is modeled on the cron scheduler's `run_job_loop` pattern in `crates/bot/src/cron.rs`:

```rust
// crates/rightclaw/src/mcp/refresh.rs
async fn run_token_refresh_loop(
    agent_dir: PathBuf,
    credentials_path: PathBuf,
    server_name: String,
    server_url: String,
    http_client: reqwest::Client,
) {
    const BUFFER_SECS: u64 = 600; // 10 minutes
    const RETRY_SECS: u64 = 300;  // 5 minutes
    const MAX_RETRIES: u32 = 3;

    let mut retries = 0u32;

    loop {
        // Read current token (may have been updated by another path)
        let token = match read_credential(&credentials_path, &server_name, &server_url) { ... };

        // REFRESH-04: skip non-expiring tokens
        if token.expires_at == 0 { return; }

        // Sleep until refresh window
        if let Some(deadline) = deadline_from_unix(token.expires_at, BUFFER_SECS) {
            sleep_until(deadline).await;
        }
        // else: expired or within buffer — refresh immediately

        // Attempt refresh
        match refresh_token_for_server(&http_client, &credentials_path, &server_name, &server_url).await {
            Ok(new_expires_at) => {
                retries = 0;
                if new_expires_at == 0 { return; } // provider issued non-expiring replacement
            }
            Err(e) => {
                tracing::warn!(server = %server_name, retries, "token refresh failed: {e:#}");
                retries += 1;
                if retries >= MAX_RETRIES {
                    tracing::error!(server = %server_name, "token refresh failed {MAX_RETRIES} times — stopping scheduler");
                    return;
                }
                tokio::time::sleep(Duration::from_secs(RETRY_SECS)).await;
            }
        }
    }
}
```

### Pattern 5: Scheduler startup scan

```rust
// crates/rightclaw/src/mcp/refresh.rs
pub async fn run_refresh_scheduler(
    agent_dir: PathBuf,
    credentials_path: PathBuf,
    http_client: reqwest::Client,
) {
    // Read .mcp.json to discover server names + URLs
    let mcp_path = agent_dir.join(".mcp.json");
    let statuses = match mcp_auth_status(&mcp_path, &credentials_path) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!("refresh scheduler: failed to read MCP status: {e:#}");
            return;
        }
    };

    for status in statuses {
        // Only schedule for servers that have a stored token
        if status.state == AuthState::Missing { continue; }

        let creds = credentials_path.clone();
        let agent = agent_dir.clone();
        let client = http_client.clone();
        let name = status.name.clone();
        let url = status.url.clone();
        tokio::spawn(async move {
            run_token_refresh_loop(agent, creds, name, url, client).await;
        });
    }
}
```

`mcp_auth_status` is already in `mcp::detect` — the scheduler reuses it to enumerate token-bearing servers without re-parsing `.mcp.json` manually.

### Pattern 6: Bot lib.rs spawn site

Model on the existing cron spawn in `crates/bot/src/lib.rs` (line 122-124):

```rust
// After the cron spawn, before axum spawn:
let refresh_agent_dir = agent_dir.clone();
let refresh_creds = credentials_path.clone();
let refresh_client = reqwest::Client::new();
tokio::spawn(async move {
    rightclaw::mcp::refresh::run_refresh_scheduler(
        refresh_agent_dir,
        refresh_creds,
        refresh_client,
    ).await;
});
```

`credentials_path` is already computed at line 143-147 of `lib.rs`. Clone before the oauth_state move.

### Pattern 7: Doctor check_mcp_tokens

Follow the `check_webhook_info_for_agents` pattern (already walks agents, parses config). The new check aggregates across ALL agents into ONE `DoctorCheck` named `"mcp-tokens"`:

```rust
fn check_mcp_tokens(home: &Path) -> DoctorCheck {
    let agents_dir = home.join("agents");
    let credentials_path = dirs::home_dir()
        .unwrap_or_default()
        .join(".claude").join(".credentials.json");

    let mut problems: Vec<String> = vec![];

    // walk agents/, call mcp_auth_status per agent
    for entry in ... {
        let mcp_path = agent_path.join(".mcp.json");
        let statuses = match mcp_auth_status(&mcp_path, &credentials_path) { ... };
        for s in statuses {
            if matches!(s.state, AuthState::Missing | AuthState::Expired) {
                problems.push(format!("{agent_name}/{}", s.name));
            }
        }
    }

    if problems.is_empty() {
        DoctorCheck { name: "mcp-tokens".into(), status: CheckStatus::Pass, detail: "all present".into(), fix: None }
    } else {
        DoctorCheck {
            name: "mcp-tokens".into(),
            status: CheckStatus::Warn,
            detail: format!("missing/expired: {}", problems.join(", ")),
            fix: Some("Run /mcp auth <server> in Telegram to authenticate".into()),
        }
    }
}
```

### Anti-Patterns to Avoid

- **Integer underflow on `expires_at=0`:** `expires_at - buffer_secs` wraps to `u64::MAX`. Always check `expires_at == 0` first. This is REFRESH-04.
- **Blocking HTTP in sync context:** `refresh_token_for_server` is async; don't call it with `block_in_place` like `fetch_webhook_url`. The scheduler lives in an async tokio task.
- **Reading `.mcp.json` manually in the scheduler:** Use `mcp_auth_status` — it already handles missing files, stdio servers, and error cases.
- **Panic on missing `refresh_token`:** Tokens may have been written without a `refresh_token` (implicit grant or already-refreshed-once providers that rotate). Log a warn and skip, don't panic.
- **Not cloning credentials_path before the oauth_state move:** `lib.rs` moves `credentials_path` into `OAuthCallbackState`. The refresh scheduler needs its own copy; clone before the `oauth_state` struct construction.

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| Token endpoint discovery | Custom URL builder | `mcp::oauth::discover_as` (already exists) | Handles RFC 9728 -> RFC 8414 -> OIDC chain, error handling included |
| HTTP form POST | Raw reqwest builder | Pattern from `exchange_token` in `oauth.rs` | Same grant mechanics, same `reqwest::Client::post().form()` pattern |
| Credential read/write | Direct file I/O | `mcp::credentials::read_credential` / `write_credential` | Atomic write, backup rotation, correct key derivation already implemented |
| Server enumeration | Parse `.mcp.json` manually | `mcp::detect::mcp_auth_status` | Handles stdio skip, missing file, sorted results |
| Async scheduling | Custom timer loop | `tokio::time::sleep_until` | Standard; handles long sleeps accurately |

**Key insight:** All infrastructure exists. Phase 35 assembles it into a scheduler — it does not introduce new primitives.

## Common Pitfalls

### Pitfall 1: expires_at == 0 Underflow
**What goes wrong:** `expires_at - 600` with `expires_at == 0` wraps to `u64::MAX` (18446744073709550616), producing a sleep of ~584 billion years.
**Why it happens:** Rust u64 arithmetic in release builds is wrapping (no panic). Linear and similar providers issue tokens with `expires_at = 0` meaning "non-expiring."
**How to avoid:** First check in the loop body: `if token.expires_at == 0 { return; }`. This is REFRESH-04.
**Warning signs:** Scheduler task spawned but never actually refreshes anything for Linear.

### Pitfall 2: Missing refresh_token Panic
**What goes wrong:** Attempting `token.refresh_token.unwrap()` crashes the bot process.
**Why it happens:** Some providers do not include a `refresh_token` in the initial token response (e.g., providers using implicit grant or short-lived tokens they expect to be re-authed). Also: some providers rotate `refresh_token` on each refresh — old token is one-time-use.
**How to avoid:** Check `token.refresh_token.is_some()` before scheduling. If absent, log `tracing::warn!` and skip the token (can't refresh without it). The doctor check will surface it as expired when it eventually expires.
**Warning signs:** Bot crash in refresh loop with "called `Option::unwrap()` on a `None` value."

### Pitfall 3: credentials_path Moved Before Refresh Spawn
**What goes wrong:** Compile error "value used after move" in `lib.rs`.
**Why it happens:** `credentials_path` is moved into `OAuthCallbackState` struct. The refresh scheduler also needs it.
**How to avoid:** Clone `credentials_path` before constructing `OAuthCallbackState`. Pattern established for `agent_dir` (already cloned for cron in `lib.rs`).

### Pitfall 4: Token expiry timestamp units
**What goes wrong:** Token is refreshed immediately on every loop iteration.
**Why it happens:** `expires_in` from the token response is in **seconds** (RFC 6749 §5.1), but code accidentally treats it as milliseconds, computing an `expires_at` already in the past.
**How to avoid:** `expires_at = now_unix + token_response.expires_in`. Both sides in seconds.

### Pitfall 5: CredentialToken Debug leaks client_secret
**What goes wrong:** `client_secret` value appears in logs.
**Why it happens:** The manually-implemented `Debug` for `CredentialToken` lists fields explicitly. New fields added without updating the impl will use `Debug` on the raw value.
**How to avoid:** Update the `Debug` impl in `credentials.rs` to redact `client_secret` the same way `refresh_token` is redacted.

### Pitfall 6: Doctor check uses blocking HTTP in sync context
**What goes wrong:** Doctor hangs or panics with "Cannot block the current thread from within an async context."
**Why it happens:** `check_mcp_tokens` is called from `run_doctor` which is synchronous. `mcp_auth_status` is sync (reads files only) — so no issue. The pitfall is if someone adds an async call inside the doctor check.
**How to avoid:** `check_mcp_tokens` must be purely synchronous (file I/O only). No HTTP calls. This mirrors every other existing doctor check.

## Code Examples

### Refresh grant POST (RFC 6749 §6)

```rust
// Source: RFC 6749 §6 — https://datatracker.ietf.org/doc/html/rfc6749#section-6
// Pattern mirrors exchange_token() in crates/rightclaw/src/mcp/oauth.rs:405
let mut params: Vec<(&str, &str)> = vec![
    ("grant_type", "refresh_token"),
    ("refresh_token", refresh_token_str),
    ("client_id", client_id_str),
];
let secret_owned: String;
if let Some(s) = client_secret {
    secret_owned = s.to_string();
    params.push(("client_secret", &secret_owned));
}
let resp = client
    .post(token_endpoint)
    .form(&params)
    .send()
    .await?;
```

### tokio::time::sleep_until pattern

```rust
// Source: tokio docs — https://docs.rs/tokio/latest/tokio/time/fn.sleep_until.html
use tokio::time::{sleep_until, Instant};
use std::time::Duration;

let deadline = Instant::now() + Duration::from_secs(secs_until_refresh);
sleep_until(deadline).await;
```

### serde skip_serializing_if for optional fields

```rust
// Source: serde docs — https://serde.rs/field-attrs.html#skip_serializing_if
#[serde(skip_serializing_if = "Option::is_none")]
pub client_id: Option<String>,
```

This prevents `"client_id": null` from appearing in credentials.json for old tokens that were written before Phase 35. CC's serde_json deserialization is lenient — absent keys deserialize to `None` for `Option<T>` fields.

### Writing client_id at OAuth completion (Phase 34 backfill)

In `crates/bot/src/telegram/oauth_callback.rs`, the `CredentialToken` construction (around line 220) must include the new fields from `PendingAuth`:

```rust
// PendingAuth already has client_id: String and client_secret: Option<String>
let cred_token = CredentialToken {
    access_token: token_resp.access_token.clone(),
    refresh_token: token_resp.refresh_token.clone(),
    token_type: token_resp.token_type.clone(),
    scope: token_resp.scope.clone(),
    expires_at,
    client_id: Some(pending.client_id.clone()),   // NEW
    client_secret: pending.client_secret.clone(), // NEW
};
```

## State of the Art

| Old Approach | Current Approach | When Changed | Impact |
|--------------|------------------|--------------|--------|
| Manual token refresh via CLI | Background bot scheduler | Phase 35 | Operator never needs to think about refresh |
| CC headless token refresh (broken) | rightclaw owns all refresh logic | v3.2 research (CC issues #28262, #29718) | Scheduler is mandatory, not optional |

## Open Questions

1. **Does the refresh token rotate on each use?**
   - What we know: Some OAuth servers (Notion, GitHub) issue a new `refresh_token` on each refresh grant. Others reuse the same one.
   - What's unclear: Which providers used in this project rotate vs. not.
   - Recommendation: Always write the new token from the refresh response (including the new `refresh_token` if present). If the provider did not return a new `refresh_token`, keep the old one. This is safe regardless.

2. **What if the bot restarts while a token is mid-expiry?**
   - What we know: The scheduler runs on startup and re-reads `expires_at` from the credential file each cycle.
   - What's unclear: Nothing — restart re-triggers the full scan, and the scheduler picks up the current expiry correctly. Not a concern.

3. **Should failed refreshes be reported to Telegram?**
   - What we know: D-03 says log `tracing::warn!`. Context says "brief Warn in chat, or silent log only" is Claude's discretion.
   - Recommendation: Silent log only for failed refreshes. Telegram message on persistent failure (after MAX_RETRIES) would be noisy for transient network errors. Doctor already surfaces expired tokens.

## Environment Availability

Step 2.6: SKIPPED (no external dependencies — all required tooling already present in workspace).

## Sources

### Primary (HIGH confidence)
- Existing codebase — `crates/rightclaw/src/mcp/credentials.rs`, `oauth.rs`, `detect.rs` — read directly
- Existing codebase — `crates/rightclaw/src/doctor.rs` — read directly for DoctorCheck patterns
- Existing codebase — `crates/bot/src/lib.rs`, `cron.rs` — read directly for tokio spawn patterns
- RFC 6749 §6 — Refresh Token grant: https://datatracker.ietf.org/doc/html/rfc6749#section-6

### Secondary (MEDIUM confidence)
- serde field attributes: https://serde.rs/field-attrs.html#skip_serializing_if
- tokio::time::sleep_until: https://docs.rs/tokio/latest/tokio/time/fn.sleep_until.html

## Metadata

**Confidence breakdown:**
- Standard stack: HIGH — no new crates, all existing
- Architecture: HIGH — all patterns have direct analogs in codebase
- Pitfalls: HIGH — identified from direct code inspection (u64 underflow, moved value, missing refresh_token)

**Research date:** 2026-04-03
**Valid until:** 2026-05-03 (stable OAuth spec, no fast-moving deps)
