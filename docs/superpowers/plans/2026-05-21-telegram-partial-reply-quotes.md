# Telegram Partial Reply Quotes Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Preserve Telegram partial reply quote text in the Claude user-turn YAML as `quoted_text`.

**Architecture:** Carry `TextQuote.text` through the existing Telegram ingress structs: `Message::quote()` in `handler.rs` -> `DebounceMsg` in `worker.rs` -> `InputMessage` in `attachments.rs` -> `format_cc_input` YAML. Keep existing full reply behavior unchanged: `reply_to:` remains only for non-bot reply targets, while `quoted_text` is emitted whenever Telegram supplies a quote.

**Tech Stack:** Rust 2024, teloxide 0.17 / teloxide-core 0.13, tokio, serde_json test fixtures, `devenv shell -- cargo test`.

---

## File Structure

- Modify `crates/bot/src/telegram/attachments.rs`
  - Add `quoted_text: Option<String>` to `InputMessage`.
  - Emit `quoted_text` from `format_cc_input`.
  - Add formatter tests for quote emission, coexistence with `reply_to:`, and YAML escaping.
- Modify `crates/bot/src/telegram/handler.rs`
  - Add a small private helper that extracts `msg.quote().text`.
  - Store quote text in `DebounceMsg` while handling an incoming Telegram message.
  - Add a serde_json fixture test for quote extraction.
- Modify `crates/bot/src/telegram/worker.rs`
  - Add `quoted_text: Option<String>` to `DebounceMsg`.
  - Pass the field into `InputMessage`.
  - Extract the existing inline `InputMessage` construction into a small private helper so pass-through has a direct unit test.
- Modify `docs/architecture/sessions.md`
  - Document the inbound user-turn YAML fields relevant to replies, including `quoted_text`.
- Inspect `PROMPT_SYSTEM.md`
  - No edit unless it already documents inbound Telegram YAML fields.

## Task 0: Worktree Baseline

**Files:**
- Read: `docs/superpowers/specs/2026-05-21-telegram-partial-reply-quotes-design.md`
- Read: `crates/bot/src/telegram/attachments.rs`
- Read: `crates/bot/src/telegram/handler.rs`
- Read: `crates/bot/src/telegram/worker.rs`
- Read: `docs/architecture/sessions.md`

- [ ] **Step 1: Confirm the execution worktree**

Run from the worktree root:

```sh
pwd
git status --short
git log -1 --oneline
```

Expected:
- `pwd` ends with `.worktrees/telegram-partial-reply-quotes`.
- `git status --short` is clean.
- `git log -1 --oneline` includes the committed plan and spec history from the source branch.

- [ ] **Step 2: Read the approved design**

Run:

```sh
devenv shell -- sed -n '1,220p' docs/superpowers/specs/2026-05-21-telegram-partial-reply-quotes-design.md
```

Expected: the design states that only `TextQuote.text` becomes `quoted_text`, with no archive/session lookup.

- [ ] **Step 3: Run the targeted baseline**

Run:

```sh
devenv shell -- cargo test -p right-bot telegram::attachments
```

Expected: PASS before edits. If it fails, record the failure before changing code and investigate only failures relevant to this feature.

## Task 1: Prompt Model And Formatter

**Files:**
- Modify: `crates/bot/src/telegram/attachments.rs`

- [ ] **Step 1: Write failing formatter tests**

In `crates/bot/src/telegram/attachments.rs`, inside the existing `#[cfg(test)] mod tests`, add these tests near `format_cc_input_includes_reply_to_id` and `format_cc_input_includes_reply_to_attachments`:

```rust
    #[test]
    fn format_cc_input_includes_quoted_text() {
        let ts = Utc::now();
        let msgs = vec![InputMessage {
            message_id: 5,
            text: Some("what do you mean here?".into()),
            timestamp: ts,
            attachments: vec![],
            author: test_author(),
            forward_info: None,
            reply_to_id: Some(3),
            quoted_text: Some("selected fragment".into()),
            chat: ChatContext::Private { id: 99 },
            reply_to_body: None,
        }];

        let result = format_cc_input(&msgs).unwrap();

        assert!(result.contains("    reply_to_id: 3\n"));
        assert!(result.contains("    quoted_text: \"selected fragment\"\n"));
    }

    #[test]
    fn format_cc_input_can_include_reply_to_body_and_quoted_text() {
        let ts = Utc::now();
        let msgs = vec![InputMessage {
            message_id: 5,
            text: Some("what about this part?".into()),
            timestamp: ts,
            attachments: vec![],
            author: test_author(),
            forward_info: None,
            reply_to_id: Some(3),
            quoted_text: Some("only this sentence".into()),
            chat: ChatContext::Private { id: 99 },
            reply_to_body: Some(ReplyToBody {
                author: MessageAuthor {
                    name: "Sender".into(),
                    username: None,
                    user_id: Some(42),
                },
                text: Some("first sentence. only this sentence. last sentence.".into()),
                attachments: vec![],
            }),
        }];

        let result = format_cc_input(&msgs).unwrap();

        assert!(result.contains("    reply_to:\n"), "missing reply_to block:\n{result}");
        assert!(result.contains("      text: \"first sentence. only this sentence. last sentence.\"\n"));
        assert!(result.contains("    quoted_text: \"only this sentence\"\n"));
    }

    #[test]
    fn format_cc_input_escapes_quoted_text() {
        let ts = Utc::now();
        let msgs = vec![InputMessage {
            message_id: 5,
            text: Some("what?".into()),
            timestamp: ts,
            attachments: vec![],
            author: test_author(),
            forward_info: None,
            reply_to_id: Some(3),
            quoted_text: Some("line1\nline2\t\"quoted\"".into()),
            chat: ChatContext::Private { id: 99 },
            reply_to_body: None,
        }];

        let result = format_cc_input(&msgs).unwrap();

        assert!(result.contains(r#"    quoted_text: "line1\nline2\t\"quoted\""#));
    }
```

- [ ] **Step 2: Run the failing formatter tests**

Run:

```sh
devenv shell -- cargo test -p right-bot quoted_text
```

Expected: FAIL to compile with `struct InputMessage has no field named quoted_text`.

- [ ] **Step 3: Add `quoted_text` to `InputMessage`**

In `crates/bot/src/telegram/attachments.rs`, update the struct:

```rust
pub struct InputMessage {
    pub message_id: i32,
    pub text: Option<String>,
    pub timestamp: DateTime<Utc>,
    pub attachments: Vec<ResolvedAttachment>,
    pub author: MessageAuthor,
    pub forward_info: Option<ForwardInfo>,
    pub reply_to_id: Option<i32>,
    pub quoted_text: Option<String>,
    pub chat: ChatContext,
    pub reply_to_body: Option<ReplyToBody>,
}
```

Then update every existing `InputMessage { ... }` literal in `crates/bot/src/telegram/attachments.rs` that is not one of the new quote tests by adding:

```rust
            quoted_text: None,
```

Place it immediately after `reply_to_id: ...` in each literal.

- [ ] **Step 4: Emit `quoted_text` in YAML**

In `format_cc_input`, immediately after the `reply_to_id` block and before the `reply_to_body` block, insert:

```rust
        // Telegram partial reply quote: the selected fragment from the
        // triggering reply, distinct from the full reply_to body.
        if let Some(ref quoted_text) = m.quoted_text {
            let escaped = yaml_escape_string(quoted_text);
            writeln!(out, "    quoted_text: \"{escaped}\"").expect("infallible");
        }
```

- [ ] **Step 5: Run the formatter tests**

Run:

```sh
devenv shell -- cargo test -p right-bot quoted_text
```

Expected: PASS.

- [ ] **Step 6: Run the attachments module tests**

Run:

```sh
devenv shell -- cargo test -p right-bot telegram::attachments
```

Expected: PASS. If compile errors mention missing `quoted_text`, add `quoted_text: None` to the reported `InputMessage` literal and rerun this same command once.

- [ ] **Step 7: Commit Task 1**

Run:

```sh
git add crates/bot/src/telegram/attachments.rs
git commit -m "feat(bot): format telegram partial reply quotes"
```

Expected: commit succeeds.

## Task 2: Telegram Ingress And Worker Pass-Through

**Files:**
- Modify: `crates/bot/src/telegram/handler.rs`
- Modify: `crates/bot/src/telegram/worker.rs`

- [ ] **Step 1: Write the handler quote extraction test**

In `crates/bot/src/telegram/handler.rs`, inside the existing `#[cfg(test)] mod tests`, add:

```rust
    #[test]
    fn telegram_quote_text_extracts_partial_reply_quote() {
        let msg: Message = serde_json::from_value(serde_json::json!({
            "message_id": 20,
            "date": 0,
            "chat": {"id": 99, "type": "private", "first_name": "User"},
            "from": {"id": 42, "is_bot": false, "first_name": "User"},
            "text": "what do you mean here?",
            "reply_to_message": {
                "message_id": 19,
                "date": 0,
                "chat": {"id": 99, "type": "private", "first_name": "Agent"},
                "from": {"id": 100, "is_bot": true, "first_name": "Agent"},
                "text": "first sentence. selected fragment. last sentence."
            },
            "quote": {
                "text": "selected fragment",
                "position": 16,
                "is_manual": true
            }
        }))
        .unwrap();

        assert_eq!(
            telegram_quote_text(&msg).as_deref(),
            Some("selected fragment")
        );
    }
```

