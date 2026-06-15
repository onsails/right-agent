# Focus Update Notification Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Send a short Telegram bot notification into the same chat/topic whenever operator focus is saved from the `/set_focus` Mini App.

**Architecture:** Keep `/set_focus` as a launcher only. Add a cloneable focus-notification boundary to `DashboardState`; production uses Telegram `BotType`, route tests use deterministic fake notifiers. `PATCH /dashboard/{agent}/api/v1/focus` writes `operator_focus` first, then sends the notification to the request scope and reports notification failure without rolling back the saved focus.

**Tech Stack:** Rust 2024, axum dashboard routes, teloxide Telegram bot, right-db thread focus storage, cargo nextest.

---

## Execution Notes

- Before writing Rust implementation code, load the project-required `rust-dev:rust-dev` skill if it is available in the execution session. If it is not available, state that and continue using the existing local Rust patterns.
- Do not touch frontend code. Existing `FocusView.vue` already displays backend error details from `DashboardApiError`.
- Do not touch `ARCHITECTURE.md`; this change adds no prompt, schema, or routing invariant.
- Ignore unrelated untracked docs and spike files unless the user explicitly redirects.

## File Structure

- Modify: `crates/bot/src/telegram/dashboard/focus.rs`
  - Owns focus route handling.
  - Add `FocusNotification`, `FocusNotifier`, `FocusNotificationError`, Telegram sender helper, test fake constructors, and notification text formatting.
  - Call the notifier after a successful `set_operator`.
- Modify: `crates/bot/src/telegram/dashboard.rs`
  - Re-export the notifier type for production startup and tests.
  - Add `focus_notifier` to `DashboardState`.
  - Add dashboard route tests and state helpers.
- Modify: `crates/bot/src/lib.rs`
  - Populate `DashboardState::focus_notifier` with a production Telegram notifier.
- No new runtime source files.
- No docs updates beyond this implementation plan.

---

### Task 0: Baseline Focus Route Tests

**Files:**
- Read: `crates/bot/src/telegram/dashboard/focus.rs`
- Read: `crates/bot/src/telegram/dashboard.rs`
- Read: `crates/bot/src/lib.rs`

- [ ] **Step 1: Run the focused baseline**

Run:

```bash
devenv shell -- cargo nextest run -p right-bot dashboard_focus
```

Expected: PASS. If it fails before any edits, stop and record the existing failure in the implementation notes before changing code.

---

### Task 1: Add Failing Dashboard Route Tests

**Files:**
- Modify: `crates/bot/src/telegram/dashboard.rs`
- Test: `crates/bot/src/telegram/dashboard.rs`

- [ ] **Step 1: Add test imports**

In `crates/bot/src/telegram/dashboard.rs`, inside `#[cfg(test)] mod tests`, extend the imports:

```rust
use std::sync::Arc;
```

Keep the existing `use std::sync::Arc;` if it is already present. Do not add a duplicate import.

- [ ] **Step 2: Add a focus notifier override helper**

In `crates/bot/src/telegram/dashboard.rs`, inside `#[cfg(test)] mod tests`, after `fn test_state(...) -> super::DashboardState`, add:

```rust
fn test_state_with_focus_notifier(
    agent_dir: std::path::PathBuf,
    focus_notifier: super::FocusNotifier,
) -> super::DashboardState {
    let mut state = test_state(agent_dir);
    state.focus_notifier = focus_notifier;
    state
}
```

- [ ] **Step 3: Add a PATCH helper that accepts custom state**

In `crates/bot/src/telegram/dashboard.rs`, inside `#[cfg(test)] mod tests`, after `async fn patch_json(...)`, add:

```rust
async fn patch_json_with_state(
    path: &str,
    auth: Option<String>,
    state: super::DashboardState,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    let router = super::build_dashboard_router(state);
    let mut builder = Request::builder()
        .uri(path)
        .method("PATCH")
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(auth) = auth {
        builder = builder.header(header::AUTHORIZATION, format!("tma {auth}"));
    }
    let response = router
        .oneshot(
            builder
                .body(Body::from(body.to_string()))
                .expect("valid request"),
        )
        .await
        .expect("router response");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 1_000_000)
        .await
        .expect("body bytes");
    let value = if bytes.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&bytes).expect("json response")
    };
    (status, value)
}
```

- [ ] **Step 4: Add the set notification regression test**

In `crates/bot/src/telegram/dashboard.rs`, inside `#[cfg(test)] mod tests`, near the existing `dashboard_focus_patch_*` tests, add:

