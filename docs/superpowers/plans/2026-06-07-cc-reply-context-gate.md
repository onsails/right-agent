# CC Reply-Context Gate Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the archive-recoverability reply-body strip with a session-context gate so the agent always has (or can cheaply locate) the message it is replying to, and never receives a bare `text: ""`.

**Architecture:** Telegram hands us the replied-to message's text inline (`reply_to_message().text`), so "inline" never needs an MCP fetch. A pure decision function (`decide_reply_render`) picks one of four renderings from three signals: is the target the bot's own message (from Telegram's `from`), is it the bot's *freshest* message (content-match against the latest archived assistant row), and is it a *recently routed* user message in this session (a `conversation_messages` EXISTS query). Non-routed/out-of-window targets get their body inlined (full ≤500 chars, else truncated + a `get_messages_by_id` note); in-context targets get a short `truncated_text` locator; the bot's freshest message gets a bare note.

**Tech Stack:** Rust 2024, tokio, `right-db` (turso), the `right-bot` Telegram worker pipeline.

**Scope note:** This plan covers reply-context rendering only. Author compaction (dropping the per-message `name` line) and the `get_chat_member` MCP tool from the spec are a separate follow-up plan; this plan does **not** change author rendering.

---

## File Structure

- **Create** `crates/bot/src/telegram/reply_context.rs` — the `ReplyRender` enum, `decide_reply_render` pure function, truncation helper, and the constants `LOCATOR_MAX` / `REPLY_BODY_INLINE_MAX` / `IN_CONTEXT_WINDOW`. Pure logic, unit-tested without a DB.
- **Modify** `crates/right-db/src/conversation.rs` — two query helpers: `is_recent_routed_target` (EXISTS, windowed by turn_id) and `latest_assistant_text`.
- **Modify** `crates/bot/src/telegram/attachments.rs` — `RawReply` (handler-built) and `ReplyToBody` (worker-built, carries `ReplyRender`); render `reply_to` from the `ReplyRender`; omit empty `text`.
- **Modify** `crates/bot/src/telegram/handler.rs` — capture the reply target's text for *all* targets (including the bot's own), set `is_bot_target`, normalize empty triggering text to `None`.
- **Modify** `crates/bot/src/telegram/worker.rs` — replace `strip_recoverable_reply_to_body` with gate wiring that calls the DB helpers + `decide_reply_render`.
- **Modify** `crates/bot/src/telegram/mod.rs` — register the new `reply_context` module.
- **Modify** `crates/right-codegen/templates/right/prompt/OPERATING_INSTRUCTIONS.md` and `PROMPT_SYSTEM.md` — document `text` vs `truncated_text` and the reply tiers.

---

## Task 1: right-db gate query helpers

**Files:**
- Modify: `crates/right-db/src/conversation.rs` (add two `pub async fn` after `fetch_by_ids`, ~line 275)
- Test: same file's `#[cfg(test)] mod tests` (uses the existing `migrated_connection()` / `mark_routed` / `archive_message` test helpers)

- [ ] **Step 1: Write the failing tests**

Add to the `tests` module in `crates/right-db/src/conversation.rs`:

```rust
#[tokio::test]
async fn is_recent_routed_target_true_for_routed_in_window() {
    let conn = migrated_connection().await;
    // session S routes message 50 at turn 5
    mark_routed(&conn, "telegram", 100, 7, 50, "S", 5).await.unwrap();
    let hit = is_recent_routed_target(&conn, "telegram", 100, 7, 50, "S", 30, 5)
        .await
        .unwrap();
    assert!(hit, "message routed in this session within window must be in-context");
}

#[tokio::test]
async fn is_recent_routed_target_false_when_not_routed() {
    let conn = migrated_connection().await;
    // archived as a seen, non-routed message (the 4569 case): no root_session_id row
    archive_message(
        &conn,
        ConversationMessage {
            platform: "telegram",
            chat_id: 100,
            thread_id: 7,
            message_id: Some(50),
            sender_user_id: Some(1),
            sender_name: Some("A"),
            addressed_to_bot: false,
            routed_to_agent: false,
            root_session_id: None,
            turn_id: None,
            role: ConversationRole::User,
            content: "Сравни по времени в море",
        },
    )
    .await
    .unwrap();
    let hit = is_recent_routed_target(&conn, "telegram", 100, 7, 50, "S", 30, 5)
        .await
        .unwrap();
    assert!(!hit, "non-routed archived message is not in the model's session context");
}

#[tokio::test]
async fn is_recent_routed_target_false_when_outside_window() {
    let conn = migrated_connection().await;
    mark_routed(&conn, "telegram", 100, 7, 50, "S", 2).await.unwrap();
    // current turn is 40; window 30 => min turn 10; turn 2 is outside
    let hit = is_recent_routed_target(&conn, "telegram", 100, 7, 50, "S", 30, 40)
        .await
        .unwrap();
    assert!(!hit, "routed but compacted-out (outside window) must inline to be safe");
}

#[tokio::test]
async fn latest_assistant_text_returns_most_recent() {
    let conn = migrated_connection().await;
    archive_assistant_row(&conn, "S", 1, "older answer").await;
    archive_assistant_row(&conn, "S", 2, "freshest answer").await;
    let got = latest_assistant_text(&conn, "S").await.unwrap();
    assert_eq!(got.as_deref(), Some("freshest answer"));
}

#[tokio::test]
async fn latest_assistant_text_none_when_no_assistant_rows() {
    let conn = migrated_connection().await;
    let got = latest_assistant_text(&conn, "S").await.unwrap();
    assert!(got.is_none());
}

// local test helper: archive an assistant row for a session/turn
async fn archive_assistant_row(conn: &Connection, session: &str, turn: u64, content: &str) {
    archive_message(
        conn,
        ConversationMessage {
            platform: "telegram",
            chat_id: 100,
            thread_id: 7,
            message_id: None,
            sender_user_id: None,
            sender_name: None,
            addressed_to_bot: false,
            routed_to_agent: true,
            root_session_id: Some(session),
            turn_id: Some(turn),
            role: ConversationRole::Assistant,
            content,
        },
    )
    .await
    .unwrap();
}
```

