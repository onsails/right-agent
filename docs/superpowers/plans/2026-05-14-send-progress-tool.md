# Send Progress Tool Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add `mcp__right__send_progress(message)` so only the current foreground Telegram invocation can send occasional standalone progress messages to the same chat/thread. Cron, reflection, delivery, and background continuations must not be able to use it.

**Architecture:**
- Foreground Telegram worker creates a per-CC-invocation id plus a bot-send token, stores the Telegram target in bot-local memory, registers the invocation with the aggregator over the existing internal UDS API, and writes a per-turn MCP config whose `right` server includes `X-Right-Invocation`.
- Aggregator exposes built-in `send_progress`, validates bearer-authenticated agent plus `X-Right-Invocation` against an in-memory per-agent registry, enforces one send per 30 seconds per invocation with no total count cap, and calls the bot UDS `/progress/send` endpoint.
- Bot UDS validates the separate bot-send token against bot-local state and sends a plain Telegram message to the registered chat/thread. Telegram send failures return MCP tool-level errors.
- Cron/reflection/delivery/background sessions get no progress header and also deny `mcp__right__send_progress` through `--disallowedTools`.

**Tech Stack:** Rust 2024, tokio, axum, rmcp, teloxide, dashmap, serde, schemars, right-mcp internal UDS client.

**Out of scope:**
- Editing the existing thinking anchor message.
- Persisting progress history.
- Exposing progress to non-Telegram, cron, reflection, delivery, or background invocations.
- User-configurable rate limits.

---

## File Structure

**Files modified:**

- `crates/right/src/main.rs` - add `progress` module.
- `crates/right/src/progress.rs` - new aggregator-side progress registry, validation, rate-limit, and tool DTOs.
- `crates/right/src/right_backend.rs` - add `send_progress` tool definition and dispatch path.
- `crates/right/src/aggregator.rs` - extract `X-Right-Invocation` from rmcp HTTP context and pass tool-call context through `ToolDispatcher`.
- `crates/right/src/internal_api.rs` - add `/progress/register` and `/progress/unregister`.
- `crates/right/src/memory_server.rs` - keep legacy stdio tool/instructions aligned; legacy tool returns `progress_unavailable`.
- `crates/right-mcp/src/internal_client.rs` - add progress register/unregister/send DTOs and client methods.
- `crates/bot/Cargo.toml` - add `subtle = { workspace = true }`.
- `crates/bot/src/telegram/mod.rs` - expose progress module and add `ProgressState` to `WorkerControlDeps`.
- `crates/bot/src/telegram/progress.rs` - new bot-side progress state and `/progress/send` router.
- `crates/bot/src/telegram/oauth_callback.rs` - merge progress router into bot UDS server.
- `crates/bot/src/lib.rs` - create shared `ProgressState` and pass it to bot UDS and Telegram dispatcher.
- `crates/bot/src/telegram/dispatch.rs` - pass progress state through dispatcher dependency wiring.
- `crates/bot/src/telegram/handler.rs` - pass progress state into `WorkerContext`.
- `crates/bot/src/telegram/worker.rs` - register/unregister foreground invocations and use per-turn MCP config.
- `crates/bot/src/cc/invocation.rs` - add deny helper and per-invocation MCP config header helper.
- `crates/bot/src/cron.rs` - deny `mcp__right__send_progress`.
- `crates/bot/src/cron_delivery.rs` - deny `mcp__right__send_progress`.
- `crates/bot/src/reflection.rs` - deny `mcp__right__send_progress`.
- `crates/right-codegen/src/agent_def.rs` - keep base MCP guidance in sync.
- `crates/right-codegen/src/agent_def_tests.rs` - pin prompt rules.
- `crates/right-codegen/templates/right/prompt/OPERATING_INSTRUCTIONS.md` - teach sparse progress usage.
- `crates/right-codegen/templates/right/prompt/CRON_INSTRUCTIONS.md` - forbid cron progress.
- `PROMPT_SYSTEM.md` - mirror prompt/MCP behavior.
- `ARCHITECTURE.md` - add the foreground-only progress contract.
- `docs/architecture/mcp.md` - document tool, header, validation, and error codes.
- `docs/architecture/sessions.md` - document foreground invocation lifecycle.