```rust
#[tokio::test]
async fn dashboard_focus_patch_sends_notification_to_scope() {
    let temp = tempfile::tempdir().expect("tempdir");
    right_db::open_connection(temp.path(), true)
        .await
        .expect("open migrated db");

    let sent = Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let state = test_state_with_focus_notifier(
        temp.path().to_path_buf(),
        super::FocusNotifier::capture(Arc::clone(&sent)),
    );
    let token = signed_focus_scope_token("alpha", 7, 11, chrono::Utc::now().timestamp() + 60);

    let (status, body) = patch_json_with_state(
        "/dashboard/alpha/api/v1/focus",
        Some(signed_init_data(42)),
        state,
        json!({
            "chat_id": 7,
            "thread_id": 11,
            "token": token,
            "operator_focus": "  operator focus  ",
        }),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, json!({ "operator_focus": "operator focus" }));

    let sent = sent.lock().await.clone();
    assert_eq!(
        sent,
        vec![super::FocusNotification {
            chat_id: 7,
            thread_id: 11,
            text: "Focus set: operator focus".to_string(),
        }]
    );
}
```

- [ ] **Step 5: Add the clear notification regression test**

In `crates/bot/src/telegram/dashboard.rs`, inside `#[cfg(test)] mod tests`, near the test from Step 4, add:

```rust
#[tokio::test]
async fn dashboard_focus_patch_sends_clear_notification_to_scope() {
    let temp = tempfile::tempdir().expect("tempdir");
    let conn = right_db::open_connection(temp.path(), true)
        .await
        .expect("open migrated db");
    right_db::thread_focus::set_operator(&conn, 7, 11, Some("old focus"))
        .await
        .expect("seed operator focus");
    drop(conn);

    let sent = Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let state = test_state_with_focus_notifier(
        temp.path().to_path_buf(),
        super::FocusNotifier::capture(Arc::clone(&sent)),
    );
    let token = signed_focus_scope_token("alpha", 7, 11, chrono::Utc::now().timestamp() + 60);

    let (status, body) = patch_json_with_state(
        "/dashboard/alpha/api/v1/focus",
        Some(signed_init_data(42)),
        state,
        json!({
            "chat_id": 7,
            "thread_id": 11,
            "token": token,
            "operator_focus": " \n\t ",
        }),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, json!({ "operator_focus": null }));

    let sent = sent.lock().await.clone();
    assert_eq!(
        sent,
        vec![super::FocusNotification {
            chat_id: 7,
            thread_id: 11,
            text: "Focus cleared".to_string(),
        }]
    );
}
```

- [ ] **Step 6: Add the notification failure regression test**

In `crates/bot/src/telegram/dashboard.rs`, inside `#[cfg(test)] mod tests`, near the tests from Steps 4 and 5, add:

```rust
#[tokio::test]
async fn dashboard_focus_patch_reports_notification_failure_after_saving() {
    let temp = tempfile::tempdir().expect("tempdir");
    right_db::open_connection(temp.path(), true)
        .await
        .expect("open migrated db");

    let state = test_state_with_focus_notifier(
        temp.path().to_path_buf(),
        super::FocusNotifier::fail("telegram unavailable"),
    );
    let token = signed_focus_scope_token("alpha", 7, 11, chrono::Utc::now().timestamp() + 60);

    let (status, body) = patch_json_with_state(
        "/dashboard/alpha/api/v1/focus",
        Some(signed_init_data(42)),
        state,
        json!({
            "chat_id": 7,
            "thread_id": 11,
            "token": token,
            "operator_focus": "operator focus",
        }),
    )
    .await;

    assert_eq!(status, StatusCode::BAD_GATEWAY);
    assert_eq!(body["error"], "focus_notification_failed");
    assert_eq!(
        body["detail"],
        "Focus saved, but notification could not be sent"
    );

    let conn = right_db::open_connection(temp.path(), false)
        .await
        .expect("reopen db");
    let row = right_db::thread_focus::get(&conn, 7, 11)
        .await
        .expect("read focus")
        .expect("focus row");
    assert_eq!(row.operator_focus.as_deref(), Some("operator focus"));
}
```

- [ ] **Step 7: Run the new focused tests and verify they fail**

Run:

```bash
devenv shell -- cargo nextest run -p right-bot dashboard_focus_patch_sends_notification_to_scope
```

Expected: FAIL to compile with missing `FocusNotifier`, `FocusNotification`, or `focus_notifier` field. That proves the regression test is ahead of implementation.

