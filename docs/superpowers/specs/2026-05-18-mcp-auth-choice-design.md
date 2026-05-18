# MCP Auth Method Choice

## Problem

`/mcp add <name> <url>` currently treats auth detection as a decision instead of
advice. If OAuth discovery succeeds, the bot immediately registers the server
as OAuth and tells the user to run `/mcp auth`. If OAuth discovery fails, the
bot uses the URL query string, Haiku auth classification, or a private-URL
fallback to choose bearer/header/query-string behavior.

That breaks servers that support more than one auth method. It also makes the
header path awkward: the user can override the header only while entering the
token, and cannot choose header auth when OAuth discovery won.

## Decision

Keep the existing heuristics, but use them only to recommend a method. `/mcp add`
will show an inline keyboard and wait for the user to choose:

- `OAuth`
- `Header`
- `URL as-is`

The recommended option is visually marked in the button label. No upstream MCP
server is registered until the user clicks one of the buttons.

`URL as-is` means "register the exact URL the user supplied and do not inject
any extra auth header." This covers no-auth servers, loopback development
servers, and URLs that already carry query-string credentials. Avoid the label
`No auth` because it is false for query-string auth.

Loopback support is intentionally narrow: allow `localhost` hostnames including
a trailing dot, IPv4 loopback, IPv6 loopback, and IPv4-mapped loopback
addresses for explicit user-managed MCP registration, including plain HTTP.
Continue rejecting broad private/link-local network ranges by default.

## Recommendation Rules

The bot computes a recommendation from the same signals used today. Priority is
top to bottom; a query string wins over OAuth discovery because preserving a
user-supplied credential-bearing URL is safer than silently stripping it.

| Signal | Recommended button |
|--------|--------------------|
| URL contains a query string | `URL as-is` |
| OAuth AS discovery succeeds | `OAuth` |
| Public URL and Haiku returns `header` or `bearer` | `Header` |
| Loopback URL without query string | `URL as-is` |
| Private/link-local URL outside loopback | reject as invalid |
| Detection fails or is unavailable for a public URL | `Header` |

For `bearer`, the header path uses `Authorization: Bearer <token>` semantics.
For `header`, it uses the detected header name.

## User Flow

1. User sends `/mcp add <name> <url>`.
2. Bot parses and validates the URL.
3. Bot strips the query string only for discovery/classification probes. The
   original URL is retained for `URL as-is`.
4. Bot runs OAuth discovery and, where useful, auth header classification.
5. Bot sends an inline keyboard with `OAuth`, `Header`, and `URL as-is`, marking
   the recommended option.
6. User clicks one option.

`OAuth` registers the bare URL with `auth_type=oauth`, then tells the user to
run `/mcp auth <name>`.

`Header` asks for a token. If a header name was detected, the prompt names it.
The user can still override by sending `HeaderName: token`. A raw token uses the
recommended header method.

`URL as-is` registers the original URL exactly as supplied, with no auth token
or auth header. If the URL has a query string, the query string is preserved.

## Components

### Pending choice state

Add a pending MCP auth-choice slot, parallel in shape to the existing pending
token slot. It stores:

- request id for supersession safety
- chat id and effective thread id
- agent name
- server name
- original URL
- bare URL
- recommended method
- detected header name, if any
- expiry time

Only one pending MCP auth-choice request may be active per bot process, matching
the current pending token behavior. A newer `/mcp add` supersedes the old choice
and notifies the old chat.

### Callback handler

Add an MCP auth-choice callback route with callback data small enough for
Telegram limits, such as `mcpauth:<request_id>:oauth`,
`mcpauth:<request_id>:header`, and `mcpauth:<request_id>:url`.

The handler validates:

- callback data parses
- pending request id matches
- the click came from the same chat/thread context when message data is present
- request has not expired

Invalid or stale clicks are acknowledged with a short toast and do not mutate
MCP state.

### Registration

The final registration continues to go through the internal `/mcp-add` API.
No direct config or credential files are edited.

`OAuth`:

- `url = bare_url`
- `auth_type = oauth`
- `auth_header = None`
- `auth_token = None`

`Header`:

- waits for token through the existing pending token path
- `url = bare_url`
- `auth_type = bearer` for bearer recommendation, or `header` for custom header
- `auth_header` is the detected/custom header only when `auth_type=header`
- `auth_token = user token`

`URL as-is`:

- `url = original_url`
- `auth_type = query_string` when the original URL has a query string
- `auth_type = None` when it does not
- `auth_header = None`
- `auth_token = None`

## Error Handling

OAuth discovery and Haiku classification are advisory. If they fail, the flow
still shows the choice buttons with `Header` recommended for public URLs and
`URL as-is` recommended for loopback/local URLs. Broad private/link-local URLs
outside loopback are rejected before the choice buttons.

If the user chooses `Header` and does not provide a token before timeout, the
pending token request is cleared and no MCP server is registered.

If registration fails, the bot reports the aggregator error and clears the
pending choice/token state for that request. The user can retry `/mcp add`.

If Dynamic Client Registration later fails during `/mcp auth`, keep the existing
fallback shape: report the DCR failure, then prompt for header token with the
same header override syntax. This fallback can reuse the header-token prompt
logic introduced for the explicit `Header` button.

## Documentation

Update `docs/architecture/mcp.md` and the `ARCHITECTURE.md` MCP auth type
section because this changes MCP add/auth data flow. The docs should state that
heuristics recommend an auth method but user selection is authoritative.

No prompt-system changes are required unless implementation changes
agent-facing MCP command instructions.

## Testing

Use TDD for the behavior change:

- test rendering of the three-button keyboard with the recommended mark
- test callback-data parse/validation
- test `/mcp add` with OAuth discovery success parks a pending choice instead
  of registering immediately
- test choosing `OAuth` registers bare URL as `oauth`
- test choosing `Header` prompts for detected header and supports
  `HeaderName: token` override
- test choosing `URL as-is` preserves query string and registers without token
- test loopback/local URL recommends `URL as-is`
- test superseded pending choice cannot clear a newer choice

Targeted verification should run the bot handler tests while iterating. Final
verification for implementation remains `devenv shell -- cargo test --workspace`.