**Files intentionally not modified:**

- Agent configs, `.mcp.json`, credential files, and sandbox state. The worker creates temporary per-invocation MCP config files only for the active CC process.
- Existing unrelated dirty files in the worktree.

---

## Task 1: Add Aggregator-Side Progress Registry

**Files:**
- Add: `crates/right/src/progress.rs`
- Modify: `crates/right/src/main.rs`
- Modify: `crates/right/src/right_backend.rs`

- [ ] **Step 1: Write failing registry tests**

Add unit tests in `crates/right/src/progress.rs`:

- `progress_registry_allows_first_send_and_rate_limits_second`
  - Register foreground invocation `inv-1`.
  - First `begin_send("inv-1")` returns target.
  - Immediate second send returns `ProgressError::RateLimited`.
  - After `tokio::time::advance(Duration::from_secs(30))`, send is allowed.
- `progress_registry_unregister_removes_invocation`
  - Register then unregister `inv-1`.
  - `get("inv-1")` returns `ProgressError::Unavailable`.
- `progress_registry_rejects_non_foreground_kind`
  - If a non-foreground kind is added later, `begin_send` must return `Forbidden`.

Run and expect compile failure because the module does not exist:

```bash
devenv shell -- cargo test -p right progress_registry -- --nocapture
```

- [ ] **Step 2: Implement registry types**

Create `crates/right/src/progress.rs` with these public crate-level contracts:

```rust
pub(crate) const PROGRESS_INVOCATION_HEADER: &str = "x-right-invocation";
pub(crate) const SEND_PROGRESS_TOOL: &str = "send_progress";
pub(crate) const SEND_PROGRESS_TOOL_FULL: &str = "mcp__right__send_progress";
pub(crate) const PROGRESS_RATE_LIMIT: Duration = Duration::from_secs(30);
```

Add:

- `SendProgressParams { message: String }` deriving `Deserialize` and `JsonSchema`.
- `ToolCallContext { invocation_id: Option<String> }`.
- `ProgressInvocationKind::Foreground`.
- `ProgressRegistration { invocation_id, kind, bot_socket_path, bot_send_token }`.
- `ProgressSendTarget { bot_socket_path, bot_send_token }`.
- `ProgressError::{Unavailable, Forbidden, RateLimited { retry_after }, InvalidArgument(String), SendFailed(String)}`.
- `ProgressRegistry` wrapping `Arc<tokio::sync::Mutex<HashMap<String, ProgressInvocation>>>`.

`begin_send` must set `last_sent_at` before returning the target, including when the later bot UDS send fails. This prevents tight retry loops on Telegram errors.

- [ ] **Step 3: Wire registry into `RightBackend`**

In `crates/right/src/main.rs`, add:

```rust
pub(crate) mod progress;
```

In `crates/right/src/right_backend.rs`:

- Add `progress: crate::progress::ProgressRegistry` to `RightBackend`.
- Initialize it in `RightBackend::new`.
- Add:

```rust
pub(crate) fn progress_registry(&self) -> crate::progress::ProgressRegistry {
    self.progress.clone()
}
```

Run:

```bash
devenv shell -- cargo test -p right progress_registry -- --nocapture
```

Expected: registry tests pass; call-signature failures from later wiring are acceptable at this checkpoint.

---

## Task 2: Add Internal UDS DTOs And Aggregator Registration Routes

**Files:**
- Modify: `crates/right-mcp/src/internal_client.rs`
- Modify: `crates/right/src/internal_api.rs`

- [ ] **Step 1: Write failing DTO tests**

In `crates/right-mcp/src/internal_client.rs`, add tests:

- `progress_register_request_serializes_expected_fields`
  - Asserts JSON fields `agent`, `invocation_id`, `kind: "foreground"`, `bot_send_token`.
- `progress_send_request_serializes_expected_fields`
  - Asserts JSON fields `invocation_id`, `token`, `message`.

Run and expect failure:

```bash
devenv shell -- cargo test -p right-mcp progress_ -- --nocapture
```

- [ ] **Step 2: Add DTOs and client methods**

In `crates/right-mcp/src/internal_client.rs`, add:

