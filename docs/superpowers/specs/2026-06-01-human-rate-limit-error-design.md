# Human-friendly CC error messages (rate-limit aware)

**Date:** 2026-06-01
**Status:** Design approved, ready for plan
**Scope:** `crates/bot/src/telegram/worker.rs` (one file)

## Problem

When a `claude -p` invocation exits non-zero, the bot dumps the raw,
truncated result JSON to the Telegram user. Real example:

```
⚠️ Agent error (exit 1):
(stderr empty, stdout): {"type":"result","subtype":"success","is_error":true,"api_error_status":429,"duration_ms":107621,...,"result":"API Error: Server is temporarily limiting requests (not your usage limit) · Rate limited","stop_reason":"stop_sequence","session_i
```

Two issues:

1. The message is raw JSON — unreadable for a user.
2. The HTTP-429 case (Anthropic temporarily throttling the shared
   account — *not* the user's usage) is indistinguishable from a genuine
   crash, and the bot still runs a reflection turn, which itself hits the
   same 429 and burns one more doomed request during the exact throttle
   window.

## Investigation result (the second ask: "are we hammering Claude?")

Read-only check of live agents `right` and `him` (`usage_events`,
stream logs, process-compose logs):

- **Not a request storm.** Steady state: `him` ~4 req/hr (only the
  `obsidian-agents-share-sync` cron every 15 min), `right` ~0/hr.
- The 429 is **upstream Anthropic account-level throttling** during a
  legitimate high-activity window (a live group conversation + heavy
  opus turns at ~1.8M cache-creation tokens / $10–12 each + a
  round-minute cron firing concurrently).
- **No retry/loop smell.** Each 429 turn triggers exactly one reflection
  turn, which also 429s, logs `WARN reflection failed`, and stops. No
  main-turn auto-retry, no reflection-on-reflection. `learning_skip`
  empty, 0 process restarts.

Contributing factor the platform already self-diagnoses: the
`obsidian-agents-share-sync` cron is on `*/15` (round minutes :00/:15/
:30/:45); `right_bot::cron` already emits a WARN advising an offset like
`:17`/`:43`. **Out of scope for this spec** — it is a config change, not
code. Tracked here as a follow-up only.

## Current code path (verified line numbers)

`crates/bot/src/telegram/worker.rs`:

- `format_error_reply(exit_code, stderr) -> String` (448) — wraps text in
  `⚠️ Agent error (exit N): <pre>…</pre>`.
- `is_auth_error(stdout) -> bool` (526) — parses the result JSON, checks
  `is_error == true` and `result` against auth patterns. Used at 3833.
- `exit_code != 0` block (3818–3984): logs full output (3820, keeps raw
  JSON in host logs), auth branch (3833), then generic branch — builds
  `error_detail = "(stderr empty, stdout): <raw 500-char JSON>"` (3969),
  wraps via `format_error_reply`, returns
  `InvokeCcFailure::Reflectable { … raw_message … }` (3978).
- `will_reflect = exit_code != 0 && !is_auth_error(stdout)` (3729) — its
  only effect is to hand the "thinking…" message off to `spawn_worker`
  instead of finalizing it inline.
- `spawn_worker` (1739+): `NonReflectable` → `send_error_to_telegram`
  (new message, does **not** touch the thinking message). `Reflectable`
  → edit thinking msg into a banner, run `reflect_on_failure`; on
  reflection success send its reply, on reflection failure show
  `raw_message` by editing the banner (1845+).
- `InvokeCcFailure` enum (2406): `Reflectable`, `NonReflectable`,
  `Backgrounded`.

`api_error_status` is currently parsed nowhere.

## Design

All changes are in `crates/bot/src/telegram/worker.rs`. Host logging at
3820 is unchanged — the raw JSON stays in logs for debuggability; only
the Telegram-facing text is humanized.

### 1. Single classifier (replaces duplicated JSON parsing)

```rust
/// Classification of a `claude -p` result JSON for a non-zero exit.
#[derive(Debug, PartialEq)]
pub(crate) enum CcResultClass {
    /// Authentication failure (existing 401/403/not-logged-in patterns).
    Auth,
    /// Anthropic transient throttle/overload — not the user's usage.
    RateLimited,
    /// Any other reported error; `result_text` is the human-readable
    /// `result` field when present.
    Other { result_text: Option<String> },
    /// JSON did not parse, or `is_error` is not true.
    NotError,
}

pub(crate) fn classify_cc_result(stdout: &str) -> CcResultClass;
```

Detection rules (parse the JSON once):

- Not parseable, or `is_error != true` → `NotError`.
- `result` matches existing auth patterns → `Auth`.
- `api_error_status` ∈ {429, 529} **OR** `result` contains any of
  `"Rate limited"`, `"temporarily limiting"`, `"Overloaded"` → `RateLimited`.
  (String match is a belt-and-suspenders fallback because
  `api_error_status` is absent in some result shapes.)
- Otherwise → `Other { result_text }`, where `result_text` is the
  trimmed `result` string if non-empty.

Order matters: auth is checked before rate-limit. They are mutually
exclusive by `api_error_status` (403/401 vs 429/529), so ordering is
safe; auth-first preserves the existing login-flow trigger.

`is_auth_error` becomes a thin wrapper:
`matches!(classify_cc_result(stdout), CcResultClass::Auth)`. Its public
signature and its callsite at 3833 are unchanged.

### 2. New `InvokeCcFailure::RateLimited` variant

```rust
/// Anthropic-side rate limit / overload. No reflection (it would just
/// 429 again and add load during the throttle window). `spawn_worker`
/// edits `thinking_msg_id` into `message`, or sends it new if absent.
RateLimited {
    message: String,
    thinking_msg_id: Option<teloxide::types::MessageId>,
},
```

A dedicated variant (rather than reusing `NonReflectable`) is required
because `NonReflectable` sends a *new* message and never edits the live
"thinking…" message, which would leave it dangling with its stop
keyboard. This variant keeps the thinking-message lifecycle owned by
`spawn_worker`, consistent with `Reflectable`/`Backgrounded`.

### 3. Rate-limit branch in the `exit_code != 0` block

After the existing auth branch (3833) and before the generic branch
(3960+), call `classify_cc_result(&stdout_str)`:

- `CcResultClass::RateLimited` → return `InvokeCcFailure::RateLimited`
  with the canned copy (below) and `thinking_msg_id`. No reflection.
- Otherwise fall through to the generic branch.

`will_reflect` (3729) already evaluates `true` for this case
(`exit_code != 0 && !is_auth_error`), so the thinking message is handed
to `spawn_worker` rather than finalized inline — exactly what the new
variant needs. **No change to `will_reflect`.**

### 4. Generic human fallback (the "B" scope)

In the generic `Reflectable` branch, build `raw_message` from the parsed
`result_text` when available:

- `Other { result_text: Some(text) }` → human copy (below) instead of the
  raw-JSON dump.
- `result_text` empty / `NotError` / unparseable → keep current
  `format_error_reply(exit_code, "(stderr empty, stdout): <raw>")`, so
  genuinely opaque crashes stay debuggable in the Telegram message too.

Reflection still runs for non-rate-limit reflectable failures; this
`raw_message` is only shown when reflection itself fails (1845+ path).

### 5. `spawn_worker` handler for `RateLimited`

Add a match arm mirroring the reflection-failure edit/fallback logic
(1845+): if `thinking_msg_id` is `Some`, edit it into `message` (HTML
parse mode, clear the inline keyboard); on edit failure delete it and
`send_error_to_telegram`. If `None`, `send_error_to_telegram` directly.

### Copy (English, platform level)

Rate-limit / overload (429/529):

> ⚠️ Claude's servers are briefly overloaded and limited this request. It's temporary and not about your account or usage — try again in a moment.

Generic `is_error` human fallback:

> ⚠️ The agent hit an error and couldn't finish: \<result text\>. Try again, or rephrase if it repeats.

Both are produced by small formatter helpers (constants/`format!`) so they
are unit-testable and HTML-safe (the `<result text>` interpolation must be
`html_escape`d, matching `format_error_reply`).

## Testing (TDD)

Pure unit tests in the existing `#[cfg(test)]` module, matching the style
of the existing `is_auth_error` / `format_error_reply` tests:

`classify_cc_result`:
- `api_error_status: 429` + `is_error: true` → `RateLimited`.
- `api_error_status: 529` → `RateLimited`.
- `result` contains "Rate limited", no `api_error_status` → `RateLimited`.
- `result` contains "Overloaded" → `RateLimited`.
- 403 auth JSON → `Auth` (not misclassified as rate-limit).
- ordinary error (`is_error: true`, plain `result`) → `Other` with
  `result_text` extracted.
- non-JSON input → `NotError`.
- `is_error: false` → `NotError`.

`is_auth_error` existing tests must still pass (wrapper unchanged).

Formatter helpers:
- rate-limit copy is the exact approved string.
- generic human-fallback copy interpolates and HTML-escapes the result
  text.

Telegram-send branches in `spawn_worker` are not unit-tested (no bot
mock exists; consistent with current coverage).

**Verification cadence:** TDD red/green on the new tests, then
`devenv shell -- cargo test -p bot` for the targeted suite, and
`devenv shell -- cargo test --workspace` as the final mandatory check.

## Out of scope

- Moving `obsidian-agents-share-sync` off `*/15` round minutes (config
  change; follow-up, not code).
- Any global outbound rate limiter / semaphore on Claude calls — the
  investigation shows no storm, so this is unwarranted (YAGNI).
- Auth flow, budget-exceeded, max-turns, timeout messaging — already
  handled or rare.
