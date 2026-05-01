# Telegram Forward Admission + Reply-To Attachments — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make agents see attachments in two missed scenarios — (1) `@bot` comment followed by N forwards, (2) `@bot` reply to any message with attachments.

**Architecture:** Two independent fixes in `crates/bot/src/telegram/`. Fix A: extend the routing filter to admit forwards (`forward_origin.is_some()`), letting the existing 500 ms worker debounce + `batch_is_addressed` gate handle intent verification — same pattern media-group siblings already use. Fix B: extract attachments from `reply_to_message` in the handler, download them in the worker pipeline, expose them under the existing `reply_to:` YAML block.

**Tech Stack:** Rust 2024, teloxide, tokio, existing `download_attachments` and `extract_attachments` helpers.

**Spec:** [`docs/superpowers/specs/2026-05-01-telegram-forward-and-reply-attachments-design.md`](../specs/2026-05-01-telegram-forward-and-reply-attachments-design.md)

---

## Files

| Path | Change |
|---|---|
| `crates/bot/src/telegram/filter.rs` | Drop predicate gains `&& msg.forward_origin().is_none()`. New unit tests. |
| `crates/bot/src/telegram/attachments.rs` | `ReplyToBody` gains `attachments: Vec<ResolvedAttachment>`. `format_cc_input` emits `attachments:` under `reply_to:`. New unit test. |
| `crates/bot/src/telegram/worker.rs` | `DebounceMsg` gains `reply_to_attachments: Vec<InboundAttachment>`. Per-batch download loop runs `download_attachments` for it and splices into `ReplyToBody.attachments` on `InputMessage`. |
| `crates/bot/src/telegram/handler.rs` | When building `reply_to_body`, also call `extract_attachments(reply_target)` and stash result in `DebounceMsg.reply_to_attachments`. Initialize `ReplyToBody.attachments` as `vec![]` (worker fills it). |

Tests live alongside source via `#[cfg(test)] mod tests`.

---

## Task 1: Failing filter test for forward admission

**Files:**
- Modify: `crates/bot/src/telegram/filter.rs` (test module at line 68+)

- [ ] **Step 1: Add the failing test**

Append to the `mod tests` block (after `vonder_repro_three_album_siblings_all_routed`):

```rust
#[test]
fn forward_origin_passes_in_open_group() {
    let identity = BotIdentity {
        username: "rightaww_bot".into(),
        user_id: 999,
    };
    let chat_id = -1001;
    let sender_id = 42;
    let allowlist = allowlist_with(vec![], vec![chat_id]);

    // Forwarded document, no caption, no @mention anywhere.
    let msg: teloxide::types::Message = serde_json::from_value(serde_json::json!({
        "message_id": 1,
        "date": 0,
        "chat": {"id": chat_id, "type": "supergroup", "title": "g"},
        "from": {"id": sender_id, "is_bot": false, "first_name": "U"},
        "forward_origin": {
            "type": "user",
            "date": 0,
            "sender_user": {"id": 99999, "is_bot": false, "first_name": "Sender"}
        },
        "document": {
            "file_id": "BAAD",
            "file_unique_id": "uniq",
            "file_name": "edf.pdf",
            "mime_type": "application/pdf",
            "file_size": 1024
        }
    }))
    .unwrap();

    let f = make_routing_filter(allowlist, identity);
    let d = f(msg).expect("forward should pass in open group");
    assert!(d.address.is_none());
    assert!(d.group_open);
}

#[test]
fn forward_origin_dropped_for_untrusted_sender_and_closed_group() {
    let identity = BotIdentity {
        username: "rightaww_bot".into(),
        user_id: 999,
    };
    let chat_id = -1001;
    let sender_id = 42;
    // No trusted users, no open groups.
    let allowlist = allowlist_with(vec![], vec![]);

    let msg: teloxide::types::Message = serde_json::from_value(serde_json::json!({
        "message_id": 1,
        "date": 0,
        "chat": {"id": chat_id, "type": "supergroup", "title": "g"},
        "from": {"id": sender_id, "is_bot": false, "first_name": "U"},
        "forward_origin": {
            "type": "user",
            "date": 0,
            "sender_user": {"id": 99999, "is_bot": false, "first_name": "Sender"}
        },
        "document": {
            "file_id": "BAAD",
            "file_unique_id": "uniq",
            "file_name": "edf.pdf",
            "mime_type": "application/pdf",
            "file_size": 1024
        }
    }))
    .unwrap();

    let f = make_routing_filter(allowlist, identity);
    assert!(f(msg).is_none());
}
```

