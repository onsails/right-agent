# Design: "Show details" button on prettified error messages

**Date:** 2026-06-01
**Status:** Approved (design)

## Problem

Recent work (commits `8127a72d`..`98f9acf3`) replaced raw `claude -p` error
dumps with human-friendly Telegram copy: a fixed rate-limit/overload notice,
a `format_human_error` fallback for other CC errors, and reflection replies.
The prettified copy hides the underlying JSON, which is exactly what is needed
to debug a failure. We want to keep the friendly message but attach a button
that reveals the raw error JSON on demand.

The exact message that motivated this is `RATE_LIMIT_MESSAGE`
(`crates/bot/src/telegram/worker.rs:463`):

> ⚠️ Claude's servers are briefly overloaded and limited this request. It's
> temporary and not about your account or usage — try again in a moment.

## Goal

Every prettified error surface carries a `🔍 Details` inline button. Pressing
it replies with the raw `claude -p` JSON (`stdout_str`) that we classified at
failure time. The detail survives a bot restart (persisted in `data.db`) and
expires after 7 days.

Non-goals: surfacing details for non-CC errors that never had a raw JSON
(e.g. internal parse failures with no stored payload — these simply get no
button); any operator/dashboard UI; editing the friendly message in place.

## Prettified error surfaces (where the button attaches)

All live in `crates/bot/src/telegram/worker.rs`. Classification is
`classify_cc_result` → `CcResultClass` (`worker.rs:541`). The worker loop
(`worker.rs:~1790`) matches `InvokeCcFailure` variants:

| Surface | Current behavior | Button attaches to |
|---|---|---|
| `RateLimited` (429/529) | edits `thinking_msg` to `RATE_LIMIT_MESSAGE` with empty keyboard; or new message | the edited `thinking_msg` (or the new message) |
| `Reflectable` → reflection **succeeds** | sends reflection reply (1+ HTML parts) | the **last** reply part |
| `Reflectable` → reflection **fails** | shows `raw_message` (edit or new message) | that message |
| `NonReflectable` / `Other` with stored JSON | `send_error_to_telegram` | that message |

When there is no stored JSON (e.g. internal parse-failure replies built without
a CC `stdout_str`), `details_id` is `None` and no button is rendered.

## Storage

New per-agent SQLite table, owned by `right_db::migrations::MIGRATIONS` (the
sole place new tables are added — see ARCHITECTURE "Migration Ownership").
New SQL file `crates/right-db/src/sql/v39_error_details.sql` (next free version
after `v38`) plus a registry entry. `CREATE TABLE IF NOT EXISTS` is naturally
idempotent.

```sql
CREATE TABLE IF NOT EXISTS error_details (
  id         INTEGER PRIMARY KEY AUTOINCREMENT,
  chat_id    INTEGER NOT NULL,
  thread_id  INTEGER NOT NULL,
  raw_json   TEXT    NOT NULL,   -- the exact stdout_str we classified
  created_at INTEGER NOT NULL    -- unix seconds
);
CREATE INDEX IF NOT EXISTS idx_error_details_created_at
  ON error_details (created_at);
```

`data.db` is per-agent, so no `agent` column. `id` is a small integer — the
callback payload `errdet:<id>` stays far under Telegram's 64-byte
`callback_data` limit (UUIDs unnecessary).

### TTL

`const ERROR_DETAILS_TTL_DAYS: i64 = 7;` Cleanup is opportunistic on insert:
`DELETE FROM error_details WHERE created_at < (now - 7d)`. No background job.

## Write path

In `invoke_cc` (`worker.rs`), where both `conn` and `stdout_str` are in scope,
each prettified `return Err(InvokeCcFailure::…)` first stores the raw JSON and
carries the resulting row id.

- New DB helper (in `right-db`, alongside other per-agent query helpers):
  `insert_error_detail(conn, chat_id, thread_id, raw_json, now) -> Result<i64>`.
  Insert + TTL delete = 2 writes → **single immediate transaction**
  (`Connection::transaction().await` … `commit().await`), per the Transaction
  Rule.
- Call site wraps it **best-effort**: on `Err`, log via
  `tracing::error!("store error_details failed: {:#}", e)` and proceed with
  `details_id = None`. Rationale: delivering the user-facing error message is
  the primary obligation; losing the debug affordance must not abort it. This
  mirrors the existing logged-and-continued `touch_session` site
  (`worker.rs:4122`). This is the one sanctioned non-propagating site and is
  documented as such (not a swallow — the failure is logged and the degraded
  state is explicit: no button).
- `raw_json` content = the raw `stdout_str` when non-empty (the JSON we
  classified); otherwise the `stderr`-derived `error_detail` already computed
  in the `_ =>` arm at `worker.rs:4081`. Store whatever raw text the friendly
  message was derived from.

### Threading the id

