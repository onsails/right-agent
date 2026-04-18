# Telegram Media Groups Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let an agent group outbound Telegram attachments into a single album by tagging them with a shared `media_group_id` in the structured reply. The bot honours the grouping when it matches Telegram's homogeneity rules and degrades with a warning otherwise.

**Architecture:** Pure helpers (`GroupKind` classifier, `classify_media_group`, `merge_group_captions`, `partition_sends`) compute a list of `Send` operations from a `[OutboundAttachment]` slice. `send_attachments` then executes each `Send` by dispatching either to the existing per-item `send_photo`/`send_document`/… path or to a new `send_media_group` path. All helpers are unit-tested; `send_attachments` stays integration-level.

**Tech Stack:** Rust (edition 2024), teloxide 0.17 (`InputMedia`, `send_media_group`, `SendMediaGroupSetters::message_thread_id`), serde/serde_json.

---

## File Structure

- `crates/rightclaw/src/codegen/agent_def.rs` — add one nullable `media_group_id` property to the three schema constants (`REPLY_SCHEMA_JSON`, `BOOTSTRAP_SCHEMA_JSON`, `CRON_SCHEMA_JSON`).
- `crates/rightclaw/src/codegen/agent_def_tests.rs` — tests that each schema's `attachments` item declares `media_group_id` as nullable; snapshot check that `OPERATING_INSTRUCTIONS` contains `Media Groups`.
- `crates/rightclaw/templates/right/prompt/OPERATING_INSTRUCTIONS.md` — new `### Media Groups (Albums)` subsection under `## Sending Attachments`.
- `crates/bot/src/telegram/attachments.rs` — add `media_group_id` to `OutboundAttachment`, add `GroupKind`, `classify_media_group`, `merge_group_captions`, `Send` enum, `partition_sends`, rewrite `send_attachments` to consume those helpers, add unit tests.
- `PROMPT_SYSTEM.md` — matching paragraph documenting the reply-schema change (project convention requires it).

All changes are additive. The new field is optional, so existing agent replies keep the current behaviour.

---

## Task 1: Schema field + struct field (TDD)

**Files:**
- Modify: `crates/rightclaw/src/codegen/agent_def.rs:21` (REPLY_SCHEMA_JSON), `:27` (BOOTSTRAP_SCHEMA_JSON), `:35` (CRON_SCHEMA_JSON)
- Modify: `crates/rightclaw/src/codegen/mod.rs:16` (ensure `CRON_SCHEMA_JSON` is reachable from tests — it already is, just verify)
- Modify: `crates/rightclaw/src/codegen/agent_def_tests.rs` (add 4 new tests, import `CRON_SCHEMA_JSON`)
- Modify: `crates/bot/src/telegram/attachments.rs:56-62` (OutboundAttachment struct) and its existing unit tests

- [ ] **Step 1: Add failing schema tests**

Append to `crates/rightclaw/src/codegen/agent_def_tests.rs` (and update the top-level `use` line to pull in `CRON_SCHEMA_JSON`):

```rust
use crate::codegen::{generate_system_prompt, BOOTSTRAP_SCHEMA_JSON, CRON_SCHEMA_JSON, REPLY_SCHEMA_JSON};

fn attachments_item_schema(schema_json: &str, path: &[&str]) -> serde_json::Value {
    let mut node: serde_json::Value = serde_json::from_str(schema_json).unwrap();
    for key in path {
        node = node.get(*key).unwrap_or_else(|| panic!("missing key {key}")).clone();
    }
    node
}

fn assert_has_nullable_media_group_id(items: &serde_json::Value) {
    let props = items.get("properties").expect("items.properties");
    let field = props.get("media_group_id").expect("media_group_id property missing");
    let ty = field.get("type").expect("media_group_id.type missing");
    let arr = ty.as_array().expect("media_group_id.type must be an array for nullable");
    let kinds: Vec<&str> = arr.iter().map(|v| v.as_str().unwrap()).collect();
    assert!(kinds.contains(&"string"), "must allow string, got {kinds:?}");
    assert!(kinds.contains(&"null"), "must allow null, got {kinds:?}");
}

#[test]
fn reply_schema_attachments_item_has_media_group_id() {
    let items = attachments_item_schema(
        REPLY_SCHEMA_JSON,
        &["properties", "attachments", "items"],
    );
    assert_has_nullable_media_group_id(&items);
}

#[test]
fn bootstrap_schema_attachments_item_has_media_group_id() {
    let items = attachments_item_schema(
        BOOTSTRAP_SCHEMA_JSON,
        &["properties", "attachments", "items"],
    );
    assert_has_nullable_media_group_id(&items);
}

#[test]
fn cron_schema_attachments_item_has_media_group_id() {
    let items = attachments_item_schema(
        CRON_SCHEMA_JSON,
        &["properties", "notify", "properties", "attachments", "items"],
    );
    assert_has_nullable_media_group_id(&items);
}
```

- [ ] **Step 2: Run tests, verify they fail**

Run: `cargo test -p rightclaw codegen::agent_def_tests::` (from repo root).
Expected: the three new tests fail with `media_group_id property missing`.

- [ ] **Step 3: Add the field to all three schemas**

In `crates/rightclaw/src/codegen/agent_def.rs`, each of the three schema constants contains the substring:

```
"caption":{"type":["string","null"]}},"required":["type","path"]
```

Replace that exact substring with:

```
"caption":{"type":["string","null"]},"media_group_id":{"type":["string","null"]}},"required":["type","path"]
```

Use `Edit` with `replace_all: true` — the substring is identical across the three constants, so one Edit call updates all of them.

- [ ] **Step 4: Run the schema tests, verify pass**

Run: `cargo test -p rightclaw codegen::agent_def_tests::`
Expected: all tests pass (including the three new ones).

- [ ] **Step 5: Add the struct field + deserialization test**

In `crates/bot/src/telegram/attachments.rs`, find the `OutboundAttachment` struct (currently around line 55–62) and add one field:

```rust
#[derive(Debug, Clone, Deserialize, serde::Serialize)]
pub struct OutboundAttachment {
    #[serde(rename = "type")]
    pub kind: OutboundKind,
    pub path: String,
    pub filename: Option<String>,
    pub caption: Option<String>,
    #[serde(default)]
    pub media_group_id: Option<String>,
}
```

