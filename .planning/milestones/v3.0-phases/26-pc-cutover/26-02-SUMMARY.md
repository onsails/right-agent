---
phase: 26-pc-cutover
plan: "02"
subsystem: bot, doctor
tags: [telegram, webhook, long-polling, doctor, PC-04, PC-05]
dependency_graph:
  requires: []
  provides: [deleteWebhook-on-startup, doctor-webhook-check]
  affects: [crates/bot/src/lib.rs, crates/rightclaw/src/doctor.rs]
tech_stack:
  added: []
  patterns: [teloxide::requests::Requester trait for one-shot Bot API calls, tokio::runtime::Runtime::new() for sync doctor context]
key_files:
  created: []
  modified:
    - crates/bot/src/lib.rs
    - crates/rightclaw/src/doctor.rs
decisions:
  - "Use teloxide::requests::Requester as _ import in a scoped block to call delete_webhook() without polluting lib.rs imports"
  - "Inline resolve_token_from_config in doctor.rs (duplicate 10-line logic) with TODO to use codegen::telegram after Plan 01 makes it pub(crate)"
  - "make_webhook_check() extracted as testable helper — avoids real network calls in tests by injecting Result<String, String>"
metrics:
  duration_seconds: 460
  completed_date: "2026-04-01"
  tasks_completed: 2
  files_modified: 2
---

# Phase 26 Plan 02: deleteWebhook + Doctor Webhook Check Summary

deleteWebhook called before long-polling startup (fatal on failure) and doctor warns when active webhook found on agents with configured Telegram tokens.

## Tasks Completed

| Task | Name | Commit | Files |
|------|------|--------|-------|
| 1 | Add deleteWebhook in bot/src/lib.rs before run_telegram | 7cd9bfb | crates/bot/src/lib.rs |
| 2 | Add check_webhook_info_for_agents to doctor.rs; wire into run_doctor | 0131c79 | crates/rightclaw/src/doctor.rs |

## What Was Built

**Task 1 (PC-04):** In `run_async()` in `crates/bot/src/lib.rs`, after the Telegram token is resolved and before `run_telegram()` is called, a one-shot `teloxide::Bot::new(token.clone()).delete_webhook().await` call is inserted. The `teloxide::requests::Requester as _` trait is imported in a scoped block. On error, the function returns `Err` with a diagnostic message explaining the failure would cause silent message drops. On success, `tracing::info!` logs "deleteWebhook succeeded" with the agent name.

**Task 2 (PC-05):** In `crates/rightclaw/src/doctor.rs`:
- `fetch_webhook_url(token: &str) -> Result<String, String>`: calls `GET https://api.telegram.org/bot{token}/getWebhookInfo` with a 5s timeout via `reqwest`, returning the `result.url` field.
- `make_webhook_check(agent_name, webhook_url_result)`: builds a `DoctorCheck` from the fetch result — Pass for empty URL, Warn with fix hint for active webhook, Warn (no fix) for HTTP errors.
- `resolve_token_from_config`: inline duplicate of `codegen::telegram::resolve_telegram_token` logic (TODO to replace after Plan 01).
- `check_webhook_info_for_agents(home)`: scans `home/agents/`, reads `agent.yaml` per subdir, skips agents without tokens, calls fetch and builds checks.
- Wired into `run_doctor()` after `check_agent_structure()`.

## Verification Results

- `cargo build -p rightclaw-bot` — clean, zero errors
- `cargo build -p rightclaw` — clean, zero errors
- `cargo test -p rightclaw --lib doctor` — 28/28 passed (4 new tests)
- `rg "delete_webhook" crates/bot/src/lib.rs` — 1 match
- `rg "check_webhook_info_for_agents" crates/rightclaw/src/doctor.rs` — 2 matches (fn def + call in run_doctor)
- `rg "make_webhook_check" crates/rightclaw/src/doctor.rs` — 4 matches (fn def + call + 3 tests)

## Deviations from Plan

### Auto-fixed Issues

None — plan executed as written with one noted deviation:

**[Plan Note - Parallel execution] resolve_telegram_token not yet pub(crate)**
- **Found during:** Task 2
- **Issue:** Plan 01 (running in parallel) makes `resolve_telegram_token` pub(crate) in codegen/telegram.rs. That change was not yet available.
- **Fix:** Inlined the 10-line token resolution logic in doctor.rs as `resolve_token_from_config()` with `// TODO: use crate::codegen::telegram::resolve_telegram_token after Plan 01` comment. This matches the plan's prescribed fallback approach exactly.
- **Files modified:** crates/rightclaw/src/doctor.rs

### Pre-existing Issues (Out of Scope)

- `rightclaw-cli` fails to compile due to changes from Plan 01 (`rightclaw::codegen::generate_telegram_channel_config` path issue). This is out of scope — caused by Plan 01's changes, not Plan 02's.
- Logged for follow-up after Plan 01 merges.

## Known Stubs

None — all new functions are fully wired with real implementations.

## Self-Check: PASSED
