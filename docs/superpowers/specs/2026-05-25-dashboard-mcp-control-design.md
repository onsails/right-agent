# Dashboard MCP Control Design

## Goal

Move user-facing MCP server management from Telegram command subcommands into
the Telegram Mini App dashboard while keeping MCP ownership in the existing
control-plane crates.

The only Telegram command that remains is `/mcp` with no arguments. It opens
the dashboard directly on the MCP view. Telegram-side `/mcp add`, `/mcp auth`,
`/mcp remove`, and `/mcp list` are removed.

The dashboard must support:

- URL-first MCP server add flow.
- OAuth discovery as a recommendation, not authority.
- Quiet handling of non-metadata well-known responses.
- Multiple HTTP headers with write-only secret values.

## Non-Goals

- Do not expose MCP management as agent-facing MCP tools.
- Do not move MCP domain logic into `right-dashboard`.
- Do not edit `.mcp.json`, agent configs, or credential files from dashboard
  routes.
- Do not add a Vue dependency for masked secret inputs or eye toggles.
- Do not support mixed OAuth plus custom headers in this pass.

## Ownership

MCP behavior stays in the crates that already own it:

- `right-mcp`: OAuth discovery, auth models, credential persistence helpers,
  proxy header injection, reconnect, refresh, and internal client DTOs.
- `right`: Aggregator internal API and live proxy backend mutation.
- `right-bot`: Telegram command entry points, Mini App HTTP routes, OAuth
  callback flow, and dashboard route authentication.
- `right-dashboard`: existing dashboard DTOs, read models, auth helpers, and
  static assets. It does not own MCP management behavior.

The Vue frontend may add an MCP tab because it is a user interface. Backend MCP
management remains bot routes calling `right-mcp::internal_client`, which talks
to the Aggregator in `right`.

## User Flow

`/mcp` sends a Mini App button for the current agent dashboard using the same
authorization model as `/dashboard`. The URL deep-links to the MCP view through
query or hash state, for example `/dashboard/<agent>/?view=mcp`.

The MCP dashboard tab shows:

- Server name.
- URL.
- Status.
- Tool count.
- Auth mode.
- Available actions.

The built-in `right` MCP server is visible but protected. It cannot be edited
or removed. Servers in `NeedsAuth` show an Authenticate action.

## URL-First Add Wizard

The add wizard starts with only server name and URL. The user clicks Detect.
The backend probes the URL and returns a structured result:

- Whether OAuth metadata was discovered.
- The recommended mode.
- Safe probe evidence: status code, content type, and broad parse outcome.
- Optional OAuth resource and scopes when discovered.

The UI shows one recommendation and explicit alternatives:

- OAuth.
- Headers.
- URL as-is.

User choice is authoritative. Detection is advisory.

Mode behavior:

- OAuth: register the server as OAuth, then start the OAuth flow from the
  dashboard.
- Headers: show the multi-header editor before saving.
- URL as-is: register the exact URL, preserving query string credentials if
  present.

## Multi-Header Auth

Multiple HTTP headers become a first-class auth mode, not an overload of the
existing single `auth_header` and `auth_token` fields.

Persisted auth compatibility:

- Existing `bearer + auth_token` rows inject `Authorization: Bearer <token>`.
- Existing `header + auth_header/auth_token` rows inject one custom header.
- New `headers` rows store multiple header name/value pairs.
- OAuth remains `Authorization: Bearer <access_token>`.

Store multi-header credentials in a new side table keyed by MCP server name and
header name. This keeps redaction and per-header replacement simple, avoids
rewriting opaque JSON blobs, and gives migration tests a clear relational
contract. Values are secrets:

- Header values are accepted by write APIs.
- Header values are never returned by list/detail APIs.
- Existing saved headers are returned as names plus masked state only.
- Header values are never logged.

The frontend header editor:

- Supports multiple name/value rows.
- Uses password inputs by default.
- Has a per-row eye icon to reveal/hide the current input.
- Shows saved headers by name with a masked placeholder.
- Allows replacing or removing saved headers.
- Uses local Vue code only.

## Dashboard API

Bot-served, Mini-App-authenticated routes:

- `GET /dashboard/{agent}/api/v1/mcp/servers`
- `POST /dashboard/{agent}/api/v1/mcp/detect`
- `POST /dashboard/{agent}/api/v1/mcp/servers`
- `PATCH /dashboard/{agent}/api/v1/mcp/servers/{name}/headers`
- `POST /dashboard/{agent}/api/v1/mcp/servers/{name}/oauth/start`
- `DELETE /dashboard/{agent}/api/v1/mcp/servers/{name}`