> Before writing, open `crates/right-db/src/conversation.rs:21-46` and confirm the exact field names/lifetimes of `ConversationMessage<'a>`; adjust the struct literals above if a field differs.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `devenv shell -- cargo test -p right-db is_recent_routed_target`
Expected: FAIL — `cannot find function is_recent_routed_target` / `latest_assistant_text`.

- [ ] **Step 3: Implement the two helpers**

Insert after `fetch_by_ids` (after line 275) in `crates/right-db/src/conversation.rs`:

```rust
/// True when `message_id` was routed to the agent in `root_session_id` and its
/// turn is within the last `window` turns of that session. This is the
/// "already in the model's context" signal: routed => the model saw it;
/// within-window => not yet compacted out. Errs toward `false` (inline) so a
/// compacted-out target is re-sent rather than silently omitted.
pub async fn is_recent_routed_target(
    conn: &Connection,
    platform: &str,
    chat_id: i64,
    thread_id: i64,
    message_id: i32,
    root_session_id: &str,
    window: i64,
    current_turn_id: i64,
) -> Result<bool> {
    let min_turn = current_turn_id.saturating_sub(window);
    let rows = conn
        .query_all(
            "SELECT 1
             FROM conversation_messages
             WHERE platform = ? AND chat_id = ? AND thread_id = ?
               AND message_id = ? AND root_session_id = ?
               AND routed_to_agent = 1 AND turn_id > ?
             LIMIT 1",
            crate::params![platform, chat_id, thread_id, message_id, root_session_id, min_turn],
            |_row: &crate::row::Row<'_>| Ok(()),
        )
        .await?;
    Ok(!rows.is_empty())
}

/// Content of the most recent assistant row for `root_session_id`, or `None`.
/// Used to detect that a reply targets the bot's *freshest* message (so it gets
/// a bare note instead of a quoted locator).
pub async fn latest_assistant_text(
    conn: &Connection,
    root_session_id: &str,
) -> Result<Option<String>> {
    let rows = conn
        .query_all(
            "SELECT content
             FROM conversation_messages
             WHERE root_session_id = ? AND role = 'assistant'
             ORDER BY turn_id DESC, id DESC
             LIMIT 1",
            crate::params![root_session_id],
            |row: &crate::row::Row<'_>| row.get::<String>(0),
        )
        .await?;
    Ok(rows.into_iter().next())
}
```

