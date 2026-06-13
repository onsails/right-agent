# Multi-message rich delivery + structured-output loop guard — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give a foreground agent a `mcp__right__send_message` tool to deliver multiple standalone rich messages (photo+caption, documents, …) mid-turn, and guard the worker against the invisible, unbounded structured-output schema-rejection loop that caused the `agent-a` hang.

**Architecture:** Part A reuses the existing `send_progress` cross-process channel (aggregator MCP server → bot Unix-socket route → Telegram) and the existing `partition_sends`/`send_attachments` delivery path; the only new wire type lives in `right-mcp` (shared by `right` and `bot`). Part B adds a counter in the `invoke_cc` stream loop that detects 3 consecutive schema rejections, kills the child, and routes to the existing reflection primitive, plus logs the previously-invisible rejections.

**Tech Stack:** Rust 2024, tokio, axum (UDS router), teloxide, schemars/serde, `right-db`, OpenShell gRPC file download.

**Spec:** `docs/superpowers/specs/2026-06-13-multi-message-delivery-and-so-loop-guard-design.md`

**Baseline before starting (run once in this worktree):**
`devenv shell -- cargo nextest run -p right -p right-mcp -p bot` — record any pre-existing failures so they are not blamed on this work. Known-flaky under load: a cc/invocation pid race and a dashboard warn-count test (re-run isolated before attributing).

---

## Crate-boundary facts (read before Task A1)

- `bot` depends on `right` and `right-mcp`. `right` depends on `right-mcp`. `right` does NOT depend on `bot`.
- `OutboundAttachment` / `OutboundKind` live in `crates/bot/src/cc/attachments_dto.rs` — usable only from `bot`.
- Therefore the attachment type carried by the MCP tool params (`right` crate) and the UDS wire request must live in `crates/right-mcp/src/internal_client.rs`, deriving `Serialize + Deserialize + JsonSchema`. The bot maps it to `OutboundAttachment` at the route handler.
- The MCP server (`right_backend` / `Aggregator`) runs in the aggregator process; the bot owns the Telegram token. The worker (in the bot process) registers each turn into BOTH the bot-side `ProgressState` (in-process, `worker.rs:2777`) and the server-side `ProgressRegistry` (HTTP `progress_register`).

---

# PART A — `mcp__right__send_message`

## Task A1: shared attachment wire type in `right-mcp`

**Files:**
- Modify: `crates/right-mcp/Cargo.toml` (add `schemars`)
- Modify: `crates/right-mcp/src/internal_client.rs`

- [ ] **Step 1: Add schemars dep**

In `crates/right-mcp/Cargo.toml` under `[dependencies]` add (match the workspace version already used by `right`):
```toml
schemars = "1"
```
Run: `devenv shell -- cargo tree -p right-mcp -i schemars` — Expected: resolves to the same version `right` uses.

- [ ] **Step 2: Write the failing test**

Append to `crates/right-mcp/src/internal_client.rs` test module:
```rust
#[test]
fn message_attachment_dto_roundtrips_snake_case() {
    let dto = MessageAttachmentDto {
        kind: MessageAttachmentKind::Photo,
        path: "/sandbox/outbox/a.png".into(),
        filename: None,
        caption: Some("hi".into()),
        media_group_id: None,
    };
    let json = serde_json::to_string(&dto).unwrap();
    assert!(json.contains("\"photo\""), "{json}");
    let back: MessageAttachmentDto = serde_json::from_str(&json).unwrap();
    assert_eq!(back, dto);
}

#[test]
fn send_message_request_carries_content_and_attachments() {
    let req = SendMessageRequest {
        invocation_id: "inv".into(),
        token: "tok".into(),
        content: None,
        attachments: vec![],
    };
    let json = serde_json::to_string(&req).unwrap();
    let back: SendMessageRequest = serde_json::from_str(&json).unwrap();
    assert_eq!(back.invocation_id, req.invocation_id);
}
```

- [ ] **Step 3: Run test to verify it fails**

Run: `devenv shell -- cargo nextest run -p right-mcp message_attachment_dto_roundtrips_snake_case`
Expected: FAIL to COMPILE — `MessageAttachmentDto` not defined.

- [ ] **Step 4: Implement the types**

Add to `crates/right-mcp/src/internal_client.rs` (near the progress DTOs, ~line 595):
```rust
use schemars::JsonSchema;

pub const SEND_MESSAGE_TOOL: &str = "send_message";
pub const SEND_MESSAGE_MCP_TOOL: &str = "mcp__right__send_message";
/// Max standalone `send_message` calls per foreground turn.
pub const MAX_SEND_MESSAGE_PER_TURN: u32 = 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MessageAttachmentKind {
    Photo,
    Document,
    Video,
    Audio,
    Voice,
    VideoNote,
    Sticker,
    Animation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MessageAttachmentDto {
    #[serde(rename = "type")]
    pub kind: MessageAttachmentKind,
    /// Absolute path inside the sandbox, under `/sandbox/outbox/`.
    pub path: String,
    #[serde(default)]
    pub filename: Option<String>,
    #[serde(default)]
    pub caption: Option<String>,
    #[serde(default)]
    pub media_group_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SendMessageRequest {
    pub invocation_id: String,
    pub token: String,
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub attachments: Vec<MessageAttachmentDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SendMessageResponse {
    pub ok: bool,
    #[serde(default)]
    pub message_ids: Vec<i32>,
}
```