- [ ] **Step 2: Run tests, verify failure**

Run: `cargo test -p right-bot --lib telegram::filter::tests::forward_origin`
Expected: `forward_origin_passes_in_open_group` FAILS (assertion: forward should pass — currently dropped). The "untrusted" variant should already pass (untrusted is rejected before the forward check).

---

## Task 2: Implement filter change

**Files:**
- Modify: `crates/bot/src/telegram/filter.rs:55-57`

- [ ] **Step 1: Extend the drop predicate**

Replace:

```rust
                if addressed.is_none() && msg.media_group_id().is_none() {
                    return None;
                }
```

with:

```rust
                if addressed.is_none()
                    && msg.media_group_id().is_none()
                    && msg.forward_origin().is_none()
                {
                    return None;
                }
```

- [ ] **Step 2: Run filter tests, verify pass**

Run: `cargo test -p right-bot --lib telegram::filter`
Expected: all green, including the two new tests and existing `vonder_repro_three_album_siblings_all_routed` and `ordinary_group_message_without_mention_still_dropped`.

- [ ] **Step 3: Commit**

```bash
git add crates/bot/src/telegram/filter.rs
git commit -m "$(cat <<'EOF'
fix(bot): admit forwards through group routing filter

Forwards from trusted senders in open groups now reach the worker
debounce, where the existing batch_is_addressed gate handles intent —
same path media-group siblings already use. Comment-then-forward
bursts (single forward operation) batch within the 500 ms debounce
and are processed together.
EOF
)"
```

---

## Task 3: Failing format_cc_input test for reply_to attachments

**Files:**
- Modify: `crates/bot/src/telegram/attachments.rs` (tests block, near `format_cc_input_includes_reply_to_id` at line 1919)

- [ ] **Step 1: Add the failing test**

Append after `format_cc_input_includes_reply_to_id`:

```rust
#[test]
fn format_cc_input_includes_reply_to_attachments() {
    let ts = Utc::now();
    let msgs = vec![InputMessage {
        message_id: 5,
        text: Some("вот этот док".into()),
        timestamp: ts,
        attachments: vec![],
        author: test_author(),
        forward_info: None,
        reply_to_id: Some(3),
        chat: ChatContext::Private { id: 99 },
        reply_to_body: Some(ReplyToBody {
            author: MessageAuthor {
                name: "Sender".into(),
                username: None,
                user_id: Some(42),
            },
            text: Some("Votre document edf.pdf".into()),
            attachments: vec![ResolvedAttachment {
                kind: AttachmentKind::Document,
                path: std::path::PathBuf::from("/sandbox/inbox/document_3_0.pdf"),
                mime_type: "application/pdf".into(),
                filename: Some("edf.pdf".into()),
            }],
        }),
    }];
    let result = format_cc_input(&msgs).unwrap();
    assert!(result.contains("    reply_to:\n"), "missing reply_to block");
    assert!(result.contains("      text: \"Votre document edf.pdf\"\n"));
    assert!(
        result.contains("      attachments:\n"),
        "missing nested attachments under reply_to:\n{result}"
    );
    assert!(result.contains("        - type: document\n"));
    assert!(result.contains("          path: /sandbox/inbox/document_3_0.pdf\n"));
    assert!(result.contains("          mime_type: application/pdf\n"));
    assert!(result.contains("          filename: \"edf.pdf\"\n"));
}
```

- [ ] **Step 2: Run test, verify it fails to compile**

