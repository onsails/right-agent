# Human-friendly CC error messages (rate-limit aware) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the raw-JSON error dump shown to Telegram users with human-friendly text, and special-case Anthropic rate-limit/overload (HTTP 429/529) so it shows a reassuring notice and skips the doomed reflection turn.

**Architecture:** All changes live in `crates/bot/src/telegram/worker.rs`. A single pure classifier (`classify_cc_result`) parses the `claude -p` result JSON once and returns `Auth | RateLimited | Other{result_text} | NotError`. The non-zero-exit path uses it to (a) return a new `InvokeCcFailure::RateLimited` variant — no reflection — for 429/529, and (b) build a human-readable fallback message from the `result` field for other errors. Host logging (the full raw JSON at the `tracing::error!("claude -p failed")` site) is untouched for debuggability.

**Tech Stack:** Rust (edition 2024), `serde_json`, `teloxide`, tokio. Crate: `right-bot`.

---

## Background for the implementer

- The bot wraps `claude -p` subprocesses. On non-zero exit, `invoke_cc`
  (in `crates/bot/src/telegram/worker.rs`) currently dumps the truncated
  raw result JSON to the user via `format_error_reply` and returns
  `InvokeCcFailure::Reflectable`, which makes `spawn_worker` run a
  reflection turn.
