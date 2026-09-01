# channel_post attachments — implementation plan

## Decision

Extend `mcp__right__channel_post` so one call publishes an optional `RichContent` body plus zero or more `MessageAttachmentDto` attachments (at least one of the two required) as **one logical Channel Publication**. Delivery order is RichContent first, then attachments in request order. Delivery stops at the first failure. The response carries **every confirmed Telegram message id** in delivery order plus `delivery_uncertain: true` when a failed request may have been accepted by Telegram without a receipt (network/timeout/429/5xx — the same ambiguity classes `is_retryable_format_error` already excludes from retry). There is never an automatic retry after an uncertain failure. The bot archives one conversation row: confirmed delivered text, captions, and attachment markers in delivery order, associated with the last confirmed message id. Archive failure after any delivery returns a typed partial result carrying the ids and forbidding resend. Full media groups (`media_group_id`, 2–10 items, split/degrade rules) remain supported via the existing `send_attachments` machinery. Aggregator and bot both keep enforcing the channel allowlist; attachment paths stay under `/sandbox/outbox/`; foreground+cron only; cap stays 10 per turn. `ARCHITECTURE.md` is untouched (its channel/attachment invariants are unchanged); ADR 0004 stands — captions remain regular Markdown paths, only the body is RichContent.

Core internal API (the "attachment delivery report"): the bot's per-send helpers stop discarding Telegram `Message` receipts. A new report type carries confirmed ids, the first failure, and its uncertainty classification. `channel_post` consumes the report in stop-at-first-failure mode; a thin adapter preserves today's continue-across-errors `Result<(), …>` behavior for the three legacy callers (`handle_message_send`, `worker.rs`, `async_delivery.rs`), whose observable behavior must not change.

Baseline: targeted suite is green (1829/1829). Rust edition 2024 (workspace). All commands run via `devenv shell --`. Every task is red test → green code. Do not run formatters or workspace-wide suites until the final gate.

## Task 1 — Wire DTOs: `ChannelPostRequest` attachments, `ChannelPostResponse` ids + uncertainty

Files: `crates/right-mcp/src/internal_client.rs`.

**Red tests** (in the existing `#[cfg(test)]` module, beside `channel_post_request_roundtrips_content_without_text_field`):

1. `channel_post_request_carries_optional_content_and_attachments` — build a `ChannelPostRequest` with `content: None` and one `MessageAttachmentDto` (photo, `/sandbox/outbox/a.png`, caption, `media_group_id: None`); serialize/deserialize; assert the attachment round-trips and `content` is absent from the JSON. Also assert a legacy JSON body without an `attachments` key deserializes to an empty vec (serde `default`).
2. `channel_post_response_carries_confirmed_ids_and_uncertainty` — round-trip `ChannelPostResponse { ok: false, message_ids: vec![10, 11], delivery_uncertain: true, error: Some(..) }`; assert a legacy body without the two new keys deserializes to empty vec / `false`.
3. Update `typed_partial_channel_post_responses_reach_the_aggregator` and `typed_channel_post_zero_delivery_failure_stays_an_error_status` to the new response shape: partials assert `message_ids` (non-empty) instead of `message_id`; zero-delivery stays an error status.

Run: `devenv shell -- cargo nextest run -p right-mcp channel_post` → FAIL (fields absent / compile error).

**Green code**:

- `ChannelPostRequest`: change `content: right_rich_content::RichContent` → `content: Option<right_rich_content::RichContent>` with `#[serde(default)]`; add `#[serde(default)] pub attachments: Vec<MessageAttachmentDto>` (reuse the existing DTO — no new attachment type). Keep `deny_unknown_fields` and the redacted `Debug` (add the `attachments` field to it, mirroring `SendMessageRequest`).
- `ChannelPostResponse`: replace `message_id: Option<i32>` with `#[serde(default)] pub message_ids: Vec<i32>` (delivery order) and add `#[serde(default)] pub delivery_uncertain: bool`. Clean cutover: migrate every `message_id` construction/read site in this crate's tests now; aggregator/bot sites are migrated in Tasks 4–5 (they won't compile until then — that is expected red pressure, fix them in their own tasks, not here; run only `-p right-mcp` at this stage).