---

### Task 2: Add the Focus Notification Boundary

**Files:**
- Modify: `crates/bot/src/telegram/dashboard/focus.rs`
- Modify: `crates/bot/src/telegram/dashboard.rs`
- Modify: `crates/bot/src/lib.rs`
- Test: `crates/bot/src/telegram/dashboard.rs`

- [ ] **Step 1: Add notifier types and Telegram send helper**

In `crates/bot/src/telegram/dashboard/focus.rs`, replace the current imports at the top with imports that include the notifier dependencies:

```rust
use std::{future::Future, pin::Pin, sync::Arc, time::Duration};

use axum::{
    Json,
    body::Bytes,
    extract::{Path as AxumPath, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use teloxide::{
    payloads::SendMessageSetters as _,
    prelude::Requester as _,
    types::{ChatId, MessageId, ThreadId},
};

use super::mcp::parse_json_body;
use super::{DashboardState, authenticate_api, json_error};
```

Then add these definitions below `OPERATOR_FOCUS_MAX_CHARS`:

```rust
const FOCUS_NOTIFICATION_TIMEOUT: Duration = Duration::from_secs(10);

type FocusNotificationFuture =
    Pin<Box<dyn Future<Output = Result<(), FocusNotificationError>> + Send + 'static>>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FocusNotification {
    pub(crate) chat_id: i64,
    pub(crate) thread_id: i64,
    pub(crate) text: String,
}

#[derive(Debug, Clone)]
pub(crate) struct FocusNotificationError {
    detail: String,
}

impl FocusNotificationError {
    fn new(detail: impl Into<String>) -> Self {
        Self {
            detail: detail.into(),
        }
    }
}

impl std::fmt::Display for FocusNotificationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.detail)
    }
}

impl std::error::Error for FocusNotificationError {}

#[derive(Clone)]
pub(crate) struct FocusNotifier {
    send_fn: Arc<dyn Fn(FocusNotification) -> FocusNotificationFuture + Send + Sync>,
}

impl FocusNotifier {
    fn new<F>(send_fn: F) -> Self
    where
        F: Fn(FocusNotification) -> FocusNotificationFuture + Send + Sync + 'static,
    {
        Self {
            send_fn: Arc::new(send_fn),
        }
    }

    pub(crate) fn telegram(bot: crate::telegram::BotType) -> Self {
        Self::new(move |notification| {
            let bot = bot.clone();
            Box::pin(async move { send_focus_notification_with_bot(&bot, notification).await })
        })
    }

    pub(crate) async fn send(
        &self,
        notification: FocusNotification,
    ) -> Result<(), FocusNotificationError> {
        (self.send_fn)(notification).await
    }

    #[cfg(test)]
    pub(crate) fn noop() -> Self {
        Self::new(|_| Box::pin(async { Ok(()) }))
    }

    #[cfg(test)]
    pub(crate) fn capture(
        sent: Arc<tokio::sync::Mutex<Vec<FocusNotification>>>,
    ) -> Self {
        Self::new(move |notification| {
            let sent = Arc::clone(&sent);
            Box::pin(async move {
                sent.lock().await.push(notification);
                Ok(())
            })
        })
    }

    #[cfg(test)]
    pub(crate) fn fail(detail: &'static str) -> Self {
        let detail = detail.to_string();
        Self::new(move |_| {
            let detail = detail.clone();
            Box::pin(async move { Err(FocusNotificationError::new(detail)) })
        })
    }
}

async fn send_focus_notification_with_bot(
    bot: &crate::telegram::BotType,
    notification: FocusNotification,
) -> Result<(), FocusNotificationError> {
    let mut send = bot.send_message(ChatId(notification.chat_id), notification.text);
    if notification.thread_id != 0 {
        send = send.message_thread_id(ThreadId(MessageId(notification.thread_id as i32)));
    }

    tokio::time::timeout(FOCUS_NOTIFICATION_TIMEOUT, send)
        .await
        .map_err(|_| FocusNotificationError::new("telegram focus notification timed out"))?
        .map(|_| ())
        .map_err(|error| {
            FocusNotificationError::new(format!(
                "telegram focus notification failed: {error:#}"
            ))
        })
}

fn focus_notification_text(value: Option<&str>) -> String {
    match value {
        Some(focus) => format!("Focus set: {focus}"),
        None => "Focus cleared".to_string(),
    }
}
```

- [ ] **Step 2: Re-export notifier types from the dashboard module**