- [ ] **Step 5: Add the client method**

Add to the `impl InternalClient` block, mirroring `progress_send`:
```rust
pub async fn message_send(
    &self,
    request: &SendMessageRequest,
) -> Result<SendMessageResponse, InternalClientError> {
    self.post_json("/message/send", request).await
}
```
(Use the same internal POST helper `progress_send` uses — match its exact call shape; if `progress_send` inlines the request build instead of a `post_json` helper, copy that shape verbatim.)

- [ ] **Step 6: Run tests to verify pass**

Run: `devenv shell -- cargo nextest run -p right-mcp message_ send_message_request`
Expected: PASS (both tests).

- [ ] **Step 7: Commit**

```bash
git add crates/right-mcp/Cargo.toml crates/right-mcp/src/internal_client.rs
git commit -m "feat(right-mcp): add send_message wire DTOs + client method"
```

---

## Task A2: `SendMessageParams` + per-turn counter in `ProgressRegistry`

**Files:**
- Modify: `crates/right/src/progress.rs`

- [ ] **Step 1: Write the failing test**

Add to the `progress.rs` test module (mirror `progress_registry_allows_first_send_and_rate_limits_second`):
```rust
#[tokio::test]
async fn begin_message_send_counts_per_turn_and_caps() {
    let registry = ProgressRegistry::default();
    registry.register(foreground_registration()).await;
    // 20 allowed.
    for _ in 0..right_mcp::internal_client::MAX_SEND_MESSAGE_PER_TURN {
        registry.begin_message_send("inv-1").await.expect("under cap");
    }
    // 21st rejected.
    let err = registry.begin_message_send("inv-1").await.unwrap_err();
    assert_eq!(err, ProgressError::RateLimited { retry_after: std::time::Duration::ZERO });
}

#[tokio::test]
async fn begin_message_send_rejects_non_foreground() {
    let registry = ProgressRegistry::default();
    let mut reg = foreground_registration();
    reg.kind = ProgressInvocationKind::NonForeground;
    registry.register(reg).await;
    assert_eq!(registry.begin_message_send("inv-1").await.unwrap_err(), ProgressError::Forbidden);
}
```
(Check the exact `foreground_registration()` helper name/shape already in the test module and reuse it. The registration's `invocation_id` is `"inv-1"` per that helper — adjust the id in the calls to match it.)

- [ ] **Step 2: Run to verify it fails**

Run: `devenv shell -- cargo nextest run -p right begin_message_send`
Expected: FAIL to compile — `begin_message_send` not defined.

- [ ] **Step 3: Add the counter field + method + params**

In `crates/right/src/progress.rs`:

Add the params struct near `SendProgressParams` (reuse the shared attachment type so the MCP schema lists it):
```rust
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub(crate) struct SendMessageParams {
    /// Optional plain-text body sent as its own message.
    #[serde(default)]
    pub(crate) content: Option<String>,
    /// Standalone rich attachments; each renders as its own Telegram message
    /// (or a media group when `media_group_id` is shared).
    #[serde(default)]
    pub(crate) attachments: Vec<right_mcp::internal_client::MessageAttachmentDto>,
}
```

Add to `struct ProgressInvocation` (after `last_sent_at`):
```rust
    /// Count of `send_message` calls this turn; capped at MAX_SEND_MESSAGE_PER_TURN.
    message_send_count: u32,
```
Set `message_send_count: 0` in `register` where `last_sent_at: None` is set.

Add the gate method to `impl ProgressRegistry` (mirror `begin_send`, but count instead of time-gate; reuse `ProgressError::RateLimited { retry_after: Duration::ZERO }` as the over-cap signal and `Forbidden` for non-foreground/unknown):
```rust
pub(crate) async fn begin_message_send(
    &self,
    invocation_id: &str,
) -> Result<ProgressSendTarget, ProgressError> {
    let mut guard = self.inner.lock().await;
    let invocation = guard.get_mut(invocation_id).ok_or(ProgressError::Unavailable)?;
    if !matches!(invocation.kind, ProgressInvocationKind::Foreground) {
        return Err(ProgressError::Forbidden);
    }
    if invocation.message_send_count >= right_mcp::internal_client::MAX_SEND_MESSAGE_PER_TURN {
        return Err(ProgressError::RateLimited { retry_after: std::time::Duration::ZERO });
    }
    invocation.message_send_count += 1;
    Ok(ProgressSendTarget {
        bot_socket_path: invocation.bot_socket_path.clone(),
        bot_send_token: invocation.bot_send_token.clone(),
    })
}
```
(Confirm `ProgressError::Unavailable` is the variant `begin_send` returns for a missing invocation, and match it. If `begin_send` decrements/rolls back on failure via `mark_send_failed`, note we do NOT roll back the count on delivery failure — the cap is an anti-runaway ceiling, not a success counter; a failed send still consumed an attempt.)

- [ ] **Step 4: Run to verify pass**

Run: `devenv shell -- cargo nextest run -p right begin_message_send`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/right/src/progress.rs
git commit -m "feat(right): SendMessageParams + per-turn send_message cap in ProgressRegistry"
```

---

## Task A3: `call_send_message` + tool registration + dispatch in `right_backend`

**Files:**
- Modify: `crates/right/src/right_backend.rs`

- [ ] **Step 1: Register the tool**

In the tool-list block (next to the `SEND_PROGRESS_TOOL` `Tool::new(...)` at ~line 191):
```rust
Tool::new(
    right_mcp::internal_client::SEND_MESSAGE_TOOL,
    "Send a standalone Telegram message (text and/or attachments like photo+caption, document) to the current chat for the current foreground invocation only. Use one call per message to deliver several messages in a turn (e.g. multiple posts). Attachment paths must be under /sandbox/outbox/. Max 20 calls per turn. The terminal reply may then be content:null.",
    schema_for_type::<crate::progress::SendMessageParams>(),
),
```

- [ ] **Step 2: Add the dispatch arm**

Next to `SEND_PROGRESS_TOOL => self.call_send_progress(...)` (~line 291):
```rust
right_mcp::internal_client::SEND_MESSAGE_TOOL => self.call_send_message(context, &args).await,
```

- [ ] **Step 3: Implement `call_send_message`**

Mirror `call_send_progress` (lines 616–713). Implement after it:
```rust
async fn call_send_message(
    &self,
    context: crate::progress::ToolCallContext,
    args: &serde_json::Value,
) -> Result<CallToolResult, anyhow::Error> {
    let params: crate::progress::SendMessageParams = serde_json::from_value(args.clone())
        .map_err(|e| anyhow::anyhow!("invalid send_message params: {e}"))?;

    // FAIL FAST: require something to send.
    let has_content = params.content.as_deref().is_some_and(|c| !c.trim().is_empty());
    if !has_content && params.attachments.is_empty() {
        return Ok(tool_error(
            "send_message_empty",
            "send_message requires non-empty content or at least one attachment",
        ));
    }
    // Reject attachment paths outside the sandbox outbox up front (defense in depth;
    // the bot also rejects). Match the marker the bot uses: "/sandbox/outbox/".
    if let Some(bad) = params.attachments.iter().find(|a| !a.path.starts_with("/sandbox/outbox/")) {
        return Ok(tool_error(
            "send_message_bad_path",
            &format!("attachment path must be under /sandbox/outbox/: {}", bad.path),
        ));
    }

    let Some(invocation_id) = context.invocation_id else {
        return Ok(progress_unavailable());
    };

    let target = match self.progress.begin_message_send(&invocation_id).await {
        Ok(t) => t,
        Err(crate::progress::ProgressError::RateLimited { .. }) => {
            return Ok(tool_error(
                "send_message_limit",
                "send_message limit reached for this turn (max 20); deliver the rest in the terminal reply attachments array",
            ));
        }
        Err(crate::progress::ProgressError::Forbidden) => {
            return Ok(tool_error(
                "send_message_forbidden",
                "send_message is available only for foreground turns",
            ));
        }
        Err(crate::progress::ProgressError::Unavailable) => return Ok(progress_unavailable()),
    };

    let request = right_mcp::internal_client::SendMessageRequest {
        invocation_id: invocation_id.clone(),
        token: target.bot_send_token,
        content: params.content,
        attachments: params.attachments,
    };
    let client = right_mcp::internal_client::InternalClient::new(target.bot_socket_path);
    match tokio::time::timeout(SEND_MESSAGE_TIMEOUT, client.message_send(&request)).await {
        Ok(Ok(resp)) if resp.ok => Ok(CallToolResult::success(vec![Content::text(
            serde_json::json!({ "status": "sent", "message_ids": resp.message_ids }).to_string(),
        )])),
        Ok(Ok(_)) => Ok(tool_error("send_message_failed", "bot reported delivery failure")),
        Ok(Err(e)) => Ok(tool_error("send_message_failed", &format!("{e:#}"))),
        Err(_) => Ok(tool_error("send_message_failed", "send_message timed out")),
    }
}
```
Add the timeout const near `PROGRESS_SEND_TIMEOUT`:
```rust
const SEND_MESSAGE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
```
(Use the exact names `tool_error`, `progress_unavailable`, `CallToolResult::success`, `Content::text` as they appear in `call_send_progress`. If `call_send_progress` does not roll back via `mark_send_failed` on the message path, do not add it here — the per-turn counter intentionally counts attempts.)

- [ ] **Step 4: Build check**

Run: `devenv shell -- cargo check -p right`
Expected: compiles. (Behavioral coverage for this arm comes from the integration test in Task A6.)

- [ ] **Step 5: Commit**

```bash
git add crates/right/src/right_backend.rs
git commit -m "feat(right): call_send_message tool handler + registration + dispatch"
```

---

## Task A4: extend `ProgressTarget` with sandbox fields (bot side)

**Files:**
- Modify: `crates/bot/src/telegram/progress.rs`
- Modify: `crates/bot/src/telegram/worker.rs` (registration site ~2777; test builders ~5331)

- [ ] **Step 1: Extend the struct**

In `crates/bot/src/telegram/progress.rs`, add to `pub(crate) struct ProgressTarget` (currently `invocation_id, token, chat_id, thread_id`):
```rust
    pub(crate) agent_dir: std::path::PathBuf,
    pub(crate) ssh_config_path: Option<std::path::PathBuf>,
    pub(crate) resolved_sandbox: Option<String>,
```
Keep these out of the `Debug` impl body or render them plainly (they are not secrets, but `token` must stay redacted — leave the existing redaction).

- [ ] **Step 2: Populate at registration**

In `crates/bot/src/telegram/worker.rs` at the `.register(super::progress::ProgressTarget { ... })` call (~line 2777), add:
```rust
            agent_dir: ctx.agent_dir.clone(),
            ssh_config_path: ctx.ssh_config_path.clone(),
            resolved_sandbox: ctx.resolved_sandbox.clone(),
```
(Confirm these three exist on `WorkerContext` — the terminal attachment path reads `ctx.agent_dir`, `ctx.ssh_config_path.as_deref()`, `ctx.resolved_sandbox.as_deref()` at worker.rs ~1979.)

- [ ] **Step 3: Fix the in-file test builders**

The test `ProgressTarget { ... }` literals at `progress.rs` ~392/413/426 and `worker.rs` ~5331 will fail to compile. Add the three fields to each with test-appropriate values:
```rust
            agent_dir: std::path::PathBuf::from("/tmp/agent"),
            ssh_config_path: None,
            resolved_sandbox: None,
```

- [ ] **Step 4: Build check**

Run: `devenv shell -- cargo check -p bot`
Expected: compiles.

- [ ] **Step 5: Commit**

```bash
git add crates/bot/src/telegram/progress.rs crates/bot/src/telegram/worker.rs
git commit -m "feat(bot): carry sandbox download context on ProgressTarget"
```

---

## Task A5: `/message/send` route + handler (bot side)

**Files:**
- Modify: `crates/bot/src/telegram/progress.rs`

- [ ] **Step 1: Write the failing test (DTO→OutboundAttachment mapping)**

Add to the `progress.rs` test module a pure mapping test (the handler itself is exercised in Task A6 integration):
```rust
#[test]
fn maps_message_attachment_dto_to_outbound() {
    use right_mcp::internal_client::{MessageAttachmentDto, MessageAttachmentKind};
    let dto = MessageAttachmentDto {
        kind: MessageAttachmentKind::Document,
        path: "/sandbox/outbox/r.csv".into(),
        filename: Some("results.csv".into()),
        caption: Some("data".into()),
        media_group_id: None,
    };
    let out = super::message_dto_to_outbound(&dto);
    assert!(matches!(out.kind, crate::cc::attachments_dto::OutboundKind::Document));
    assert_eq!(out.path, "/sandbox/outbox/r.csv");
    assert_eq!(out.filename.as_deref(), Some("results.csv"));
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `devenv shell -- cargo nextest run -p bot maps_message_attachment_dto_to_outbound`
Expected: FAIL to compile — `message_dto_to_outbound` not defined.

- [ ] **Step 3: Implement mapping + handler + route**

In `crates/bot/src/telegram/progress.rs`:
```rust
pub(crate) fn message_dto_to_outbound(
    dto: &right_mcp::internal_client::MessageAttachmentDto,
) -> crate::cc::attachments_dto::OutboundAttachment {
    use right_mcp::internal_client::MessageAttachmentKind as K;
    use crate::cc::attachments_dto::OutboundKind as O;
    let kind = match dto.kind {
        K::Photo => O::Photo,
        K::Document => O::Document,
        K::Video => O::Video,
        K::Audio => O::Audio,
        K::Voice => O::Voice,
        K::VideoNote => O::VideoNote,
        K::Sticker => O::Sticker,
        K::Animation => O::Animation,
    };
    crate::cc::attachments_dto::OutboundAttachment {
        kind,
        path: dto.path.clone(),
        filename: dto.filename.clone(),
        caption: dto.caption.clone(),
        media_group_id: dto.media_group_id.clone(),
    }
}

async fn handle_message_send(
    State(state): State<ProgressEndpointState>,
    Json(req): Json<right_mcp::internal_client::SendMessageRequest>,
) -> impl IntoResponse {
    let Some(target) = state.progress.get(&req.invocation_id) else {
        return (StatusCode::NOT_FOUND, Json(error_body("invocation not found"))).into_response();
    };
    if !target.token_matches(&req.token) {
        return (StatusCode::FORBIDDEN, Json(error_body("token mismatch"))).into_response();
    }

    let mut message_ids: Vec<i32> = Vec::new();

    // 1) optional text body, reusing the progress HTML send path.
    if let Some(content) = req.content.as_deref().filter(|c| !c.trim().is_empty()) {
        match send_progress_text(&state.bot, &target, content).await {
            Ok(Some(id)) => message_ids.push(id),
            Ok(None) => {}
            Err(e) => {
                tracing::warn!("message_send text failed: {e:#}");
                return (StatusCode::BAD_GATEWAY, Json(error_body("text send failed"))).into_response();
            }
        }
    }

    // 2) attachments via the shared delivery path.
    if !req.attachments.is_empty() {
        let outbound: Vec<_> = req.attachments.iter().map(message_dto_to_outbound).collect();
        if let Err(e) = crate::telegram::attachments::send_attachments(
            &outbound,
            &state.bot,
            teloxide::types::ChatId(target.chat_id),
            target.thread_id,
            &target.agent_dir,
            target.ssh_config_path.as_deref(),
            target.resolved_sandbox.as_deref(),
        )
        .await
        {
            tracing::warn!("message_send attachments failed: {e:#}");
            return (StatusCode::BAD_GATEWAY, Json(error_body("attachment send failed"))).into_response();
        }
    }

    (
        StatusCode::OK,
        Json(right_mcp::internal_client::SendMessageResponse { ok: true, message_ids }),
    )
        .into_response()
}
```
Notes for the implementer:
- `send_progress_text` is a helper to extract: refactor the HTML-send body of `handle_progress_send` (the `md_to_telegram_html` → `bot.send_message(...).parse_mode(Html)` + plain-text retry + thread id) into `async fn send_progress_text(bot: &Bot, target: &ProgressTarget, message: &str) -> Result<Option<i32>, ...>` and have BOTH `handle_progress_send` and `handle_message_send` call it. Do not duplicate the HTML/retry logic. Keep `handle_progress_send`'s existing length validation in that handler (message_send has no 2000-char cap; captions are bounded by `send_attachments`/Telegram).
- `error_body(...)` / the existing error JSON helper used by `handle_progress_send` — reuse the exact one (the report shows it returns a `{ error: String }` body). Match its real name.
- `Json`, `StatusCode`, `IntoResponse` are already imported for the other handlers.

Add the route in `build_progress_router`:
```rust
        .route("/message/send", post(handle_message_send))
```

- [ ] **Step 4: Run mapping test + build**

Run: `devenv shell -- cargo nextest run -p bot maps_message_attachment_dto_to_outbound`
Expected: PASS.
Run: `devenv shell -- cargo check -p bot`
Expected: compiles.

- [ ] **Step 5: Commit**

```bash
git add crates/bot/src/telegram/progress.rs
git commit -m "feat(bot): /message/send route delivering text + sandbox attachments"
```

---

## Task A6: end-to-end integration test (tool → UDS → mocked send)

**Files:**
- Test: `crates/bot/src/telegram/progress.rs` (test module) — drive the router with a registered target and assert text path returns a message id.

- [ ] **Step 1: Write the test**

Use the existing axum/tower test style already present for `handle_progress_send` (look for an existing router test; if none, use `tower::ServiceExt::oneshot`). Register a foreground `ProgressTarget` with `thread_id: 0`, `resolved_sandbox: None`, post a `SendMessageRequest { content: Some("hi"), attachments: vec![] }`, and assert HTTP 200 + `SendMessageResponse.ok == true`. Mock or stub the teloxide `Bot` exactly as the existing progress-send test does (if the existing tests hit a fake server / use `Bot::new` with a dummy token and assert request shaping, follow that pattern). If the existing tests cannot exercise a real send, assert instead that a missing invocation → 404 and a token mismatch → 403, plus the mapping test from A5 — and leave the real-send assertion to the manual smoke test in Task A8.

```rust
#[tokio::test]
async fn message_send_unknown_invocation_is_404() {
    let state = ProgressEndpointState {
        bot: test_bot(),
        progress: ProgressState::default(),
    };
    let app = build_progress_router(state);
    let req = right_mcp::internal_client::SendMessageRequest {
        invocation_id: "missing".into(), token: "t".into(), content: Some("hi".into()), attachments: vec![],
    };
    let resp = app
        .oneshot(post_json("/message/send", &req))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}
```
(Reuse whatever `test_bot()` / `post_json(...)` helpers the existing progress tests use; if they are inline, copy their construction.)

- [ ] **Step 2: Run**

Run: `devenv shell -- cargo nextest run -p bot message_send_unknown_invocation_is_404`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add crates/bot/src/telegram/progress.rs
git commit -m "test(bot): message_send route auth/lookup coverage"
```

---

## Task A7: aggregator `with_instructions()` sync + prompt teaching

**Files:**
- Modify: `crates/right/src/aggregator.rs` (`get_info` / `with_instructions`, ~lines 583–631)
- Modify: `crates/right-codegen/templates/right/prompt/OPERATING_INSTRUCTIONS.md`
- Modify: `PROMPT_SYSTEM.md`

- [ ] **Step 1: Update aggregator instructions**

In the `## Progress` (or adjacent tools) section of `Aggregator::get_info`, add a line after the `send_progress` bullet:
```
- mcp__right__send_message: Send a standalone Telegram message (text and/or attachments such as photo+caption or document) for the current foreground invocation only. Call once per message to deliver several messages in a turn (e.g. multiple posts); attachment paths must be under /sandbox/outbox/. Max 20 calls per turn. After sending, the terminal reply may be content:null.
```
Keep `right_backend.rs` registration description (Task A3 Step 1) and this text consistent.

- [ ] **Step 2: Teach the prompt**

In `OPERATING_INSTRUCTIONS.md`, in the delivery/attachments guidance, add 1–2 sentences (prompt-tier brevity — declarative, no JSON example):
> To send several standalone messages in one turn, call `mcp__right__send_message` once per message (text and/or attachments); the terminal reply may then be `content: null`. Do not retry the terminal structured reply to emit multiple messages.

- [ ] **Step 3: Update PROMPT_SYSTEM.md**

Document the new tool in the tool inventory / delivery section to match the actual registration and instructions (operator-facing narration is allowed here).

- [ ] **Step 4: Verify codegen tests**

Run: `devenv shell -- cargo nextest run -p right-codegen`
Expected: PASS (no schema/registry test asserts the exact tool set in a way this breaks; if a test enumerates MCP tool names, add `send_message`).

- [ ] **Step 5: Commit**

```bash
git add crates/right/src/aggregator.rs crates/right-codegen/templates/right/prompt/OPERATING_INSTRUCTIONS.md PROMPT_SYSTEM.md
git commit -m "docs(prompt): teach send_message + sync aggregator instructions"
```

---

## Task A8: manual smoke (record result, do not skip)

- [ ] **Step 1:** Build the worktree binary: `devenv shell -- cargo build --bin right`.
- [ ] **Step 2:** With a dev agent running, in a DM ask the agent to "send me three messages, one per line, each as its own message." Confirm three separate Telegram messages arrive and the worker log shows three `send_message` deliveries and a silent terminal reply.
- [ ] **Step 3:** Record the outcome in the PR description. If sandbox file download is involved, repeat with an image the agent writes to `/sandbox/outbox/`.

---

# PART B — structured-output loop guard + visibility

## Task B1: schema-rejection detector helper (pure, TDD)

**Files:**
- Modify: `crates/bot/src/cc/stream.rs`

- [ ] **Step 1: Write the failing test**

Add to the `stream.rs` test module:
```rust
#[test]
fn detects_structured_output_schema_rejection() {
    let line = r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","content":"Output does not match required schema: root: must have required property 'content'","is_error":true,"tool_use_id":"x"}]}}"#;
    assert!(is_structured_output_rejection(line));
}

#[test]
fn non_error_tool_result_is_not_rejection() {
    let line = r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","content":"ok","is_error":false,"tool_use_id":"x"}]}}"#;
    assert!(!is_structured_output_rejection(line));
}

#[test]
fn assistant_and_result_lines_are_not_rejection() {
    assert!(!is_structured_output_rejection(r#"{"type":"assistant","message":{"content":[{"type":"text","text":"hi"}]}}"#));
    assert!(!is_structured_output_rejection(r#"{"type":"result","is_error":true,"result":"boom"}"#));
}
```

- [ ] **Step 2: Run to verify fail**

Run: `devenv shell -- cargo nextest run -p bot is_structured_output_rejection`
Expected: FAIL to compile.

- [ ] **Step 3: Implement (reuse the existing persisted-event parser)**

Add to `crates/bot/src/cc/stream.rs`:
```rust
/// The substring CC's structured-output validator emits when the model's
/// StructuredOutput tool call does not satisfy `--json-schema`.
pub(crate) const SCHEMA_REJECTION_MARKER: &str = "does not match required schema";

/// True when this stream line is a `tool_result` error reporting a
/// structured-output schema violation. Reuses `parse_persisted_stream_events`
/// so the matching rules stay in one place.
pub(crate) fn is_structured_output_rejection(line: &str) -> bool {
    parse_persisted_stream_events(line).iter().any(|e| {
        e.kind == PersistedStreamEventKind::ToolError
            && e.content_text.contains(SCHEMA_REJECTION_MARKER)
    })
}

/// True when this line is a successful `tool_result` (resets the rejection run).
pub(crate) fn is_successful_tool_result(line: &str) -> bool {
    parse_persisted_stream_events(line)
        .iter()
        .any(|e| e.kind == PersistedStreamEventKind::ToolResult)
}
```

- [ ] **Step 4: Run to verify pass**

Run: `devenv shell -- cargo nextest run -p bot is_structured_output_rejection`
Expected: PASS (all three).

- [ ] **Step 5: Commit**

```bash
git add crates/bot/src/cc/stream.rs
git commit -m "feat(bot): structured-output schema-rejection stream detector"
```

---

## Task B2: `FailureKind::StructuredOutputLoop` + reflection wording

**Files:**
- Modify: `crates/bot/src/reflection.rs`

- [ ] **Step 1: Write the failing test**

Add to the `reflection.rs` test module:
```rust
#[test]
fn failure_reason_text_for_structured_output_loop() {
    let s = failure_reason_text(&FailureKind::StructuredOutputLoop { rejections: 3 });
    assert!(s.contains("structured"), "{s}");
    assert!(s.contains('3'), "{s}");
}
```

- [ ] **Step 2: Run to verify fail**

Run: `devenv shell -- cargo nextest run -p bot failure_reason_text_for_structured_output_loop`
Expected: FAIL to compile — variant missing.

- [ ] **Step 3: Add the variant + arm**

In `FailureKind` (reflection.rs ~27):
```rust
    StructuredOutputLoop { rejections: u32 },
```
In `failure_reason_text` (~97):
```rust
        FailureKind::StructuredOutputLoop { rejections } => format!(
            "could not produce a valid structured reply after {rejections} attempts (its output kept failing schema validation)"
        ),
```

- [ ] **Step 4: Fix the worker banner match**

In `crates/bot/src/telegram/worker.rs` (~2068) the banner `match &kind` has a `_ =>` arm that already covers the new variant ("⚠️ Previous turn did not complete — thinking again…"). Confirm it compiles; no change needed unless the match is exhaustive without a wildcard (then add an arm).

- [ ] **Step 5: Run to verify pass**

Run: `devenv shell -- cargo nextest run -p bot failure_reason_text_for_structured_output_loop`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/bot/src/reflection.rs
git commit -m "feat(bot): StructuredOutputLoop failure kind + reflection wording"
```

---

## Task B3: detect + abort in the `invoke_cc` stream loop

**Files:**
- Modify: `crates/bot/src/telegram/worker.rs`

- [ ] **Step 1: Add the counters**

Near `let mut timed_out = false;` / `let mut stopped = false;` (~3596), add:
```rust
    let mut consecutive_schema_rejections: u32 = 0;
    let mut schema_loop_detected = false;
    /// Abort after this many consecutive structured-output schema rejections.
    const MAX_SCHEMA_REJECTIONS: u32 = 3;
```
(Place the `const` at module scope if local consts are not idiomatic here; match the file's style.)

- [ ] **Step 2: Count inside the stdout branch**

In the `line_result = lines.next_line()` arm, after the existing `let event = crate::cc::stream::parse_stream_event(&line);` (~3796) and before/after the `ring_buffer.push(&event)`, add the detector (operating on the raw `line`, since rejections are `Other` events the ring buffer drops):
```rust
                        if crate::cc::stream::is_structured_output_rejection(&line) {
                            consecutive_schema_rejections += 1;
                            total_assistant_events += 1;
                            log_stream_update(
                                &log_ctx,
                                total_assistant_events,
                                &format!(
                                    "⚠️ StructuredOutput rejected (schema) [{consecutive_schema_rejections}/{MAX_SCHEMA_REJECTIONS}]"
                                ),
                            );
                            if consecutive_schema_rejections >= MAX_SCHEMA_REJECTIONS {
                                schema_loop_detected = true;
                                child.kill().await.ok();
                                break;
                            }
                        } else if crate::cc::stream::is_successful_tool_result(&line) {
                            consecutive_schema_rejections = 0;
                        }
```
Rationale baked into the reset rule: between rejections the model emits an `assistant` `tool_use` (StructuredOutput) line — that is NOT a successful tool_result, so it must NOT reset the run. Only a genuine successful `tool_result` resets it. This is why the reset keys on `is_successful_tool_result`, not on any non-rejection line.

- [ ] **Step 3: Return a Reflectable before the exit_code branch**

After the loop, alongside `if stopped { return Ok(CcReply { output: None, .. }) }` (~4203) and BEFORE the `if exit_code != 0` branch (the kill makes exit_code nonzero, which would otherwise build a `NonZeroExit` reflectable), add:
```rust
    if schema_loop_detected {
        return Err(InvokeCcFailure::Reflectable {
            kind: crate::reflection::FailureKind::StructuredOutputLoop {
                rejections: consecutive_schema_rejections,
            },
            ring_buffer_tail: ring_buffer.events().clone(),
            session_uuid: session_uuid.clone(),
            raw_message: format!(
                "aborted after {consecutive_schema_rejections} consecutive structured-output schema rejections"
            ),
            thinking_msg_id,
            details_id,
        });
    }
```
(Confirm `details_id` and `thinking_msg_id` are in scope at that point — they are used by the `NonZeroExit` construction at ~4456; if `details_id` is computed later, pass `None`.)

- [ ] **Step 4: Build check**

Run: `devenv shell -- cargo check -p bot`
Expected: compiles.

- [ ] **Step 5: Commit**

```bash
git add crates/bot/src/telegram/worker.rs
git commit -m "feat(bot): abort + reflect on 3 consecutive structured-output rejections"
```

---

## Task B4: detector-logic unit test (extracted pure helper)

The stream loop is not unit-testable in place. Extract the count/reset/abort decision into a pure helper and test it.

**Files:**
- Modify: `crates/bot/src/telegram/worker.rs` (or a small sibling module)

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn schema_loop_fsm_aborts_on_third_consecutive() {
    let mut s = SchemaRejectionRun::default();
    let rej = r#"{"type":"user","message":{"content":[{"type":"tool_result","content":"Output does not match required schema","is_error":true}]}}"#;
    assert!(!s.observe(rej));          // 1
    assert!(!s.observe(rej));          // 2
    assert!(s.observe(rej));           // 3 -> abort
}

#[test]
fn schema_loop_fsm_resets_on_success() {
    let mut s = SchemaRejectionRun::default();
    let rej = r#"{"type":"user","message":{"content":[{"type":"tool_result","content":"Output does not match required schema","is_error":true}]}}"#;
    let ok = r#"{"type":"user","message":{"content":[{"type":"tool_result","content":"done","is_error":false}]}}"#;
    assert!(!s.observe(rej));
    assert!(!s.observe(ok));            // reset
    assert!(!s.observe(rej));
    assert!(!s.observe(rej));
    assert!(s.observe(rej));            // 3 in a row after reset
}

#[test]
fn schema_loop_fsm_ignores_assistant_tool_use_between_rejections() {
    let mut s = SchemaRejectionRun::default();
    let rej = r#"{"type":"user","message":{"content":[{"type":"tool_result","content":"Output does not match required schema","is_error":true}]}}"#;
    let tool_use = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"StructuredOutput","input":{}}]}}"#;
    assert!(!s.observe(rej));
    assert!(!s.observe(tool_use));      // must NOT reset
    assert!(!s.observe(rej));
    assert!(s.observe(rej));            // still reaches 3
}
```

- [ ] **Step 2: Run to verify fail**

Run: `devenv shell -- cargo nextest run -p bot schema_loop_fsm`
Expected: FAIL to compile.

- [ ] **Step 3: Implement the helper and use it in the loop**

```rust
#[derive(Default)]
pub(crate) struct SchemaRejectionRun {
    consecutive: u32,
}

impl SchemaRejectionRun {
    const LIMIT: u32 = 3;
    pub(crate) fn count(&self) -> u32 { self.consecutive }
    /// Feed one raw stream line. Returns true when the abort threshold is hit.
    pub(crate) fn observe(&mut self, line: &str) -> bool {
        if crate::cc::stream::is_structured_output_rejection(line) {
            self.consecutive += 1;
        } else if crate::cc::stream::is_successful_tool_result(line) {
            self.consecutive = 0;
        }
        self.consecutive >= Self::LIMIT
    }
}
```
Refactor Task B3's inline block to use `SchemaRejectionRun` (replace the two locals with `let mut schema_run = SchemaRejectionRun::default();`, call `if schema_run.observe(&line) { schema_loop_detected = true; child.kill().await.ok(); break; }`, and read `schema_run.count()` for the reflectable's `rejections` and the log line). Keep the `log_stream_update` "rejected (schema)" emission for visibility (emit it whenever `is_structured_output_rejection(&line)` is true, using `schema_run.count()`).

- [ ] **Step 4: Run to verify pass + build**

Run: `devenv shell -- cargo nextest run -p bot schema_loop_fsm`
Expected: PASS (all three).
Run: `devenv shell -- cargo check -p bot`
Expected: compiles.

- [ ] **Step 5: Commit**

```bash
git add crates/bot/src/telegram/worker.rs
git commit -m "refactor(bot): extract SchemaRejectionRun fsm + unit tests"
```

---

## Task B5: docs — ARCHITECTURE + satellites

**Files:**
- Modify: `ARCHITECTURE.md` (Claude Invocation Contract / Reflection Primitive — one-line rule)
- Modify: `docs/architecture/mcp.md` (send_message tool + scope rule)
- Modify: `docs/architecture/sessions.md` (schema-rejection detector)

- [ ] **Step 1: ARCHITECTURE.md**

In the MCP Aggregator section's tool list, add one line: `mcp__right__send_message` is foreground-only, scope server-resolved, capped at 20/turn. In the Reflection Primitive section, add: a worker aborts after 3 consecutive structured-output schema rejections and routes to reflection (`FailureKind::StructuredOutputLoop`). Keep each to ≤1–2 sentences (40k budget).

- [ ] **Step 2: Satellites**

`docs/architecture/mcp.md`: describe the `/message/send` UDS route, DTO mapping, and the scope-from-target invariant. `docs/architecture/sessions.md`: describe the detector (counter, reset rule, abort→reflect, restart-breaking).

- [ ] **Step 3: Commit**

```bash
git add ARCHITECTURE.md docs/architecture/mcp.md docs/architecture/sessions.md
git commit -m "docs(arch): send_message tool + structured-output loop guard"
```

---

# Final verification (mandatory, from this worktree)

- [ ] `devenv shell -- cargo nextest run --workspace`
- [ ] `devenv shell -- cargo test --doc --workspace`
- [ ] `devenv shell -- cargo build --workspace`
- [ ] Run the rust-dev:review-rust-code subagent over the diff; turn issues into TODOs and fix them.
- [ ] Confirm `registry_covers_all_per_agent_writes` and any MCP-tool-name enumeration tests still pass (no new codegen outputs were added, but the tool list changed).

---

## Notes for the implementer

- **FAIL FAST:** every `?`/error propagates; never `.ok()`-swallow except the deliberate `child.kill().await.ok()` (process already dying) which matches existing code.
- **Secrets:** never log `bot_send_token`; the new `ProgressTarget` fields are not secrets but keep the existing `Debug` redaction of `token`.
- **No `right` binary in-repo:** use `cargo run --bin right` / `target/devenv/...`, never bare `right`.
- **Reflection recursion:** reflection runs on a separate code path (`reflect_on_failure`), not `invoke_cc`, so the detector cannot fire during reflection — no guard needed.
- **Reset rule is load-bearing:** reset only on a successful `tool_result`, never on the `assistant` StructuredOutput `tool_use` that sits between rejections (Task B4 test `schema_loop_fsm_ignores_assistant_tool_use_between_rejections` locks this).