Run: `devenv shell -- cargo nextest run -p right-mcp channel_post` → PASS. Commit.

## Task 2 — Bot: receipt-carrying attachment delivery report

Files: `crates/bot/src/telegram/attachments.rs` (plus its inline test module).

**Design** (state it in code comments, keep names exact):

```rust
/// Classification of the first failed Telegram request in an attachment batch.
pub(crate) enum SendFailure {
    /// Telegram deterministically rejected before delivery (400-class,
    /// path/validation Skip): the request is certainly NOT published.
    Certain(String),
    /// Network/timeout/429/5xx: Telegram may have accepted the request
    /// without returning a receipt. Never retried.
    Uncertain(String),
}

pub(crate) struct AttachmentSendReport {
    /// Telegram ids of every confirmed message, in delivery order.
    pub(crate) confirmed: Vec<i32>,
    /// Archive fragments for confirmed sends, delivery order: caption text
    /// and/or `[{kind}]` / `[{kind}: {filename}]` markers (same format as
    /// `archive::archive_content`).
    pub(crate) delivered_fragments: Vec<String>,
    /// First failure, if any. Delivery stopped here (stop-on-first-failure
    /// mode) or continued (legacy mode) depending on the entry point.
    pub(crate) failure: Option<SendFailure>,
}
```

Classification rule: `SendError::Skip` and `FallbackToSingles`-resolved-to-singles failures with a typed 400 API error → `Certain`; `SendError::Api` where the error is a typed `error_code == 400` → `Certain`; every other `Api` (429/5xx/network/`TgError::Timeout`/`Other`) → `Uncertain`. Reuse the exact reasoning documented on `is_retryable_format_error` (400 is pre-delivery; everything ambiguous may have been delivered).

**Red tests** (unit tests on the pure pieces; no live Telegram):

1. `send_failure_classification_is_400_certain_else_uncertain` — feed `SendError::Api` wrappers built like `is_retryable_format_error_gates_retry_correctly` does (400 / 429 / 500 / `TgError::Timeout` / `TgError::Other`) into the new classifier fn `classify_send_error(&SendError) -> SendFailure`; assert Certain only for typed 400 and `Skip`.
2. `attachment_fragments_use_archive_marker_format` — a helper `attachment_fragment(&OutboundAttachment) -> String` returns `caption` when present else `[photo]` / `[document: report.pdf]` (kind snake-case + optional filename), matching `archive_content`'s bracket format.

Run: `devenv shell -- cargo nextest run -p right-bot classify_send_error attachment_fragment` → FAIL (symbols absent).

**Green code**:

- Change `send_single_attempt` and `send_single` to return the delivered `Message`(s): `send_single(...) -> Result<Message, SendError>` (drop the `.map(|_| ())` in each `send_*` arm of `send_single_attempt`; `tg_bot` methods already return `Message`). Change `send_group` to `-> Result<Vec<Message>, SendError>` (`send_media_group` already returns `Vec<Message>`; stop discarding with `.map(|_| ())`).
- Add `classify_send_error` and `attachment_fragment` per the tests.
- Add `pub(crate) async fn send_attachments_reported(attachments, bot, chat_id, eff_thread_id, agent_dir, sandbox, stop_on_first_failure: bool) -> AttachmentSendReport`: the current `send_attachments` loop body, but each successful `OutboundSend::Single` pushes one id + fragment, each successful `Group` pushes all group ids + the group's fragments (per item, delivery order), and a failure records `classify_send_error` — then `break` when `stop_on_first_failure`, else keep the legacy continue behavior and record only the FIRST failure (later errors still logged, matching today's error-string join in spirit). The `FallbackToSingles` degrade path stays: degraded singles report individually.
- Rewrite `send_attachments` as the legacy adapter: call `send_attachments_reported(.., stop_on_first_failure: false)` and map `report.failure` to the old `Result<(), Box<dyn Error + Send + Sync>>` (Some → `Err(message)`, None → `Ok(())`). Callers in `worker.rs`, `async_delivery.rs`, `handle_message_send` compile unchanged and keep their behavior.

