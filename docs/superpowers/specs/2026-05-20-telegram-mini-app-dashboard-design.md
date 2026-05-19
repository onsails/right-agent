# Telegram Mini App Dashboard Design

## Problem

Right Agent users have weak visibility into long-running cron jobs and agent
activity. Telegram shows a foreground thinking anchor for active chats and a
final delivery message for cron/background work, but there is no mobile-first
operator surface for checking scheduled jobs, recent runs, status, logs, or
costs without CLI access.

Issue #57 proposes a broad Telegram Mini App dashboard. The first shippable
slice should be narrower: a read-only, cron-centered dashboard for one agent.

## Scope

The MVP is a Telegram Mini App opened from the agent's Telegram bot. It gives
trusted allowlisted users a global operator view for that single agent.

In scope:

- `/dashboard` sends a Telegram Web App button.
- The bot sets the persistent Telegram menu button when supported.
- The Mini App is a Vue 3 + Vite + TypeScript single-page app.
- The dashboard is cron-centered: scheduled jobs are the primary screen.
- Non-cron activity appears in a compact secondary section.
- Data refreshes by polling every few seconds.
- Read endpoints show cron specs, next/last run, active status, recent run
  history, completed-run cost, and run detail.
- Run detail shows metadata, delivery state, summary, cost when known, and the
  last capped log/event lines.

Out of scope:

- Stop, retry, trigger, create, edit, or delete actions.
- Budget alerts and budget editing.
- Agent configuration editing.
- MCP server configuration editing.
- Full raw log viewer.
- Multi-agent dashboard.
- Live WebSocket or SSE streaming.

For active cron/background runs, cost should be shown as pending unless existing
live usage is available through an already-reliable source. The current usage
table records result-event costs after invocations complete, so the MVP should
not promise "cost so far" unless implementation discovers a clean persisted
signal.

## Approach

Add a new library crate, `right-dashboard`, but do not add a new process.

`right-dashboard` owns dashboard domain types and read-side services:

- Telegram Mini App auth validation helpers.
- Stable API DTOs.
- SQLite read models for cron specs, async runs, usage totals, and recent
  log/event summaries.
- Future command DTO module with no write endpoints in the MVP.

`right-bot` remains the runtime owner:

- Mounts dashboard routes on the existing per-agent Axum UDS server.
- Supplies bot token, allowlist handle, agent name, agent directory, and DB
  connection/path.
- Sends the `/dashboard` Web App button.
- Sets the persistent menu button when supported.
- Serves the built Vue assets.

This keeps dashboard query logic out of `right-bot` without creating a premature
`right-dashboard` daemon. It also preserves the current cloudflared and Telegram
webhook topology.

## Crate Boundary

`right-dashboard` should be a leaf library crate with no Teloxide dispatcher
ownership and no process-compose/cloudflared ownership.

Suggested modules:

- `auth`: validates Telegram Mini App `initData` and returns an authorized
  dashboard user identity.
- `api_types`: versioned request/response DTOs for backend routes and frontend
  contract tests.
- `read_model`: query functions that convert SQLite and log/event sources into
  UI-ready snapshots.
- `commands`: reserved for future typed dashboard commands; no mutating routes
  are exposed in the MVP.

`right-bot` owns the Axum route adapter because it already has the bot token,
allowlist, agent name, and per-agent runtime context.

## Frontend-Backend Contract

The Vue app is a thin client. It does not know SQLite schema details and does
not talk to the aggregator, process-compose, or files directly.

Client behavior:

- Reads raw `window.Telegram.WebApp.initData`.
- Sends raw init data on API requests in an auth header such as
  `Authorization: tma <raw-init-data>`.
- Uses Telegram theme and viewport APIs for presentation only.
- Never trusts `initDataUnsafe` for authorization.
- Polls overview data; fetches detail only for the selected run.
- Shows stale/offline state when polling fails.

Backend behavior:

- Validates Telegram auth on every API request.
- Authorizes the Telegram user against the trusted-users allowlist.
- Returns UI-ready JSON, not raw DB rows.
- Fails closed when auth or authorization is missing.

## Routes

Use versioned API routes from day one:

- `GET /dashboard/<agent>/` serves the Vue app.
- `GET /dashboard/<agent>/api/v1/bootstrap` returns agent identity, feature
  flags, polling interval, and authorization state.
- `GET /dashboard/<agent>/api/v1/overview` returns the cron-centered polling
  payload: cron cards, recent runs, compact active non-cron activity, and usage
  totals.