Then append two new unit tests to the existing `#[cfg(test)] mod tests` block (next to `outbound_kind_deserialize`):

```rust
#[test]
fn outbound_attachment_deserialize_without_media_group_id_defaults_none() {
    let json = r#"{"type":"photo","path":"/sandbox/outbox/a.jpg"}"#;
    let att: OutboundAttachment = serde_json::from_str(json).unwrap();
    assert!(att.media_group_id.is_none());
}

#[test]
fn outbound_attachment_deserialize_with_media_group_id() {
    let json = r#"{"type":"photo","path":"/sandbox/outbox/a.jpg","media_group_id":"shots"}"#;
    let att: OutboundAttachment = serde_json::from_str(json).unwrap();
    assert_eq!(att.media_group_id.as_deref(), Some("shots"));
}
```

- [ ] **Step 6: Run attachments tests, verify pass**

Run: `cargo test -p rightclaw-bot telegram::attachments::tests::outbound_`
Expected: all four `outbound_*` tests pass.

- [ ] **Step 7: Commit**

```bash
git add crates/rightclaw/src/codegen/agent_def.rs \
        crates/rightclaw/src/codegen/agent_def_tests.rs \
        crates/bot/src/telegram/attachments.rs
git commit -m "feat(schema): add media_group_id to outbound attachment schemas

Nullable string field on every attachments item in REPLY_SCHEMA_JSON,
BOOTSTRAP_SCHEMA_JSON, and CRON_SCHEMA_JSON. OutboundAttachment gains
a matching Option<String>. Existing replies that omit the field
behave as before."
```

---

## Task 2: GroupKind and category helper (TDD)

**Files:**
- Modify: `crates/bot/src/telegram/attachments.rs`

`GroupKind` identifies which of the three Telegram-compatible album families an attachment belongs to, or says it is ungroupable.

- [ ] **Step 1: Write the failing test**

Append inside the existing `#[cfg(test)] mod tests` block in `crates/bot/src/telegram/attachments.rs`:

```rust
#[test]
fn group_kind_from_outbound_kind_covers_all_variants() {
    use OutboundKind::*;
    assert_eq!(GroupKind::of(&Photo), Some(GroupKind::PhotoVideo));
    assert_eq!(GroupKind::of(&Video), Some(GroupKind::PhotoVideo));
    assert_eq!(GroupKind::of(&Document), Some(GroupKind::Document));
    assert_eq!(GroupKind::of(&Audio), Some(GroupKind::Audio));
    assert_eq!(GroupKind::of(&Voice), None);
    assert_eq!(GroupKind::of(&VideoNote), None);
    assert_eq!(GroupKind::of(&Sticker), None);
    assert_eq!(GroupKind::of(&Animation), None);
}
```

- [ ] **Step 2: Run test, verify it fails**

Run: `cargo test -p rightclaw-bot telegram::attachments::tests::group_kind_from_outbound_kind_covers_all_variants`
Expected: FAIL with `cannot find type GroupKind in this scope` (or similar).

- [ ] **Step 3: Implement GroupKind**

Insert immediately after the `OutboundKind` enum definition in `crates/bot/src/telegram/attachments.rs`:

```rust
/// Category of Telegram media-group album. A `None` from [`GroupKind::of`] means
/// the attachment kind (voice / video_note / sticker / animation) cannot live in
/// any media group and must be sent individually.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupKind {
    /// Photos and videos can mix in the same album.
    PhotoVideo,
    /// Documents form a documents-only album.
    Document,
    /// Audios form an audios-only album.
    Audio,
}

impl GroupKind {
    pub fn of(kind: &OutboundKind) -> Option<Self> {
        match kind {
            OutboundKind::Photo | OutboundKind::Video => Some(Self::PhotoVideo),
            OutboundKind::Document => Some(Self::Document),
            OutboundKind::Audio => Some(Self::Audio),
            OutboundKind::Voice
            | OutboundKind::VideoNote
            | OutboundKind::Sticker
            | OutboundKind::Animation => None,
        }
    }
}
```

- [ ] **Step 4: Run the test, verify pass**

Run: `cargo test -p rightclaw-bot telegram::attachments::tests::group_kind_from_outbound_kind_covers_all_variants`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/bot/src/telegram/attachments.rs
git commit -m "feat(attachments): GroupKind enum categorising album compatibility"
```

---

## Task 3: classify_media_group (TDD, table-driven)

**Files:**
- Modify: `crates/bot/src/telegram/attachments.rs`

`classify_media_group` decides what to do with a candidate group: send as-is, split into chunks of ≤10 same-kind items, or degrade to individual sends.

- [ ] **Step 1: Write the failing tests**

Append to the `#[cfg(test)] mod tests` block in `crates/bot/src/telegram/attachments.rs`:

```rust
fn att(kind: OutboundKind) -> OutboundAttachment {
    OutboundAttachment {
        kind,
        path: format!("/sandbox/outbox/{}.bin", kind_to_ext(kind)),
        filename: None,
        caption: None,
        media_group_id: Some("g".into()),
    }
}

fn kind_to_ext(k: OutboundKind) -> &'static str {
    match k {
        OutboundKind::Photo => "jpg",
        OutboundKind::Video => "mp4",
        OutboundKind::Document => "pdf",
        OutboundKind::Audio => "mp3",
        OutboundKind::Voice => "ogg",
        OutboundKind::VideoNote => "mp4",
        OutboundKind::Sticker => "webp",
        OutboundKind::Animation => "gif",
    }
}

fn atts(kinds: &[OutboundKind]) -> Vec<OutboundAttachment> {
    kinds.iter().copied().map(att).collect()
}

fn refs(v: &[OutboundAttachment]) -> Vec<&OutboundAttachment> {
    v.iter().collect()
}

#[test]
fn classify_two_photos_sends_as_group() {
    let items = atts(&[OutboundKind::Photo, OutboundKind::Photo]);
    assert_eq!(
        classify_media_group(&refs(&items)),
        GroupPlan::SendAsGroup(GroupKind::PhotoVideo),
    );
}

#[test]
fn classify_photo_and_video_mix_sends_as_group() {
    let items = atts(&[OutboundKind::Photo, OutboundKind::Video]);
    assert_eq!(
        classify_media_group(&refs(&items)),
        GroupPlan::SendAsGroup(GroupKind::PhotoVideo),
    );
}

#[test]
fn classify_two_documents_sends_as_group() {
    let items = atts(&[OutboundKind::Document, OutboundKind::Document]);
    assert_eq!(
        classify_media_group(&refs(&items)),
        GroupPlan::SendAsGroup(GroupKind::Document),
    );
}

#[test]
fn classify_two_audios_sends_as_group() {
    let items = atts(&[OutboundKind::Audio, OutboundKind::Audio]);
    assert_eq!(
        classify_media_group(&refs(&items)),
        GroupPlan::SendAsGroup(GroupKind::Audio),
    );
}

#[test]
fn classify_photo_and_voice_degrades() {
    let items = atts(&[OutboundKind::Photo, OutboundKind::Voice]);
    match classify_media_group(&refs(&items)) {
        GroupPlan::Degrade { reason } => assert!(reason.contains("incompatible")),
        other => panic!("expected Degrade, got {other:?}"),
    }
}

#[test]
fn classify_photo_and_document_degrades() {
    let items = atts(&[OutboundKind::Photo, OutboundKind::Document]);
    match classify_media_group(&refs(&items)) {
        GroupPlan::Degrade { reason } => assert!(reason.contains("incompatible")),
        other => panic!("expected Degrade, got {other:?}"),
    }
}

#[test]
fn classify_single_item_degrades() {
    let items = atts(&[OutboundKind::Photo]);
    match classify_media_group(&refs(&items)) {
        GroupPlan::Degrade { reason } => assert!(reason.contains("group of 1")),
        other => panic!("expected Degrade, got {other:?}"),
    }
}

#[test]
fn classify_empty_group_degrades() {
    let items: Vec<OutboundAttachment> = vec![];
    match classify_media_group(&refs(&items)) {
        GroupPlan::Degrade { .. } => (),
        other => panic!("expected Degrade, got {other:?}"),
    }
}

#[test]
fn classify_eleven_photos_splits_into_chunks() {
    let items = atts(&vec![OutboundKind::Photo; 11]);
    match classify_media_group(&refs(&items)) {
        GroupPlan::Split { chunks, kind, .. } => {
            assert_eq!(kind, GroupKind::PhotoVideo);
            assert_eq!(chunks.len(), 2, "expected 2 chunks (10 + 1)");
            assert_eq!(chunks[0], (0..10).collect::<Vec<_>>());
            assert_eq!(chunks[1], vec![10]);
        }
        other => panic!("expected Split, got {other:?}"),
    }
}

#[test]
fn classify_twenty_five_photos_splits_into_three_chunks() {
    let items = atts(&vec![OutboundKind::Photo; 25]);
    match classify_media_group(&refs(&items)) {
        GroupPlan::Split { chunks, .. } => {
            assert_eq!(chunks.len(), 3);
            assert_eq!(chunks[0].len(), 10);
            assert_eq!(chunks[1].len(), 10);
            assert_eq!(chunks[2].len(), 5);
        }
        other => panic!("expected Split, got {other:?}"),
    }
}

#[test]
fn classify_eleven_mixed_with_voice_degrades() {
    let mut kinds = vec![OutboundKind::Photo; 10];
    kinds.push(OutboundKind::Voice);
    let items = atts(&kinds);
    match classify_media_group(&refs(&items)) {
        GroupPlan::Degrade { reason } => assert!(reason.contains("incompatible")),
        other => panic!("expected Degrade, got {other:?}"),
    }
}
```

- [ ] **Step 2: Run tests, verify they fail**

Run: `cargo test -p rightclaw-bot telegram::attachments::tests::classify_`
Expected: all `classify_*` tests fail with `cannot find value classify_media_group`.

- [ ] **Step 3: Implement GroupPlan and classify_media_group**

Add these declarations to `crates/bot/src/telegram/attachments.rs` (after `GroupKind` from Task 2):

```rust
/// Outcome of classifying a candidate media group.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GroupPlan {
    /// 2–10 compatible items: one `sendMediaGroup` call.
    SendAsGroup(GroupKind),
    /// More than 10 compatible same-kind items: split into consecutive
    /// chunks. Each chunk is a list of indices into the input slice. The last
    /// chunk may be size 1 — the caller must fall back to an individual send
    /// for a size-1 chunk because `sendMediaGroup` rejects it.
    Split {
        chunks: Vec<Vec<usize>>,
        kind: GroupKind,
        reason: String,
    },
    /// Incompatible mix, size 0 or 1, or oversize-with-incompatible-mix: the
    /// caller falls back to individual sends for every item in the group.
    Degrade { reason: String },
}

/// Maximum items per Telegram media group.
const MEDIA_GROUP_MAX: usize = 10;

pub fn classify_media_group(items: &[&OutboundAttachment]) -> GroupPlan {
    if items.len() < 2 {
        return GroupPlan::Degrade {
            reason: if items.is_empty() { "group of 0".into() } else { "group of 1".into() },
        };
    }

    // All items must share a single GroupKind; any ungroupable item → degrade.
    let Some(first) = GroupKind::of(&items[0].kind) else {
        return GroupPlan::Degrade {
            reason: format!("incompatible types: {:?} cannot appear in a media group", items[0].kind),
        };
    };
    for it in &items[1..] {
        match GroupKind::of(&it.kind) {
            Some(k) if k == first => (),
            _ => {
                let summary: Vec<_> = items.iter().map(|i| i.kind).collect();
                return GroupPlan::Degrade {
                    reason: format!("incompatible types {summary:?} in one media group"),
                };
            }
        }
    }

    if items.len() <= MEDIA_GROUP_MAX {
        return GroupPlan::SendAsGroup(first);
    }

    let chunks: Vec<Vec<usize>> = (0..items.len())
        .collect::<Vec<_>>()
        .chunks(MEDIA_GROUP_MAX)
        .map(<[usize]>::to_vec)
        .collect();
    GroupPlan::Split {
        chunks,
        kind: first,
        reason: format!("group of {} exceeds Telegram limit of {MEDIA_GROUP_MAX}", items.len()),
    }
}
```

- [ ] **Step 4: Run the tests, verify pass**

Run: `cargo test -p rightclaw-bot telegram::attachments::tests::classify_`
Expected: all `classify_*` tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/bot/src/telegram/attachments.rs
git commit -m "feat(attachments): classify_media_group pure helper