Run: `cargo test -p right-bot --lib telegram::attachments::tests::format_cc_input_includes_reply_to_attachments`
Expected: COMPILE ERROR — `ReplyToBody` has no field `attachments`.

---

## Task 4: Extend ReplyToBody and format_cc_input

**Files:**
- Modify: `crates/bot/src/telegram/attachments.rs:415-418` (struct)
- Modify: `crates/bot/src/telegram/attachments.rs:534-553` (format_cc_input)
- Modify: every existing test that constructs `ReplyToBody` (currently zero call sites — `reply_to_body` is always `None` in tests) and every production constructor of `ReplyToBody` (handler.rs)

- [ ] **Step 1: Add the field**

Replace `ReplyToBody` definition:

```rust
/// Body of the replied-to message — populated only when the user's message is
/// a Telegram reply AND the reply target is not the bot's own message.
#[derive(Debug, Clone)]
pub struct ReplyToBody {
    pub author: MessageAuthor,
    pub text: Option<String>,
    pub attachments: Vec<ResolvedAttachment>,
}
```

- [ ] **Step 2: Emit attachments in format_cc_input**

In `format_cc_input`, find the `reply_to:` block (around line 534-553) and append attachment emission after the `text` write. Replace the existing block:

```rust
        // Reply-to body: present only when the user replied to a non-bot message.
        if let Some(ref r) = m.reply_to_body {
            out.push_str("    reply_to:\n");
            out.push_str("      author:\n");
            writeln!(
                out,
                "        name: \"{}\"",
                yaml_escape_string(&r.author.name)
            )
            .expect("infallible");
            if let Some(ref un) = r.author.username {
                writeln!(out, "        username: \"{}\"", yaml_escape_string(un))
                    .expect("infallible");
            }
            if let Some(uid) = r.author.user_id {
                writeln!(out, "        user_id: {uid}").expect("infallible");
            }
            if let Some(ref t) = r.text {
                writeln!(out, "      text: \"{}\"", yaml_escape_string(t)).expect("infallible");
            }
        }
```

with:

```rust
        // Reply-to body: present only when the user replied to a non-bot message.
        if let Some(ref r) = m.reply_to_body {
            out.push_str("    reply_to:\n");
            out.push_str("      author:\n");
            writeln!(
                out,
                "        name: \"{}\"",
                yaml_escape_string(&r.author.name)
            )
            .expect("infallible");
            if let Some(ref un) = r.author.username {
                writeln!(out, "        username: \"{}\"", yaml_escape_string(un))
                    .expect("infallible");
            }
            if let Some(uid) = r.author.user_id {
                writeln!(out, "        user_id: {uid}").expect("infallible");
            }
            if let Some(ref t) = r.text {
                writeln!(out, "      text: \"{}\"", yaml_escape_string(t)).expect("infallible");
            }
            if !r.attachments.is_empty() {
                out.push_str("      attachments:\n");
                for att in &r.attachments {
                    writeln!(out, "        - type: {}", att.kind.as_str())
                        .expect("infallible");
                    writeln!(out, "          path: {}", att.path.display())
                        .expect("infallible");
                    writeln!(out, "          mime_type: {}", att.mime_type)
                        .expect("infallible");
                    if let Some(ref fname) = att.filename {
                        let escaped = yaml_escape_string(fname);
                        writeln!(out, "          filename: \"{escaped}\"")
                            .expect("infallible");
                    }
                }
            }
        }
```

- [ ] **Step 3: Run the new test, verify pass**

Run: `cargo test -p right-bot --lib telegram::attachments::tests::format_cc_input_includes_reply_to_attachments`
Expected: PASS.

- [ ] **Step 4: Run the full attachments test suite**