- `GET /dashboard/<agent>/api/v1/runs/{run_id}` returns run detail: metadata,
  delivery state, summary, cost when known, and capped recent log/event lines.

The `<agent>` path must match the running bot's agent name. Cross-agent routing
is not allowed.

## Auth

The dashboard is public at the URL level because cloudflared exposes the bot UDS
server, so all data access must require Telegram-signed identity plus allowlist
membership.

Every API request must pass all checks:

- Raw Telegram Mini App `initData` is present in an auth header.
- The backend validates the HMAC using the agent bot token.
- `auth_date` is within the configured freshness window. Reads can use a
  moderate window such as 24 hours; future writes should require a shorter
  window.
- Validated init data contains a Telegram `user.id`.
- The user id is in the trusted users allowlist.
- The requested agent path matches the running bot.

Failure behavior:

- Missing, invalid, or expired Telegram auth returns `401`.
- Valid Telegram auth from a non-trusted user returns `403`.
- Agent path mismatch returns an authorization-style failure and logs the
  mismatch category.
- Static assets may load, but no dashboard data is returned without valid auth.
- The frontend shows a locked or unauthorized state after rejected API calls.
- Raw `initData` is never logged; logs may include derived user id and failure
  category.
- Auth must not fall back to cookies, query params, chat ids, display names, or
  `initDataUnsafe`.

## Data Sources

Read models should reuse existing state:

- `cron_specs` for configured jobs.
- `async_runs` for cron/background run state and history.
- `usage_events` for completed-run cost and usage totals.
- Existing foreground/background/cron stream event or log files for capped
  recent detail lines.

Missing log files should produce a detail payload with `log_unavailable`, not a
failed whole dashboard. Unknown run ids return `404`.

## UI Shape

The primary screen is cron-centered and mobile-first:

- Top summary: active cron count, recent failures, today's completed cost when
  available, and last refresh state.
- Cron list: job name, schedule/run kind, target, status, next/last run, latest
  delivery state, and latest completed cost.
- Detail panel or drill-in: selected cron/run metadata, summary, delivery state,
  recent capped logs/events, and cost when known.
- Secondary compact section: active foreground sessions and background runs.

The UI should be restrained and operational, not a marketing landing page. It
should optimize for scanning and repeated checks inside Telegram on mobile.

## Future Mutations

The MVP is read-only, but the boundary must not block future write operations.

Future writes must preserve the project rule that the Telegram bot is the
control plane:

- MCP mutations go through bot-managed aggregator/internal APIs, not direct
  `.mcp.json` or credential file edits.
- Agent settings mutations go through validated config services and trigger the
  same codegen/reload behavior as the bot or CLI.
- Vue never writes files or talks to aggregator/process-compose directly.
- State-changing routes should be explicit `POST` command endpoints with typed
  request/result DTOs, not partial DB updates.
- Writes should require stricter auth than reads, operation-specific policy,
  explicit destructive confirmations, and audit/event logging.

`right-dashboard::commands` can exist as a reserved module, but implementation
should not expose command routes until a separate design covers the first
mutation.

## Error Handling

- API errors are explicit JSON errors with stable categories.
- Database/query errors are logged and returned as API errors; the backend must
  not invent fake empty data.
- Missing optional logs are represented in the response without failing the run
  detail request.
- Polling failures surface as stale/offline UI state.
- The backend should cap log/event payload sizes to protect mobile performance.

## Testing And Verification

Use TDD for behavior changes.

Required focused tests:

- Auth validation rejects invalid hash, expired `auth_date`, missing user, and
  malformed init data.
- Authorization rejects valid Telegram users who are not trusted.
- Agent path mismatch is rejected.
- Overview read model returns cron specs, latest run state, secondary active
  non-cron activity, and usage totals from fixture SQLite data.
- Run detail handles missing logs as `log_unavailable`.
- Route tests cover `401`, `403`, agent mismatch, overview success, and run
  detail missing-log behavior.
- A contract fixture or snapshot ties backend JSON shape to frontend
  expectations.

Required frontend checks:

- Vue typecheck.
- Vue production build.

Required final verification:

- Targeted Rust tests for `right-dashboard` and bot route integration while
  iterating.
- `devenv shell -- cargo test --workspace`
- Final frontend typecheck/build in addition to Cargo tests.

## Non-Goals

- Do not add dashboard write operations in this spec.
- Do not create a separate dashboard process.
- Do not expose aggregator or process-compose APIs directly to the Vue app.
- Do not make dashboard access chat-scoped for the MVP.
- Do not promise live cost-so-far without a reliable persisted source.
