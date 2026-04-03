# Phase 35: Token Refresh — Context

**Gathered:** 2026-04-03
**Status:** Ready for planning

<domain>
## Phase Boundary

Phase 35 delivers automatic MCP OAuth token refresh. The bot process owns all refresh
logic — there is no CLI `rightclaw mcp refresh` command. The bot runs a smart scheduler
(tokio task) that refreshes tokens proactively before expiry and immediately on startup
if any token is already expired.

**What's NOT in this phase:**
- `rightclaw mcp refresh` CLI command — eliminated (REFRESH-01 superseded, see D-01)
- `rightclaw up` proactive refresh — eliminated (REFRESH-02 superseded by bot startup, see D-02)
- `expiresAt=0` tokens are skipped by the refresh loop (REFRESH-04 still applies — non-expiring)
- `/mcp refresh` Telegram bot command — no user-facing command; refresh is fully automatic

</domain>

<decisions>
## Implementation Decisions

### D-01: No CLI `rightclaw mcp refresh` command
REFRESH-01 is superseded. The only refresh mechanism is the bot's background task.
Rationale: rightclaw up already warns on expired tokens (DETECT-02 from Phase 33).
The bot handles refresh at startup and on schedule — operator never needs to invoke refresh manually.

### D-02: No proactive refresh in `rightclaw up`
REFRESH-02 is superseded. `rightclaw up` continues to warn (Warn, non-fatal) when tokens are
expired (existing DETECT-02 behaviour from Phase 33). It does NOT attempt to refresh.
The bot refreshes immediately on startup if it finds expired tokens.

### D-03: Bot smart refresh scheduler
The bot process runs a background tokio task that implements this algorithm:

**On startup:**
1. Scan all credentials for this agent from `~/.claude/.credentials.json`
2. For each token where `expires_at > 0`:
   - If already expired → refresh immediately
   - If expiry is within 10 minutes → refresh immediately
   - Otherwise → schedule for `expires_at - 10min`
3. `expires_at == 0` → skip (non-expiring, REFRESH-04)

**Scheduled refresh:**
1. `tokio::time::sleep_until(expires_at - 10min)`
2. Attempt refresh (POST to token_endpoint with `refresh_token`)
3. On success → write new token, reschedule for new `expires_at - 10min`
4. On failure → log `tracing::warn!`, do NOT crash; reschedule retry in 5 minutes
5. Retry up to 3 times; if all fail → log error, stop scheduling (operator sees Warn in doctor)

### D-04: Refresh mechanism — re-run AS discovery
On each refresh cycle, re-run the AS discovery chain (RFC 9728 → RFC 8414 → OIDC) to get
`token_endpoint`. The existing `mcp::oauth::discover_as` function is reused.
Trade-off: 2–3 HTTP calls per refresh cycle (acceptable — refresh runs every ~50 min).
Benefit: no stale endpoint URLs; always reflects current AS configuration.

### D-05: client_id stored in CredentialToken
AS discovery gives `token_endpoint` but not `client_id`. The refresh POST requires `client_id`.
`CredentialToken` is extended with:
- `client_id: Option<String>` — stored at OAuth flow completion time
- `client_secret: Option<String>` — stored if DCR returned one; None for public clients

CC ignores unknown fields in credentials.json (serde_json deserialization is lenient).
These fields are written by the bot at the end of the OAuth flow (Phase 34) AND read
by the refresh scheduler (Phase 35). Phase 34's bot code must be updated to write these
when it writes the `CredentialToken`.

**Note:** The refresh POST for a public client uses `client_id` only (no client_secret).
For confidential clients, both are sent as form parameters.

### D-06: Doctor aggregated MCP token check
REFRESH-03 adds one new `DoctorCheck` named `"mcp-tokens"` to `doctor::run_doctor`.
- Walks all agents, calls `mcp::detect::mcp_auth_status` per agent
- If any server has `AuthState::Missing` or `AuthState::Expired` → Warn with message listing `agent/server` pairs
- If all tokens are Present or `expiresAt=0` (non-expiring) → Pass
- Warn severity (non-fatal), same pattern as existing doctor checks
- cloudflared binary check already exists from Phase 34 (D-03) — no change needed