- `ProgressInvocationKindDto` with `#[serde(rename_all = "snake_case")]` and variant `Foreground`.
- `ProgressRegisterRequest { agent, invocation_id, kind, bot_send_token }`.
- `ProgressRegisterResponse { ok: bool }`.
- `ProgressUnregisterRequest { agent, invocation_id }`.
- `ProgressUnregisterResponse { ok: bool }`.
- `ProgressSendRequest { invocation_id, token, message }`.
- `ProgressSendResponse { ok, message_id: Option<i32> }`.

Add methods:

- `progress_register(&ProgressRegisterRequest) -> ProgressRegisterResponse` posting `/progress/register`.
- `progress_unregister(&ProgressUnregisterRequest) -> ProgressUnregisterResponse` posting `/progress/unregister`.
- `progress_send(&ProgressSendRequest) -> ProgressSendResponse` posting `/progress/send`.

- [ ] **Step 3: Write failing internal API route tests**

In `crates/right/src/internal_api.rs`, add tests using `internal_router`:

- `progress_register_adds_foreground_invocation`
  - POST `/progress/register`.
  - Inspect `registry.right.progress_registry().get("inv-1")` and assert it exists.
- `progress_unregister_removes_invocation`
  - Register then POST `/progress/unregister`.
  - Assert `get("inv-1")` returns unavailable.
- `progress_register_rejects_unknown_agent`
  - POST unknown `agent`.
  - Assert `404`.

If aggregator test helpers are private, create a minimal local test dispatcher helper instead of widening production visibility.

Run and expect failure:

```bash
devenv shell -- cargo test -p right progress_register progress_unregister -- --nocapture
```

- [ ] **Step 4: Implement routes**

In `crates/right/src/internal_api.rs`:

- Add routes:

```rust
.route("/progress/register", post(handle_progress_register))
.route("/progress/unregister", post(handle_progress_unregister))
```

- `handle_progress_register`:
  - Look up `req.agent`; unknown returns `404`.
  - Reject non-foreground kind with `400`.
  - Derive `bot_socket_path` as `registry.agent_dir.join("bot.sock")`.
  - Register `ProgressRegistration` on `registry.right.progress_registry()`.
  - Return `{ "ok": true }`.
- `handle_progress_unregister`:
  - Look up agent; unknown returns `404`.
  - Unregister by invocation id.
  - Return `{ "ok": true }` even if already absent.

Run:

```bash
devenv shell -- cargo test -p right progress_register progress_unregister -- --nocapture
devenv shell -- cargo test -p right-mcp progress_ -- --nocapture
```

---

## Task 3: Add `send_progress` MCP Tool In Aggregator

**Files:**
- Modify: `crates/right/src/right_backend.rs`
- Modify: `crates/right/src/aggregator.rs`
- Modify: `crates/right/src/memory_server.rs`

- [ ] **Step 1: Write failing tool tests**

In `crates/right/src/aggregator.rs` tests, add:

- `tools_list_includes_send_progress`
  - `dispatcher.tools_list("test-agent")` includes `send_progress`.
- `send_progress_without_invocation_header_returns_tool_error`
  - Dispatch `send_progress` with default `ToolCallContext`.
  - Assert body error code `progress_unavailable`.
- `send_progress_rate_limited_returns_tool_error`
  - Register `inv-1` with a missing bot socket.
  - First dispatch returns `progress_send_failed`.
  - Immediate second dispatch returns `progress_rate_limited`.

Run and expect failure:

```bash
devenv shell -- cargo test -p right send_progress -- --nocapture
```

- [ ] **Step 2: Pass invocation context through aggregator dispatch**

In `crates/right/src/aggregator.rs`:

- Add `fn invocation_from_context(context: &RequestContext<RoleServer>) -> Option<String>`.
- Read `http::request::Parts` from `context.extensions`.
- Read header `PROGRESS_INVOCATION_HEADER`; trim and return non-empty string.
- Change `ToolDispatcher::dispatch` to accept `crate::progress::ToolCallContext`.
- Pass context only to `registry.right.tools_call`; hindsight, proxy, and rightmeta ignore it.
- In `Aggregator::call_tool`, build `ToolCallContext` from the request header and pass it into dispatch.
- Update all test dispatch calls to pass `ToolCallContext::default()` unless they are testing progress.