Run: `devenv shell -- cargo nextest run -p right-bot attachments` → PASS (new tests plus the existing classification/caption/partition suite). `devenv shell -- cargo check -p right-bot` → clean. Commit.

## Task 3 — Bot endpoint: ordered publication, first-failure stop, archive once

Files: `crates/bot/src/telegram/progress.rs`, `crates/bot/src/telegram/archive.rs`.

**Red endpoint tests** (extend the existing mock-Telegram tests in `progress.rs`; use `RightBot::new_for_test`, an allowlisted home-shaped agent dir, and a UDS archive mock as `channel_post_partial_publication_returns_200_with_message_id` does):

1. `channel_post_rejects_missing_content_and_attachments_before_claiming` — raw body with neither field returns 422 and leaves `channel_post_count == 0`. Preserve the existing whitespace-only-content rejection; attachments-only is valid.
2. `channel_post_delivers_content_before_attachments_and_archives_one_publication` — request one RichContent body, a single photo with caption, then a two-photo media group. Mock `sendRichMessage`, `sendPhoto`, and `sendMediaGroup`; capture request method order and Telegram receipts (e.g. ids `[101]`, `[102]`, `[103,104]`). Assert HTTP 200 `ok:true`, `message_ids == [101,102,103,104]`, `delivery_uncertain == false`; assert the archive UDS route is called exactly once, with `message_id: 104` and content fragments in delivery order: delivered body, caption, marker(s). Assert media group remained one `sendMediaGroup` call.
3. `channel_post_stops_at_first_certain_attachment_failure` — first attachment succeeds with id 201, second returns a typed 400, third endpoint would succeed; assert third is never called, response is HTTP 200 `ok:false`, ids `[201]`, uncertainty false, archive contains only the first attachment's confirmed marker/caption, and error says partial/no resend.
4. `channel_post_uncertain_attachment_failure_returns_ids_and_forbids_retry` — body succeeds (id 301), first attachment returns 500 or stalls to `TgError::Timeout`; assert exactly one attachment attempt, no subsequent attachment, HTTP 200 `ok:false`, ids `[301]`, `delivery_uncertain:true`, and error explicitly says Telegram may have delivered the failed request and **do not resend**. Archive only confirmed body; never manufacture an id or fragment for the ambiguous request.
5. `channel_post_zero_confirmed_uncertain_failure_is_typed_200` — attachments-only request whose first Telegram request is ambiguous: assert HTTP 200 typed `ok:false`, empty `message_ids`, `delivery_uncertain:true`, no archive call, and no retry. This differs deliberately from a certain zero-delivery failure, which remains an error HTTP status and `delivery_uncertain:false`.
6. `channel_post_archive_failure_returns_confirmed_ids_and_no_resend` — all Telegram sends return ids, archive UDS returns failure; assert HTTP 200 `ok:false`, all confirmed ids, uncertainty false, and error names archive failure + forbids resend.
7. Update existing `channel_post_partial_publication_returns_200_with_message_id` to `...message_ids`: with the new stop rule, first rich part succeeds and second fails; assert ids `[777]`, no later attempt, one archive row for delivered text. Update `channel_post_zero_delivery_stays_error_status` to assert empty ids + false uncertainty.

Run: `devenv shell -- cargo nextest run -p right-bot channel_post` → FAIL (request/response fields and handler behavior do not meet contract).

**Green code**:

- Add request-level `content.is_none() && attachments.is_empty()` validation before `claim_channel_post`; serde/RichContent still rejects present-but-empty/whitespace content. Keep token check, bot-side cap, and bot-side allowlist exactly where they are authoritative. Require `target.sandbox` only if attachments are present; return a certain `sandbox_unavailable` zero-delivery failure when none is available.
- In `handle_channel_post`, accumulate `message_ids` and `archive_fragments`. If content exists, call `send_rich_content` first. Change this channel path to stop on its first rich failure: use/introduce a stop-on-first-failure rich adapter rather than the current best-effort-to-end semantics; preserve `send_message`, worker, async-delivery callers through the existing rich adapter. Append each confirmed rich id and its delivered normalized text. If rich failed, skip all attachments.
- Only if rich completed, map DTOs with existing `message_dto_to_outbound` and call `send_attachments_reported(..., true)`. Extend ids/fragments, then stop/return on its first failure.
- Centralize response construction in a helper that always emits the new fields. Rules:
  - complete + ≥1 confirmed: HTTP 200, `ok:true`, all ids, false uncertainty;
  - certain failure + zero confirmed: error status (502/503 as appropriate), empty ids, false uncertainty;
  - any failure + confirmed ids: HTTP 200, `ok:false`, ids, classifier's uncertainty, `error` with unambiguous **do not resend** wording;
  - uncertain failure + zero confirmed: HTTP 200 `ok:false`, empty ids, true uncertainty, same no-resend wording (uncertainty itself makes retry unsafe).
- Build archive content by joining non-empty confirmed delivery fragments with `\n\n`. Captions remain regular strings (ADR 0004), not `RichContent`; markers use the existing `[kind]` / `[kind: filename]` convention. Never archive failed/uncertain pieces. If no id was confirmed, do not archive. Otherwise call `archive_outbound_channel_post` once, after Telegram delivery stops/completes, with the **last** confirmed id and the one logical publication content.
- `archive_outbound_channel_post` keeps its IPC interface and single-row behavior; update its parameter name/docs from body-only `content` to Channel Publication content. Do not alter schema or create one archive row per Telegram message.

Run: `devenv shell -- cargo nextest run -p right-bot channel_post` → PASS. Run `devenv shell -- cargo nextest run -p right-bot message_send async_delivery` to prove the legacy adapters kept existing behavior. Commit.

## Task 4 — Aggregator schema, validation, request mapping, typed results

Files: `crates/right/src/right_backend.rs`, `crates/right/src/right_backend_tests.rs`.

**Red tests**:

1. Extend `standalone_delivery_tools_expose_rich_content_not_text`: `channel_post` schema has optional `content` (`anyOf` includes `RichContent` and null) and `attachments` using the same `MessageAttachmentDto` definitions as `send_message`; no `text`; neither field individually required.
2. `channel_post_requires_content_or_attachments_before_allowlist_and_uds` — `{channel}` returns `empty_content`/`invalid_argument` before allowlist read/attempt claim/UDS. Cases: missing both, `content:null` + empty attachments. `content:null` + one valid attachment proceeds past this check.
3. `channel_post_rejects_attachment_outside_sandbox_outbox_before_uds` — `/tmp/a.png`, `/sandbox/outbox` (no trailing slash/file), and relative paths return `invalid_argument`; use/reuse `SANDBOX_OUTBOX_PREFIX` and the same validation helper as `SendMessageParams`, not a second convention. Valid `/sandbox/outbox/a.png` is accepted. Preserve every existing attachment kind, caption, filename, and media-group field.
4. Add a canned UDS response helper and tests:
   - success with ids `[11,12]` returns JSON `{"status":"sent","message_ids":[11,12]}`;
   - partial certain returns `channel_post_partially_sent`, error text includes every id and “do not resend”;
   - uncertain with ids (or empty ids) returns a distinct machine-checkable `channel_post_delivery_uncertain`, includes `delivery_uncertain:true`, every confirmed id, and forbids retry/resend;
   - archive failure (`ok:false`, ids present, uncertainty false) remains `channel_post_partially_sent` and forbids resend.

Run: `devenv shell -- cargo nextest run -p right channel_post standalone_delivery_tools` → FAIL.

**Green code**:

- `ChannelPostParams`: `content: Option<RichContent>` with serde default; add `attachments: Vec<MessageAttachmentDto>` with serde default. Reuse the existing send-message attachment schema/type and path validator; at least one required in `call_channel_post` because JSON Schema cannot express the cross-field non-empty invariant cleanly without diverging from existing generated schemas.
- Validate content only when present; reject when both absent/empty. Validate every path before aggregator allowlist lookup and before `begin_channel_post`, so invalid calls do not consume the cap. Keep aggregator `kind == Channel` allowlist preflight independently of the bot check.
- Build `ChannelPostRequest { content, attachments, .. }` without changing invocation/target authority.
- Consume `ChannelPostResponse.message_ids` and `delivery_uncertain`:
  - success exposes all ids;
  - uncertainty always maps to `channel_post_delivery_uncertain`, even with zero confirmed ids, and explicitly says the ambiguous request and confirmed ids must not be resent;
  - other `ok:false` with non-empty ids maps to `channel_post_partially_sent` with all ids/no-resend;
  - other zero-id failures map to `channel_post_failed`.
- Update tool description to “validated RichContent and/or attachments”, `/sandbox/outbox/`, read-first, foreground+cron, max 10. Remove the obsolete “arguments are channel and content” wording; retain “no text argument.”

Run: `devenv shell -- cargo nextest run -p right channel_post standalone_delivery_tools` → PASS. Commit.

## Task 5 — Guidance and architecture documentation

Files: `crates/right/src/aggregator.rs` (`RightBackend` server `with_instructions` string and its test), `crates/right-codegen/templates/right/prompt/OPERATING_INSTRUCTIONS.md`, `crates/right-codegen/src/agent_def_tests.rs`, `PROMPT_SYSTEM.md`, `docs/architecture/mcp.md`.

**Red tests**:

1. Extend `with_instructions_mentions_get_messages_by_id` (or rename to describe its inventory) to require channel guidance containing: RichContent and/or attachments; `/sandbox/outbox/`; foreground and cron; max 10; read before publishing; do not resend on partial/uncertain delivery. Keep untrusted-content guidance.
2. Add `operating_instructions_document_channel_post_attachments` in `agent_def_tests.rs`: assert the template says `channel_post(channel, content?, attachments?)`, at least one required, content delivered first, paths under `/sandbox/outbox/`, media groups supported, and partial/uncertain results must not be resent. Assert it still says captions are separate regular/Markdown strings (ADR 0004).

Run: `devenv shell -- cargo nextest run -p right with_instructions && devenv shell -- cargo nextest run -p right-codegen operating_instructions_document_channel_post_attachments` → FAIL.

**Green docs/code text**:

- `aggregator.rs::with_instructions`: describe the complete agent-facing contract without duplicating implementation details.
- `OPERATING_INSTRUCTIONS.md`: in “Sending Attachments,” teach channel posts can include body, attachments, or both; at least one; body first; outbox prefix; media-group semantics unchanged; stop at first failure; never resend a partial or delivery-uncertain publication. Keep caption/RichContent distinction and “read channel first.”
- `PROMPT_SYSTEM.md`: update only the existing `channel_post` contract paragraph (not the operating procedure boundary): optional body + all attachment kinds, order, first-failure, ids/uncertainty/no retry, one logical archive row, allowlist/cap/scope. Do not duplicate normal-turn how-to already in the template.
- `docs/architecture/mcp.md`: replace obsolete best-effort-to-end/single-id text with wire DTOs, internal report/adapters, ordered stop semantics, uncertain classification, typed HTTP-200 partial/uncertain shapes, no automatic retry, one Channel Publication archive under last confirmed id, captions/markers, media groups, independent allowlists, outbox/scope/cap. Explicitly state `send_message`, worker, and async delivery retain their legacy adapter behavior.
- Do not modify `ARCHITECTURE.md`; its `/sandbox/outbox/`, dual-allowlist, and RichContent/caption invariants do not change. Do not modify ADR 0004 or `CONTEXT.md`; they already define this decision.

Run: both red commands above → PASS. Then `devenv shell -- cargo nextest run -p right-codegen` → PASS. Commit.

## Task 6 — Mandatory Rust review, repairs, and final verification

No new feature work starts here.