### D-07: REFRESH-04 — expiresAt=0 is non-expiring
Tokens with `expires_at == 0` are NEVER scheduled for refresh. The scheduler skips them.
The doctor check also counts them as "ok" (not missing, not expired).
This handles Linear and similar providers that issue non-expiring tokens.

### Claude's Discretion
- Token endpoint URL caching within a single refresh cycle (fine to re-discover each time)
- Exact retry backoff strategy (3 retries × 5 min is acceptable)
- How the scheduler handles multiple tokens for the same agent (one task per token, or one task per agent)
- Error message format for failed refresh in Telegram (brief Warn in chat, or silent log only)

</decisions>

<canonical_refs>
## Canonical References

**Downstream agents MUST read these before planning or implementing.**

### Requirements
- `.planning/REQUIREMENTS.md` — REFRESH-01 (superseded), REFRESH-02 (superseded), REFRESH-03, REFRESH-04

### Foundation (Phase 32–34)
- `crates/rightclaw/src/mcp/credentials.rs` — `CredentialToken` (to be extended), `write_credential`, `read_credential`, `mcp_oauth_key`
- `crates/rightclaw/src/mcp/detect.rs` — `mcp_auth_status`, `AuthState` (used by doctor check)
- `crates/rightclaw/src/mcp/oauth.rs` — `discover_as` (reused for token_endpoint), `PendingAuth`, `TokenResponse`
- `crates/rightclaw/src/doctor.rs` — existing DoctorCheck pattern, `run_doctor` function, `check_cloudflared_binary` (already added in Phase 34)

### Bot process (Phase 23–26, extended Phase 34)
- `crates/bot/` — find the bot crate, existing tokio spawn patterns, Telegram message sending

### External references
- OAuth 2.1 refresh token grant: RFC 6749 §6 (token refresh flow)
- CC issue #29718: <https://github.com/anthropics/claude-code/issues/29718> — why CC doesn't refresh in headless mode (rightclaw owns this)

</canonical_refs>

<code_context>
## Existing Code Insights

### Reusable Assets
- `mcp::oauth::discover_as(client, server_url)` — returns `AsMetadata` with `token_endpoint`; reuse directly in refresh loop
- `mcp::credentials::read_credential` — read stored `CredentialToken` (with new fields after struct extension)
- `mcp::credentials::write_credential` — atomic write of refreshed token
- `mcp::detect::mcp_auth_status` — used in new doctor check to enumerate expired/missing tokens
- `reqwest::Client` — already in workspace, use for refresh POST

### What needs to change
- `CredentialToken` struct: add `client_id: Option<String>`, `client_secret: Option<String>` with `#[serde(skip_serializing_if = "Option::is_none")]`
- Phase 34 bot OAuth completion code: must write `client_id`/`client_secret` into `CredentialToken` when calling `write_credential`
- `doctor::run_doctor`: add new `check_mcp_tokens(home)` function call

### New code to write
- `mcp::refresh` module (or `mcp::scheduler`): smart refresh scheduler, `refresh_token_for_server()` function
- Bot crate: spawn the refresh background task alongside the main bot polling loop
- `doctor::check_mcp_tokens(home)`: walk agents, enumerate auth status, produce one aggregated check

### Integration Points
- Bot startup: `tokio::spawn(refresh_scheduler(agent_dir, credentials_path))`
- Doctor: one new call in `run_doctor`

</code_context>

<specifics>
## Specific Ideas

- Refresh buffer: 10 minutes before `expires_at` (standard for ~1 hour OAuth tokens from Notion/similar providers)
- Failed refresh should log `tracing::warn!` with server name and error — not crash the bot
- Scheduler is per-bot-process (each agent's bot owns its refresh loop for its own tokens)
- `expiresAt=0` = "treat as non-expiring" — the scheduler MUST check for this before computing sleep duration, otherwise integer underflow on `expires_at - 10min`

</specifics>

<deferred>
## Deferred Ideas

- `/mcp refresh <server>` Telegram bot command — deferred or eliminated (auto-refresh makes it unnecessary)
- Per-agent credential isolation (SEED-004 territory) — deferred to v2.1

None — discussion stayed within phase scope.

</deferred>

---

*Phase: 35-token-refresh*
*Context gathered: 2026-04-03*