- [ ] **Step 3: Implement tool in `RightBackend`**

In `crates/right/src/right_backend.rs`:

- Add `send_progress` to `tools_list()` with `schema_for_type::<crate::progress::SendProgressParams>()`.
- Update tests expecting Right tool count from 9 to 10.
- Change `tools_call` to accept `ToolCallContext`.
- Add `call_send_progress(agent_name, context, args).await`.

Behavior:

- Parse `SendProgressParams`.
- `message.trim()` must be non-empty and at most 3900 chars; otherwise return an `invalid_argument` tool error.
- Missing invocation id returns a `progress_unavailable` tool error.
- Registry errors map to:
  - `Unavailable` -> `progress_unavailable`
  - `Forbidden` -> `progress_forbidden`
  - `RateLimited` -> `progress_rate_limited` with `details.retry_after_secs`
  - `InvalidArgument` -> `invalid_argument`
- Call bot UDS through `InternalClient::new(target.bot_socket_path).progress_send`.
- Success returns JSON text `{ "status": "sent" }`.
- UDS or Telegram failure returns `tool_error("progress_send_failed", format!("{e:#}"), None)`.

- [ ] **Step 4: Keep legacy `MemoryServer` aligned**

In `crates/right/src/memory_server.rs`:

- Import `right_mcp::tool_error::tool_error`.
- Add a `send_progress` tool using `Parameters<crate::progress::SendProgressParams>`.
- Always return `progress_unavailable` with message that foreground HTTP aggregator context is required.
- Add `mcp__right__send_progress` to `with_instructions()`.

Run:

```bash
devenv shell -- cargo test -p right send_progress tools_list_includes_send_progress -- --nocapture
```

---

## Task 4: Add Bot-Side Progress Endpoint

**Files:**
- Modify: `crates/bot/Cargo.toml`
- Add: `crates/bot/src/telegram/progress.rs`
- Modify: `crates/bot/src/telegram/mod.rs`
- Modify: `crates/bot/src/telegram/oauth_callback.rs`
- Modify: `crates/bot/src/lib.rs`

- [ ] **Step 1: Write failing bot progress tests**

In `crates/bot/src/telegram/progress.rs`, add tests:

- `progress_state_register_get_unregister_roundtrip`
  - Register `inv-1` for `chat_id=42`, `thread_id=7`.
  - `get("inv-1")` returns same target.
  - `unregister("inv-1")` removes it.
- `progress_target_token_matches`
  - Correct token returns true.
  - Incorrect token returns false.

Run and expect failure:

```bash
devenv shell -- cargo test -p right-bot progress_state progress_target_token -- --nocapture
```

- [ ] **Step 2: Implement `ProgressState` and `/progress/send`**

In `crates/bot/Cargo.toml`, add:

```toml
subtle = { workspace = true }
```

In `crates/bot/src/telegram/progress.rs`, implement:

- `ProgressState` as cloneable `Arc<DashMap<String, ProgressTarget>>`.
- `ProgressTarget { invocation_id, token, chat_id, thread_id }`.
- `token_matches(&self, token: &str) -> bool` with `subtle::ConstantTimeEq` when lengths match.
- `build_progress_router(state: ProgressState) -> Router`.
- `handle_progress_send(State(state), Json(req): Json<ProgressSendRequest>)`.

Handler behavior:

- Unknown invocation returns `404`.
- Token mismatch returns `403`.
- Empty or >3900-char message returns `400`.
- Send plain Telegram text through `bot.send_message(ChatId(target.chat_id), trimmed_message)`.
- If `target.thread_id != 0`, set `message_thread_id(ThreadId(MessageId(target.thread_id as i32)))`.
- Telegram success returns `ProgressSendResponse { ok: true, message_id: Some(message.id.0) }`.
- Telegram failure returns `502` JSON error.

- [ ] **Step 3: Wire router into bot UDS**

In `crates/bot/src/telegram/mod.rs`:

- Add `pub(crate) mod progress;`.
- Add `progress: progress::ProgressState` to `WorkerControlDeps`.

In `crates/bot/src/telegram/oauth_callback.rs`:

- Add `progress_state` parameter to `build_router` and `run_bot_uds_server`.
- Merge `super::progress::build_progress_router(progress_state)`.