> Confirm the row-accessor form against `fetched_from_row` (line 277): if `row.get::<T>(0)` is not the idiom in this crate, match whatever `fetched_from_row` uses.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `devenv shell -- cargo test -p right-db is_recent_routed_target` then `devenv shell -- cargo test -p right-db latest_assistant_text`
Expected: PASS (5 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/right-db/src/conversation.rs
git commit -m "feat(right-db): session-context gate queries for reply rendering"
```

---

## Task 2: reply_context module — ReplyRender + decide_reply_render

**Files:**
- Create: `crates/bot/src/telegram/reply_context.rs`
- Modify: `crates/bot/src/telegram/mod.rs` (add `pub(crate) mod reply_context;` alongside the other `mod` lines)
- Test: inline `#[cfg(test)] mod tests` in `reply_context.rs`

- [ ] **Step 1: Create the module with the failing test**

Create `crates/bot/src/telegram/reply_context.rs`:

```rust
//! Pure decision logic for how a replied-to message is rendered into CC input.
//!
//! Telegram supplies the replied-to text inline, so "inline" never costs an MCP
//! fetch. The gate only decides how much to show, based on whether the model
//! already has the target in its session context.

/// Max chars of a locator quote — just enough to identify *which* message.
pub const LOCATOR_MAX: usize = 120;
/// Max chars of an inlined full body before truncation + fetch note.
pub const REPLY_BODY_INLINE_MAX: usize = 500;
/// How many recent session turns count as "still in the model's context".
pub const IN_CONTEXT_WINDOW: i64 = 30;

/// How `reply_to` text should be rendered for one message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReplyRender {
    /// Bot's own freshest message — emit a bare note, no quoted text.
    OwnPrevious,
    /// In-context target — short locator so the model knows which message.
    Locator { text: String },
    /// Not in context, short — full body inline.
    Full { text: String },
    /// Not in context, long — truncated body + a `get_messages_by_id` note.
    Truncated { text: String, reply_to_id: i32 },
    /// No text on the target (e.g. media-only) — render author/attachments only.
    NoText,
}

/// Truncate to `max` Unicode scalar values, appending `…` when shortened.
fn truncate_with_ellipsis(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut s: String = text.chars().take(max).collect();
    s.push('…');
    s
}

/// Decide the reply rendering from the three context signals.
///
/// - `is_bot_target`: the reply target was authored by the bot itself.
/// - `is_latest_assistant`: the target equals the bot's freshest archived reply.
/// - `is_recent_routed_user`: the target is a user message routed in this
///   session within the window.
pub fn decide_reply_render(
    reply_to_id: i32,
    target_text: Option<&str>,
    is_bot_target: bool,
    is_latest_assistant: bool,
    is_recent_routed_user: bool,
) -> ReplyRender {
    let Some(text) = target_text.map(str::trim).filter(|t| !t.is_empty()) else {
        return ReplyRender::NoText;
    };
    if is_bot_target {
        return if is_latest_assistant {
            ReplyRender::OwnPrevious
        } else {
            ReplyRender::Locator { text: truncate_with_ellipsis(text, LOCATOR_MAX) }
        };
    }
    if is_recent_routed_user {
        return ReplyRender::Locator { text: truncate_with_ellipsis(text, LOCATOR_MAX) };
    }
    if text.chars().count() <= REPLY_BODY_INLINE_MAX {
        ReplyRender::Full { text: text.to_string() }
    } else {
        ReplyRender::Truncated {
            text: truncate_with_ellipsis(text, REPLY_BODY_INLINE_MAX),
            reply_to_id,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bot_freshest_message_is_own_previous() {
        let r = decide_reply_render(10, Some("hi there"), true, true, false);
        assert_eq!(r, ReplyRender::OwnPrevious);
    }

    #[test]
    fn bot_older_message_is_locator() {
        let r = decide_reply_render(10, Some("an earlier answer"), true, false, false);
        assert_eq!(r, ReplyRender::Locator { text: "an earlier answer".into() });
    }

    #[test]
    fn in_context_user_message_is_locator() {
        let r = decide_reply_render(10, Some("recent question"), false, false, true);
        assert_eq!(r, ReplyRender::Locator { text: "recent question".into() });
    }

    #[test]
    fn out_of_context_short_user_message_is_full() {
        // the 4569 case: non-routed, short -> inline full text
        let r = decide_reply_render(4569, Some("Сравни по времени в море"), false, false, false);
        assert_eq!(r, ReplyRender::Full { text: "Сравни по времени в море".into() });
    }

    #[test]
    fn out_of_context_long_user_message_is_truncated_with_id() {
        let long = "a".repeat(REPLY_BODY_INLINE_MAX + 50);
        let r = decide_reply_render(77, Some(&long), false, false, false);
        match r {
            ReplyRender::Truncated { text, reply_to_id } => {
                assert_eq!(reply_to_id, 77);
                assert_eq!(text.chars().count(), REPLY_BODY_INLINE_MAX + 1); // + the …
                assert!(text.ends_with('…'));
            }
            other => panic!("expected Truncated, got {other:?}"),
        }
    }

    #[test]
    fn empty_or_whitespace_target_is_no_text() {
        assert_eq!(decide_reply_render(1, Some("   "), false, false, false), ReplyRender::NoText);
        assert_eq!(decide_reply_render(1, None, false, false, false), ReplyRender::NoText);
    }

    #[test]
    fn locator_truncates_long_in_context_text() {
        let long = "b".repeat(LOCATOR_MAX + 10);
        let r = decide_reply_render(1, Some(&long), false, false, true);
        match r {
            ReplyRender::Locator { text } => {
                assert_eq!(text.chars().count(), LOCATOR_MAX + 1);
                assert!(text.ends_with('…'));
            }
            other => panic!("expected Locator, got {other:?}"),
        }
    }
}
```

Add to `crates/bot/src/telegram/mod.rs` (next to the existing `mod` declarations):

```rust
pub(crate) mod reply_context;
```

- [ ] **Step 2: Run the tests to verify they pass (logic is fully implemented above)**

Run: `devenv shell -- cargo test -p right-bot reply_context`
Expected: PASS (7 tests). If it fails to compile, fix the module wiring in `mod.rs` only.

- [ ] **Step 3: Commit**

```bash
git add crates/bot/src/telegram/reply_context.rs crates/bot/src/telegram/mod.rs
git commit -m "feat(bot): reply-render decision logic + constants"
```

---

## Task 3: ReplyToBody/RawReply types + handler changes

**Files:**
- Modify: `crates/bot/src/telegram/attachments.rs:404-414` (replace `ReplyToBody`)
- Modify: `crates/bot/src/telegram/handler.rs:278-301`
- Modify: `crates/bot/src/telegram/worker.rs:229-250` (DebounceMsg field type)

This task is a refactor that must compile before later tasks; its behavior is verified by Task 5/6 tests. Commit at the end.

- [ ] **Step 1: Replace the `ReplyToBody` struct with `RawReply` + a render-carrying `ReplyToBody`**

In `crates/bot/src/telegram/attachments.rs`, replace the struct at lines 404-414 with:

```rust
/// Replied-to message as captured at receive time (handler), before the
/// session-context gate runs. `text` is Telegram's copy of the target body and
/// is present even when the target is the bot's own message.
#[derive(Debug, Clone)]
pub struct RawReply {
    pub author: MessageAuthor,
    pub text: Option<String>,
    pub attachments: Vec<ResolvedAttachment>,
    /// The reply target was authored by the bot itself.
    pub is_bot_target: bool,
}

/// Replied-to message after the gate has decided how to render it (worker).
#[derive(Debug, Clone)]
pub struct ReplyToBody {
    pub author: MessageAuthor,
    pub attachments: Vec<ResolvedAttachment>,
    pub render: super::reply_context::ReplyRender,
}
```

- [ ] **Step 2: Update the handler to build `RawReply`, capture bot-own text, and normalize empty text**

In `crates/bot/src/telegram/handler.rs`, replace the reply-body block at lines 278-297 with:

```rust
    // Capture the replied-to message for ALL targets (including the bot's own).
    // The session-context gate in the worker decides how/whether to render it.
    let (reply_to_body, reply_to_attachments) = match msg.reply_to_message() {
        Some(r) => {
            let from = r.from.as_ref();
            let is_bot_target = from
                .map(|f| f.is_bot && f.id.0 == identity.user_id)
                .unwrap_or(false);
            let author = match from {
                Some(f) => super::attachments::MessageAuthor {
                    name: f.full_name(),
                    username: f.username.as_ref().map(|u| format!("@{u}")),
                    user_id: Some(f.id.0 as i64),
                },
                None => super::attachments::MessageAuthor {
                    name: String::new(),
                    username: None,
                    user_id: None,
                },
            };
            let body = super::attachments::RawReply {
                author,
                text: r.text().or(r.caption()).map(|t| t.to_string()),
                attachments: vec![], // populated post-debounce in worker
                is_bot_target,
            };
            let inbound = super::attachments::extract_attachments(r);
            (Some(body), inbound)
        }
        None => (None, vec![]),
    };
```

Then normalize the triggering message's own empty text to `None`. Replace line 301:

```rust
    let text = text
        .map(|t| super::mention::strip_bot_mentions(&t, &identity.username))
        .filter(|t| !t.trim().is_empty());
```

- [ ] **Step 3: Update the `DebounceMsg` field type**

In `crates/bot/src/telegram/worker.rs`, change line 242:

```rust
    pub reply_to_body: Option<super::attachments::RawReply>,
```

- [ ] **Step 4: Run the build to find every breakage**

Run: `devenv shell -- cargo build -p right-bot`
Expected: FAIL — the compiler lists every site that referenced `ReplyToBody.text` / `.omitted` (notably `worker.rs:1358-1386` and the renderer in `attachments.rs:516-555`). These are fixed in Tasks 4 and 5. Do not patch them yet; confirm the only errors are in `worker.rs` (strip/build region) and `attachments.rs` (`format_cc_input` + its tests).

- [ ] **Step 5: Commit the type/handler changes**

```bash
git add crates/bot/src/telegram/attachments.rs crates/bot/src/telegram/handler.rs crates/bot/src/telegram/worker.rs
git commit -m "refactor(bot): split RawReply (capture) from ReplyToBody (rendered)"
```

---

## Task 4: worker gate wiring (replace strip_recoverable_reply_to_body)

**Files:**
- Modify: `crates/bot/src/telegram/worker.rs:1015-1072` (delete `strip_recoverable_reply_to_body`, add `gate_reply_to_body`)
- Modify: `crates/bot/src/telegram/worker.rs:1358-1386` (call the gate)
- Modify: `crates/bot/src/telegram/worker.rs:5301-5495` (delete the obsolete strip tests; add a gate test)

- [ ] **Step 1: Write the failing gate test**

In the `worker.rs` tests module, delete the five `strip_recoverable_reply_body_*` tests (lines ~5351-5495) and the `reply_body` helper if unused, then add:

```rust
#[tokio::test]
async fn gate_inlines_full_text_for_non_routed_user_target() {
    // The 4569 case: archived but never routed in this session -> inline full.
    let temp = tempfile::tempdir().unwrap();
    let conn = right_db::open_connection(temp.path(), true).await.unwrap();
    right_db::conversation::archive_message(
        &conn,
        right_db::conversation::ConversationMessage {
            platform: "telegram",
            chat_id: 100,
            thread_id: 7,
            message_id: Some(4569),
            sender_user_id: Some(1),
            sender_name: Some("Andrey"),
            addressed_to_bot: false,
            routed_to_agent: false,
            root_session_id: None,
            turn_id: None,
            role: right_db::conversation::ConversationRole::User,
            content: "Сравни по времени в море",
        },
    )
    .await
    .unwrap();

    let raw = super::super::attachments::RawReply {
        author: super::super::attachments::MessageAuthor {
            name: "Andrey".into(),
            username: Some("@brainsmith".into()),
            user_id: Some(85743491),
        },
        text: Some("Сравни по времени в море".into()),
        attachments: vec![],
        is_bot_target: false,
    };

    let body = gate_reply_to_body(temp.path(), "telegram", 100, 7, Some(4569), 6, "S", raw)
        .await
        .unwrap();
    assert_eq!(
        body.render,
        super::super::reply_context::ReplyRender::Full {
            text: "Сравни по времени в море".into()
        }
    );
}
```

The signature includes `root_session_id: &str` (the session whose context we
gate against); the test passes `"S"` because the archived target has no
`root_session_id`, so it is correctly judged out-of-context.

- [ ] **Step 2: Run to verify it fails**

Run: `devenv shell -- cargo test -p right-bot gate_inlines_full_text_for_non_routed_user_target`
Expected: FAIL — `cannot find function gate_reply_to_body`.

- [ ] **Step 3: Replace `strip_recoverable_reply_to_body` with `gate_reply_to_body`**

Delete `strip_recoverable_reply_to_body` (lines 1015-1072) and insert:

```rust
/// Decide how the replied-to message is rendered, using the session-context
/// gate. Telegram already gave us `raw.text`, so this never needs a fetch — it
/// only decides full vs locator vs note vs truncated+fetch-note.
async fn gate_reply_to_body(
    agent_dir: &Path,
    platform: &str,
    chat_id: i64,
    eff_thread_id: i64,
    reply_to_id: Option<i32>,
    current_turn_id: u64,
    root_session_id: &str,
    raw: super::attachments::RawReply,
) -> Option<super::attachments::ReplyToBody> {
    use super::reply_context::{decide_reply_render, IN_CONTEXT_WINDOW};

    let reply_to_id = reply_to_id?;
    let current_turn_id = i64::try_from(current_turn_id).unwrap_or(i64::MAX);

    let (is_latest_assistant, is_recent_routed_user) =
        match right_db::open_connection(agent_dir, false).await {
            Ok(conn) => {
                let target = raw.text.as_deref().map(str::trim).unwrap_or("");
                let is_latest = if raw.is_bot_target && !target.is_empty() {
                    // Compare against the most recent archived assistant reply,
                    // matched by content (we do not persist outgoing message ids).
                    match right_db::conversation::latest_assistant_text(&conn, root_session_id)
                        .await
                    {
                        Ok(Some(latest)) => {
                            latest.trim().contains(target) || target.contains(latest.trim())
                        }
                        Ok(None) => false,
                        Err(e) => {
                            tracing::warn!("reply gate: latest_assistant_text failed: {e:#}");
                            false
                        }
                    }
                } else {
                    false
                };
                let is_routed = if raw.is_bot_target {
                    false
                } else {
                    right_db::conversation::is_recent_routed_target(
                        &conn, platform, chat_id, eff_thread_id, reply_to_id,
                        root_session_id, IN_CONTEXT_WINDOW, current_turn_id,
                    )
                    .await
                    .unwrap_or(false)
                };
                (is_latest, is_routed)
            }
            Err(e) => {
                tracing::warn!("reply gate: open_connection failed: {e:#}");
                (false, false)
            }
        };

    let render = decide_reply_render(
        reply_to_id,
        raw.text.as_deref(),
        raw.is_bot_target,
        is_latest_assistant,
        is_recent_routed_user,
    );
    Some(super::attachments::ReplyToBody {
        author: raw.author,
        attachments: raw.attachments,
        render,
    })
}
```

After Step 3 the crate will **not** compile yet — the call site at lines
1358-1386 still references the deleted `strip_recoverable_reply_to_body`. That is
wired in Step 4; do not run tests until then.

- [ ] **Step 4: Wire the gate at the call site**

In `crates/bot/src/telegram/worker.rs`, replace the block at lines 1358-1386 (the `reply_to_body` map + `strip_recoverable_reply_to_body` call + `build_input_message_from_debounce` push) with:

```rust
                let raw_reply = msg.reply_to_body.clone().map(|mut raw| {
                    raw.attachments = resolved_reply_to;
                    raw.text = crate::stt::combine_markers_with_text(
                        &reply_to_voice_markers,
                        raw.text.as_deref(),
                    );
                    raw
                });

                let reply_to_body = match raw_reply {
                    Some(raw) => {
                        gate_reply_to_body(
                            &ctx.agent_dir,
                            "telegram",
                            chat_id,
                            eff_thread_id,
                            msg.reply_to_id,
                            turn_id,
                            &session_uuid,
                            raw,
                        )
                        .await
                    }
                    None => None,
                };

                input_messages.push(build_input_message_from_debounce(
                    msg,
                    resolved,
                    &voice_markers,
                    reply_to_body,
                ));
```

> Confirm `turn_id` and `session_uuid` are in scope at this point (they are used
> elsewhere in the same loop — see `worker.rs:1493`, `:1763`). If `turn_id` is a
> different binding here, pass the per-invocation turn counter used for archiving
> this batch.

The voice-marker carve-out is now inside the gate: when `reply_to_voice_markers`
is non-empty, `raw.text` already carries the STT marker, so `decide_reply_render`
inlines it as `Full`/`Truncated` (never a `Locator` that would hide the marker) —
no separate `had_voice_markers` flag is needed because a voice target is never a
recoverable archive row anyway. Verify no other caller referenced
`strip_recoverable_reply_to_body`.

- [ ] **Step 5: Run the gate test + the package build**

Run: `devenv shell -- cargo test -p right-bot gate_inlines_full_text_for_non_routed_user_target`
Expected: PASS.
Run: `devenv shell -- cargo build -p right-bot`
Expected: only `attachments.rs` `format_cc_input` rendering errors remain (Task 5).

- [ ] **Step 6: Commit**

```bash
git add crates/bot/src/telegram/worker.rs
git commit -m "feat(bot): session-context reply gate replaces archive-recoverability strip"
```

---

## Task 5: attachments rendering (text omission + ReplyRender)

**Files:**
- Modify: `crates/bot/src/telegram/attachments.rs:515-561` (reply_to render + text render)
- Test: inline tests in `attachments.rs`

- [ ] **Step 1: Write failing renderer tests**

Add to the `attachments.rs` tests module (use the existing `InputMessage` construction pattern in that module; `ReplyToBody` now needs `author`, `attachments`, `render`):

```rust
#[test]
fn empty_text_is_omitted_not_rendered() {
    let msgs = vec![input_message_group_no_text_with_reply(
        4570,
        ReplyToBody {
            author: MessageAuthor { name: "Andrey".into(), username: Some("@brainsmith".into()), user_id: Some(85743491) },
            attachments: vec![],
            render: super::reply_context::ReplyRender::Full { text: "Сравни по времени в море".into() },
        },
    )];
    let out = format_cc_input(&msgs).unwrap();
    assert!(!out.contains("text: \"\""), "must never emit empty text:\n{out}");
    assert!(out.contains("text: \"Сравни по времени в море\""), "full reply body inlined:\n{out}");
}

#[test]
fn truncated_locator_uses_truncated_text_key() {
    let msgs = vec![input_message_group_no_text_with_reply(
        4570,
        ReplyToBody {
            author: MessageAuthor { name: "Andrey".into(), username: Some("@brainsmith".into()), user_id: Some(85743491) },
            attachments: vec![],
            render: super::reply_context::ReplyRender::Locator { text: "recent q…".into() },
        },
    )];
    let out = format_cc_input(&msgs).unwrap();
    assert!(out.contains("truncated_text: \"recent q…\""), "{out}");
    assert!(!out.contains("\n      text:"), "locator must not use the `text` key:\n{out}");
}

#[test]
fn own_previous_emits_note_no_text() {
    let msgs = vec![input_message_group_no_text_with_reply(
        4570,
        ReplyToBody {
            author: MessageAuthor { name: "bot".into(), username: None, user_id: Some(42) },
            attachments: vec![],
            render: super::reply_context::ReplyRender::OwnPrevious,
        },
    )];
    let out = format_cc_input(&msgs).unwrap();
    assert!(out.contains("note: \"your own previous message\""), "{out}");
    assert!(!out.contains("text:"), "own-previous carries no quoted text:\n{out}");
}

#[test]
fn truncated_not_in_context_emits_fetch_note() {
    let msgs = vec![input_message_group_no_text_with_reply(
        4570,
        ReplyToBody {
            author: MessageAuthor { name: "Andrey".into(), username: Some("@brainsmith".into()), user_id: Some(85743491) },
            attachments: vec![],
            render: super::reply_context::ReplyRender::Truncated { text: "long…".into(), reply_to_id: 4569 },
        },
    )];
    let out = format_cc_input(&msgs).unwrap();
    assert!(out.contains("truncated_text: \"long…\""), "{out}");
    assert!(out.contains("get_messages_by_id(4569)"), "fetch note for the long tail:\n{out}");
}

// helper: a group message with no own text that is a reply
fn input_message_group_no_text_with_reply(id: i32, reply: ReplyToBody) -> InputMessage {
    InputMessage {
        message_id: id,
        text: None,
        timestamp: chrono::Utc::now(),
        attachments: vec![],
        author: MessageAuthor { name: "Andrey".into(), username: Some("@brainsmith".into()), user_id: Some(85743491) },
        forward_info: None,
        reply_to_id: Some(4569),
        quoted_text: None,
        chat: ChatContext::Group { id: -100, title: Some("aibots".into()), topic_id: Some(458) },
        reply_to_body: Some(reply),
    }
}
```

> `chrono::Utc::now()` is fine in a unit test here (not a workflow). If the
> `attachments.rs` tests already have a timestamp constant/helper, reuse it.

- [ ] **Step 2: Run to verify they fail**

Run: `devenv shell -- cargo test -p right-bot -- empty_text_is_omitted truncated_locator own_previous truncated_not_in_context`
Expected: FAIL (compile error: old `ReplyToBody` fields / render branch missing).

- [ ] **Step 3: Rewrite the reply_to render block and guard empty text**

In `crates/bot/src/telegram/attachments.rs`, replace the reply-to body block (lines 515-555) with:

```rust
        // Reply-to body: rendered from the gate decision.
        if let Some(ref r) = m.reply_to_body {
            out.push_str("    reply_to:\n");
            out.push_str("      author:\n");
            writeln!(out, "        name: \"{}\"", yaml_escape_string(&r.author.name))
                .expect("infallible");
            if let Some(ref un) = r.author.username {
                writeln!(out, "        username: \"{}\"", yaml_escape_string(un))
                    .expect("infallible");
            }
            if let Some(uid) = r.author.user_id {
                writeln!(out, "        user_id: {uid}").expect("infallible");
            }
            match &r.render {
                super::reply_context::ReplyRender::OwnPrevious => {
                    out.push_str("      note: \"your own previous message\"\n");
                }
                super::reply_context::ReplyRender::Locator { text } => {
                    writeln!(out, "      truncated_text: \"{}\"", yaml_escape_string(text))
                        .expect("infallible");
                }
                super::reply_context::ReplyRender::Full { text } => {
                    writeln!(out, "      text: \"{}\"", yaml_escape_string(text))
                        .expect("infallible");
                }
                super::reply_context::ReplyRender::Truncated { text, reply_to_id } => {
                    writeln!(out, "      truncated_text: \"{}\"", yaml_escape_string(text))
                        .expect("infallible");
                    writeln!(
                        out,
                        "      note: \"full: mcp__right__get_messages_by_id({reply_to_id})\""
                    )
                    .expect("infallible");
                }
                super::reply_context::ReplyRender::NoText => {}
            }
            if !r.attachments.is_empty() {
                out.push_str("      attachments:\n");
                for att in &r.attachments {
                    writeln!(out, "        - type: {}", att.kind.as_str()).expect("infallible");
                    writeln!(out, "          path: {}", att.path.display()).expect("infallible");
                    writeln!(out, "          mime_type: {}", att.mime_type).expect("infallible");
                    if let Some(ref fname) = att.filename {
                        writeln!(out, "          filename: \"{}\"", yaml_escape_string(fname))
                            .expect("infallible");
                    }
                }
            }
        }
```

The text block at lines 557-561 already only emits when `m.text.is_some()`; because the handler now stores `None` for empty text (Task 3 Step 2), `text: ""` can no longer be produced. Leave that block as-is.

- [ ] **Step 4: Run the renderer tests**

Run: `devenv shell -- cargo test -p right-bot -- empty_text_is_omitted truncated_locator own_previous truncated_not_in_context`
Expected: PASS (4 tests). Fix any other `attachments.rs` tests that constructed the old `ReplyToBody { text, omitted, .. }` — update them to the new `{ author, attachments, render }` shape (search the test module for `ReplyToBody {` and `omitted`).

- [ ] **Step 5: Commit**

```bash
git add crates/bot/src/telegram/attachments.rs
git commit -m "feat(bot): render reply_to from gate decision; never emit empty text"
```

---

## Task 6: end-to-end regression for the 4569 bug

**Files:**
- Test: `crates/bot/src/telegram/worker.rs` tests module (exercises gate → render together)

- [ ] **Step 1: Write the failing regression test**

```rust
#[tokio::test]
async fn regression_bare_mention_reply_to_nonrouted_inlines_body_no_empty_text() {
    // Reproduces the "🍄?" failure: bare @mention (empty own text) replying to a
    // non-routed archived user message. The body must be inlined, with no text:"".
    let temp = tempfile::tempdir().unwrap();
    let conn = right_db::open_connection(temp.path(), true).await.unwrap();
    right_db::conversation::archive_message(
        &conn,
        right_db::conversation::ConversationMessage {
            platform: "telegram",
            chat_id: 100,
            thread_id: 458,
            message_id: Some(4569),
            sender_user_id: Some(85743491),
            sender_name: Some("Andrey Kuznetsov"),
            addressed_to_bot: false,
            routed_to_agent: false,
            root_session_id: None,
            turn_id: None,
            role: right_db::conversation::ConversationRole::User,
            content: "Сравни по времени в море",
        },
    )
    .await
    .unwrap();

    let raw = super::super::attachments::RawReply {
        author: super::super::attachments::MessageAuthor {
            name: "Andrey Kuznetsov".into(),
            username: Some("@brainsmith".into()),
            user_id: Some(85743491),
        },
        text: Some("Сравни по времени в море".into()),
        attachments: vec![],
        is_bot_target: false,
    };
    let body = gate_reply_to_body(temp.path(), "telegram", 100, 458, Some(4570), 6, "S", raw)
        .await
        .unwrap();

    let msg = super::super::attachments::InputMessage {
        message_id: 4570,
        text: None, // bare mention -> empty -> None
        timestamp: chrono::Utc::now(),
        attachments: vec![],
        author: super::super::attachments::MessageAuthor {
            name: "Andrey Kuznetsov".into(),
            username: Some("@brainsmith".into()),
            user_id: Some(85743491),
        },
        forward_info: None,
        reply_to_id: Some(4569),
        quoted_text: None,
        chat: super::super::attachments::ChatContext::Group {
            id: 100,
            title: Some("aibots".into()),
            topic_id: Some(458),
        },
        reply_to_body: Some(body),
    };
    let out = super::super::attachments::format_cc_input(&[msg]).unwrap();
    assert!(!out.contains("text: \"\""), "no empty text:\n{out}");
    assert!(out.contains("text: \"Сравни по времени в море\""), "body inlined:\n{out}");
    assert!(!out.contains("body omitted"), "no stale fetch-note path:\n{out}");
}
```

- [ ] **Step 2: Run to verify it passes**

Run: `devenv shell -- cargo test -p right-bot regression_bare_mention_reply_to_nonrouted`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add crates/bot/src/telegram/worker.rs
git commit -m "test(bot): regression for bare-mention reply to non-routed target"
```

---

## Task 7: documentation

**Files:**
- Modify: `crates/right-codegen/templates/right/prompt/OPERATING_INSTRUCTIONS.md` (the reply-metadata bullet changed by commit 954b0976)
- Modify: `PROMPT_SYSTEM.md`
- Check: `crates/right-codegen/src/agent_def_tests.rs` (the `operating_instructions_document_inbound_reply_metadata` assertion list)

- [ ] **Step 1: Update OPERATING_INSTRUCTIONS reply-metadata bullet**

Replace the `reply_to_id` / `reply_to` bullet with:

```markdown
- `reply_to_id` / `reply_to` — Telegram reply chain. `reply_to_id` is the target id; `reply_to.author` is who you are replying to. The body renders one of: `text` (the complete replied-to message), `truncated_text` (a shortened preview — ends with `…`, fetch the rest only if you need it via `mcp__right__get_messages_by_id(<id>)` when a `note` says so), or `note: "your own previous message"` (you are being replied to on the message you just sent — it is already above in this conversation).
```

- [ ] **Step 2: Update the codegen assertion list**

In `crates/right-codegen/src/agent_def_tests.rs`, update the needle list in `operating_instructions_document_inbound_reply_metadata` so each asserted substring exists in the new bullet (e.g. `"reply_to_id` is the target id"`, `"truncated_text"`, `"your own previous message"`, `"get_messages_by_id"`). Remove needles that no longer appear (e.g. `"fetch note when an archived/recoverable body is omitted"`).

- [ ] **Step 3: Update PROMPT_SYSTEM.md**

Find the section describing inbound message YAML fields and mirror the same `text` vs `truncated_text` vs `note` semantics and the three reply tiers. Keep it operator-facing (longer narration is allowed here, unlike the prompt-tier file).

- [ ] **Step 4: Run the codegen tests**

Run: `devenv shell -- cargo test -p right-codegen operating_instructions`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/right-codegen/templates/right/prompt/OPERATING_INSTRUCTIONS.md crates/right-codegen/src/agent_def_tests.rs PROMPT_SYSTEM.md
git commit -m "docs: document text vs truncated_text and reply context tiers"
```

---

## Task 8: final workspace verification

- [ ] **Step 1: Full workspace test (mandatory)**

Run: `devenv shell -- cargo test --workspace`
Expected: PASS. Known-flaky under parallel load: a `cc/invocation` pid-race test and a dashboard warn-count test — if either fails, re-run it isolated before blaming this change.

- [ ] **Step 2: Clippy gate**

Run: `devenv shell -- cargo clippy --workspace --all-targets`
Expected: no new warnings.

- [ ] **Step 3: Final commit if anything was touched**

```bash
git add -A && git commit -m "chore(bot): clippy + workspace green for reply-context gate"
```

---

## Self-Review Notes (resolve during execution)

- **`session_uuid` vs `turn_id` at the call site (Task 4 Step 5):** confirm both bindings exist where the gate is called; the plan assumes the per-batch turn counter and the session uuid are in scope (they are referenced nearby at `worker.rs:1493`/`:1763`). If the turn counter is named differently, thread the correct one.
- **`is_latest_assistant` content match** is intentionally fuzzy (substring both ways) — a false negative only downgrades a bare note to a short locator, which is harmless. Do not over-engineer it.
- **Voice/STT carve-out:** verify a voice reply target still surfaces its STT marker (it arrives in `raw.text`, so `decide_reply_render` yields `Full`/`Truncated`, never a marker-hiding `Locator`). Add a targeted test if `combine_markers_with_text` interacts unexpectedly.
- **Not in this plan:** author `name` removal and `get_chat_member` (follow-up plan). Author rendering is unchanged here.
