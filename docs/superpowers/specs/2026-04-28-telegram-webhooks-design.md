# Telegram Webhooks via Cloudflare Tunnel

## Overview

Replace teloxide long-polling with Telegram webhooks delivered through the existing Cloudflare Tunnel. Each agent's bot process terminates its own webhook on a per-agent path of the host's tunnel hostname. The Cloudflare Tunnel becomes a mandatory part of the platform.

## Goals

- Eliminate per-agent long-polling load (each agent currently holds an open HTTPS poll to `api.telegram.org`).
- Reduce inbound message latency to roughly Telegram-edge → Cloudflare-edge → tunnel one-way.
- Reuse the tunnel infrastructure that already exists for OAuth callbacks; no new daemons, no new TLS endpoints, no DNS changes beyond what is already configured.
- Hard cutover. Long-polling code is removed; the tunnel is required.

## Non-Goals

- On-demand bot processes (bots stay 1:1 per agent, always running). Idle-agent cost reduction is out of scope.
- Multi-host failover for a single agent (Telegram supports one webhook URL per bot token; running the same agent on two hosts produces a "last-wins" conflict, identical to long-polling's 409 conflict).
- Provider-neutral webhook receiver. Cloudflare Tunnel is the only inbound transport.
- Sub-paths or non-standard ports. Telegram requires HTTPS on 443/80/88/8443; Cloudflare Tunnel terminates on 443.

---

## Architecture Overview

### Components touched

| Component | Crate / file | Change |
|-----------|--------------|--------|
| Global config schema | `crates/right-agent/src/config/mod.rs` | `tunnel: Option<TunnelConfig>` → `tunnel: TunnelConfig`; deserializer rejects missing tunnel block. |
| Global init wizard | `crates/right/src/wizard.rs` | Drop `Skip` branch from `ExistingTunnelChoice`; `tunnel_setup()` returns `TunnelConfig` (not `Option`). |
| Pipeline / codegen | `crates/right-agent/src/codegen/pipeline.rs` | Remove `if let Some(tunnel_cfg) = ...` branches; cloudflared script + ingress unconditional. |
| Cloudflared template | `templates/cloudflared-config.yml.j2` | Add `path: /tg/<agent>/.*` ingress per agent, pointing at the same UDS that serves OAuth. |
| Process-compose template | `templates/process-compose.yaml.j2` | `cloudflared` process becomes unconditional. |
| Bot UDS server | `crates/bot/src/telegram/oauth_callback.rs`, new `crates/bot/src/telegram/webhook.rs` | UDS renamed from `oauth-callback.sock` → `bot.sock`. Axum router gains `/tg/<agent_name>/...` mount via `webhooks::axum_no_setup`. |
| Bot dispatcher | `crates/bot/src/telegram/dispatch.rs`, `crates/bot/src/lib.rs` | `Dispatcher::dispatch()` (long-poll) → `Dispatcher::dispatch_with_listener(...)`; teloxide `webhooks-axum` feature added. |
| setWebhook lifecycle | `crates/bot/src/lib.rs` | New `webhook_register_loop` task: retry-with-backoff `setWebhook` after axum is up. No `deleteWebhook` on shutdown. |
| Doctor | `crates/right-agent/src/doctor.rs` | Validate tunnel presence, cloudflared health, per-agent `getWebhookInfo`, per-agent `/healthz`. |
| Agent removal | (existing `right agent remove` path) | Best-effort `bot.delete_webhook()` before agent dir cleanup. |

### Inbound data flow

```
Telegram → Cloudflare edge (TLS) → tunnel → cloudflared
   ingress match: hostname=<tunnel.hostname>, path=/tg/<agent>/.*
   → unix:<agent_dir>/bot.sock
   → axum router: nest("/tg/<agent>", teloxide_webhook_router)
   → teloxide UpdateListener (verifies X-Telegram-Bot-Api-Secret-Token)
   → existing Dispatcher / handle_message / DashMap<SessionKey, _> worker
```

Past the dispatcher, nothing changes: workers, debounce, attachments, claude invocation are untouched.

### Outbound data flow

Unchanged. `bot.send_message(...)` still calls Bot API via the system proxy; webhooks are inbound-only.

### What goes away

- The long-polling task started inside `Dispatcher::dispatch()`.
- The `Option<TunnelConfig>` shape and every `if let Some(tunnel)` branch in codegen and runtime.
- `ExistingTunnelChoice::Skip` and `TunnelSetupOutcome::Skipped`.

---

## Cloudflare Tunnel Becomes Mandatory

### Schema

`crates/right-agent/src/config/mod.rs`:

- `GlobalConfig.tunnel: Option<TunnelConfig>` → `GlobalConfig.tunnel: TunnelConfig`.
- `RawGlobalConfig` deserializer fails with a miette error if the `tunnel:` block is missing or has empty `tunnel_uuid`, `credentials_file`, or `hostname`. Help text: `run: right init --tunnel-name NAME --tunnel-hostname HOSTNAME` (already exists at line 204).
- Existing `~/.right/config.yaml` files without a tunnel block break loudly on first `right up` after upgrade.

### Wizard

`crates/right/src/wizard.rs`:

- Remove `ExistingTunnelChoice::Skip` variant.
- Remove the branch in `tunnel_setup()` that returns `TunnelSetupOutcome::Skipped`.
- If `cloudflared tunnel login` has not been run, the wizard prints the instruction and exits non-zero (currently it returns `Skipped`).
- Signature: `tunnel_setup(...) -> Result<TunnelConfig>` (was `Result<TunnelSetupOutcome>`).

### Pipeline

`crates/right-agent/src/codegen/pipeline.rs`:

- Lines 247–306: unwrap the `if let Some(ref tunnel_cfg) = global_cfg.tunnel { ... }` blocks. Cloudflared script generation and `cloudflared tunnel route dns` run unconditionally.
- `cloudflared_script_path: Option<PathBuf>` → `PathBuf`.
- The credentials-file existence check (line 254) stays — it's the right failure mode.

### Process-compose template

`templates/process-compose.yaml.j2`:

- The `{% if cloudflared %}` block for the cloudflared process becomes unconditional.
- Per-agent bot processes gain `depends_on: { cloudflared: { condition: process_started } }`. **Not** `process_healthy` — `setWebhook` retries handle the case where cloudflared has started but isn't yet reachable.

### Doctor

`right doctor` becomes stricter:

- Missing `global_config.tunnel` → ERROR (was: warning).
- Missing `cloudflared` binary in PATH → ERROR.
- Missing tunnel credentials file → ERROR.
- Cloudflared process not `Running` in process-compose → ERROR.
- Per agent: `getWebhookInfo` mismatch with expected URL → ERROR; non-empty `last_error_message` → WARN; `pending_update_count > 100` → WARN.
- Per agent: `GET /healthz` over UDS returns non-200 → ERROR.

### Tests

- `crates/right-agent/src/config/mod.rs` (lines 348, 361): rewrite `tunnel.is_none()` tests to assert deserialization fails when the tunnel block is missing.
- `crates/right-agent/src/codegen/pipeline.rs`: delete fixtures that exercise the no-tunnel branch; add a tunnel block to any other fixture that depended on `tunnel: None`.

### What does not change

- Agent init wizard: tunnel is host-level, not per-agent. Untouched.
- `agent.yaml` schema.
- One named tunnel per `right home`. Multi-tunnel is out of scope.

---

## Bot HTTP Server: Webhook Handler and Secret

### UDS rename

`<agent_dir>/oauth-callback.sock` → `<agent_dir>/bot.sock`. The socket now serves both OAuth callbacks and Telegram webhooks. Existing agents pick it up automatically on `right restart` — sockets are recreated each bot startup.

### Cloudflared ingress

`templates/cloudflared-config.yml.j2`, per agent (order matters: first match wins, keep both rules above the catch-all 404):

```yaml
- hostname: {{ tunnel.hostname }}
  path: /tg/{{ agent.name }}/.*
  service: unix:{{ agent.dir }}/bot.sock
- hostname: {{ tunnel.hostname }}
  path: /oauth/{{ agent.name }}/callback
  service: unix:{{ agent.dir }}/bot.sock
```

Agent names are constrained to `[a-z0-9_-]+` already; no path-escaping required.

### Axum router

New file `crates/bot/src/telegram/webhook.rs`:

```rust
// `BotType` = `CacheMe<Throttle<Bot>>`, the existing alias in
// `crates/bot/src/telegram/mod.rs:22`.
pub fn build_webhook_router(
    bot: BotType,
    secret: String,
    webhook_url: url::Url,
) -> (impl UpdateListener<Err = Infallible>, impl Future<Output = ()> + Send, axum::Router) {
    let options = teloxide::update_listeners::webhooks::Options::new(
        ([127, 0, 0, 1], 0).into(), // ignored by axum_no_setup
        webhook_url,
    )
    .secret_token(secret);
    teloxide::update_listeners::webhooks::axum_no_setup(options)
}
```

The returned `axum::Router` is mounted on the existing UDS app at `/tg/<agent_name>` via `Router::nest`. Same outer router serves `/oauth/<agent_name>/callback` (existing) and `GET /healthz` (new). One UDS, one server, one shutdown signal.

Why `axum_no_setup` and not `axum_to_router`:

1. `axum_to_router` calls `setWebhook` synchronously on construction and `deleteWebhook` on graceful stop. We want neither — we manage `setWebhook` with a retry loop, and we never want to delete the webhook on bot shutdown (clean restarts should keep Telegram delivering on resume).
2. teloxide 0.17 ships only an axum integration for webhooks (`webhooks-axum` feature). `axum_no_setup` is the lower-level entry point that lets us own the lifecycle.
3. The `Options::address: SocketAddr` field is unused by `axum_no_setup` and `axum_to_router`; only the bundled `axum()` helper consumes it. Pass a dummy.

Cargo: add `"webhooks-axum"` to the teloxide feature list in the workspace `Cargo.toml`.

### Webhook URL

```
https://<global_config.tunnel.hostname>/tg/<agent_name>/
```

Trailing slash matters: teloxide's returned `Router` handles `POST /` at its root; nesting under `/tg/<agent_name>` makes the full path `/tg/<agent_name>/`. `setWebhook` is called with this exact URL.

### Webhook secret

Reuse the existing per-agent secret pattern (`crates/right-agent/src/mcp/mod.rs`):

```rust
let webhook_secret = derive_token(&agent_secret, "tg-webhook")?;
```

- `agent_secret` is the 32-byte base64url string already stored in `agent.yaml` under `secret:` (via `ensure_agent_secret`).
- `derive_token(secret, label)` is HMAC-SHA256 keyed by the agent secret. Already used for `derive_token(secret, "right-mcp")` to produce the MCP Bearer token.
- Output: 43-char base64url string. Telegram's `secret_token` allows `[A-Za-z0-9_-]` (1–256 chars) — base64url's alphabet fits.

Properties:

- **Independent of bot token.** Rotating the Telegram bot token does not invalidate the webhook secret.
- **Persistent across restarts.** No race window where Telegram has one secret and the bot expects another.
- **No new state.** Reuses the existing `agent.yaml::secret` field; nothing new in `data.db`, no new files in the agent dir.
- **Safe to log keys but not the value.** HMAC is one-way; even if leaked, it doesn't reveal the agent secret. We don't log it.

The secret is passed to `webhooks::Options::secret_token(s)` (teloxide enforces the `X-Telegram-Bot-Api-Secret-Token` header) and to `bot.set_webhook(url).secret_token(s.clone())`.

---

## Webhook Lifecycle

### Startup

In `crates/bot/src/lib.rs`:

1. Bring up the UDS axum server with the webhook router (from `axum_no_setup`), the OAuth router, and `GET /healthz`.
2. Start the dispatcher with `Dispatcher::builder(bot, handler).build().dispatch_with_listener(update_listener, LoggingErrorHandler::new())`.
3. Spawn `webhook_register_loop`:

```rust
let mut delay = Duration::from_secs(2);
loop {
    match bot.set_webhook(url.clone())
        .secret_token(secret.clone())
        .allowed_updates(vec![AllowedUpdate::Message,
                              AllowedUpdate::EditedMessage,
                              AllowedUpdate::CallbackQuery])
        .max_connections(40)
        .await
    {
        Ok(_) => { tracing::info!(target: "bot::webhook", url = %url, "webhook registered"); break; }
        Err(RequestError::Api(ApiError::Unauthorized)) => {
            tracing::error!(target: "bot::webhook", "bot token invalid; exiting");
            std::process::exit(2);
        }
        Err(e) => {
            tracing::warn!(target: "bot::webhook", error = %e, retry_in = ?delay, "setWebhook failed");
            tokio::time::sleep(jittered(delay)).await;
            delay = (delay * 2).min(Duration::from_secs(60));
        }
    }
}
```

Telegram validates URL reachability during `setWebhook`. If cloudflared is not yet ready, the call fails and the loop retries. The dispatcher is already attached, so the moment `setWebhook` succeeds and the first `POST /tg/<agent>/` arrives, updates flow through teloxide's listener channel.

### `allowed_updates`

Explicit, not "all":

- `message`, `edited_message`, `callback_query`. Covers every update type the existing handler graph processes.
- Adding a new update type later: one-line code change + redeploy; next `setWebhook` picks it up.

### `max_connections`

40 (Telegram default). Per-bot, plenty.

### Shutdown

- The existing `axum_handle` (`crates/bot/src/lib.rs:441-443`) tears down the UDS server.
- The `webhook_register_loop` task is cancelled by the same shutdown signal (after `setWebhook` succeeds, the task exits anyway).
- We do **not** call `deleteWebhook`. Process-compose `on_failure` restarts queue Telegram-side until the bot's UDS comes back. Telegram's retry window is ~24h.

### Bot token rotation

The webhook URL is unchanged. The bot starts with the new token, calls `setWebhook` again — same URL, same secret (derived from the immutable `agent.secret`), new token. Telegram associates the URL with the new token from that call onward. Clean.

### Agent removal

`right agent remove <name>`:

- Before moving the agent dir to trash, the command spawns a one-shot `bot.delete_webhook()` using the agent's stored token (read from `agent.yaml`).
- Failure (network, invalid token) → log WARN and proceed. A stale webhook URL on Telegram's side is a soft leak.
- Help text documents the manual recovery: `curl https://api.telegram.org/bot<TOKEN>/deleteWebhook`.

### Conflict if same agent runs on two hosts

Telegram delivers to whichever host called `setWebhook` last. The earlier host's bot serves the UDS but receives no traffic. Same conflict mode as long-polling's 409 — we surface it the same way. Out of scope.

---

## Error Handling and Observability

### Failure modes

| Failure | Handling |
|---------|----------|
| Cloudflared down at startup | `setWebhook` retries with capped exponential backoff (2s → 60s, jittered). INFO log per attempt. ERROR log after 5 consecutive failures, then keeps retrying. Bot otherwise functional (outbound `bot.send_message` works). |
| Cloudflared dies mid-run | Telegram retries deliveries for ~24h. Process-compose restarts cloudflared (`on_failure`). When tunnel recovers, queued updates flow. |
| Secret mismatch on incoming POST | teloxide returns 401. WARN log with source IP and path. |
| Malformed update body | teloxide returns 400. Internal teloxide logs. No layered handling. |
| Bot down (cloudflared up) | cloudflared returns 502. Telegram retries. Resumes when bot returns. |
| Invalid bot token (`401 Unauthorized` on setWebhook) | ERROR log, exit non-zero. Process-compose `on_failure` restarts → loop. Operator must fix `agent.yaml`. |
| Telegram rate limit (`429 Too Many Requests`) | teloxide's `Throttle` adaptor handles via `Retry-After`. Self-correcting. |

### Health endpoint

`GET /healthz` on `<agent_dir>/bot.sock` returns:

```json
200 OK
{"agent": "<name>", "webhook_set": true|false, "uptime_secs": N}
```

- `webhook_set` flips to `true` when the register loop's first successful `setWebhook` returns.
- UDS-only. Used by `right doctor` from the host. Not exposed through cloudflared — the only ingress route to the UDS is `/tg/<agent>/.*`, and that prefix is owned by the webhook router. External uptime checks can use Telegram's own `getWebhookInfo` instead.

### `right doctor` additions

- Validates global tunnel block + credentials file presence.
- Confirms `cloudflared` process is `Running`.
- For each agent with a Telegram token: `getWebhookInfo` → reports `url`, `pending_update_count`, `last_error_date`, `last_error_message`. Mismatches and recent errors raise WARN/ERROR per the table above.
- Hits each bot's `/healthz` over UDS.

### Logging

- New tracing target `bot::webhook`: registration, retries, secret mismatches, healthz hits.
- Flows into the existing `~/.right/logs/<agent>.log` daily-rotated file.
- No new metrics infrastructure. `getWebhookInfo` provides Telegram-side counts; log grep covers bot-side.

### Out of scope

- Replay protection. Telegram does not sign updates with a nonce; the `secret_token` is the only auth.
- `X-Forwarded-For` parsing. Cloudflare rewrites it; we don't use the value.

---

## Testing

### Unit tests

| File | Test |
|------|------|
| `crates/right-agent/src/mcp/mod.rs` | `derive_token(secret, "tg-webhook")` produces a string matching `^[A-Za-z0-9_-]{1,256}$`. |
| `crates/right-agent/src/config/mod.rs` (lines 348, 361) | Rewrite `tunnel.is_none()` tests: assert `RawGlobalConfig` rejects configs without `tunnel:`. Add positive test for the happy path. |
| `crates/right-agent/src/codegen/cloudflared.rs` | Snapshot of rendered ingress YAML for two agents — both `path: /tg/<agent>/.*` and `path: /oauth/<agent>/callback` rules emitted, both above the catch-all 404. |
| `crates/bot/src/telegram/webhook.rs` | Webhook router accepts a POST with valid secret header → forwards `Update`; wrong secret → 401; garbage JSON → 400. |

### Integration tests

Per project rules: real OpenShell sandbox, no `#[ignore]`.

- `crates/bot/tests/webhook_integration.rs` (new): bot process bound to a tempdir `right_home`, with a stubbed Bot API (httpmock or wiremock-rs) replacing Telegram. Confirm:
  - `setWebhook` is called with the expected URL, secret, and `allowed_updates`.
  - On stub `502`, bot retries with backoff (≥3 attempts in 30s).
  - On stub `401` (invalid token), bot logs ERROR and exits non-zero.
  - POSTing a constructed `Update` to the bot's UDS with the right header lands in the dispatcher (test-only handler that records dispatched updates).
  - POSTing with a wrong header → 401, dispatcher sees nothing.
- `crates/right/tests/right_up_requires_tunnel.rs` (new): `right up` with a `~/.right/config.yaml` missing the tunnel block exits non-zero with a miette error mentioning `right init`. `assert_cmd` + tempdir `--home`.
- `crates/right-agent/tests/cloudflared_ingress_lifecycle.rs` (new, optional): start cloudflared in a sandbox-friendly mode (or local HTTP mock); a request to `/tg/<agent>/...` reaches the bot's UDS. May already be covered by an existing OAuth-callback integration test — confirm before duplicating.

### Snapshot updates

Existing codegen snapshot tests covering `process-compose.yaml` or `cloudflared-config.yml` need fixtures updated for:

- The new `/tg/<agent>/.*` ingress rule.
- The now-unconditional cloudflared section.

Re-snapshot once, commit alongside the spec implementation.

### Manual / out-of-scope

- End-to-end with a live Telegram bot: not automated. Manual checklist post-deploy: send a message → confirm reply → run `right doctor` to inspect `getWebhookInfo`.
- Telegram retry semantics for old-secret in-flight updates: not testable. The secret doesn't change across restarts (it's derived from the persistent agent secret), so the corner case doesn't apply.