In `crates/bot/src/lib.rs`:

- Construct one `telegram::progress::ProgressState::default()`.
- Pass clones to `run_bot_uds_server` and `run_telegram`.

Run:

```bash
devenv shell -- cargo test -p right-bot progress_state progress_target_token -- --nocapture
```

Expected: progress state tests pass; dispatcher compile errors may remain until Task 6 updates all signatures.

---

## Task 5: Add Per-Invocation MCP Config Header Injection

**Files:**
- Modify: `crates/bot/src/cc/invocation.rs`

- [ ] **Step 1: Write failing invocation tests**

Add tests in `crates/bot/src/cc/invocation.rs`:

- `invocation_mcp_config_adds_progress_header_and_preserves_authorization`
  - Start with JSON containing `mcpServers.right.headers.Authorization`.
  - Assert output also has `X-Right-Invocation: inv-1`.
  - Assert `Authorization` is unchanged.
- `disallow_progress_adds_full_mcp_tool_name`
  - Assert helper includes `mcp__right__send_progress`.
- `disallow_send_progress_is_idempotent`
  - Calling helper twice produces one entry.

Run and expect failure:

```bash
devenv shell -- cargo test -p right-bot invocation_mcp_config disallow_progress disallow_send_progress -- --nocapture
```

- [ ] **Step 2: Implement deny helper**

In `crates/bot/src/cc/invocation.rs`, add:

```rust
pub(crate) const SEND_PROGRESS_MCP_TOOL: &str = "mcp__right__send_progress";

pub(crate) fn disallow_send_progress(mut tools: Vec<String>) -> Vec<String> {
    if !tools.iter().any(|tool| tool == SEND_PROGRESS_MCP_TOOL) {
        tools.push(SEND_PROGRESS_MCP_TOOL.to_owned());
    }
    tools
}
```

- [ ] **Step 3: Implement MCP config helper**

Add:

- `with_progress_invocation_header(config: serde_json::Value, invocation_id: &str) -> anyhow::Result<serde_json::Value>`.
- `write_invocation_mcp_config(agent_dir: &Path, invocation_id: &str) -> anyhow::Result<PathBuf>`.

Rules:

- Require existing `mcpServers.right.headers`; missing structure returns an error.
- Insert `X-Right-Invocation` only under `mcpServers.right.headers`.
- Preserve all existing config keys and the existing `Authorization` header.
- Write output to `agent_dir/.claude/mcp-{invocation_id}.json`.

Run:

```bash
devenv shell -- cargo test -p right-bot invocation_mcp_config disallow_progress disallow_send_progress -- --nocapture
```

---

## Task 6: Register Current Foreground Invocation In Worker

**Files:**
- Modify: `crates/bot/src/telegram/dispatch.rs`
- Modify: `crates/bot/src/telegram/handler.rs`
- Modify: `crates/bot/src/telegram/worker.rs`

- [ ] **Step 1: Write focused helper tests**

In `crates/bot/src/telegram/worker.rs`, add:

- `progress_sandbox_mcp_path_points_inside_sandbox_claude_dir`
  - `progress_sandbox_mcp_path("inv-1")` equals `/sandbox/.claude/mcp-inv-1.json`.
- `progress_registration_target_uses_effective_thread_id`
  - Construct `ProgressTarget` and assert thread id is the effective thread id.

Run and expect failure:

```bash
devenv shell -- cargo test -p right-bot progress_sandbox_mcp_path progress_registration_target -- --nocapture
```

- [ ] **Step 2: Pass `ProgressState` into `WorkerContext`**

In `crates/bot/src/telegram/dispatch.rs`:

- Add `progress_state: super::progress::ProgressState` parameter to `run_telegram`.
- Put it into `WorkerControlDeps`.

In `crates/bot/src/telegram/handler.rs`:

- Set `progress_state: worker_ctl.progress.clone()` when building `WorkerContext`.

In `crates/bot/src/telegram/worker.rs`:

- Add `pub progress_state: super::progress::ProgressState` to `WorkerContext`.

- [ ] **Step 3: Implement lifecycle helpers**

In `crates/bot/src/telegram/worker.rs`, add:

- `ActiveProgressInvocation { invocation_id, local_mcp_config_path, claude_mcp_config_path }`.
- `progress_sandbox_mcp_path(invocation_id: &str) -> String`.
- `start_progress_invocation(ctx, chat_id, eff_thread_id) -> Option<ActiveProgressInvocation>`.
- `finish_progress_invocation(ctx, active).await`.

`start_progress_invocation` rules:

- Generate invocation id with `Uuid::new_v4()`.
- Generate bot-send token with `right_runtime_state::generate_pc_api_token()`.
- Register bot local target with `chat_id` and `eff_thread_id`.
- Call `ctx.internal_client.progress_register` with `kind: Foreground`.
- If aggregator registration fails, unregister bot-local target, log warning, and return `None`; do not fail the user task.
- Write per-invocation MCP config.
- If sandboxed, upload it to `/sandbox/.claude/` via `right_openshell::openshell::upload_file`.
- If upload/config write fails after registration, unregister both sides, log warning, and return `None`.

`finish_progress_invocation` rules:

- Best-effort `progress_unregister`.
- Remove bot-local target.
- Best-effort remove local temp config file.
- Do not require sandbox file deletion for this first implementation.

- [ ] **Step 4: Use per-turn MCP config in `invoke_cc`**

In `invoke_cc`:

- Start progress after `reply_schema` read succeeds and before `ClaudeInvocation` is created.
- Use `active_progress.claude_mcp_config_path` when present; otherwise use base `mcp_config_path`.
- Ensure cleanup on:
  - `ProcessGroupChild::spawn` failure.
  - stdin write failure.
  - normal completion.
  - timeout/background/stop paths.
  - parse failure after process completion.

Run:

```bash
devenv shell -- cargo test -p right-bot progress_sandbox_mcp_path progress_registration_target invocation_mcp_config -- --nocapture
```

---

## Task 7: Deny Progress Outside Foreground Invocations

**Files:**
- Modify: `crates/bot/src/cron.rs`
- Modify: `crates/bot/src/cron_delivery.rs`
- Modify: `crates/bot/src/reflection.rs`
- Modify: `crates/bot/src/cc/invocation.rs`

- [ ] **Step 1: Verify deny helper tests fail before implementation**

Use tests from Task 5:

```bash
devenv shell -- cargo test -p right-bot disallow_send_progress -- --nocapture
```

- [ ] **Step 2: Apply deny helper**

Update:

- `crates/bot/src/cron.rs`: wrap baseline with `disallow_send_progress`.
- `crates/bot/src/cron_delivery.rs`: wrap baseline with `disallow_send_progress`.
- `crates/bot/src/reflection.rs`: apply helper after adding `"Agent"`.

Do not apply this helper to the foreground worker path.

Run:

```bash
devenv shell -- cargo test -p right-bot disallow_send_progress -- --nocapture
devenv shell -- rg -n "mcp__right__send_progress|disallow_send_progress" crates/bot/src/cron.rs crates/bot/src/cron_delivery.rs crates/bot/src/reflection.rs crates/bot/src/telegram/worker.rs
```

Expected `rg` result:

- Cron, cron delivery, and reflection mention `disallow_send_progress`.
- Foreground worker does not deny `mcp__right__send_progress`.

---

## Task 8: Update Prompts And MCP Instructions

**Files:**
- Modify: `crates/right/src/aggregator.rs`
- Modify: `crates/right/src/memory_server.rs`
- Modify: `crates/right-codegen/src/agent_def.rs`
- Modify: `crates/right-codegen/src/agent_def_tests.rs`
- Modify: `crates/right-codegen/templates/right/prompt/OPERATING_INSTRUCTIONS.md`
- Modify: `crates/right-codegen/templates/right/prompt/CRON_INSTRUCTIONS.md`
- Modify: `PROMPT_SYSTEM.md`

- [ ] **Step 1: Write failing prompt tests**

In `crates/right-codegen/src/agent_def_tests.rs`, add:

- `operating_instructions_teach_sparse_progress_updates`
  - Asserts `OPERATING_INSTRUCTIONS` contains `mcp__right__send_progress`, `30 seconds`, `complex, long-running`, and `parallel or sequential subagents`.