Decides whether a group of OutboundAttachments can be sent via
sendMediaGroup, must be split into ≤10-item chunks, or must
degrade to individual sends. Covered by table-driven tests."
```

---

## Task 4: merge_group_captions helper (TDD)

**Files:**
- Modify: `crates/bot/src/telegram/attachments.rs`

Telegram shows only the first item's caption in an album. If the agent puts captions on later items, silently dropping them would lose text. The helper concatenates every non-empty caption into the first item's caption with `"\n\n"` separators and blanks the others.

- [ ] **Step 1: Write the failing tests**

Append to the test block in `crates/bot/src/telegram/attachments.rs`:

```rust
#[test]
fn merge_captions_first_only_is_preserved() {
    let mut caps = vec![Some("first".to_owned()), None, None];
    merge_group_captions(&mut caps);
    assert_eq!(caps, vec![Some("first".to_owned()), None, None]);
}

#[test]
fn merge_captions_all_none_stays_none() {
    let mut caps: Vec<Option<String>> = vec![None, None, None];
    merge_group_captions(&mut caps);
    assert_eq!(caps, vec![None, None, None]);
}

#[test]
fn merge_captions_later_items_fold_into_first() {
    let mut caps = vec![Some("a".to_owned()), None, Some("b".to_owned())];
    merge_group_captions(&mut caps);
    assert_eq!(caps, vec![Some("a\n\nb".to_owned()), None, None]);
}

#[test]
fn merge_captions_only_later_item_moves_to_first() {
    let mut caps = vec![None, Some("only".to_owned())];
    merge_group_captions(&mut caps);
    assert_eq!(caps, vec![Some("only".to_owned()), None]);
}

#[test]
fn merge_captions_all_three_set_joined() {
    let mut caps = vec![
        Some("a".to_owned()),
        Some("b".to_owned()),
        Some("c".to_owned()),
    ];
    merge_group_captions(&mut caps);
    assert_eq!(caps, vec![Some("a\n\nb\n\nc".to_owned()), None, None]);
}
```

- [ ] **Step 2: Run tests, verify they fail**

Run: `cargo test -p rightclaw-bot telegram::attachments::tests::merge_captions_`
Expected: FAIL with `cannot find function merge_group_captions`.

- [ ] **Step 3: Implement merge_group_captions**

Add to `crates/bot/src/telegram/attachments.rs` (near the other group helpers):

```rust
/// Fold every non-empty caption into the first slot, separated by blank lines,
/// and blank the rest. Telegram only shows the first item's caption in a media
/// group; without folding, later captions would be silently dropped.
pub fn merge_group_captions(captions: &mut [Option<String>]) {
    if captions.is_empty() {
        return;
    }
    let mut parts: Vec<String> = Vec::new();
    for cap in captions.iter_mut() {
        if let Some(c) = cap.take() {
            if !c.is_empty() {
                parts.push(c);
            }
        }
    }
    if parts.is_empty() {
        return;
    }
    captions[0] = Some(parts.join("\n\n"));
}
```

- [ ] **Step 4: Run tests, verify pass**

Run: `cargo test -p rightclaw-bot telegram::attachments::tests::merge_captions_`
Expected: all five tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/bot/src/telegram/attachments.rs
git commit -m "feat(attachments): merge_group_captions helper

Folds later captions into the first item with blank-line separators.
Prevents silent loss of text when the agent captions non-first items
of a media group."
```

---

## Task 5: Send enum and partition_sends (TDD)

**Files:**
- Modify: `crates/bot/src/telegram/attachments.rs`

`partition_sends` walks a reply's `[OutboundAttachment]` once and produces an ordered `Vec<Send>` — one entry per eventual Telegram API call. It also returns any WARN messages the classifier emitted so the caller can log them.

- [ ] **Step 1: Write the failing tests**

Append to the test block in `crates/bot/src/telegram/attachments.rs`:

```rust
fn att_with(kind: OutboundKind, group: Option<&str>, caption: Option<&str>) -> OutboundAttachment {
    OutboundAttachment {
        kind,
        path: format!("/sandbox/outbox/{}-{}.bin", kind_to_ext(kind), caption.unwrap_or("x")),
        filename: None,
        caption: caption.map(str::to_owned),
        media_group_id: group.map(str::to_owned),
    }
}

#[test]
fn partition_no_group_ids_produces_all_singles() {
    let atts = vec![
        att_with(OutboundKind::Photo, None, None),
        att_with(OutboundKind::Document, None, None),
    ];
    let (sends, warnings) = partition_sends(&atts);
    assert_eq!(sends.len(), 2);
    assert!(warnings.is_empty());
    assert!(matches!(sends[0], Send::Single(_)));
    assert!(matches!(sends[1], Send::Single(_)));
}

#[test]
fn partition_two_photo_group_produces_one_group_send() {
    let atts = vec![
        att_with(OutboundKind::Photo, Some("shots"), Some("a")),
        att_with(OutboundKind::Photo, Some("shots"), Some("b")),
    ];
    let (sends, warnings) = partition_sends(&atts);
    assert!(warnings.is_empty());
    assert_eq!(sends.len(), 1);
    match &sends[0] {
        Send::Group { kind, items } => {
            assert_eq!(*kind, GroupKind::PhotoVideo);
            assert_eq!(items.len(), 2);
            assert_eq!(items[0].caption.as_deref(), Some("a\n\nb"));
            assert!(items[1].caption.is_none());
        }
        other => panic!("expected Send::Group, got {other:?}"),
    }
}

#[test]
fn partition_group_preserves_first_occurrence_order() {
    // Reply order: group A item, single, group A item, group B item, group B item
    let atts = vec![
        att_with(OutboundKind::Photo, Some("a"), None),
        att_with(OutboundKind::Document, None, None),
        att_with(OutboundKind::Photo, Some("a"), None),
        att_with(OutboundKind::Document, Some("b"), None),
        att_with(OutboundKind::Document, Some("b"), None),
    ];
    let (sends, warnings) = partition_sends(&atts);
    assert!(warnings.is_empty());
    // Expected send order: group "a" (where it first appeared), then single,
    // then group "b".
    assert_eq!(sends.len(), 3);
    assert!(matches!(sends[0], Send::Group { kind: GroupKind::PhotoVideo, .. }));
    assert!(matches!(sends[1], Send::Single(_)));
    assert!(matches!(sends[2], Send::Group { kind: GroupKind::Document, .. }));
}

#[test]
fn partition_incompatible_group_degrades_and_warns() {
    let atts = vec![
        att_with(OutboundKind::Photo, Some("bad"), None),
        att_with(OutboundKind::Voice, Some("bad"), None),
    ];
    let (sends, warnings) = partition_sends(&atts);
    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].contains("bad"), "warning must mention group id, got: {}", warnings[0]);
    assert_eq!(sends.len(), 2, "both items fall back to Single");
    assert!(sends.iter().all(|s| matches!(s, Send::Single(_))));
}

#[test]
fn partition_lone_group_member_degrades_and_warns() {
    let atts = vec![
        att_with(OutboundKind::Photo, Some("only"), None),
        att_with(OutboundKind::Document, None, None),
    ];
    let (sends, warnings) = partition_sends(&atts);
    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].contains("only"));
    assert_eq!(sends.len(), 2);
    assert!(sends.iter().all(|s| matches!(s, Send::Single(_))));
}

#[test]
fn partition_split_oversize_group_yields_multiple_group_sends_plus_trailing_single() {
    let atts: Vec<OutboundAttachment> = (0..11)
        .map(|_| att_with(OutboundKind::Photo, Some("big"), None))
        .collect();
    let (sends, warnings) = partition_sends(&atts);
    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].contains("big"));
    // 11 → one group of 10 + one trailing single.
    assert_eq!(sends.len(), 2);
    match &sends[0] {
        Send::Group { items, .. } => assert_eq!(items.len(), 10),
        other => panic!("expected Group first, got {other:?}"),
    }
    assert!(matches!(sends[1], Send::Single(_)));
}
```

