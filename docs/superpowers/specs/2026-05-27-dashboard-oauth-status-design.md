# Dashboard OAuth Status Design

## Goal

Keep MCP OAuth completion inside the dashboard flow. When a user starts OAuth
from the MCP dashboard, the dashboard must show the final success or failure.
The bot must not send OAuth completion status directly to Telegram DMs.

The current behavior violates the dashboard MCP control-plane direction:
`/mcp` opens the dashboard, but `oauth_callback.rs` still broadcasts the final
OAuth result to Telegram after the browser callback. A readiness failure such as
Composio returning `mcp_reconnect_failed` is therefore invisible in the
dashboard even though the dashboard initiated the flow.

## Non-Goals

- Do not add persisted OAuth attempt history.
- Do not send Telegram fallback notifications for dashboard-started OAuth.
- Do not move MCP registration, token storage, or reconnect logic out of the
  existing internal API and aggregator ownership.
- Do not change agent-facing MCP tools or expose MCP management to agents.
- Do not change OAuth discovery semantics.

## Ownership

OAuth completion status is bot-owned, transient UI state:

- `right-bot::telegram::dashboard::mcp` starts OAuth, creates the status entry,
  and exposes a Mini-App-authenticated status route.
- `right-bot::telegram::oauth_callback` consumes `PendingAuth`, performs the
  existing token exchange and internal `/set-token` call, and records the final
  status instead of notifying Telegram.
- `right` remains responsible for `/set-token`, persistence, refresh scheduling,
  and reconnect readiness.
- `right-dashboard` frontend renders and polls the status. It does not own MCP
  domain behavior.

The status store is in memory. If the bot restarts during OAuth, the dashboard
sees `unknown` or `expired`, and the user restarts OAuth from the dashboard.
That is intentional for a transient browser flow.

## Data Flow

1. The dashboard calls
   `POST /dashboard/{agent}/api/v1/mcp/servers/{server_name}/oauth/start`.
2. The bot lists registered MCP servers through the internal API, discovers
   OAuth metadata, optionally performs Dynamic Client Registration, generates
   PKCE and OAuth `state`, stores `PendingAuth`, and inserts a matching
   `pending` status entry.
3. The OAuth start response returns `auth_url` and `flow_id`.
4. The dashboard opens `auth_url` and starts polling
   `GET /dashboard/{agent}/api/v1/mcp/oauth/{flow_id}/status`.
5. The OAuth provider redirects to `/oauth/{agent}/callback`.
6. The callback consumes `PendingAuth`, spawns the existing completion task,
   and returns a browser page that says the dashboard will update.
7. The completion task exchanges the code for tokens and calls internal
   `/set-token`.
8. The completion task records:
   - `succeeded` when token exchange and MCP readiness both pass.
   - `failed` when the provider reports an error, token exchange fails, or
     `/set-token` returns a readiness/auth/internal failure.
9. The dashboard stops polling on terminal status and refreshes the MCP server
   list.

Use the generated OAuth `state` as `flow_id`. It is already high-entropy,
one-shot, and contains no token material, so a second correlation id would add
complexity without improving security.

## Status API

Add a dashboard route:

```text
GET /dashboard/{agent}/api/v1/mcp/oauth/{flow_id}/status
```

Response shape:

```json
{
  "flow_id": "opaque-state",
  "server_name": "composio",
  "status": "pending",
  "message": null,
  "updated_at": "2026-05-27T00:00:00Z"
}
```

Allowed statuses:

- `pending`: OAuth was opened and is waiting for callback or completion.
- `succeeded`: token storage and MCP readiness passed.
- `failed`: provider error, token exchange failure, invalid completion state,
  or internal `/set-token` failure.
- `expired`: the flow aged out before completion.
- `unknown`: the flow id is not in memory, usually because the bot restarted or
  the dashboard has a stale tab.

`unknown` is returned as `200` with `status=unknown`, not as an HTTP error.
This keeps the polling path typed as a normal terminal OAuth status instead of
routing it through the dashboard's global API-error handling. The dashboard
must render it as a terminal state and offer the normal Authenticate action
again.

## Error Handling

Dashboard failure messages must be useful and redacted. For readiness failures,
show a compact message such as:

```text
Token exchange completed, but MCP readiness failed: Server error (502): mcp_reconnect_failed
```

The callback should log full alternate-display error detail for diagnostics but
store only a sanitized dashboard message. Tokens, refresh tokens, client
secrets, PKCE verifiers, and secret-bearing upstream response bodies must never
enter the status store or dashboard response.

The callback browser page must no longer tell the user to check Telegram. It
should acknowledge that authorization was received and that the dashboard will
update.

Provider-side callback errors should also record `failed` when the callback
contains a valid known `state`. If no state is present or the state is unknown,
the browser can still return an HTTP error; no dashboard status exists to
update.

## Frontend Behavior

`McpView.vue` should keep OAuth status local to the MCP view:

- After `mcpStartOAuth`, store the returned `flow_id` by server name.
- Open the provider URL using the existing Telegram Mini App link helper.
- Poll the new status route with a bounded interval until a terminal state.
- Show pending/success/failure inline near the affected server row or MCP panel
  error area.
- Refresh the MCP server list on `succeeded`, `failed`, `expired`, or
  `unknown`.
- Stop polling when the view unmounts or when another OAuth flow replaces the
  same server's flow id.

The dashboard must not rely on Telegram DMs to explain what happened.

## Testing

Rust tests:

- OAuth start creates both `PendingAuth` and a `pending` flow status.
- The status route authenticates like other dashboard APIs and returns pending
  state.
- A successful completion updates status to `succeeded`.
- A `/set-token` readiness failure updates status to `failed` with redacted
  detail and does not call the Telegram notification path.
- Unknown or expired flow ids produce terminal OAuth status responses.

Frontend tests:

- `mcpStartOAuth` consumes both `auth_url` and `flow_id`.
- OAuth polling stops on success/failure/expired/unknown.
- Terminal OAuth status triggers an MCP server refresh.
- Starting a newer OAuth flow for the same server ignores stale poll results.

Verification cadence:

- Baseline targeted tests before implementation if the work starts in a new
  worktree.
- Narrow Rust and frontend tests after the behavior slice is implemented.
- Final mandatory check: `devenv shell -- cargo test --workspace`.

## Documentation

When implementing, re-read and update `docs/architecture/mcp.md` because this
changes the dashboard MCP OAuth management flow. Update `ARCHITECTURE.md` only
if the implementation introduces or changes a prescriptive contract; the
expected change is descriptive and should stay in the satellite doc.
