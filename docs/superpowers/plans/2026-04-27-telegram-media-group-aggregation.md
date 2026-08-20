# Telegram Media Group Aggregation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** When a user sends a Telegram media group ("album") addressed to the bot, the agent must receive **all** siblings in the same logical CC turn. Privacy semantics for ordinary group chatter remain unchanged.

**Architecture:** Three small, layered changes. (1) `RoutingDecision.address` becomes `Option<AddressKind>` and the routing filter passes media-group siblings even without their own bot mention, but only after the existing trust gate. (2) `DebounceMsg` learns about `media_group_id` and the optional address kind. (3) The worker debounce loop becomes adaptive: while the current batch contains any media-group sibling, it switches from the fixed 500 ms window to "idle 1000 ms / hard cap 2500 ms"; after the batch closes, an "addressed?" gate drops group batches whose every sibling came in unaddressed.

**Tech Stack:** Rust (edition 2024), tokio (workspace `full` features, `tokio::time::pause()` for clock-controlled tests), teloxide, serde_json (test fixtures only).

---

### Task 1: Make `RoutingDecision.address` optional

**Files:**
- Modify: `crates/bot/src/telegram/filter.rs`
- Modify: `crates/bot/src/telegram/handler.rs:301`
- Modify: `crates/bot/src/telegram/worker.rs:60`

This is a pure refactor: today `RoutingDecision.address` is always `Some` because the filter only emits a decision for addressed messages. Making it `Option<_>` is a no-op so far, but it sets up the next task.

- [ ] **Step 1: Change the field type**

In `crates/bot/src/telegram/filter.rs`, change the struct definition:

```rust
#[derive(Debug, Clone)]
pub struct RoutingDecision {
    pub address: Option<AddressKind>,
    /// True iff the sender is in the global trusted-users list.
    pub sender_trusted: bool,
    /// Set to `true` for group messages when the group is opened. `false` for DM.
    pub group_open: bool,
}
```

And both call sites in the same file (lines ~41 and ~52) — wrap the `address` value in `Some(...)`:

```rust
            Some(AddressKind::DirectMessage) => {
                if !sender_trusted {
                    return None;
                }
                Some(RoutingDecision {
                    address: Some(AddressKind::DirectMessage),
                    sender_trusted: true,
                    group_open: false,
                })
            }
            Some(addr) => {
                debug_assert!(!matches!(msg.chat.kind, ChatKind::Private(_)));
                if !sender_trusted && !group_open {
                    return None;
                }
                Some(RoutingDecision {
                    address: Some(addr),
                    sender_trusted,
                    group_open,
                })
            }
```

The existing test at `crates/bot/src/telegram/filter.rs::routing_decision_constructs` must change too:

```rust
    #[test]
    fn routing_decision_constructs() {
        let d = RoutingDecision {
            address: Some(AddressKind::DirectMessage),
            sender_trusted: true,
            group_open: false,
        };
        assert!(d.sender_trusted);
        assert!(!d.group_open);
    }
```

- [ ] **Step 2: Update `DebounceMsg.address` to match**

In `crates/bot/src/telegram/worker.rs:60`, change:

```rust
    pub address: Option<super::mention::AddressKind>,
```

In `crates/bot/src/telegram/handler.rs:301`, the construction was `address: decision.address.clone()`. Now `decision.address` is already `Option<AddressKind>`, so this line is unchanged.

- [ ] **Step 3: Compile**

Run:
```
cargo build -p right-bot
```
Expected: clean build.

- [ ] **Step 4: Run existing tests**

Run:
```
cargo test -p right-bot --lib telegram::filter
cargo test -p right-bot --lib telegram::mention
```
Expected: all tests pass.

- [ ] **Step 5: Commit**

```
git add crates/bot/src/telegram/filter.rs crates/bot/src/telegram/worker.rs crates/bot/src/telegram/handler.rs
git commit -m "refactor(bot/filter): RoutingDecision.address becomes Option<AddressKind>"
```

---

### Task 2: Filter accepts media-group siblings without per-message mention

**Files:**
- Modify: `crates/bot/src/telegram/filter.rs`