- [ ] **Step 2: Run tests, verify they fail**

Run: `cargo test -p rightclaw-bot telegram::attachments::tests::partition_`
Expected: FAIL — `Send` and `partition_sends` are undefined.

- [ ] **Step 3: Implement Send and partition_sends**

Add to `crates/bot/src/telegram/attachments.rs` (below `classify_media_group`):

```rust
/// One Telegram API call the bot must make to honour a reply's attachments.
/// `Single` reuses the per-type `send_*` path; `Group` becomes one
/// `sendMediaGroup`.
#[derive(Debug)]
pub enum Send {
    Single(OutboundAttachment),
    Group {
        kind: GroupKind,
        items: Vec<OutboundAttachment>,
    },
}

/// Partition a reply's attachments into the ordered sends the bot must perform.
/// Returns the list of sends and a list of WARN strings describing any group
/// that had to be degraded or split — the caller logs them.
pub fn partition_sends(attachments: &[OutboundAttachment]) -> (Vec<Send>, Vec<String>) {
    use std::collections::BTreeMap;

    // Collect indices per group, preserving first-occurrence order via a
    // secondary Vec (BTreeMap orders by key, which is not what we want).
    let mut group_order: Vec<String> = Vec::new();
    let mut group_indices: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    let mut singles: Vec<usize> = Vec::new();
    let mut first_slot: BTreeMap<String, usize> = BTreeMap::new();

    for (i, a) in attachments.iter().enumerate() {
        match &a.media_group_id {
            None => singles.push(i),
            Some(id) => {
                if !group_indices.contains_key(id) {
                    group_order.push(id.clone());
                    first_slot.insert(id.clone(), i);
                }
                group_indices.entry(id.clone()).or_default().push(i);
            }
        }
    }

    // Build a timeline: every single keeps its original position; every group
    // replaces the position of its first member.
    //
    // slot_kind[i] = Some(Slot::Single) for a pure single,
    //                Some(Slot::GroupAnchor(id)) for the first member of a group,
    //                None for later members of a group (they are emitted together
    //                with the anchor).
    #[derive(Clone)]
    enum Slot {
        Single,
        GroupAnchor(String),
        GroupMember,
    }
    let mut slots: Vec<Slot> = vec![Slot::Single; attachments.len()];
    for id in &group_order {
        let indices = &group_indices[id];
        for (n, idx) in indices.iter().enumerate() {
            slots[*idx] = if n == 0 {
                Slot::GroupAnchor(id.clone())
            } else {
                Slot::GroupMember
            };
        }
    }

    let mut warnings: Vec<String> = Vec::new();
    let mut sends: Vec<Send> = Vec::new();

    for (i, slot) in slots.iter().enumerate() {
        match slot {
            Slot::Single => sends.push(Send::Single(attachments[i].clone())),
            Slot::GroupAnchor(id) => {
                let indices = &group_indices[id];
                let group_items: Vec<&OutboundAttachment> =
                    indices.iter().map(|&idx| &attachments[idx]).collect();
                let plan = classify_media_group(&group_items);
                match plan {
                    GroupPlan::SendAsGroup(kind) => {
                        let mut items: Vec<OutboundAttachment> =
                            indices.iter().map(|&idx| attachments[idx].clone()).collect();
                        let mut caps: Vec<Option<String>> =
                            items.iter().map(|it| it.caption.clone()).collect();
                        merge_group_captions(&mut caps);
                        for (it, c) in items.iter_mut().zip(caps.into_iter()) {
                            it.caption = c;
                        }
                        sends.push(Send::Group { kind, items });
                    }
                    GroupPlan::Split { chunks, kind, reason } => {
                        warnings.push(format!(
                            "media_group_id={id:?}: {reason} — splitting into ≤10-item chunks"
                        ));
                        for chunk in chunks {
                            if chunk.len() < 2 {
                                // size-1 trailing chunk: emit as Single
                                let src_idx = indices[chunk[0]];
                                sends.push(Send::Single(attachments[src_idx].clone()));
                            } else {
                                let mut items: Vec<OutboundAttachment> = chunk
                                    .iter()
                                    .map(|&local| attachments[indices[local]].clone())
                                    .collect();
                                let mut caps: Vec<Option<String>> =
                                    items.iter().map(|it| it.caption.clone()).collect();
                                merge_group_captions(&mut caps);
                                for (it, c) in items.iter_mut().zip(caps.into_iter()) {
                                    it.caption = c;
                                }
                                sends.push(Send::Group { kind, items });
                            }
                        }
                    }
                    GroupPlan::Degrade { reason } => {
                        warnings.push(format!(
                            "media_group_id={id:?}: {reason} — falling back to individual sends"
                        ));
                        for &idx in indices {
                            sends.push(Send::Single(attachments[idx].clone()));
                        }
                    }
                }
            }
            Slot::GroupMember => { /* emitted with the anchor */ }
        }
    }

    (sends, warnings)
}
```