- [ ] **Step 2: Run the failing handler test**

Run:

```sh
devenv shell -- cargo test -p right-bot telegram::handler::tests::telegram_quote_text_extracts_partial_reply_quote
```

Expected: FAIL to compile with `cannot find function telegram_quote_text`.

- [ ] **Step 3: Add the handler helper and store the quote**

In `crates/bot/src/telegram/handler.rs`, add this private helper near the other small message helpers, before `handle_message`:

```rust
fn telegram_quote_text(msg: &Message) -> Option<String> {
    msg.quote().map(|quote| quote.text.clone())
}
```

In `handle_message`, immediately after:

```rust
    let reply_to_id = msg.reply_to_message().map(|m| m.id.0);
```

add:

```rust
    let quoted_text = telegram_quote_text(&msg);
```

In the `DebounceMsg` literal, immediately after `reply_to_id,`, add:

```rust
        quoted_text,
```

- [ ] **Step 4: Write the worker pass-through test**

In `crates/bot/src/telegram/worker.rs`, inside the existing `#[cfg(test)] mod tests`, add this test after `debug_msg`:

```rust
    #[test]
    fn build_input_message_passes_quoted_text() {
        let mut msg = debug_msg(7, None);
        msg.text = Some("what do you mean?".into());
        msg.reply_to_id = Some(6);
        msg.quoted_text = Some("selected fragment".into());

        let input = build_input_message_from_debounce(&msg, vec![], &[], None);

        assert_eq!(input.reply_to_id, Some(6));
        assert_eq!(input.quoted_text.as_deref(), Some("selected fragment"));
    }
```

- [ ] **Step 5: Run the failing worker test**

Run:

```sh
devenv shell -- cargo test -p right-bot telegram::worker::tests::build_input_message_passes_quoted_text
```

Expected: FAIL to compile because `DebounceMsg` and the helper do not yet expose `quoted_text`.

- [ ] **Step 6: Add `quoted_text` to `DebounceMsg`**

In `crates/bot/src/telegram/worker.rs`, update `DebounceMsg`:

```rust
pub struct DebounceMsg {
    pub message_id: i32,
    pub text: Option<String>,
    pub timestamp: DateTime<Utc>,
    pub attachments: Vec<super::attachments::InboundAttachment>,
    pub author: super::attachments::MessageAuthor,
    pub forward_info: Option<super::attachments::ForwardInfo>,
    pub reply_to_id: Option<i32>,
    pub quoted_text: Option<String>,
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

In the `debug_msg` test helper, add:

```rust
            quoted_text: None,
```

immediately after `reply_to_id: None,`.

- [ ] **Step 7: Extract the worker conversion helper**

In `crates/bot/src/telegram/worker.rs`, add this private function near `collect_batch` and `batch_is_addressed`:

```rust
fn build_input_message_from_debounce(
    msg: &DebounceMsg,
    resolved: Vec<super::attachments::ResolvedAttachment>,
    voice_markers: &[String],
    reply_to_body: Option<super::attachments::ReplyToBody>,
) -> super::attachments::InputMessage {
    super::attachments::InputMessage {
        message_id: msg.message_id,
        text: crate::stt::combine_markers_with_text(voice_markers, msg.text.as_deref()),
        timestamp: msg.timestamp,
        attachments: resolved,
        author: msg.author.clone(),
        forward_info: msg.forward_info.clone(),
        reply_to_id: msg.reply_to_id,
        quoted_text: msg.quoted_text.clone(),
        chat: msg.chat.clone(),
        reply_to_body,
    }
}
```

Then replace the inline `input_messages.push(super::attachments::InputMessage { ... });` block inside the worker loop with:

```rust
                input_messages.push(build_input_message_from_debounce(
                    &msg,
                    resolved,
                    &voice_markers,
                    reply_to_body,
                ));