1. Run focused integration gates:

   ```sh
   devenv shell -- cargo nextest run -p right-mcp channel_post
   devenv shell -- cargo nextest run -p right -E 'test(channel_post) | test(standalone_delivery_tools) | test(with_instructions)'
   devenv shell -- cargo nextest run -p right-bot -E 'test(channel_post) | test(attachments) | test(message_send) | test(async_delivery)'
   devenv shell -- cargo nextest run -p right-codegen
   ```

   Expected: all pass. These collectively prove DTO compatibility, schema/validation, both allowlist guards, content-before-attachment order, all attachment types through the reused mapper/sender, albums, first-failure stop, confirmed-id order, uncertainty/no-retry, single-row archive content/id, and legacy caller behavior.

2. Invoke the mandatory Rust review agent `review-rust-code` over the complete diff. Require explicit review of:
   - no error is merely logged and then treated as success (Rust FAIL FAST rule);
   - no allocation/copy added per send beyond the report data actually returned/archived;
   - receipt order for `sendMediaGroup` matches Telegram response order;
   - uncertain classification cannot label a network/timeout/429/5xx failure certain;
   - neither rich nor attachment channel delivery continues after the first terminal failure;
   - no ambiguous failure is retried (including HTML/plain fallback: only deterministic typed 400 formatting rejection may retry);
   - all four response states are lossless: complete, certain zero-delivery, partial confirmed, uncertain (with or without confirmed ids), plus archive-after-delivery failure;
   - legacy `send_attachments` callers still continue/report exactly as before;
   - archive contains only confirmed fragments, once, under the last confirmed id;
   - aggregator and bot independently enforce `kind == Channel`; `/sandbox/outbox/`, foreground+cron, and cap 10 remain intact;
   - captions remain regular paths and full media groups remain supported.

3. Convert every supported review finding into a concrete repair. Critical/warning findings must be fixed one at a time through `rust-coder`, with the narrow failing test added or strengthened first, then rerun the smallest focused command. Triage suggestions explicitly; do not batch unrelated fixes. Re-run `review-rust-code` on repaired areas until no critical/warning finding remains.

4. Run the required final gates, in this order:

   ```sh
   devenv shell -- cargo nextest run --workspace
   devenv shell -- cargo test --doc --workspace
   devenv shell -- cargo build --workspace
   ```

   All must pass. Fix any regression at its source and repeat the affected focused test, mandatory review if Rust behavior changed, then repeat all three final commands. Commit the completed clean cutover with no compatibility aliases, stale single-`message_id` channel response code, obsolete best-effort channel docs, TODOs, or production scaffolding.

## Edge cases and risk checklist

- Empty contract: missing/null content plus no attachments is invalid before either cap is consumed; attachment-only and content-only are valid.
- Ordering: RichContent receipts precede attachments; attachment partitioning preserves request/group-anchor order; group receipt ids remain Telegram response order.
- A media group is one Telegram request but may confirm multiple ids. If it errors ambiguously, none are confirmed without a receipt and the whole request is uncertain; do not infer partial album ids.
- Deterministic album incompatibility may degrade to singles through the existing pre-delivery fallback. Once singles begin, stop on the first failed single in channel mode.
- Caption formatting retries are allowed only for the existing known typed 400 pre-delivery cases. The plain retry's ambiguous failure is uncertain and terminal.
- A path/metadata/size/sandbox failure is certain and terminal; no Telegram request occurred for that item.
- On a confirmed prefix followed by failure, archive the confirmed prefix before replying. If archive then fails, return all ids and no-resend; never retry Telegram to compensate for archive loss.
- On uncertain zero-receipt failure, there is no archiveable confirmed row and no safe retry. The typed response must preserve this distinction even though `message_ids` is empty.
- Existing `send_message`, foreground worker replies, cron/background async delivery, and their retry/drop policies are out of product scope: adapters preserve them while the new report powers `channel_post`.
- `ARCHITECTURE.md`, accepted ADR 0004, and `CONTEXT.md` remain unchanged because their scope invariants already match the approved contract.