- [ ] **Step 4: Run the tests, verify pass**

Run: `cargo test -p rightclaw-bot telegram::attachments::tests::partition_`
Expected: all six `partition_*` tests pass.

- [ ] **Step 5: Run the full attachments test set, verify pass**

Run: `cargo test -p rightclaw-bot telegram::attachments::tests::`
Expected: every test in the module passes.

- [ ] **Step 6: Commit**

```bash
git add crates/bot/src/telegram/attachments.rs
git commit -m "feat(attachments): partition_sends turns reply into ordered sends

Groups attachments by media_group_id, classifies each group, applies
caption merging, and degrades with WARN messages when Telegram rules
are violated. Pure function, fully unit-tested."
```

---

## Task 6: Rewrite send_attachments to execute the partition

**Files:**
- Modify: `crates/bot/src/telegram/attachments.rs:483` (`send_attachments` function body)

The existing loop becomes: run `partition_sends`, log every warning, then iterate the resulting `Vec<Send>` — `Single` keeps the current per-type match (extracted into a helper); `Group` builds `Vec<InputMedia>` and calls `bot.send_media_group`.

- [ ] **Step 1: Extract the existing per-item send into `send_single`**

Replace the body of `send_attachments` (from `let mut errors: Vec<String> = Vec::new();` through the closing `}` before the `if errors.is_empty()`) with a call to a new function, and move the existing per-item logic verbatim into that function. The signature:

```rust
async fn send_single(
    att: &OutboundAttachment,
    bot: &super::BotType,
    chat_id: teloxide::types::ChatId,
    eff_thread_id: i64,
    agent_dir: &std::path::Path,
    resolved_sandbox: Option<&str>,
    sandboxed: bool,
    outbox_prefix: &str,
    outbox_path: &std::path::Path,
) -> Result<(), teloxide::RequestError>
```

Move everything inside the existing `for att in attachments { ... }` body — path validation, host-path resolution, size check, the big `match att.kind` that calls `send_photo` / `send_document` / … — into `send_single`. Replace every `continue` that was skipping the item with an early `return Ok(())` (the intent was "silent skip", and an error-free return is the closest equivalent). The host-path temp cleanup stays inside `send_single`.

Two behavioural details preserved from today:
- `canonicalize` failure or out-of-outbox path → early `return Ok(())` (silent skip, same as the existing `continue`).
- Oversize → early `return Ok(())` after the warn log.

- [ ] **Step 2: Write send_group**

Add (in the same file):

The caller already tracks `GroupKind` via `Send::Group { kind, .. }`. `send_group` itself does not need it — the per-item `OutboundKind` drives the `InputMedia` variant choice. Signature:

```rust
async fn send_group(
    items: &[OutboundAttachment],
    bot: &super::BotType,
    chat_id: teloxide::types::ChatId,
    eff_thread_id: i64,
    agent_dir: &std::path::Path,
    resolved_sandbox: Option<&str>,
    sandboxed: bool,
    outbox_prefix: &str,
    outbox_path: &std::path::Path,
) -> Result<(), teloxide::RequestError> {
    use teloxide::payloads::SendMediaGroupSetters;
    use teloxide::requests::Requester;
    use teloxide::types::{
        InputFile, InputMedia, InputMediaAudio, InputMediaDocument, InputMediaPhoto,
        InputMediaVideo, MessageId, ThreadId,
    };

    // Resolve every item's host path up front so we can clean them all up
    // after the send attempt, regardless of success.
    let mut host_paths: Vec<std::path::PathBuf> = Vec::with_capacity(items.len());
    for att in items {
        // Same path-validation checks as send_single; a validation failure
        // inside a group aborts the group (silent skip is wrong here because
        // the agent asked for grouping).
        if !att.path.starts_with(outbox_prefix) {
            tracing::warn!(
                "Outbound attachment path {} is outside outbox prefix {outbox_prefix} — skipping media group",
                att.path,
            );
            cleanup_host_paths(&host_paths, sandboxed).await;
            return Ok(());
        }
        let host = if sandboxed {
            let tmp_dir = agent_dir.join("tmp/outbox");
            let file_name = std::path::Path::new(&att.path)
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            let dest = tmp_dir.join(&file_name);
            let sandbox = resolved_sandbox.unwrap();
            if let Err(e) = rightclaw::openshell::download_file(sandbox, &att.path, &dest).await {
                tracing::warn!(
                    "download_file failed for {}: {:#} — skipping media group",
                    att.path, e,
                );
                cleanup_host_paths(&host_paths, sandboxed).await;
                return Ok(());
            }
            dest
        } else {
            match std::fs::canonicalize(std::path::PathBuf::from(&att.path)) {
                Ok(p) => {
                    match std::fs::canonicalize(outbox_path) {
                        Ok(outbox_c) if p.starts_with(&outbox_c) => p,
                        Ok(_) => {
                            tracing::warn!(
                                "Outbound attachment {} resolves outside outbox — skipping media group",
                                att.path,
                            );
                            return Ok(());
                        }
                        Err(e) => {
                            tracing::warn!("canonicalize outbox failed: {e} — skipping media group");
                            return Ok(());
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        "canonicalize {} failed: {e} — skipping media group",
                        att.path,
                    );
                    return Ok(());
                }
            }
        };

        // Size check: any oversize member aborts the entire group.
        let meta = match tokio::fs::metadata(&host).await {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!("metadata failed for {}: {e} — skipping media group", host.display());
                cleanup_host_paths(&[host.clone()], sandboxed).await;
                cleanup_host_paths(&host_paths, sandboxed).await;
                return Ok(());
            }
        };
        let limit = match att.kind {
            OutboundKind::Photo => TELEGRAM_PHOTO_UPLOAD_LIMIT,
            _ => TELEGRAM_FILE_UPLOAD_LIMIT,
        };
        if meta.len() > limit {
            tracing::warn!(
                "Outbound {} ({:.1} MB) exceeds upload limit — skipping media group",
                att.path,
                meta.len() as f64 / (1024.0 * 1024.0),
            );
            cleanup_host_paths(&[host.clone()], sandboxed).await;
            cleanup_host_paths(&host_paths, sandboxed).await;
            return Ok(());
        }

        host_paths.push(host);
    }

    // Build InputMedia list. kind tells us which InputMedia variant to use
    // for each item.
    let media: Vec<InputMedia> = items
        .iter()
        .zip(host_paths.iter())
        .map(|(att, host)| {
            let file = InputFile::file(host);
            let cap = att.caption.clone();
            match att.kind {
                OutboundKind::Photo => {
                    let mut m = InputMediaPhoto::new(file);
                    if let Some(c) = cap {
                        m = m.caption(c);
                    }
                    InputMedia::Photo(m)
                }
                OutboundKind::Video => {
                    let mut m = InputMediaVideo::new(file);
                    if let Some(c) = cap {
                        m = m.caption(c);
                    }
                    InputMedia::Video(m)
                }
                OutboundKind::Document => {
                    let mut m = InputMediaDocument::new(file);
                    if let Some(c) = cap {
                        m = m.caption(c);
                    }
                    InputMedia::Document(m)
                }
                OutboundKind::Audio => {
                    let mut m = InputMediaAudio::new(file);
                    if let Some(c) = cap {
                        m = m.caption(c);
                    }
                    InputMedia::Audio(m)
                }
                // Other kinds are filtered out by classify_media_group, so this
                // branch is unreachable in practice. Treating it as a silent
                // skip (not panic) keeps the bot alive if the classifier is
                // ever changed.
                _ => {
                    tracing::error!(
                        "send_group received ungroupable kind {:?} for {} — classifier bug",
                        att.kind, att.path,
                    );
                    InputMedia::Document(InputMediaDocument::new(file))
                }
            }
        })
        .collect();
    let thread_id = if eff_thread_id != 0 {
        Some(ThreadId(MessageId(eff_thread_id as i32)))
    } else {
        None
    };

    let mut req = bot.send_media_group(chat_id, media);
    if let Some(tid) = thread_id {
        req = req.message_thread_id(tid);
    }
    let result = req.await.map(|_| ());

    cleanup_host_paths(&host_paths, sandboxed).await;
    result
}

async fn cleanup_host_paths(paths: &[std::path::PathBuf], sandboxed: bool) {
    if !sandboxed {
        return;
    }
    for p in paths {
        let _ = tokio::fs::remove_file(p).await;
    }
}
```