Add `details_id: Option<i64>` to the `InvokeCcFailure` variants that carry a
prettified surface: `RateLimited`, `Reflectable`, and
`NonReflectable`/`Other`-derived sends. The worker reads it and renders the
keyboard at each send/edit site.

## Button rendering

A small helper builds the markup:

```rust
fn details_keyboard(details_id: Option<i64>) -> InlineKeyboardMarkup {
    match details_id {
        Some(id) => InlineKeyboardMarkup::new(vec![vec![
            InlineKeyboardButton::callback("🔍 Details", format!("errdet:{id}")),
        ]]),
        None => InlineKeyboardMarkup::default(),
    }
}
```

- `send_error_to_telegram` (`worker.rs:4153`) gains an optional
  `reply_markup: InlineKeyboardMarkup` parameter (default empty) threaded
  through both the HTML send and the plain-text fallback.
- Rate-limit edit and reflection-failure edit replace
  `InlineKeyboardMarkup::default()` with `details_keyboard(details_id)`.
- Reflection-success: attach the keyboard to the **last** part's send only.

## Read path (callback `errdet:<id>`)

New dispatcher branch in `crates/bot/src/telegram/dispatch.rs:593`, mirroring
the `model:` / `think:` / `bg:` branches:

```rust
.branch(
    dptree::filter(|q: CallbackQuery| {
        q.data.as_deref().is_some_and(|d| d.starts_with("errdet:"))
    })
    .endpoint(handle_error_details_callback),
)
```

New handler `handle_error_details_callback(bot, q, agent_dir)` in
`crates/bot/src/telegram/handler.rs` (mirrors `handle_bg_callback`):

1. Parse `errdet:<id>` → `i64`. Malformed → `answer_callback_query` with a
   short alert, return.
2. Resolve `data.db` path from `agent_dir`; open read connection
   (`right_db::open_connection(path, migrate: false)` — runtime opens never
   migrate).
3. `SELECT raw_json, chat_id FROM error_details WHERE id = ?`.
4. **Scope check:** the row's `chat_id` MUST equal `q.message`'s chat id.
   Mismatch (or row absent / expired) → treat as not found. Prevents a user in
   one chat of this agent from enumerating ids to read another chat's errors.
5. Found → reply (as a reply to the button's message) with the JSON in a
   `<pre>` block, HTML-escaped. If escaped-JSON + `<pre>` tags would exceed
   Telegram's 4096-char message limit → `send_document` a `error.json` file
   (raw bytes, `InputFile::memory`). Threading: preserve the message's topic
   thread id. Then `answer_callback_query` (brief ack).
6. Not found / expired / scope mismatch →
   `answer_callback_query.text("Details no longer available.").show_alert(true)`.
   The button stays; a later press just re-attempts (and re-sends if the row
   still exists).

The pre-vs-file decision and keyboard construction are pure functions,
unit-tested directly; the Telegram send itself is not mocked.

## Copy

- Button label: `🔍 Details`
- Unavailable alert: `Details no longer available.`

English, matching existing product copy (RATE_LIMIT_MESSAGE et al.).

## Testing

Targeted, per project cadence:

- **Migration idempotency:** re-running the migration over an existing
  `error_details` table is a no-op (covered by the migration runner's
  idempotency contract).
- **DB round-trip:** `insert_error_detail` then select returns the same
  `raw_json`; a row older than 7 days is deleted by the next insert's TTL
  sweep; a fresh row survives.
- **callback parsing/scope (pure):** `errdet:42` → `Some(42)`; `errdet:` /
  `errdet:abc` → rejected; row with non-matching `chat_id` → not revealed.
- **pre-vs-file decision (pure):** payload under limit → `<pre>` path; over
  limit → document path; HTML escaping applied.
- **keyboard builder (pure):** `Some(id)` → one `errdet:<id>` button;
  `None` → empty markup.
- Existing `classify_cc_result` tests unchanged.

Final: `devenv shell -- cargo test --workspace` (mandatory before completion).

## Files touched

- `crates/right-db/src/sql/v39_error_details.sql` (new) + `migrations.rs`
  registry entry.
- `crates/right-db/src/…` — `insert_error_detail` / `get_error_detail`
  helpers (single immediate transaction for insert+TTL).
- `crates/bot/src/telegram/worker.rs` — store on failure, add
  `details_id` to `InvokeCcFailure` variants, `details_keyboard` helper,
  `send_error_to_telegram` markup param, attach at each send/edit site.
- `crates/bot/src/telegram/handler.rs` — `handle_error_details_callback`.
- `crates/bot/src/telegram/dispatch.rs` — `errdet:` callback branch.

## ARCHITECTURE / docs impact

No new contract or invariant. New table is registered through the existing
migration registry; no ARCHITECTURE.md change required. No PROMPT_SYSTEM.md
impact (no prompt/tooling change). Telegram UX rule (escape untrusted text
before `ParseMode::Html`) is honored on the `<pre>` path.