Run: `cargo test -p right-bot --lib telegram::attachments`
Expected: all green. (No existing test constructs `ReplyToBody`, so the new field doesn't break callers in this file.)

---

## Task 5: Add `reply_to_attachments` to DebounceMsg

**Files:**
- Modify: `crates/bot/src/telegram/worker.rs:61-76` (DebounceMsg)

- [ ] **Step 1: Add the field**

Replace the `DebounceMsg` definition:

```rust
/// A single Telegram message queued into the debounce channel.
#[derive(Clone)]
pub struct DebounceMsg {
    pub message_id: i32,
    pub text: Option<String>,
    pub timestamp: DateTime<Utc>,
    pub attachments: Vec<super::attachments::InboundAttachment>,
    pub author: super::attachments::MessageAuthor,
    pub forward_info: Option<super::attachments::ForwardInfo>,
    pub reply_to_id: Option<i32>,
    pub address: Option<super::mention::AddressKind>,
    pub group_open: bool,
    pub chat: super::attachments::ChatContext,
    pub reply_to_body: Option<super::attachments::ReplyToBody>,
    /// Inbound attachments from the replied-to message, downloaded in the
    /// worker pipeline alongside primary attachments. Always empty if the
    /// user did not reply to a non-bot message.
    pub reply_to_attachments: Vec<super::attachments::InboundAttachment>,
    /// `Some(id)` when this message is part of a Telegram album (media group);
    /// shared by all siblings of the album.
    pub media_group_id: Option<String>,
}
```

- [ ] **Step 2: Try to build (will fail at handler.rs)**

Run: `cargo build -p right-bot`
Expected: COMPILE ERROR in `handler.rs` — `DebounceMsg` constructor missing `reply_to_attachments`.

---

## Task 6: Populate reply_to_attachments in handler.rs

**Files:**
- Modify: `crates/bot/src/telegram/handler.rs:274-306` (reply_to_body construction + DebounceMsg construction)

- [ ] **Step 1: Extract reply-to attachments alongside body**

Replace the existing `reply_to_body` block (around line 274-287):

```rust
    // Populate reply_to_body only when the user replied to a non-bot message.
    // When they reply to our own bot message, the context is already in the CC
    // session history — emitting it again would be noisy and duplicative.
    let reply_to_body = msg.reply_to_message().and_then(|r| {
        let from = r.from.as_ref()?;
        if from.is_bot && from.id.0 == identity.user_id {
            return None;
        }
        Some(super::attachments::ReplyToBody {
            author: super::attachments::MessageAuthor {
                name: from.full_name(),
                username: from.username.as_ref().map(|u| format!("@{u}")),
                user_id: Some(from.id.0 as i64),
            },
            text: r.text().or(r.caption()).map(|t| t.to_string()),
        })
    });
```

with:

```rust
    // Populate reply_to_body only when the user replied to a non-bot message.
    // When they reply to our own bot message, the context is already in the CC
    // session history — emitting it again would be noisy and duplicative.
    // `reply_to_attachments` mirrors `reply_to_body`: empty when the body is
    // None, otherwise the inbound attachments of the replied-to message.
    let (reply_to_body, reply_to_attachments) = match msg.reply_to_message() {
        Some(r) => match r.from.as_ref() {
            Some(from) if !(from.is_bot && from.id.0 == identity.user_id) => {
                let body = super::attachments::ReplyToBody {
                    author: super::attachments::MessageAuthor {
                        name: from.full_name(),
                        username: from.username.as_ref().map(|u| format!("@{u}")),
                        user_id: Some(from.id.0 as i64),
                    },
                    text: r.text().or(r.caption()).map(|t| t.to_string()),
                    attachments: vec![], // populated post-debounce in worker
                };
                let inbound = super::attachments::extract_attachments(r);
                (Some(body), inbound)
            }
            _ => (None, vec![]),
        },
        None => (None, vec![]),
    };
```

- [ ] **Step 2: Pass through to DebounceMsg**

Update the `DebounceMsg` literal (around line 293-306) to include the new field. Replace:

```rust
    let debounce_msg = DebounceMsg {
        message_id: msg.id.0,
        text,
        timestamp: chrono::Utc::now(),
        attachments,
        author,
        forward_info,
        reply_to_id,
        address: decision.address.clone(),
        group_open: decision.group_open,
        chat: chat_ctx,
        reply_to_body,
        media_group_id: msg.media_group_id().map(|m| m.0.clone()),
    };
```

with:

```rust
    let debounce_msg = DebounceMsg {
        message_id: msg.id.0,
        text,
        timestamp: chrono::Utc::now(),
        attachments,
        author,
        forward_info,
        reply_to_id,
        address: decision.address.clone(),
        group_open: decision.group_open,
        chat: chat_ctx,
        reply_to_body,
        reply_to_attachments,
        media_group_id: msg.media_group_id().map(|m| m.0.clone()),
    };
```

- [ ] **Step 3: Build, verify it compiles**

Run: `cargo build -p right-bot`
Expected: SUCCESS (worker.rs still works because the new field is just stored, not yet read).

---

## Task 7: Download reply_to attachments in the worker

**Files:**
- Modify: `crates/bot/src/telegram/worker.rs:476-516` (per-msg loop in batch processing)

- [ ] **Step 1: Run download for reply_to_attachments and splice into ReplyToBody**

In the batch-processing loop, locate the existing `for msg in &batch { ... }` block (lines 476-516). Replace it:

```rust
            // Download attachments for all messages in batch
            let mut input_messages = Vec::with_capacity(batch.len());
            let mut skip_batch = false;
            for msg in &batch {
                let (resolved, voice_markers) = if msg.attachments.is_empty() {
                    (vec![], vec![])
                } else {
                    match super::attachments::download_attachments(
                        &msg.attachments,
                        msg.message_id,
                        &ctx.bot,
                        &ctx.agent_dir,
                        ctx.ssh_config_path.as_deref(),
                        ctx.resolved_sandbox.as_deref(),
                        tg_chat_id,
                        eff_thread_id,
                        ctx.stt.as_deref(),
                    )
                    .await
                    {
                        Ok(r) => r,
                        Err(e) => {
                            tracing::error!(?key, "attachment download failed: {:#}", e);
                            let _ = send_tg(&ctx.bot, tg_chat_id, eff_thread_id, &format!("⚠️ Failed to download attachments: {e:#}\nYour message was not forwarded.")).await;
                            skip_batch = true;
                            break;
                        }
                    }
                };

                // Reply-to attachments: same pipeline, separate batch keyed off
                // the replied-to message id so files land at predictable paths
                // (document_<replied_to_id>_<idx>.pdf, etc).
                let resolved_reply_to = if msg.reply_to_attachments.is_empty() {
                    vec![]
                } else {
                    let reply_to_msg_id = msg.reply_to_id.unwrap_or(msg.message_id);
                    match super::attachments::download_attachments(
                        &msg.reply_to_attachments,
                        reply_to_msg_id,
                        &ctx.bot,
                        &ctx.agent_dir,
                        ctx.ssh_config_path.as_deref(),
                        ctx.resolved_sandbox.as_deref(),
                        tg_chat_id,
                        eff_thread_id,
                        ctx.stt.as_deref(),
                    )
                    .await
                    {
                        Ok((resolved, _markers)) => resolved,
                        Err(e) => {
                            tracing::error!(
                                ?key,
                                "reply_to attachment download failed: {:#}",
                                e
                            );
                            let _ = send_tg(
                                &ctx.bot,
                                tg_chat_id,
                                eff_thread_id,
                                &format!(
                                    "⚠️ Failed to download attachment from replied-to message: {e:#}",
                                ),
                            )
                            .await;
                            skip_batch = true;
                            break;
                        }
                    }
                };

                let reply_to_body = msg.reply_to_body.clone().map(|mut body| {
                    body.attachments = resolved_reply_to;
                    body
                });

                input_messages.push(super::attachments::InputMessage {
                    message_id: msg.message_id,
                    text: crate::stt::combine_markers_with_text(
                        &voice_markers,
                        msg.text.as_deref(),
                    ),
                    timestamp: msg.timestamp,
                    attachments: resolved,
                    author: msg.author.clone(),
                    forward_info: msg.forward_info.clone(),
                    reply_to_id: msg.reply_to_id,
                    chat: msg.chat.clone(),
                    reply_to_body,
                });
            }
            if skip_batch {
                continue;
            }
```

- [ ] **Step 2: Build the workspace**

Run: `cargo build --workspace`
Expected: SUCCESS, no warnings introduced.

- [ ] **Step 3: Run the full bot test suite**

Run: `cargo test -p right-bot --lib`
Expected: all green. No test currently exercises the worker's download loop end-to-end (it depends on a live bot + sandbox), so this confirms nothing got broken at the unit level.

---

## Task 8: Final workspace check + commit Fix B

- [ ] **Step 1: Workspace build (debug)**

Run: `cargo build --workspace`
Expected: SUCCESS.

- [ ] **Step 2: Workspace clippy**

Run: `cargo clippy --workspace --all-targets -- -D warnings`
Expected: no new warnings.

- [ ] **Step 3: Workspace tests**

Run: `cargo test --workspace`
Expected: all green.

- [ ] **Step 4: Commit Fix B**

```bash
git add crates/bot/src/telegram/attachments.rs \
        crates/bot/src/telegram/worker.rs \
        crates/bot/src/telegram/handler.rs
git commit -m "$(cat <<'EOF'
fix(bot): extract attachments from reply_to_message

When a user replies to a message with @mention, the bot now downloads
the replied-to message's attachments and surfaces them under the
existing reply_to: YAML block. Same pipeline as primary attachments
(STT for voice/video_note, 20 MB Telegram-download limit, sandbox
inbox upload).
EOF
)"
```

---

## Task 9: Manual reproduction

Both fixes need confirmation in a real Telegram chat — there is no integration harness that exercises the dispatcher → filter → worker → CC path end-to-end with a real sandbox.

- [ ] **Step 1: Restart agent**

Run: `right restart <agent>`
Expected: agent reconnects, bot identity log line appears.

- [ ] **Step 2: Reproduce Fix A (comment + forwards)**

In an open group:
1. Type `@<botname> вот документы` and send.
2. Within ~2 seconds, forward 2-3 documents from another chat.

Expected:
- Single `claude -p` invocation processes all forwards in one batch.
- `~/.right/logs/<agent>.log.<date>` shows `attachment_count=N` for the forwards on subsequent `handle_message` lines (no longer dropped at the filter).
- Agent's reply references the forwarded files.

- [ ] **Step 3: Reproduce Fix B (reply with mention)**

In any chat:
1. Forward a single document cold (no preceding `@bot` text).
2. Reply to that forward with `@<botname> вот этот док`.

Expected:
- Worker log shows `attachment_count=0` on the reply (the reply itself has no media), but the input passed to CC contains a `reply_to:` block with `attachments:` listing `/sandbox/inbox/document_<reply_to_id>_0.pdf`.
- Agent confirms it sees the document.

- [ ] **Step 4: Verify drop semantics still work**

In an open group, from a trusted user:
1. Forward a document with no preceding `@bot` text and no reply.

Expected: no `claude -p` invocation. Log shows the forward arrived at the dispatcher but the worker's `batch_is_addressed` gate dropped the lone-forward batch (look for `media-group batch had no addressed sibling — dropping without CC` debug log — message text matches both lone forwards and lone media-group siblings since they share the post-debounce path).

---

## Self-review checklist (already done by author)

- **Spec coverage:** Filter change → Tasks 1-2. ReplyToBody extension → Tasks 3-4. DebounceMsg field → Task 5. Handler population → Task 6. Worker download → Task 7. Manual repro for both scenarios → Task 9. ✓
- **Placeholders:** none.
- **Type consistency:** `ReplyToBody.attachments: Vec<ResolvedAttachment>` (worker output type) used by both `format_cc_input` and `InputMessage.reply_to_body`. `DebounceMsg.reply_to_attachments: Vec<InboundAttachment>` (handler output type) only flows through worker download. Field initialization in handler uses `vec![]` for `ReplyToBody.attachments` so the struct is constructible before download.
- **No unused changes:** every introduced field has a producer (handler) and a consumer (worker / format_cc_input).