- [ ] **Step 3: Rewrite send_attachments**

Replace the body of `send_attachments` (keeping the existing signature and the pre-loop setup that prepares `outbox_prefix`, `outbox_path`, `sandboxed`, and the `tmp/outbox` mkdir) with:

```rust
    let (sends, warnings) = partition_sends(attachments);
    for w in &warnings {
        tracing::warn!("{w}");
    }

    let mut errors: Vec<String> = Vec::new();
    for send in &sends {
        let result: Result<(), teloxide::RequestError> = match send {
            Send::Single(att) => {
                send_single(
                    att,
                    bot,
                    chat_id,
                    eff_thread_id,
                    agent_dir,
                    resolved_sandbox,
                    sandboxed,
                    &outbox_prefix,
                    &outbox_path,
                )
                .await
            }
            Send::Group { kind: _, items } => {
                send_group(
                    items,
                    bot,
                    chat_id,
                    eff_thread_id,
                    agent_dir,
                    resolved_sandbox,
                    sandboxed,
                    &outbox_prefix,
                    &outbox_path,
                )
                .await
            }
        };
        if let Err(e) = result {
            let label = match send {
                Send::Single(att) => format!("{:?} attachment {}", att.kind, att.path),
                Send::Group { kind, items } => {
                    format!("{kind:?} media group of {} items", items.len())
                }
            };
            let msg = format!(
                "failed to send {label}: {}",
                rightclaw::error::display_error_chain(&e),
            );
            tracing::error!("{msg}");
            errors.push(msg);
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("; ").into())
    }
```

- [ ] **Step 4: Build**

Run: `cargo build --workspace`
Expected: clean build.

- [ ] **Step 5: Run the full bot test suite**

Run: `cargo test -p rightclaw-bot`
Expected: all tests pass. The new logic has no integration test, but all the pure helpers that feed it are already covered.

- [ ] **Step 6: Commit**

```bash
git add crates/bot/src/telegram/attachments.rs
git commit -m "feat(attachments): execute partitioned sends via sendMediaGroup

send_attachments now runs partition_sends, logs every WARN, and
dispatches each Send::Single through the existing per-type path or
each Send::Group through a new send_group helper that assembles
InputMedia and calls bot.send_media_group. Caption merge and size
validation happen before the API call; temp files are cleaned up
regardless of outcome."
```

---

## Task 7: Prompt updates + snapshot test

**Files:**
- Modify: `crates/rightclaw/templates/right/prompt/OPERATING_INSTRUCTIONS.md` (insert a new subsection after the existing `## Sending Attachments` block)
- Modify: `crates/rightclaw/src/codegen/agent_def_tests.rs` (add one substring-assertion test)
- Modify: `PROMPT_SYSTEM.md` (add matching paragraph)

- [ ] **Step 1: Write the failing prompt test**

Append to `crates/rightclaw/src/codegen/agent_def_tests.rs`:

```rust
#[test]
fn operating_instructions_documents_media_groups() {
    let ops = crate::codegen::OPERATING_INSTRUCTIONS;
    assert!(ops.contains("Media Groups"), "missing media-group docs");
    assert!(ops.contains("media_group_id"), "missing media_group_id mention");
    assert!(
        ops.contains("2–10") || ops.contains("2-10"),
        "must mention the 2–10 item limit"
    );
}
```

- [ ] **Step 2: Run the test, verify it fails**

Run: `cargo test -p rightclaw operating_instructions_documents_media_groups`
Expected: FAIL — the prompt file does not yet contain `Media Groups`.

- [ ] **Step 3: Extend OPERATING_INSTRUCTIONS.md**