- `cron_instructions_forbid_progress_updates`
  - Asserts `CRON_INSTRUCTIONS` contains `mcp__right__send_progress` and `must not send progress`.

Run and expect failure:

```bash
devenv shell -- cargo test -p right-codegen progress_updates -- --nocapture
```

- [ ] **Step 2: Update operating instructions**

In `crates/right-codegen/templates/right/prompt/OPERATING_INSTRUCTIONS.md`, add a concise `## Progress Updates` section:

- Agent may call `mcp__right__send_progress` only during the current foreground user request.
- Use only for complex, long-running work, deep research, long tool chains, or parallel/sequential subagents.
- Do not call it for trivial tasks, every tool call, or routine short work.
- One progress message per 30 seconds per invocation; no total count limit; each message must add useful new information.
- Cron, reflection, delivery, and background sessions cannot send progress.

- [ ] **Step 3: Update cron instructions**

In `crates/right-codegen/templates/right/prompt/CRON_INSTRUCTIONS.md`, add a short `## Progress Messages` section:

- Cron sessions must not send progress messages.
- `mcp__right__send_progress` is only for the current foreground Telegram invocation.

- [ ] **Step 4: Update MCP server instructions and prompt docs**

In `crates/right/src/aggregator.rs` `with_instructions()`:

- Add built-in tool `mcp__right__send_progress`.
- State foreground-only, current invocation only, 30 second per-invocation limit, and tool-level error behavior.

In `crates/right/src/memory_server.rs` `with_instructions()`:

- Add same full tool name with legacy stdio mode returning `progress_unavailable`.

In `crates/right-codegen/src/agent_def.rs`:

- Add concise base MCP guidance for progress.

In `PROMPT_SYSTEM.md`:

- Mirror the actual generated prompt and MCP tool list.
- Use full agent-facing name `mcp__right__send_progress`.

Run:

```bash
devenv shell -- cargo test -p right-codegen progress_updates -- --nocapture
devenv shell -- rg -n "mcp__right__send_progress|send_progress" PROMPT_SYSTEM.md crates/right-codegen/src/agent_def.rs crates/right-codegen/templates/right/prompt crates/right/src/aggregator.rs crates/right/src/memory_server.rs
```

---

## Task 9: Update Architecture Docs

**Files:**
- Modify: `ARCHITECTURE.md`
- Modify: `docs/architecture/mcp.md`
- Modify: `docs/architecture/sessions.md`

- [ ] **Step 1: Re-read docs before editing**

```bash
devenv shell -- sed -n '1,220p' docs/architecture/mcp.md
devenv shell -- sed -n '1,220p' docs/architecture/sessions.md
devenv shell -- sed -n '1,220p' ARCHITECTURE.md
```

- [ ] **Step 2: Update docs**

In `ARCHITECTURE.md`, add the load-bearing rule:

- `mcp__right__send_progress` is foreground-only.
- Aggregator-side validation of bearer agent plus `X-Right-Invocation` registry state is the security boundary.
- Cron/reflection/background sessions must deny the tool and never receive the header.
- Bot UDS validates a separate bot-send token before posting Telegram messages.

In `docs/architecture/mcp.md`, document:

- Built-in `send_progress` contract.
- Header `X-Right-Invocation`.
- Register/unregister lifecycle via internal UDS.
- Tool-level error codes `progress_unavailable`, `progress_forbidden`, `progress_rate_limited`, `progress_send_failed`, and `invalid_argument`.

In `docs/architecture/sessions.md`, document:

- Foreground worker registers progress only for the exact current CC invocation.
- Cleanup unregisters from aggregator and bot-local state.
- Background continuations/cron delivery do not inherit progress capability.

- [ ] **Step 3: Verify docs**

```bash
devenv shell -- rg -n "X-Right-Invocation|mcp__right__send_progress|progress_unavailable|progress_rate_limited" ARCHITECTURE.md docs/architecture/mcp.md docs/architecture/sessions.md
```

---

## Task 10: Full Verification And Commit

**Files:**
- All implementation files above.

- [ ] **Step 1: Run focused tests**

```bash
devenv shell -- cargo test -p right progress_ send_progress tools_list_includes_send_progress -- --nocapture
devenv shell -- cargo test -p right-mcp progress_ -- --nocapture
devenv shell -- cargo test -p right-bot progress_ invocation_mcp_config disallow_send_progress -- --nocapture
devenv shell -- cargo test -p right-codegen progress_updates -- --nocapture
```