Privacy invariant preserved: untrusted senders in non-open groups still drop. Only difference: in trusted/open groups, a message carrying `media_group_id` is admitted regardless of mention; its `address` may be `None`.

- [ ] **Step 1: Add a failing test for media-group-sibling-without-mention**

Append to `crates/bot/src/telegram/filter.rs::tests`:

```rust
    use chrono::Utc;
    use right_agent::agent::allowlist::{AllowedGroup, AllowedUser, AllowlistFile, AllowlistState};
    use std::sync::Arc;

    fn allowlist_with(users: Vec<i64>, groups: Vec<i64>) -> AllowlistHandle {
        let now = Utc::now();
        let users = users
            .into_iter()
            .map(|id| AllowedUser {
                id,
                label: None,
                added_by: None,
                added_at: now,
            })
            .collect();
        let groups = groups
            .into_iter()
            .map(|id| AllowedGroup {
                id,
                label: None,
                opened_by: None,
                opened_at: now,
            })
            .collect();
        let file = AllowlistFile {
            version: right_agent::agent::allowlist::CURRENT_VERSION,
            users,
            groups,
        };
        AllowlistHandle(Arc::new(std::sync::RwLock::new(
            AllowlistState::from_file(file),
        )))
    }

    fn group_msg_with_media_group(
        chat_id: i64,
        sender_id: i64,
        media_group_id: Option<&str>,
        caption_with_mention: bool,
        bot_username: &str,
    ) -> teloxide::types::Message {
        let mut payload = serde_json::json!({
            "message_id": 1,
            "date": 0,
            "chat": {"id": chat_id, "type": "supergroup", "title": "g"},
            "from": {"id": sender_id, "is_bot": false, "first_name": "U"},
            "photo": [{
                "file_id": "AgAD",
                "file_unique_id": "u",
                "width": 1, "height": 1
            }],
        });
        if let Some(mgid) = media_group_id {
            payload["media_group_id"] = serde_json::Value::String(mgid.to_string());
        }
        if caption_with_mention {
            let cap = format!("@{bot_username} hi");
            payload["caption"] = serde_json::Value::String(cap.clone());
            payload["caption_entities"] = serde_json::json!([{
                "type": "mention",
                "offset": 0,
                "length": bot_username.len() as i64 + 1
            }]);
        }
        serde_json::from_value(payload).unwrap()
    }

    #[test]
    fn media_group_sibling_without_mention_passes_for_open_group() {
        let identity = BotIdentity {
            username: "rightaww_bot".into(),
            user_id: 999,
        };
        let chat_id = -1001;
        let sender_id = 42;
        let allowlist = allowlist_with(vec![], vec![chat_id]);

        let msg = group_msg_with_media_group(
            chat_id,
            sender_id,
            Some("alb"),
            /*caption_with_mention=*/ false,
            &identity.username,
        );

        let f = make_routing_filter(allowlist, identity);
        let d = f(msg).expect("media-group sibling should pass in open group");
        assert!(d.address.is_none());
        assert!(d.group_open);
    }
```

- [ ] **Step 2: Run test, expect failure**

Run:
```
cargo test -p right-bot --lib telegram::filter::tests::media_group_sibling_without_mention_passes_for_open_group
```
Expected: FAIL with `assertion 'media-group sibling should pass in open group'` (current filter returns `None` for unaddressed group messages).

- [ ] **Step 3: Update the filter to admit media-group siblings**

Replace the body of `make_routing_filter` in `crates/bot/src/telegram/filter.rs`:

```rust
pub fn make_routing_filter(
    allowlist: AllowlistHandle,
    identity: BotIdentity,
) -> impl Fn(Message) -> Option<RoutingDecision> + Send + Sync + Clone + 'static {
    move |msg: Message| {
        // No `from` means channel post or anonymous — ignore.
        let sender = msg.from.as_ref()?;
        let sender_id = sender.id.0 as i64;
        let chat_id = msg.chat.id.0;

        let state = allowlist.0.read().expect("allowlist lock poisoned");
        let sender_trusted = state.is_user_trusted(sender_id);
        let group_open = state.is_group_open(chat_id);
        drop(state);

        let addressed = is_bot_addressed(&msg, &identity);

        match &msg.chat.kind {
            ChatKind::Private(_) => {
                if !sender_trusted {
                    return None;
                }
                Some(RoutingDecision {
                    address: Some(AddressKind::DirectMessage),
                    sender_trusted: true,
                    group_open: false,
                })
            }
            _ => {
                if !sender_trusted && !group_open {
                    return None;
                }
                // Non-album group messages still require an explicit address.
                // Album siblings are admitted unaddressed; the worker aggregates
                // them and applies a final addressed-batch gate before invoking CC.
                if addressed.is_none() && msg.media_group_id().is_none() {
                    return None;
                }
                Some(RoutingDecision {
                    address: addressed,
                    sender_trusted,
                    group_open,
                })
            }
        }
    }
}
```

- [ ] **Step 4: Run the new test**

Run:
```
cargo test -p right-bot --lib telegram::filter::tests::media_group_sibling_without_mention_passes_for_open_group
```
Expected: PASS.

- [ ] **Step 5: Add privacy-regression and trusted-DM tests**

Append to the same `tests` module:

```rust
    #[test]
    fn ordinary_group_message_without_mention_still_dropped() {
        let identity = BotIdentity {
            username: "rightaww_bot".into(),
            user_id: 999,
        };
        let chat_id = -1001;
        let sender_id = 42;
        let allowlist = allowlist_with(vec![], vec![chat_id]);

        // No media_group_id, no caption mention — a plain text post.
        let msg: teloxide::types::Message = serde_json::from_value(serde_json::json!({
            "message_id": 1,
            "date": 0,
            "chat": {"id": chat_id, "type": "supergroup", "title": "g"},
            "from": {"id": sender_id, "is_bot": false, "first_name": "U"},
            "text": "hello there"
        }))
        .unwrap();

        let f = make_routing_filter(allowlist, identity);
        assert!(f(msg).is_none());
    }

    #[test]
    fn media_group_sibling_without_mention_dropped_for_untrusted_sender() {
        let identity = BotIdentity {
            username: "rightaww_bot".into(),
            user_id: 999,
        };
        let chat_id = -1001;
        let sender_id = 42;
        // No trusted users, no open groups → sender is neither trusted nor in an open group.
        let allowlist = allowlist_with(vec![], vec![]);

        let msg = group_msg_with_media_group(
            chat_id,
            sender_id,
            Some("alb"),
            /*caption_with_mention=*/ false,
            &identity.username,
        );

        let f = make_routing_filter(allowlist, identity);
        assert!(f(msg).is_none());
    }

    #[test]
    fn media_group_sibling_with_mention_passes_with_some_address() {
        let identity = BotIdentity {
            username: "rightaww_bot".into(),
            user_id: 999,
        };
        let chat_id = -1001;
        let sender_id = 42;
        let allowlist = allowlist_with(vec![], vec![chat_id]);

        let msg = group_msg_with_media_group(
            chat_id,
            sender_id,
            Some("alb"),
            /*caption_with_mention=*/ true,
            &identity.username,
        );

        let f = make_routing_filter(allowlist, identity);
        let d = f(msg).expect("captioned sibling must pass");
        assert!(matches!(
            d.address,
            Some(AddressKind::GroupMentionText)
        ));
    }
```

- [ ] **Step 6: Run filter tests**

```
cargo test -p right-bot --lib telegram::filter
```
Expected: all four new tests + existing test pass.

- [ ] **Step 7: Commit**

```
git add crates/bot/src/telegram/filter.rs
git commit -m "feat(bot/filter): admit Telegram media-group siblings without per-message mention

The Telegram Bot API places the caption (and therefore any @bot mention
entity) on exactly one sibling of a media group; the others arrive
caption-less. The previous filter dropped them as 'group non-mention',
so the agent received only the captioned file. Pass siblings through in
trusted/open groups; the worker aggregates and gates on addressedness."
```

---

### Task 3: Propagate `media_group_id` into `DebounceMsg`