---

## Migration

### Users without a tunnel today (breaking)

Major-version change. Documented in release notes.

1. Upgrade binary.
2. Next `right up` fails with miette error: "tunnel block is required — run `right init --tunnel-name NAME --tunnel-hostname HOSTNAME`".
3. Operator runs `right init` (no `Skip` option). Wizard creates tunnel, writes `~/.right/config.yaml`.
4. `right up` succeeds. All existing agents pick up webhooks automatically.

### Users with a tunnel today

Zero operator action beyond `right restart <agent>` (or natural process-compose `on_failure` cycle):

1. New bot binary starts; per-agent codegen runs.
2. Codegen emits cloudflared config with both existing `/oauth/<agent>/callback` and new `/tg/<agent>/.*` rules.
3. Cloudflared restarts (already `Regenerated(BotRestart)` in the codegen registry — nothing new here).
4. Bot brings up UDS axum server with both routers.
5. `setWebhook` registers the URL with the derived secret. Telegram's URL validation succeeds via cloudflared.
6. Telegram disables `getUpdates` on its side automatically when `setWebhook` is set; the new bot binary's dispatcher uses the webhook listener (the long-poll path is gone in this version).

### Cloudflared config reload

- `cloudflared` stays as `Regenerated(BotRestart)` in the codegen registry.
- Config rewrite triggers process restart (process-compose `on_failure` cycles it). Brief tunnel outage (seconds). Telegram retries inbound, OAuth callbacks queue. Both tolerant.