The bot routes are thin:

- Authenticate Telegram Mini App init data.
- Validate request shape.
- Call `right_mcp::internal_client`.
- Convert internal errors to dashboard JSON responses.

The Aggregator remains responsible for live backend changes and persistence.

Internal API changes in `right` and `right-mcp` should support:

- Registering a server with `headers` auth.
- Listing saved header names without values.
- Updating multi-header state.
- Removing a server.
- Starting OAuth from the dashboard flow without requiring Telegram command
  prompts.

## OAuth Discovery Semantics

Non-metadata well-known responses are not errors. They mean the probed URL did
not contain OAuth metadata.

Examples that must not surface as Right Agent failures:

- HTTP 200 with `text/html` from
  `/.well-known/oauth-protected-resource/...`.
- HTTP 404 from a well-known location.
- HTTP 401 without a `WWW-Authenticate` `resource_metadata` parameter.
- Non-JSON bodies at speculative metadata locations.

Discovery should keep probing fallbacks when a speculative well-known response
is not parseable metadata.

Real errors still propagate:

- Invalid server URL.
- Network timeout when probes cannot complete.
- Malformed authorization-server metadata after a valid protected-resource
  document points to it.
- DCR failure when the user starts OAuth.
- Token exchange failure.
- Aggregator registration/reconnect failure.

The dashboard detect endpoint returns structured advisory state such as:

- `oauth_discovered`.
- `recommended_mode`.
- `reason`, for example `well_known_not_metadata`.
- Redacted probe summaries.

It must not include response bodies, tokens, or secret-bearing headers.

## Error Handling And Logs

User-facing detection messages should be neutral. For a Nango-style endpoint,
the UI should say: "No OAuth metadata found. Header auth is recommended."

Logging rules:

- Probe outcomes are summarized without response bodies.
- Non-metadata well-known responses log at `debug`.
- Header values and tokens are never logged.
- OAuth callback and token exchange logs stay token-redacted.

HTTP response mapping:

- Invalid input: `400`.
- Unknown server: `404`.
- Protected `right` modification: `403` or `409`.
- Live MCP operation failure from Aggregator: `502`.
- Local dashboard route bug: `500`.

## Frontend Integration

Add `mcp` as a dashboard tab and support selecting it from a URL query or hash
deep link. `/mcp` should open that tab directly.

The MCP view should match the existing operational dashboard style: compact,
scannable rows, restrained controls, no marketing copy, and no in-app tutorial
text. Controls should be explicit and stable on mobile.

Expected components:

- `McpView.vue`.
- A local masked secret input or header-row component.
- API functions and TypeScript interfaces in the existing frontend API/type
  files.

Build output must refresh the embedded static dashboard assets.

## Documentation Updates

Update:

- `ARCHITECTURE.md` MCP auth type table and security rule.
- `docs/architecture/mcp.md` for the dashboard-owned UX and unchanged
  Aggregator ownership.
- `docs/architecture/lifecycle.md` for dashboard MCP routes and `/mcp` command
  behavior.
- Any prompt or user-facing text that still tells users to run
  `/mcp add|auth|remove|list`.

## Testing

Use TDD for behavior changes.

Regression tests:

- `right-mcp::oauth`: 200 HTML from protected-resource well-known is skipped
  as non-metadata and does not produce a misleading parse failure.
- OAuth discovery still succeeds for valid protected-resource and AS metadata
  chains.
- Detection recommendation cases: OAuth discovered, no metadata, query-string
  URL, loopback/no-auth.
- Multi-header persistence and proxy injection.
- Invalid header names are rejected before persistence.
- List/detail APIs never return header values.
- Existing bearer and single-header rows remain compatible.
- Internal Aggregator API add/list/remove/update behavior for multi-header
  auth.
- Dashboard bot routes: Mini App auth, detect, add, header update, OAuth start,
  remove, protected `right`.
- Frontend tests/typecheck for URL-first wizard, masked header values, eye
  toggle, and `/mcp` deep-link selection.

Verification cadence:

- Run targeted baseline tests before implementation in touched areas.
- Run the narrow failing regression test before each behavior fix.
- Run targeted tests after each coherent implementation slice.
- After UI work, run dashboard frontend tests, typecheck, and build.
- Final verification is mandatory: `devenv shell -- cargo test --workspace`.