**Files:**
- Modify: `crates/bot/src/telegram/worker.rs:50-64` (struct)
- Modify: `crates/bot/src/telegram/handler.rs:293-305` (construction)

- [ ] **Step 1: Add the field**

In `crates/bot/src/telegram/worker.rs`, extend the struct (line 50-64):

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
    /// `Some(id)` when this message is part of a Telegram album (media group);
    /// shared by all siblings of the album.
    pub media_group_id: Option<String>,
}
```

- [ ] **Step 2: Populate it in `handle_message`**

In `crates/bot/src/telegram/handler.rs:293`, extend the `DebounceMsg` constructor:

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

- [ ] **Step 3: Compile**

Run:
```
cargo build -p right-bot
```
Expected: clean build.

- [ ] **Step 4: Sanity-run all bot lib tests**

Run:
```
cargo test -p right-bot --lib
```
Expected: all tests pass.

- [ ] **Step 5: Commit**

```
git add crates/bot/src/telegram/worker.rs crates/bot/src/telegram/handler.rs
git commit -m "feat(bot/worker): carry media_group_id on DebounceMsg"
```

---

### Task 4: Extract the debounce loop into a testable function

**Files:**
- Modify: `crates/bot/src/telegram/worker.rs`

Pull the inner debounce `loop { tokio::select! { ... } }` out of `spawn_worker` so the next task can change its timing logic with `#[tokio::test(start_paused = true)]` coverage.

- [ ] **Step 1: Add the constants and the function**

In `crates/bot/src/telegram/worker.rs`, near the existing `DEBOUNCE_MS` (line 32):

```rust
/// Fixed 500ms debounce window for non-media-group batches (D-01).
const DEBOUNCE_MS: u64 = 500;

/// While the current batch contains any media-group sibling, close the window
/// after this many milliseconds of inactivity from the latest arrival.
const MEDIA_GROUP_IDLE_MS: u64 = 1000;

/// Hard cap on the total time spent collecting a batch that contains
/// media-group siblings, measured from the first arrival.
const MEDIA_GROUP_HARD_CAP_MS: u64 = 2500;
```

Then insert this new function below the existing helpers (e.g. just before `pub fn spawn_worker`):

```rust
/// Collect a single debounce batch starting from `first`, draining additional
/// messages from `rx` according to the windowing rules:
///
/// - If no message in the batch carries a `media_group_id`, the window is a
///   fixed `DEBOUNCE_MS` measured from the first arrival.
/// - Once any message in the batch carries a `media_group_id`, the window
///   becomes "idle `MEDIA_GROUP_IDLE_MS` from the latest arrival, capped at
///   `MEDIA_GROUP_HARD_CAP_MS` from the first arrival".
///
/// Returns when the window closes or `rx` is closed (whichever happens first).
async fn collect_batch(
    first: DebounceMsg,
    rx: &mut mpsc::Receiver<DebounceMsg>,
) -> Vec<DebounceMsg> {
    use tokio::time::{Instant, sleep_until};

    let first_arrival = Instant::now();
    let mut last_arrival = first_arrival;
    let mut media_group_seen = first.media_group_id.is_some();
    let mut batch = vec![first];

    loop {
        let deadline = if media_group_seen {
            std::cmp::min(
                last_arrival + Duration::from_millis(MEDIA_GROUP_IDLE_MS),
                first_arrival + Duration::from_millis(MEDIA_GROUP_HARD_CAP_MS),
            )
        } else {
            first_arrival + Duration::from_millis(DEBOUNCE_MS)
        };

        tokio::select! {
            biased;
            msg = rx.recv() => {
                match msg {
                    Some(m) => {
                        if m.media_group_id.is_some() {
                            media_group_seen = true;
                        }
                        last_arrival = Instant::now();
                        batch.push(m);
                    }
                    None => break,
                }
            }
            _ = sleep_until(deadline) => break,
        }
    }
    batch
}
```

- [ ] **Step 2: Replace the inline loop in `spawn_worker`**

Find the existing block in `crates/bot/src/telegram/worker.rs:377-391`:

```rust
            let mut batch = vec![first];

            // Collect additional messages within debounce window (D-01)
            loop {
                tokio::select! {
                    biased;
                    msg = rx.recv() => {
                        match msg {
                            Some(m) => batch.push(m),
                            None => break,
                        }
                    }
                    _ = sleep(window) => break,
                }
            }
```

Replace with:

```rust
            let batch = collect_batch(first, &mut rx).await;
```

Also remove the now-unused `let window = Duration::from_millis(DEBOUNCE_MS);` two lines above (`crates/bot/src/telegram/worker.rs:361`) — it was the only consumer.

- [ ] **Step 3: Compile**

Run:
```
cargo build -p right-bot
```
Expected: clean build.

- [ ] **Step 4: Confirm behaviour did not regress**

Run:
```
cargo test -p right-bot --lib
```
Expected: all tests pass — `collect_batch` is currently a no-op refactor for non-media-group traffic (the window math already collapses to the existing 500 ms behaviour when `media_group_seen` is false).

- [ ] **Step 5: Commit**

```
git add crates/bot/src/telegram/worker.rs
git commit -m "refactor(bot/worker): extract debounce loop into collect_batch helper"
```

---

### Task 5: Tests for the adaptive debounce window

**Files:**
- Modify: `crates/bot/src/telegram/worker.rs` (test module)

The window logic is now in `collect_batch`. Cover the four cases that matter: pure album, slow album with idle reset, hard cap, and the regression guard for non-media-group batches.