```

- [ ] **Step 8: Run focused handler and worker tests**

Run:

```sh
devenv shell -- cargo test -p right-bot telegram::handler::tests::telegram_quote_text_extracts_partial_reply_quote
devenv shell -- cargo test -p right-bot telegram::worker::tests::build_input_message_passes_quoted_text
```

Expected: both PASS.

- [ ] **Step 9: Run broader Telegram tests affected by struct changes**

Run:

```sh
devenv shell -- cargo test -p right-bot telegram::handler
devenv shell -- cargo test -p right-bot telegram::worker
devenv shell -- cargo test -p right-bot telegram::attachments
```

Expected: PASS. If compile errors report missing `quoted_text` on a `DebounceMsg` or `InputMessage` literal, add `quoted_text: None` in that literal and rerun this same command once.

- [ ] **Step 10: Commit Task 2**

Run:

```sh
git add crates/bot/src/telegram/handler.rs crates/bot/src/telegram/worker.rs
git commit -m "feat(bot): capture telegram partial reply quotes"
```

Expected: commit succeeds.

## Task 3: Architecture Documentation

**Files:**
- Modify: `docs/architecture/sessions.md`
- Inspect: `PROMPT_SYSTEM.md`

- [ ] **Step 1: Re-read the architecture doc and prompt docs**

Run:

```sh
devenv shell -- sed -n '1,140p' docs/architecture/sessions.md
devenv shell -- rg -n "quoted_text|reply_to|messages:|InputMessage|Telegram" PROMPT_SYSTEM.md
```

Expected:
- `sessions.md` describes Telegram ingress and transcript archive but not `quoted_text`.
- `PROMPT_SYSTEM.md` has no inbound Telegram YAML field list. If it does list inbound fields, update it in the next step with the same `quoted_text` semantics.

- [ ] **Step 2: Update `docs/architecture/sessions.md`**

In `docs/architecture/sessions.md`, after the Telegram transcript archiving bullet list and before `Archived transcript search results are conversation content...`, add:

```markdown
Telegram user turns sent to Claude are formatted as YAML with one `messages:`
entry per debounced Telegram message. Reply metadata is split by meaning:
`reply_to_id` identifies the Telegram message being replied to; `reply_to:`
contains the full available non-bot reply target body and attachments; and
`quoted_text` contains only Telegram's partial reply quote text when the user
selected a fragment. Replies to the bot's own messages keep omitting
`reply_to:` because the bot response is already in Claude session history, but
they still include `quoted_text` when Telegram supplies one.
```

If Step 1 found inbound YAML documentation in `PROMPT_SYSTEM.md`, add the same rule there. Otherwise leave `PROMPT_SYSTEM.md` untouched.

- [ ] **Step 3: Check docs diff**

Run:

```sh
git diff -- docs/architecture/sessions.md PROMPT_SYSTEM.md
```

Expected: diff only documents `quoted_text` prompt shape. No unrelated wording churn.

- [ ] **Step 4: Commit Task 3**

Run:

```sh
git add docs/architecture/sessions.md PROMPT_SYSTEM.md
git commit -m "docs(bot): document telegram partial reply quotes"
```

Expected: commit succeeds. If `PROMPT_SYSTEM.md` was untouched, `git add` leaves it unchanged and the commit includes only `docs/architecture/sessions.md`.

## Task 4: Final Verification

**Files:**
- Verify all files changed by Tasks 1-3.

- [ ] **Step 1: Review changed files**

Run:

```sh
git status --short
git diff --stat HEAD~3..HEAD
git diff HEAD~3..HEAD -- crates/bot/src/telegram/attachments.rs crates/bot/src/telegram/handler.rs crates/bot/src/telegram/worker.rs docs/architecture/sessions.md PROMPT_SYSTEM.md
```

Expected:
- Worktree is clean.
- Diff is limited to `quoted_text` plumbing, tests, and docs.
- No deletion of unrelated imports, variables, functions, or comments.

- [ ] **Step 2: Run targeted package tests**

Run:

```sh
devenv shell -- cargo test -p right-bot telegram::attachments
devenv shell -- cargo test -p right-bot telegram::handler
devenv shell -- cargo test -p right-bot telegram::worker
```

Expected: PASS.

- [ ] **Step 3: Run mandatory full workspace tests**

Run:

```sh
devenv shell -- cargo test --workspace
```

Expected: PASS. This is mandatory before claiming implementation complete.

- [ ] **Step 4: Final commit if verification required fixes**

If Step 2 or Step 3 required fixes after Task 3, commit them:

```sh
git add crates/bot/src/telegram/attachments.rs crates/bot/src/telegram/handler.rs crates/bot/src/telegram/worker.rs docs/architecture/sessions.md PROMPT_SYSTEM.md
git commit -m "fix(bot): verify telegram partial reply quotes"
```

Expected: commit succeeds only when verification caused additional changes. If no files changed, do not create an empty commit.