In `crates/bot/src/telegram/dashboard.rs`, after `mod focus;`, add:

```rust
pub(crate) use focus::FocusNotifier;
#[cfg(test)]
pub(crate) use focus::FocusNotification;
```

- [ ] **Step 3: Add the notifier to dashboard state**

In `crates/bot/src/telegram/dashboard.rs`, add this field to `DashboardState` after `bot_token`:

```rust
    pub focus_notifier: FocusNotifier,
```

- [ ] **Step 4: Populate the notifier in production startup**

In `crates/bot/src/lib.rs`, in the `telegram::dashboard::DashboardState { ... }` initializer passed to `build_dashboard_router`, add this field immediately after `bot_token: token.clone(),`:

```rust
            focus_notifier: telegram::dashboard::FocusNotifier::telegram(
                telegram::bot::build_bot(token.clone()),
            ),
```

- [ ] **Step 5: Populate the notifier in test state**

In `crates/bot/src/telegram/dashboard.rs`, inside `fn test_state(...) -> super::DashboardState`, add this field immediately after `bot_token: BOT_TOKEN.to_string(),`:

```rust
            focus_notifier: super::FocusNotifier::noop(),
```

- [ ] **Step 6: Run the focused set notification test**

Run:

```bash
devenv shell -- cargo nextest run -p right-bot dashboard_focus_patch_sends_notification_to_scope
```

Expected: FAIL at assertion because the route now compiles, but `sent` is still empty. That proves the notification boundary exists and route behavior is still missing.

---

### Task 3: Send Notification After Successful Focus Write

**Files:**
- Modify: `crates/bot/src/telegram/dashboard/focus.rs`
- Test: `crates/bot/src/telegram/dashboard.rs`

- [ ] **Step 1: Call the notifier after `set_operator` succeeds**

In `crates/bot/src/telegram/dashboard/focus.rs`, replace the final success response in `handle_update`:

```rust
    Json(serde_json::json!({ "operator_focus": value })).into_response()
```

with:

```rust
    let notification = FocusNotification {
        chat_id: req.chat_id,
        thread_id: req.thread_id,
        text: focus_notification_text(value),
    };
    if let Err(error) = state.focus_notifier.send(notification).await {
        tracing::warn!(
            agent = %state.agent_name,
            chat_id = req.chat_id,
            thread_id = req.thread_id,
            "focus update: notification failed after save: {error:#}"
        );
        return json_error(
            StatusCode::BAD_GATEWAY,
            "focus_notification_failed",
            Some("Focus saved, but notification could not be sent"),
        );
    }

    Json(serde_json::json!({ "operator_focus": value })).into_response()
```

- [ ] **Step 2: Run the new focused notification tests**

Run:

```bash
devenv shell -- cargo nextest run -p right-bot dashboard_focus_patch_sends
```

Expected: PASS for:

```text
dashboard_focus_patch_sends_notification_to_scope
dashboard_focus_patch_sends_clear_notification_to_scope
```

- [ ] **Step 3: Run the notification failure test**

Run:

```bash
devenv shell -- cargo nextest run -p right-bot dashboard_focus_patch_reports_notification_failure_after_saving
```

Expected: PASS.

- [ ] **Step 4: Run the existing focus route tests**

Run:

```bash
devenv shell -- cargo nextest run -p right-bot dashboard_focus
```

Expected: PASS. This verifies existing trim, clear, scope token, and length-cap behavior still works.

- [ ] **Step 5: Commit the implementation**

Run:

```bash
git status --short
```

Expected: only the implementation files from this plan are modified, plus unrelated pre-existing untracked files if they were already present.

Then run:

```bash
git add crates/bot/src/telegram/dashboard/focus.rs
git add crates/bot/src/telegram/dashboard.rs
git add crates/bot/src/lib.rs
git commit -m "feat(focus): notify chat on focus update"
```

Expected: commit succeeds.

---

### Task 4: Final Verification

**Files:**
- Verify: workspace

- [ ] **Step 1: Run the full workspace nextest suite**

Run:

```bash
devenv shell -- cargo nextest run --workspace
```

Expected: PASS. If a failure is unrelated and pre-existing, capture the exact failing test name and error before deciding whether to continue.

- [ ] **Step 2: Run workspace doctests**

Run:

```bash
devenv shell -- cargo test --doc --workspace
```

Expected: PASS.

- [ ] **Step 3: Inspect final status**

Run:

```bash
git status --short
```

Expected: no modified tracked files from this plan. Pre-existing untracked docs may remain untouched.