- [ ] **Step 1: Add a fixture + helper to the existing `#[cfg(test)] mod tests` block in `worker.rs` (or create one if it doesn't exist).**

If `crates/bot/src/telegram/worker.rs` has no `#[cfg(test)] mod tests` block at the bottom of the file, append one:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use tokio::sync::mpsc;
    use tokio::time::{Duration, advance};

    fn debug_msg(message_id: i32, media_group_id: Option<&str>) -> DebounceMsg {
        DebounceMsg {
            message_id,
            text: None,
            timestamp: Utc::now(),
            attachments: vec![],
            author: super::super::attachments::MessageAuthor {
                name: "u".into(),
                username: None,
                user_id: None,
            },
            forward_info: None,
            reply_to_id: None,
            address: None,
            group_open: true,
            chat: super::super::attachments::ChatContext::Group {
                id: -1001,
                title: None,
                topic_id: None,
            },
            reply_to_body: None,
            media_group_id: media_group_id.map(|s| s.to_string()),
        }
    }
}
```

If a `tests` module already exists, just add the helper near the top of it. Either way, add `use tokio::time::{Duration, advance};` at the top of the test module.

- [ ] **Step 2: Add the four window tests**

Inside the same test module:

```rust
    #[tokio::test(start_paused = true)]
    async fn fast_album_closes_after_idle_window() {
        let (tx, mut rx) = mpsc::channel::<DebounceMsg>(8);
        let first = debug_msg(1, Some("alb"));

        // Push siblings 2 and 3 with simulated 200 ms gaps before invoking collect_batch.
        let task = tokio::spawn(async move { collect_batch(first, &mut rx).await });
        advance(Duration::from_millis(200)).await;
        tx.send(debug_msg(2, Some("alb"))).await.unwrap();
        advance(Duration::from_millis(200)).await;
        tx.send(debug_msg(3, Some("alb"))).await.unwrap();

        // Now no more arrivals — idle 1000 ms from msg 3 should close the window.
        advance(Duration::from_millis(1100)).await;

        let batch = task.await.unwrap();
        assert_eq!(batch.len(), 3);
        assert_eq!(
            batch.iter().map(|m| m.message_id).collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn slow_album_idle_reset_keeps_batch_open() {
        let (tx, mut rx) = mpsc::channel::<DebounceMsg>(8);
        let first = debug_msg(1, Some("alb"));

        let task = tokio::spawn(async move { collect_batch(first, &mut rx).await });

        // 600 ms — past the 500 ms non-media window, but in media-group mode the
        // idle window is 1000 ms from last arrival, so this still falls in.
        advance(Duration::from_millis(600)).await;
        tx.send(debug_msg(2, Some("alb"))).await.unwrap();
        advance(Duration::from_millis(900)).await;
        tx.send(debug_msg(3, Some("alb"))).await.unwrap();

        // Idle 1000 ms from msg 3.
        advance(Duration::from_millis(1100)).await;

        let batch = task.await.unwrap();
        assert_eq!(batch.len(), 3);
    }

    #[tokio::test(start_paused = true)]
    async fn album_hits_hard_cap_at_2500ms() {
        let (tx, mut rx) = mpsc::channel::<DebounceMsg>(8);
        let first = debug_msg(1, Some("alb"));

        let task = tokio::spawn(async move { collect_batch(first, &mut rx).await });

        // Drip-feed siblings every 700 ms. Idle alone never closes; hard cap at
        // 2500 ms from first arrival must terminate the batch.
        advance(Duration::from_millis(700)).await;
        tx.send(debug_msg(2, Some("alb"))).await.unwrap();
        advance(Duration::from_millis(700)).await;
        tx.send(debug_msg(3, Some("alb"))).await.unwrap();
        advance(Duration::from_millis(700)).await;
        tx.send(debug_msg(4, Some("alb"))).await.unwrap();
        // At t=2100; cap fires at t=2500. The 500 ms advance below crosses the
        // cap, so the task closes the batch and drops the receiver before the
        // next send. Use .ok() since the send is expected to fail.
        advance(Duration::from_millis(500)).await;
        let _ = tx.send(debug_msg(5, Some("alb"))).await;

        let batch = task.await.unwrap();
        assert_eq!(
            batch.iter().map(|m| m.message_id).collect::<Vec<_>>(),
            vec![1, 2, 3, 4],
            "hard cap must close at 2500 ms, leaving msg 5 outside the batch"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn non_album_keeps_500ms_window() {
        let (tx, mut rx) = mpsc::channel::<DebounceMsg>(8);
        let first = debug_msg(1, None);

        let task = tokio::spawn(async move { collect_batch(first, &mut rx).await });

        // 600 ms — past the 500 ms window. Task closes, receiver is dropped,
        // so the follow-up send returns Err; .ok() swallows it.
        advance(Duration::from_millis(600)).await;
        let _ = tx.send(debug_msg(2, None)).await;

        let batch = task.await.unwrap();
        assert_eq!(batch.len(), 1, "non-album message must use 500 ms window");
        assert_eq!(batch[0].message_id, 1);
    }

    #[tokio::test(start_paused = true)]
    async fn text_widens_window_when_album_joins() {
        let (tx, mut rx) = mpsc::channel::<DebounceMsg>(8);
        let first = debug_msg(1, None); // plain text

        let task = tokio::spawn(async move { collect_batch(first, &mut rx).await });

        // Album sibling joins at 200 ms — flips the batch into media-group mode.
        advance(Duration::from_millis(200)).await;
        tx.send(debug_msg(2, Some("alb"))).await.unwrap();
        // Another sibling 700 ms later — still inside the new 1000 ms idle window.
        advance(Duration::from_millis(700)).await;
        tx.send(debug_msg(3, Some("alb"))).await.unwrap();

        advance(Duration::from_millis(1100)).await;

        let batch = task.await.unwrap();
        assert_eq!(batch.len(), 3);
    }
```

- [ ] **Step 3: Run worker tests**

```
cargo test -p right-bot --lib telegram::worker
```
Expected: all five new tests pass.

- [ ] **Step 4: Commit**

```
git add crates/bot/src/telegram/worker.rs
git commit -m "test(bot/worker): adaptive debounce window for media-group batches"
```

---

### Task 6: Drop unaddressed group batches before invoking CC

**Files:**
- Modify: `crates/bot/src/telegram/worker.rs`

Now that media-group siblings can flow through the filter unaddressed, an album from a trusted user with no `@mention` anywhere can reach the worker. The worker must drop such batches without invoking CC.

- [ ] **Step 1: Add a failing test**

In the same `tests` module from Task 5, append a tiny pure helper test (the gate is one boolean predicate — keep the cost low; the wider integration is exercised in Task 7):

```rust
    /// The post-debounce gate: in groups, drop the batch when no message in
    /// it was actually addressed to the bot. DM batches always have
    /// `address: Some(DirectMessage)` and pass.
    fn batch_is_addressed(batch: &[DebounceMsg]) -> bool {
        batch.iter().any(|m| m.address.is_some())
    }

    #[test]
    fn batch_is_addressed_drops_all_none_group_batch() {
        let batch = vec![debug_msg(1, Some("alb")), debug_msg(2, Some("alb"))];
        assert!(!batch_is_addressed(&batch));
    }

    #[test]
    fn batch_is_addressed_passes_when_one_sibling_addressed() {
        let mut a = debug_msg(1, Some("alb"));
        a.address = Some(super::super::mention::AddressKind::GroupMentionText);
        let batch = vec![a, debug_msg(2, Some("alb"))];
        assert!(batch_is_addressed(&batch));
    }
```

- [ ] **Step 2: Run tests**

```
cargo test -p right-bot --lib telegram::worker::tests::batch_is_addressed
```
Expected: PASS (function is added in the next step — first ensure it compiles in the test). If `batch_is_addressed` is undefined when running this step, the test fails to compile — that's the "failing test" state for TDD; proceed to Step 3.

- [ ] **Step 3: Promote the helper to module scope and use it from `spawn_worker`**

In `crates/bot/src/telegram/worker.rs`, lift `batch_is_addressed` out of the test module to a `pub(crate)` (or private — same crate) function near `collect_batch`:

```rust
/// Post-debounce addressedness gate. In groups, the worker must drop the
/// batch if no message in it was addressed to the bot — this is the case
/// for media-group siblings whose only "addressed" sibling never made it
/// into the batch (e.g. arrived after the hard cap).
fn batch_is_addressed(batch: &[DebounceMsg]) -> bool {
    batch.iter().any(|m| m.address.is_some())
}
```

Delete the duplicate definition from inside `mod tests`. The two `#[test]` blocks remain — they now exercise the module-level function.

In `crates/bot/src/telegram/worker.rs`, immediately after the line that computes `is_group` (currently lines 395-398) and before the attachment download loop:

```rust
            let is_group = matches!(
                batch.first().map(|m| &m.chat),
                Some(super::attachments::ChatContext::Group { .. })
            );
            if is_group && !batch_is_addressed(&batch) {
                tracing::debug!(
                    ?key,
                    batch_size = batch.len(),
                    "media-group batch had no addressed sibling — dropping without CC"
                );
                continue;
            }
            if is_group && ctx.show_thinking {
                tracing::debug!(?key, "show_thinking suppressed in group");
            }
```

- [ ] **Step 4: Run worker tests**

```
cargo test -p right-bot --lib telegram::worker
```
Expected: all tests pass.

- [ ] **Step 5: Build the whole workspace, run all bot tests**

```
cargo build --workspace
cargo test -p right-bot --lib
```
Expected: clean build, all tests green.

- [ ] **Step 6: Commit**

```
git add crates/bot/src/telegram/worker.rs
git commit -m "feat(bot/worker): drop unaddressed group batches before invoking CC

Media-group siblings now flow through the filter without their own bot
mention. The worker debounce groups them; if no sibling was actually
addressed (e.g. the captioned message arrived after the hard cap, or
the user posted an album in an open group with no mention at all), drop
the batch without spawning a CC subprocess."
```

---

### Task 7: End-to-end regression test for the original Vonder bug

**Files:**
- Modify: `crates/bot/src/telegram/filter.rs` (test module)

The unit tests cover filter and worker independently. Add one filter-level test that mirrors the actual bug report — three Telegram messages sharing a `media_group_id`, one with caption containing `@mention`, two caption-less — and asserts that the filter emits a `Some(_)` for **all three** when sender is in an open group.

- [ ] **Step 1: Add the test**

Append to `crates/bot/src/telegram/filter.rs::tests`:

```rust
    #[test]
    fn vonder_repro_three_album_siblings_all_routed() {
        // Reproduces the bug from ~/.right/logs/him.log.2026-04-27 lines 137-152:
        // three messages sharing media_group_id, only the third carries the @mention.
        let identity = BotIdentity {
            username: "rightaww_bot".into(),
            user_id: 999,
        };
        let chat_id = -4996137249;
        let sender_id = 42;
        let allowlist = allowlist_with(vec![], vec![chat_id]);

        let f = make_routing_filter(allowlist, identity.clone());

        let s1 = group_msg_with_media_group(
            chat_id,
            sender_id,
            Some("vonder-album"),
            /*caption_with_mention=*/ false,
            &identity.username,
        );
        let s2 = group_msg_with_media_group(
            chat_id,
            sender_id,
            Some("vonder-album"),
            /*caption_with_mention=*/ false,
            &identity.username,
        );
        let s3 = group_msg_with_media_group(
            chat_id,
            sender_id,
            Some("vonder-album"),
            /*caption_with_mention=*/ true,
            &identity.username,
        );

        assert!(f(s1).is_some(), "sibling 1 must reach handle_message");
        assert!(f(s2).is_some(), "sibling 2 must reach handle_message");
        let d3 = f(s3).expect("captioned sibling must reach handle_message");
        assert!(d3.address.is_some());
    }
```

`BotIdentity` is `Clone` (`#[derive(Debug, Clone)]` in `mention.rs`), so `identity.clone()` works. `make_routing_filter` consumes the identity by value, so the clone is required for the second use of `identity.username` below.

- [ ] **Step 2: Run the regression test**

```
cargo test -p right-bot --lib telegram::filter::tests::vonder_repro_three_album_siblings_all_routed
```
Expected: PASS.

- [ ] **Step 3: Commit**

```
git add crates/bot/src/telegram/filter.rs
git commit -m "test(bot/filter): regression for lost media-group siblings"
```

---

### Task 8: Workspace build, clippy, full bot test pass

- [ ] **Step 1: Workspace build**

```
cargo build --workspace
```
Expected: clean build.

- [ ] **Step 2: Clippy**

```
cargo clippy -p right-bot --all-targets -- -D warnings
```
Expected: no warnings.

- [ ] **Step 3: All bot tests**

```
cargo test -p right-bot --lib
```
Expected: all green, including the five new debounce tests, four new filter tests, two `batch_is_addressed` tests, and one Vonder regression.

- [ ] **Step 4: Commit anything that needed minor lint fixes**

If clippy required adjustments:

```
git add -p
git commit -m "chore(bot): clippy fixups for media-group changes"
```

(Skip if no fixups needed.)

---

## Self-Review

**Spec coverage** — every requirement in `docs/superpowers/specs/2026-04-27-telegram-media-group-aggregation-design.md`:

- "Routing filter changes" → Task 1 (Option type) + Task 2 (sibling acceptance + tests).
- "`DebounceMsg` shape" → Task 1 (Option<AddressKind>) + Task 3 (media_group_id).
- "Adaptive debounce window" → Task 4 (extract) + Task 5 (logic + tests).
- "Post-debounce address gate" → Task 6.
- "Testing — filter unit tests" → Task 2 + Task 7.
- "Testing — worker debounce, all five scenarios" → Task 5.
- "Testing — post-debounce gate" → Task 6.
- "Testing — Vonder regression" → Task 7.
- "Migration: none" → no task needed; the change is a pure code change picked up on bot restart.
- "Risks: 2500 ms cap means slow albums split" → covered by Task 5's `album_hits_hard_cap_at_2500ms` test, which documents that behaviour.

No spec gaps.

**Placeholder scan:** No "TBD"/"TODO"/"add tests for the above". Each step ships concrete code or a runnable command with expected output.

**Type consistency:** `Option<AddressKind>` introduced in Task 1 is the type used in Task 2 (filter), Task 3 (DebounceMsg), Task 6 (gate). `collect_batch` signature in Task 4 matches its usage in Task 5 tests. `batch_is_addressed` defined once at module scope in Task 6, called from `spawn_worker` and from two test functions.
