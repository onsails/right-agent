# MCP Backend Error Convention Alignment

**Issue:** [onsails/right-agent#7](https://github.com/onsails/right-agent/issues/7)
**Status:** Design
**Date:** 2026-04-28

## Problem

Today the MCP aggregator mixes two error conventions implicitly. Any failure
that can `?` out of a backend `tools_call` becomes a JSON-RPC error
(`McpError::internal_error` at the rmcp boundary). Only successes return
`CallToolResult`. The agent-visible error shape therefore depends on
implementation detail (whether a path uses `?` or builds a result manually)
rather than on whether the failure is a protocol error or a tool operation
error.

MCP convention is the inverse: `is_error: true` on `CallToolResult` is for
operation errors (the call ran, the operation failed semantically); JSON-RPC
errors are for protocol errors (the call could not be dispatched at all).

This spec aligns all aggregator backends to that convention.

## Goals

- Every operation-level failure returns `Ok(CallToolResult)` with
  `is_error: Some(true)` and a structured JSON content body.
- Every protocol-level / infrastructure failure stays as `Err`.
- Agent-visible error shape is the same regardless of which backend handled
  the call.
- Per-tool tests cover both branches.

## Non-goals

- Changing the Hindsight retry / circuit-breaker / queue behavior. Memory
  failure handling is owned by `2026-04-20-memory-failure-handling-design.md`.
- A central registry / enum of error codes. Codes are free-form `&str` per
  backend; documentation describes the cross-cutting ones.
- Restructuring `ProxyError` or `ProxyBackend::tools_call`'s typed return.

## Decisions

### Error JSON shape

```json
{
  "error": {
    "code": "<machine-readable>",
    "message": "<human-readable>",
    "details": { "...": "..." }
  }
}
```

Nested under `error` so `code` / `message` / `details` cannot collide with
any future success-shape field. `details` is omitted when empty.

### Protocol vs operation taxonomy

| Class | Site | Outcome |
|---|---|---|
| Protocol | unknown tool name in dispatcher | `Err(anyhow!(...))` |
| Protocol | missing required arg, malformed args, deserialize failure | `Err(anyhow!(...))` |
| Protocol | infrastructure: `get_conn?`, `lock_conn?`, `conn.prepare()?`, JSON serialization | `Err(anyhow!(...))` |
| Operation | upstream HTTP error (Hindsight 4xx/5xx, rmcp transport) | `Ok(tool_error(...))` |
| Operation | circuit-open with AuthFailed status | `Ok(tool_error(...))` |
| Operation | `ProxyBackend` `NeedsAuth`, `Unreachable`, `NoSession`, `CallToolFailed` | `Ok(tool_error(...))` |
| Operation | tool-specific logical failure (allowlist reject, bootstrap files missing) | `Ok(tool_error(...))` |

The existing `memory_retain` "queued" path (Transient/RateLimited or
non-Auth circuit-open) stays as `Ok(success)` with `status: "queued"`. A
queued retain is a deferred success, not a failure — only the today-`Err`
sites change.

`cron_show_run` not-found stays as `Ok(success)` with the existing text
message — an empty result is not an error.

### Code taxonomy

Free-form `&str`, no central enum. Codes are documented:
- Cross-cutting codes (those any backend can emit) in the aggregator-level
  `instructions` block.
- Tool-specific codes (only one tool emits them) in that tool's
  `description`.

Cross-cutting codes initially defined:
- `upstream_unreachable` — backend service unreachable, transport failure,
  no active session.
- `upstream_auth` — backend authentication required or rejected.
- `upstream_invalid` — backend rejected the request (4xx, malformed, client
  error).
- `circuit_open` — local circuit breaker open with hard-rejected status
  (i.e. status is not `AuthFailed`, otherwise the call maps to
  `upstream_auth`).
- `invalid_argument` — semantic argument validation failed (not a deserialize
  error — that's protocol).
- `tool_failed` — upstream MCP tool returned its own error (proxy passthrough).
- `server_not_found` — referenced MCP server is not registered for this agent.

Tool-specific codes (initial set):
- `chat_id_not_in_allowlist` — `cron_create`, `cron_update`.
- `bootstrap_files_missing` — `bootstrap_done`. `details.missing` lists the
  missing filenames.

### Translation boundary for `ProxyBackend`

`ProxyBackend::tools_call` keeps its current signature
`Result<CallToolResult, ProxyError>`. Translation happens in
`BackendRegistry::dispatch_to_proxy` via a `From<ProxyError> for CallToolResult`
impl. Typed `ProxyError` stays a clean library type for any non-MCP consumer.

`ProxyError` variant → code mapping:

| Variant | Code |
|---|---|
| `NeedsAuth` | `upstream_auth` |
| `Unreachable` | `upstream_unreachable` |
| `NoSession` | `upstream_unreachable` |
| `CallToolFailed { source }` | `tool_failed`; `details.detail = format!("{source:#}")` |
| `InitFailed`, `ListToolsFailed`, `InstructionsCacheFailed` | `upstream_unreachable` (only fire during `connect`, not `tools_call`; mapped for completeness) |

## Architecture

One new module: `crates/right-agent/src/mcp/tool_error.rs`. Single shared
surface:

```rust
/// Build an MCP operation error: Ok(CallToolResult) with is_error: true and
/// JSON content { "error": { "code", "message", "details"? } }.
pub fn tool_error(
    code: &str,
    message: impl Into<String>,
    details: Option<serde_json::Value>,
) -> CallToolResult;

impl From<ProxyError> for CallToolResult { ... }
```

No traits, no enums, no per-backend wrappers.

The module is reachable via `right_agent::mcp::tool_error` from the `right`
crate (where all current consumers live).

## Per-backend changes

### `HindsightBackend::tools_call` (`crates/right/src/aggregator.rs`)

Replace the four `Err(anyhow!(...))` operation-error sites with
`Ok(tool_error(...))`:

| Today | Becomes |
|---|---|
| `memory_retain`: `Err(anyhow!("{e:#}"))` on Hindsight Auth | `Ok(tool_error("upstream_auth", format!("{e:#}"), None))` |
| `memory_retain`: `Err(anyhow!("{e:#}"))` on Hindsight Client/Malformed | `Ok(tool_error("upstream_invalid", format!("{e:#}"), None))` |
| `memory_retain`: `Err(anyhow!("memory auth failed; retain rejected"))` on circuit-open + AuthFailed | `Ok(tool_error("upstream_auth", "memory auth failed; retain rejected", None))` |
| `memory_recall` / `memory_reflect`: `.map_err(\|e\| anyhow!("{e:#}"))?` | match the `ResilientError`. `Upstream(e)`: branch on `e.classify()` — Transient/RateLimited → `upstream_unreachable`, Auth → `upstream_auth`, Client/Malformed → `upstream_invalid`. `CircuitOpen { retry_after }`: branch on `self.client.status()` — AuthFailed → `upstream_auth`; otherwise → `circuit_open` with `details.retry_after_secs = retry_after.map(\|d\| d.as_secs())` |

The "queued" success path (memory_retain Transient/RateLimited and non-Auth
circuit-open) is unchanged.

Protocol-error sites stay as `Err`:
- `missing required param: content` / `missing required param: query`.
- `unknown hindsight tool: {other}`.

### `RightBackend::tools_call` (`crates/right/src/right_backend.rs`)

Most of this file is untouched. The two existing `CallToolResult::error(...)`
sites are rewritten through the helper for shape uniformity:

| Site | Today | Becomes |
|---|---|---|
| `validate_target_against_allowlist` reject (in `cron_create` / `cron_update`) | `CallToolResult::error(text)` | `tool_error("chat_id_not_in_allowlist", msg, None)` |
| `bootstrap_done` missing files | `CallToolResult::error(text)` | `tool_error("bootstrap_files_missing", msg, Some(json!({"missing": [...]})))` |

Unchanged:
- `cron_show_run` not-found stays as `Ok(success)` with text.
- `cron_*` arg-validation errors (`.map_err(|e| anyhow!("invalid params: {e}"))?`) stay as `Err`.
- `get_conn?`, `lock_conn?`, SQLite query errors, `serde_json::to_string_pretty?` stay as `Err`.

### `BackendRegistry::dispatch_to_proxy` (`crates/right/src/aggregator.rs`)

```rust
pub(crate) async fn dispatch_to_proxy(
    &self,
    proxy_name: &str,
    tool: &str,
    args: serde_json::Value,
) -> Result<CallToolResult, anyhow::Error> {
    let proxies = self.proxies.read().await;
    let proxy = match proxies.get(proxy_name) {
        Some(p) => p,
        None => {
            return Ok(tool_error(
                "server_not_found",
                format!("Server '{proxy_name}' not found. It may have been removed."),
                None,
            ));
        }
    };
    match proxy.tools_call(tool, args).await {
        Ok(result) => Ok(result),
        Err(e) => Ok(CallToolResult::from(e)),
    }
}
```

`ProxyBackend::tools_call`'s signature is unchanged.

The existing test `dispatch_unknown_proxy_returns_error` flips: it now asserts
`Ok` with `is_error: Some(true)` and `error.code == "server_not_found"`.

## Documentation

### `Aggregator::get_info().with_instructions(...)` (`aggregator.rs`)

Extend the existing block with a cross-cutting error-convention section:

```
Error convention (operation errors):
On operation failure, tools return is_error: true with content
  { "error": { "code": "<code>", "message": "<human readable>", "details"?: {...} } }
Cross-cutting codes any tool may emit:
  upstream_unreachable — backend service unreachable / transport failure
  upstream_auth        — backend authentication required or rejected
  upstream_invalid     — backend rejected the request (4xx, malformed)
  circuit_open         — local circuit breaker open; retry later
  invalid_argument     — semantic argument validation failed
  tool_failed          — upstream tool returned its own error (see details)
  server_not_found     — referenced MCP server is not registered
Tool-specific codes are documented in each tool's description.
```

The "Memory tools" sub-section of the same instructions block gets one
extra line: *"Errors follow the aggregator-level error convention; see
above."*

### Tool descriptions

Append a one-line `Errors:` clause to each tool that emits a tool-specific
code. Tools that only emit cross-cutting codes need no description change.

- `bootstrap_done`: append *"Errors: `bootstrap_files_missing` (one or more
  identity files not yet created — see `details.missing`)."*
- `cron_create`, `cron_update`: append *"Errors: `chat_id_not_in_allowlist`
  (the target chat must first be approved via /allow or /allow_all)."*

### `PROMPT_SYSTEM.md`

Add a short "Error convention" section right after the existing MCP
instructions section, mirroring the aggregator's instructions block plus a
sentence on intent: *"Operation errors are normal and recoverable; the agent
reads `error.code` to decide whether to retry, surface to the user, or take
a different path. Protocol errors (JSON-RPC) indicate a bug in the agent's
tool call."*

No changes to TOOLS.md (agent-owned) or skills.

## Tests

New file: `crates/right/src/aggregator_error_tests.rs`. New cases added to
`right_backend_tests.rs`. Helper unit tests live in `tool_error.rs` itself.

A single assertion helper, `assert_tool_error(result, expected_code)`,
verifies the JSON shape (`error.code`, non-empty `error.message`) once and
is reused across all backend tests.

### Helper unit tests (`mcp/tool_error.rs`)

- `tool_error` produces `is_error: Some(true)` and `content[0]` is
  JSON-serialized text matching `{"error":{"code","message"}}`.
- With `details: Some(...)`, JSON includes `details`.
- `From<ProxyError>` table-driven test mapping each variant to its expected
  code.

### `HindsightBackend` operation-error tests

Stub the resilient client (or use its existing test fakes) to force
classified errors:

- `memory_retain` Auth → `code == "upstream_auth"`.
- `memory_retain` Client/Malformed → `code == "upstream_invalid"`.
- `memory_retain` circuit-open + AuthFailed status → `code == "upstream_auth"`.
- `memory_recall` / `memory_reflect` for each error class → expected code.
- Regression: `memory_retain` Transient/RateLimited still returns
  `Ok(is_error=false)` with `status: "queued"`.

### `RightBackend` operation-error tests

- `cron_create` with `target_chat_id` outside allowlist →
  `code == "chat_id_not_in_allowlist"`.
- `bootstrap_done` with missing files →
  `code == "bootstrap_files_missing"`, `details.missing == [...]`.
- Regression: `cron_create` with deserialize-malformed args returns `Err`
  (protocol).

### Aggregator dispatch tests

- Unknown proxy name → `code == "server_not_found"` (updates the existing
  `dispatch_unknown_proxy_returns_error` test).
- A `ProxyBackend` stub forced into `BackendStatus::NeedsAuth` →
  `code == "upstream_auth"`.
- `BackendStatus::Unreachable` → `code == "upstream_unreachable"`.

`ProxyError::CallToolFailed` end-to-end is covered only by the unit test on
the `From<ProxyError>` impl — it would otherwise need a real upstream MCP
server.

## Risks

- **Agent-side surprise.** Running agents have prior session memory of the
  old `Err`-style error messages. After deployment, the same condition
  surfaces as `is_error: true` JSON. The agent may briefly retry against
  stale assumptions; the PROMPT_SYSTEM.md update plus aggregator
  `instructions` should resolve it within one prompt-cache-cold cycle. No
  on-disk migration is needed.
- **Test coverage of stub paths.** `HindsightBackend` operation-error tests
  depend on whatever fake / mockable surface `ResilientHindsight` already
  exposes. If the existing surface is too narrow, the implementation step
  may need to widen it (or use an injectable trait). To be confirmed during
  plan writing — does not affect this design.
- **`ProxyError::NoSession` mapping.** Mapped to `upstream_unreachable`. If
  the bot's UI elsewhere distinguishes "no session" from "unreachable" for
  reconnect logic, that distinction is lost at the MCP surface. Acceptable
  — the bot already pattern-matches on `ProxyError` via the typed return,
  not on the agent-facing `code`.

## Cross-references

- `2026-04-20-memory-failure-handling-design.md` — finding #16, where this
  alignment was deferred. Memory queue / circuit-breaker behavior is
  unchanged here.
- `crates/right-agent/src/mcp/proxy.rs` — `ProxyError` definition.
- `crates/right/src/aggregator.rs` — `HindsightBackend`, `BackendRegistry`,
  `ToolDispatcher`.
- `crates/right/src/right_backend.rs` — `RightBackend`.
- `PROMPT_SYSTEM.md` — agent-facing prompt; receives the new error-convention
  section.