Insert immediately after line 100 of `crates/rightclaw/templates/right/prompt/OPERATING_INSTRUCTIONS.md` (the "split into multiple smaller files" line), **before** the blank line preceding `## Cron Management`:

```markdown

### Media Groups (Albums)

Multiple attachments can arrive as a single Telegram message ("media group") by
sharing the same `media_group_id` string across items in your `attachments`
array. This mirrors the `media_group_id` field Telegram puts on inbound
messages — same field name, same semantics.

Use media groups when attachments belong together (photos from one event, pages
of one report). Without a `media_group_id`, each attachment arrives as its own
Telegram message.

Telegram rules — the bot warns and falls back to individual sends if violated:

- A group must contain 2–10 items.
- Photos and videos can mix in one group.
- Documents form a documents-only group (no photos, videos, or audio).
- Audios form an audios-only group.
- Voice, video_note, sticker, and animation cannot be grouped — send them one by one.

Captions: Telegram shows one caption per media group, taken from the first
item. If multiple items carry a caption, the bot joins them with blank lines
into the first item's caption.

Example — two grouped photos plus one standalone document:

```json
{
  "content": "Here are the shots and the report.",
  "attachments": [
    {"type": "photo",    "path": "/sandbox/outbox/a.jpg", "media_group_id": "shots", "caption": "Front view"},
    {"type": "photo",    "path": "/sandbox/outbox/b.jpg", "media_group_id": "shots", "caption": "Side view"},
    {"type": "document", "path": "/sandbox/outbox/report.pdf"}
  ]
}
```

The value of `media_group_id` is arbitrary — only equality within one reply
matters.
```

- [ ] **Step 4: Run the prompt test, verify pass**

Run: `cargo test -p rightclaw operating_instructions_documents_media_groups`
Expected: PASS.

- [ ] **Step 5: Update PROMPT_SYSTEM.md**

Open `PROMPT_SYSTEM.md` and locate the section that documents the reply schema and outbound attachments (search for `attachments` or `reply schema`). Append a paragraph:

```markdown

**Media groups.** Each item in `attachments` accepts an optional `media_group_id`
(nullable string). Items sharing the same value are delivered as a single
Telegram media group (album). Validation and degradation rules match Telegram's
`sendMediaGroup` constraints — see `### Media Groups (Albums)` in
`OPERATING_INSTRUCTIONS.md` for the full rules shown to the agent.
```

- [ ] **Step 6: Commit**

```bash
git add crates/rightclaw/templates/right/prompt/OPERATING_INSTRUCTIONS.md \
        crates/rightclaw/src/codegen/agent_def_tests.rs \
        PROMPT_SYSTEM.md
git commit -m "docs(prompt): describe media_group_id to the agent

Agents now get a concrete rule-set for building Telegram albums:
2–10 items, photo+video mixable, documents-only, audios-only,
voice/video_note/sticker/animation ungroupable. PROMPT_SYSTEM.md
cross-links the operating instructions."
```

---

## Task 8: Final build, clippy, and Rust review

**Files:** none direct — review artefacts only

- [ ] **Step 1: Full workspace build**

Run: `cargo build --workspace`
Expected: no errors, no warnings.

- [ ] **Step 2: Full workspace test**

Run: `cargo test --workspace`
Expected: every test passes, including the new `media_group_id`, `classify_*`, `merge_captions_*`, `partition_*`, and `operating_instructions_documents_media_groups` tests.

- [ ] **Step 3: Clippy**

Run: `cargo clippy --workspace --all-targets -- -D warnings`
Expected: clean.

- [ ] **Step 4: Delegate code review to rust-dev:review-rust-code subagent**

Dispatch the `rust-dev:review-rust-code` agent with a prompt like:

> Review the media-groups feature added across these commits:
> - `crates/rightclaw/src/codegen/agent_def.rs` (media_group_id field added to three JSON schemas)
> - `crates/rightclaw/src/codegen/agent_def_tests.rs` (new schema + prompt tests)
> - `crates/rightclaw/templates/right/prompt/OPERATING_INSTRUCTIONS.md` (new Media Groups subsection)
> - `crates/bot/src/telegram/attachments.rs` (GroupKind, classify_media_group, merge_group_captions, Send, partition_sends, rewritten send_attachments, new send_single and send_group helpers)
> - `PROMPT_SYSTEM.md` (cross-reference paragraph)
>
> Focus on: error-handling discipline (CLAUDE.rust.md FAIL FAST), leaks of sandbox temp files on every failure path in `send_group`, captions correctness, and whether the `SendLabel` helper is actually used or dead code after the rewrite. The plan is at `docs/superpowers/plans/2026-04-18-telegram-media-groups.md`.

- [ ] **Step 5: Triage review findings**

For every issue the reviewer raises at priority high or medium:

- Add a TODO note to your working task list.
- Fix it in a focused commit referencing the review finding.
- Re-run `cargo test --workspace` and `cargo clippy --workspace --all-targets -- -D warnings` after the fix.

Low-priority / style findings may be applied or skipped at discretion; note which were skipped and why.

- [ ] **Step 6: Verify the commit count and push readiness**

Run: `git log --oneline master..HEAD`
Expected: one commit per task (7–8 commits total, depending on whether review required fix-up commits). Each commit message follows the `type(scope): subject` pattern shown above.

Do **not** push. Wait for the user's go-ahead per CLAUDE.md (`Don't do git push unless creating or syncing a pull request`).

---

## Self-Review Summary

- Spec section **Schema Change** → Task 1.
- Spec section **Partition** → Task 5.
- Spec section **Classify** (including the 7-row table, size-1, 11+ same-kind, 11+ mixed) → Task 3.
- Spec section **Send** (media group build, caption merge, errors, cleanup) → Tasks 4 and 6.
- Spec section **Prompt Change** → Task 7.
- Spec section **Tests** (`media_group_id` schema parse, classify table, caption merge, partitioner, `Media Groups` prompt substring) → Tasks 1, 3, 4, 5, 7.
- Spec **Rollout** (additive, backward compatible) → covered by Task 1 (`#[serde(default)]`) and by each step's explicit "no migration / no sandbox state" scope.

No placeholders. Every step either states explicit file paths and code, or lists exact commands and expected outputs. The type `Send` introduced in Task 5 is used by Task 6; `GroupKind` introduced in Task 2 is consumed by Tasks 3, 5, and 6.