- [ ] **Step 2: Run affected crate tests**

```bash
devenv shell -- cargo test -p right -- --nocapture
devenv shell -- cargo test -p right-mcp -- --nocapture
devenv shell -- cargo test -p right-bot -- --nocapture
devenv shell -- cargo test -p right-codegen -- --nocapture
```

- [ ] **Step 3: Run final workspace build**

```bash
devenv shell -- cargo build --workspace
```

- [ ] **Step 4: Inspect diffs and unrelated changes**

```bash
devenv shell -- git status --short
devenv shell -- git diff -- crates/right/src/main.rs crates/right/src/progress.rs crates/right/src/right_backend.rs crates/right/src/aggregator.rs crates/right/src/internal_api.rs crates/right/src/memory_server.rs crates/right-mcp/src/internal_client.rs crates/bot/Cargo.toml crates/bot/src/telegram/mod.rs crates/bot/src/telegram/progress.rs crates/bot/src/telegram/oauth_callback.rs crates/bot/src/lib.rs crates/bot/src/telegram/dispatch.rs crates/bot/src/telegram/handler.rs crates/bot/src/telegram/worker.rs crates/bot/src/cc/invocation.rs crates/bot/src/cron.rs crates/bot/src/cron_delivery.rs crates/bot/src/reflection.rs crates/right-codegen/src/agent_def.rs crates/right-codegen/src/agent_def_tests.rs crates/right-codegen/templates/right/prompt/OPERATING_INSTRUCTIONS.md crates/right-codegen/templates/right/prompt/CRON_INSTRUCTIONS.md PROMPT_SYSTEM.md ARCHITECTURE.md docs/architecture/mcp.md docs/architecture/sessions.md
```

Confirm:

- No unrelated pre-existing dirty files were reverted or reformatted.
- No secrets or tokens are logged.
- `bot_send_token` is never included in agent-facing MCP config or prompt text.
- `send_progress` is denied in cron/reflection/delivery and absent from their invocation headers.
- Foreground worker can use `send_progress` only through the current per-turn header.

- [ ] **Step 5: Commit implementation**

Stage only files changed by this plan:

```bash
devenv shell -- git add crates/right/src/main.rs crates/right/src/progress.rs crates/right/src/right_backend.rs crates/right/src/aggregator.rs crates/right/src/internal_api.rs crates/right/src/memory_server.rs crates/right-mcp/src/internal_client.rs crates/bot/Cargo.toml crates/bot/src/telegram/mod.rs crates/bot/src/telegram/progress.rs crates/bot/src/telegram/oauth_callback.rs crates/bot/src/lib.rs crates/bot/src/telegram/dispatch.rs crates/bot/src/telegram/handler.rs crates/bot/src/telegram/worker.rs crates/bot/src/cc/invocation.rs crates/bot/src/cron.rs crates/bot/src/cron_delivery.rs crates/bot/src/reflection.rs crates/right-codegen/src/agent_def.rs crates/right-codegen/src/agent_def_tests.rs crates/right-codegen/templates/right/prompt/OPERATING_INSTRUCTIONS.md crates/right-codegen/templates/right/prompt/CRON_INSTRUCTIONS.md PROMPT_SYSTEM.md ARCHITECTURE.md docs/architecture/mcp.md docs/architecture/sessions.md
devenv shell -- git commit -m "feat: add foreground progress MCP tool"
```

---

## Risk Notes

- Registration is fail-open for the user task: if progress plumbing cannot be prepared, Claude still runs with the base MCP config and `send_progress` remains unavailable.
- Rate limiting consumes the slot before bot UDS send, preventing tight retries on Telegram errors.
- Bot-send token is separate from the agent-visible MCP bearer token and never appears in the per-invocation MCP config. The agent only sees the invocation id header.
- Cron/reflection/background denial is defense in depth. Aggregator validation remains the security boundary.
- Local per-invocation MCP configs are best-effort cleaned. Sandbox copies may remain; they contain only the existing MCP bearer token plus a stale invocation id, and stale ids are rejected after unregister.