- For an HTTP-429 ("Server is temporarily limiting requests · Rate
  limited") the reflection turn also 429s, so it wastes one more request
  during the throttle window and then shows the raw JSON anyway.
- Existing helpers to mirror in style:
  - `format_error_reply(exit_code, stderr) -> String` (worker.rs:448) —
    `⚠️ Agent error (exit N): <pre>…</pre>`.
  - `is_auth_error(stdout) -> bool` (worker.rs:526) — parses JSON, checks
    `is_error == true` + auth patterns. Callsites: `will_reflect`
    (worker.rs:3729) and the non-zero-exit block (worker.rs:3833).
  - `send_error_to_telegram(ctx, chat_id, eff_thread_id, message)`
    (worker.rs:4038) — HTML send with plain-text fallback.
  - `html_escape` and `strip_html_tags` are already imported in worker.rs.
- `InvokeCcFailure` enum: worker.rs:2406 (`Reflectable`, `NonReflectable`,
  `Backgrounded`).
- `spawn_worker` failure-handling match: worker.rs:1739+ (`NonReflectable`
  at 1739, `Reflectable` at 1743, reflection-failure fallback at ~1845,
  `Backgrounded` at 1889).
- Tests module: `#[cfg(test)] mod tests` at worker.rs:4078; existing
  `is_auth_error` / `format_error_reply` tests use `#[tokio::test] async fn`.
- The warning-sign convention in this file is the escape `\u{26a0}\u{fe0f}`;
  literal `—` and `…` are already used in banner strings, so literals are
  fine.

**Baseline check before starting** (record any pre-existing failures):

Run: `devenv shell -- cargo test -p right-bot`
Expected: builds; note any already-failing tests so they are not blamed
on this change.

---

## Task 1: `classify_cc_result` classifier + `CcResultClass` enum

**Files:**
- Modify: `crates/bot/src/telegram/worker.rs` (add enum + fn near
  `is_auth_error` at ~526; refactor `is_auth_error` to delegate)
- Test: `crates/bot/src/telegram/worker.rs` (tests module at 4078)

- [ ] **Step 1: Write the failing tests**

Add to the `#[cfg(test)] mod tests` block (worker.rs:4078), near the
existing `is_auth_error` tests:

```rust
#[tokio::test]
async fn classify_detects_429_rate_limit() {
    let stdout = r#"{"type":"result","is_error":true,"api_error_status":429,"result":"API Error: Server is temporarily limiting requests (not your usage limit) · Rate limited"}"#;
    assert_eq!(classify_cc_result(stdout), CcResultClass::RateLimited);
}

#[tokio::test]
async fn classify_detects_529_overloaded_status() {
    let stdout = r#"{"is_error":true,"api_error_status":529,"result":"API Error: Overloaded"}"#;
    assert_eq!(classify_cc_result(stdout), CcResultClass::RateLimited);
}

#[tokio::test]
async fn classify_detects_rate_limit_string_without_status() {
    let stdout = r#"{"is_error":true,"result":"Something · Rate limited"}"#;
    assert_eq!(classify_cc_result(stdout), CcResultClass::RateLimited);
}

#[tokio::test]
async fn classify_detects_overloaded_string() {
    let stdout = r#"{"is_error":true,"result":"API Error: Overloaded, retry later"}"#;
    assert_eq!(classify_cc_result(stdout), CcResultClass::RateLimited);
}

#[tokio::test]
async fn classify_403_is_auth_not_rate_limit() {
    let stdout = r#"{"is_error":true,"result":"API Error: 403 Forbidden"}"#;
    assert_eq!(classify_cc_result(stdout), CcResultClass::Auth);
}

#[tokio::test]
async fn classify_ordinary_error_extracts_result_text() {
    let stdout = r#"{"is_error":true,"result":"Tool execution failed"}"#;
    assert_eq!(
        classify_cc_result(stdout),
        CcResultClass::Other {
            result_text: Some("Tool execution failed".to_string())
        }
    );
}

#[tokio::test]
async fn classify_non_json_is_not_error() {
    assert_eq!(classify_cc_result("not json at all"), CcResultClass::NotError);
}

#[tokio::test]
async fn classify_is_error_false_is_not_error() {
    let stdout = r#"{"is_error":false,"result":"ok"}"#;
    assert_eq!(classify_cc_result(stdout), CcResultClass::NotError);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `devenv shell -- cargo test -p right-bot classify_`
Expected: compile error — `CcResultClass` / `classify_cc_result` not found.

- [ ] **Step 3: Add the enum, classifier, and refactor `is_auth_error`**

Add immediately above `is_auth_error` (worker.rs:~525). Move the existing
local `AUTH_PATTERNS` const out of `is_auth_error` into the classifier:

```rust
/// Classification of a `claude -p` result JSON on a non-zero exit.
#[derive(Debug, PartialEq)]
pub(crate) enum CcResultClass {
    /// Authentication failure (401/403/not-logged-in patterns).
    Auth,
    /// Anthropic transient throttle/overload — not the user's usage.
    RateLimited,
    /// Any other reported error; `result_text` is the trimmed `result`
    /// field when non-empty.
    Other { result_text: Option<String> },
    /// JSON did not parse, or `is_error` is not `true`.
    NotError,
}

const AUTH_PATTERNS: &[&str] = &[
    "API Error: 403",
    "API Error: 401",
    "Failed to authenticate",
    "Not logged in",
    "Please run /login",
];

const RATE_LIMIT_PATTERNS: &[&str] = &["Rate limited", "temporarily limiting", "Overloaded"];

/// Parse the CC result JSON once and classify the failure. Auth is checked
/// before rate-limit; the two are mutually exclusive by `api_error_status`
/// (401/403 vs 429/529), and auth-first preserves the login-flow trigger.
pub(crate) fn classify_cc_result(stdout: &str) -> CcResultClass {
    let parsed: serde_json::Value = match serde_json::from_str(stdout) {
        Ok(v) => v,
        Err(_) => return CcResultClass::NotError,
    };
    let is_error = parsed
        .get("is_error")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    if !is_error {
        return CcResultClass::NotError;
    }
    let result = parsed.get("result").and_then(|v| v.as_str()).unwrap_or("");

    if AUTH_PATTERNS.iter().any(|p| result.contains(p)) {
        return CcResultClass::Auth;
    }

    let status = parsed.get("api_error_status").and_then(|v| v.as_u64());
    let rate_limited = matches!(status, Some(429) | Some(529))
        || RATE_LIMIT_PATTERNS.iter().any(|p| result.contains(p));
    if rate_limited {
        return CcResultClass::RateLimited;
    }

    let trimmed = result.trim();
    let result_text = if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    };
    CcResultClass::Other { result_text }
}
```

Then replace the body of `is_auth_error` (worker.rs:526) — keep the
public signature and doc comment, delete its local `AUTH_PATTERNS` and
parsing:

```rust
pub fn is_auth_error(stdout: &str) -> bool {
    matches!(classify_cc_result(stdout), CcResultClass::Auth)
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `devenv shell -- cargo test -p right-bot classify_ is_auth_error_`
Expected: PASS — all `classify_*` and existing `is_auth_error_*` tests
green (the existing auth tests confirm the refactor preserved behavior).

- [ ] **Step 5: Commit**

```bash
git add crates/bot/src/telegram/worker.rs
git commit -m "feat(bot): classify CC result errors (auth/rate-limit/other)"
```

---

## Task 2: Human-message formatters

**Files:**
- Modify: `crates/bot/src/telegram/worker.rs` (add const + fn near
  `format_error_reply` at ~448)
- Test: `crates/bot/src/telegram/worker.rs` (tests module at 4078)

- [ ] **Step 1: Write the failing tests**

Add near the existing `format_error_reply` tests in the tests module:

```rust
#[tokio::test]
async fn rate_limit_message_is_reassuring_and_not_about_usage() {
    assert!(RATE_LIMIT_MESSAGE.contains("not about your account or usage"));
    assert!(RATE_LIMIT_MESSAGE.contains("try again"));
    assert!(RATE_LIMIT_MESSAGE.starts_with('\u{26a0}'));
}

#[tokio::test]
async fn human_error_interpolates_and_escapes_result_text() {
    let msg = format_human_error("boom <x> & y");
    assert!(msg.contains("couldn't finish: boom &lt;x&gt; &amp; y."));
    assert!(msg.contains("Try again, or rephrase if it repeats."));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `devenv shell -- cargo test -p right-bot rate_limit_message human_error_`
Expected: compile error — `RATE_LIMIT_MESSAGE` / `format_human_error` not found.

- [ ] **Step 3: Add the const and formatter**

Add immediately below `format_error_reply` (worker.rs:~459):

```rust
/// User-facing notice for an Anthropic-side rate limit / overload
/// (HTTP 429/529). Reassures the user it is transient and account-neutral.
pub(crate) const RATE_LIMIT_MESSAGE: &str = "\u{26a0}\u{fe0f} Claude's servers are briefly overloaded and limited this request. It's temporary and not about your account or usage — try again in a moment.";

/// Human-readable error notice built from the CC `result` text, for the
/// generic (non-auth, non-rate-limit) failure fallback. `result_text` is
/// HTML-escaped because the reply is sent with `ParseMode::Html`.
pub(crate) fn format_human_error(result_text: &str) -> String {
    format!(
        "\u{26a0}\u{fe0f} The agent hit an error and couldn't finish: {}. Try again, or rephrase if it repeats.",
        html_escape(result_text)
    )
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `devenv shell -- cargo test -p right-bot rate_limit_message human_error_`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/bot/src/telegram/worker.rs
git commit -m "feat(bot): human-friendly rate-limit and generic error copy"
```

---

## Task 3: `InvokeCcFailure::RateLimited` variant + wire the non-zero-exit branch

**Files:**
- Modify: `crates/bot/src/telegram/worker.rs` (enum at 2406; non-zero-exit
  block at ~3960–3984)

No new unit test — this is wiring of the Task 1/2 helpers, whose logic is
already covered. Correctness is confirmed by compile + the final
workspace test. (The Telegram-send side is handled in Task 4; no bot mock
exists, consistent with current coverage.)

- [ ] **Step 1: Add the enum variant**

In `InvokeCcFailure` (worker.rs:2406), after the `NonReflectable` variant:

```rust
    /// Anthropic-side rate limit / overload (HTTP 429/529). Reflection is
    /// skipped — it would just 429 again and add load during the throttle
    /// window. `spawn_worker` edits `thinking_msg_id` into `message`, or
    /// sends it as a new message when there is no thinking message.
    RateLimited {
        message: String,
        thinking_msg_id: Option<teloxide::types::MessageId>,
    },
```

- [ ] **Step 2: Wire the non-zero-exit branch**

In the `exit_code != 0` block, locate the generic-error tail (worker.rs
~3966–3984): the `let error_detail = …` / `let raw = format_error_reply…`
/ `return Err(InvokeCcFailure::Reflectable { … })`. It sits AFTER the
`if is_first_call { deactivate_current … }` block. Replace that tail with:

```rust
        // Classify the failure (auth was already handled above). Rate-limit
        // gets a human notice and skips reflection; other errors keep
        // reflection but use a human-readable fallback message.
        let cc_class = classify_cc_result(&stdout_str);
        if matches!(cc_class, CcResultClass::RateLimited) {
            return Err(InvokeCcFailure::RateLimited {
                message: RATE_LIMIT_MESSAGE.to_string(),
                thinking_msg_id,
            });
        }

        let raw = match &cc_class {
            CcResultClass::Other {
                result_text: Some(text),
            } => format_human_error(text),
            _ => {
                let error_detail =
                    if stderr_str.trim().is_empty() && !stdout_str.trim().is_empty() {
                        format!(
                            "(stderr empty, stdout): {}",
                            stdout_str.chars().take(500).collect::<String>()
                        )
                    } else {
                        stderr_str.to_string()
                    };
                format_error_reply(exit_code, &error_detail)
            }
        };
        return Err(InvokeCcFailure::Reflectable {
            kind: FailureKind::NonZeroExit { code: exit_code },
            ring_buffer_tail: ring_buffer.events().clone(),
            session_uuid: session_uuid.clone(),
            raw_message: raw,
            thinking_msg_id,
        });
```

Note: the rate-limit branch is placed AFTER `if is_first_call { … }` so a
first-call 429 still deactivates the never-created session. `will_reflect`
(worker.rs:3729) already evaluates `true` here (`exit_code != 0 &&
!is_auth_error`), so the "thinking…" message is handed to `spawn_worker`
rather than finalized inline — exactly what the new variant needs. Do not
change `will_reflect`.

- [ ] **Step 3: Verify it compiles (expect a non-exhaustive-match error)**

Run: `devenv shell -- cargo check -p right-bot`
Expected: FAIL — `spawn_worker`'s `match` on `InvokeCcFailure` is now
non-exhaustive (missing `RateLimited`). This is fixed in Task 4. (If you
prefer a clean checkpoint, do Task 4 before compiling.)

- [ ] **Step 4: Commit (after Task 4 compiles — see Task 4 Step 4)**

Combined commit happens in Task 4 so the tree never has a non-compiling
commit. Proceed directly to Task 4.

---

## Task 4: `spawn_worker` handler for `RateLimited`

**Files:**
- Modify: `crates/bot/src/telegram/worker.rs` (spawn_worker match at
  ~1739–1888)

- [ ] **Step 1: Add the match arm**

In `spawn_worker`, add this arm immediately after the
`Err(InvokeCcFailure::NonReflectable { message }) => { … }` arm
(worker.rs:1739) and before the `Reflectable` arm:

```rust
                Err(InvokeCcFailure::RateLimited {
                    message,
                    thinking_msg_id,
                }) => {
                    tracing::info!(
                        ?key,
                        "rate-limited turn — sending human notice, skipping reflection"
                    );
                    match thinking_msg_id {
                        Some(msg_id) => {
                            // Edit the live "thinking…" message into the notice and
                            // clear the stop keyboard. Mirror the reflection-failure
                            // fallback: on edit failure, delete and send anew.
                            let edit_result = ctx
                                .bot
                                .edit_message_text(tg_chat_id, msg_id, &message)
                                .parse_mode(teloxide::types::ParseMode::Html)
                                .reply_markup(teloxide::types::InlineKeyboardMarkup::default())
                                .await;
                            if let Err(edit_err) = edit_result {
                                tracing::warn!(
                                    ?key,
                                    "rate-limit banner edit failed ({:#}); sending as new message",
                                    edit_err
                                );
                                let _ = ctx.bot.delete_message(tg_chat_id, msg_id).await;
                                send_error_to_telegram(
                                    &ctx,
                                    tg_chat_id,
                                    eff_thread_id,
                                    &message,
                                )
                                .await;
                            }
                        }
                        None => {
                            send_error_to_telegram(&ctx, tg_chat_id, eff_thread_id, &message)
                                .await;
                        }
                    }
                }
```

- [ ] **Step 2: Verify it compiles**

Run: `devenv shell -- cargo check -p right-bot`
Expected: PASS — match is now exhaustive.

- [ ] **Step 3: Run the targeted test suite**

Run: `devenv shell -- cargo test -p right-bot`
Expected: PASS — all classifier/formatter tests plus the existing suite.
Compare against the baseline; only pre-existing failures (if any) remain.

- [ ] **Step 4: Commit Tasks 3+4 together**

```bash
git add crates/bot/src/telegram/worker.rs
git commit -m "feat(bot): skip reflection on rate-limit, show human notice"
```

---

## Task 5: Final workspace verification

- [ ] **Step 1: Full workspace test (mandatory)**

Run: `devenv shell -- cargo test --workspace`
Expected: PASS (modulo any pre-existing failures recorded at baseline;
the two known flaky tests in `cc/invocation` pid-race and dashboard
warn-count may need an isolated re-run before blaming this change).

- [ ] **Step 2: Clippy + build**

Run: `devenv shell -- cargo clippy -p right-bot && devenv shell -- cargo build --workspace`
Expected: no warnings introduced by these changes; debug build succeeds.

---

## Self-Review (completed)

**Spec coverage:**
- Classifier `classify_cc_result` (spec §1) → Task 1.
- `RateLimited` variant (spec §2) → Task 3 Step 1.
- Rate-limit branch, order after auth + is_first_call (spec §3) → Task 3 Step 2.
- Generic human fallback from `result_text` (spec §4) → Task 3 Step 2 (`format_human_error`, Task 2).
- `spawn_worker` handler mirroring reflection-failure edit/fallback (spec §5) → Task 4.
- Copy strings (spec "Copy") → Task 2.
- Host logging untouched → no task modifies the `tracing::error!("claude -p failed")` site at worker.rs:3820.
- Tests (spec "Testing") → Task 1 Step 1, Task 2 Step 1; final workspace test → Task 5.
- Out of scope (cron offset, global rate limiter) → not in any task, as intended.

**Placeholder scan:** none — all steps contain concrete code and commands.

**Type consistency:** `CcResultClass` { `Auth`, `RateLimited`, `Other { result_text: Option<String> }`, `NotError` }, `classify_cc_result`, `RATE_LIMIT_MESSAGE`, `format_human_error`, and `InvokeCcFailure::RateLimited { message, thinking_msg_id }` are used identically across Tasks 1–4. `FailureKind::NonZeroExit { code }` matches reflection.rs:33.