### Webhook URL stability

- URL = `https://<tunnel.hostname>/tg/<agent_name>/`. Stable as long as `tunnel.hostname` and `agent_name` don't change.
- Hostname change (`right init --tunnel-hostname new`): bot's next startup calls `setWebhook` with the new URL. Telegram replaces atomically.
- Agent rename: out of scope (agents don't get renamed in this codebase).

### Codegen registry update

- `templates/cloudflared-config.yml.j2`: same `Regenerated(BotRestart)` category, more content per agent.
- `<agent_dir>/bot.sock`: runtime artifact, not a codegen output. Outside the registry.
- Webhook secret: derived. No new on-disk artifacts.

### Stale long-poll state on Telegram

- First `setWebhook` after upgrade implicitly disables `getUpdates`. No explicit `deleteWebhook` ritual.
- The teloxide long-poll loop is removed from the binary; nothing left to attempt long-polling.

### Rollback

Downgrading to the long-poll version requires manual `deleteWebhook` (curl `https://api.telegram.org/bot<TOKEN>/deleteWebhook`); otherwise teloxide's `getUpdates` returns `409 Conflict` until the webhook is cleared. Documented in release notes' rollback section. Not engineering for it.

### Data migration

None.

- No SQLite schema changes.
- No file format changes in `agent.yaml`, `config.yaml`, `data.db`.
- The user-facing schema change is `tunnel: Option<TunnelConfig>` → `tunnel: TunnelConfig` in the in-memory type, manifesting as a deserialization error if the field is missing.

---

## Deferred to Implementation

These are choices intentionally left to the implementation, not unresolved design questions:

- Exact placement of `webhook_register_loop` task spawning relative to `axum_handle` startup. Constraint: axum UDS server up first, then `setWebhook` retry loop.
- Whether `getWebhookInfo` mismatches in `right doctor` should auto-call `setWebhook` to repair, or just report. Default: "report only". Revisit if operational pain emerges.
- Compat note for the `axum` version: teloxide 0.17 `webhooks-axum` and the workspace both pin `axum 0.8` (verified at design time). Implementation should re-verify after any teloxide bump.
