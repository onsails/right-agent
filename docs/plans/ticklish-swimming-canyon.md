# Remove max_turns and max_budget_usd from chat path

## Context

Cron delivery fails with `error_max_budget_usd` because `--resume` reuses an existing session
whose accumulated `total_cost_usd` ($0.15) exceeds the delivery's `--max-budget-usd 0.05`.
CC's `--max-budget-usd` applies to the **entire session**, not per-invocation.

Rather than patching the delivery budget, we remove budget/turn limits from the chat path entirely
(they were never user-facing knobs — just hardcoded defaults). Cron keeps its own per-job
`max_budget_usd` in the DB, which is the only place budget limits make sense.

## Changes

### 1. Remove fields from `AgentConfig`
**File:** `crates/rightclaw/src/agent/types.rs`
- Delete `default_max_turns()` (line 24-26) and `default_max_budget_usd()` (line 28-30)
- Delete `max_turns` field (lines 174-177) and `max_budget_usd` field (lines 179-182)

### 2. Remove from propagation chain
**Files & fields to remove:**
- `crates/bot/src/telegram/dispatch.rs` — `max_turns`, `max_budget_usd` params (lines 69-70), `AgentSettings` fields (lines 105-106)
- `crates/bot/src/telegram/handler.rs` — `AgentSettings` fields (lines 80-81), unpacking into `WorkerContext` (lines 217-218)
- `crates/bot/src/telegram/worker.rs` — `WorkerContext` fields (lines 73-76), claude args (lines 735-738)
- `crates/bot/src/lib.rs` — default values (lines 102-103), passing to `run_telegram()` (lines 407-408)

### 3. Remove from claude args in delivery
**File:** `crates/bot/src/cron_delivery.rs`
- Delete `--max-budget-usd 0.05` (lines 341-342)
- Delete `--max-turns 3` (lines 343-344)

### 4. Update thinking message display
**File:** `crates/bot/src/telegram/stream.rs`
- Remove `max_turns` param from `format_thinking_message()` (line 121)
- Change footer from `"Turn {}/{}"` to just `"Turn {}"` (line 142-143)
- Update all callers in `worker.rs` (lines 955, 979, 1052, 1069) — stop passing `ctx.max_turns`
- Update tests in `stream.rs` that call `format_thinking_message`

### 5. Remove from test fixtures
**File:** `crates/rightclaw/src/codegen/process_compose_tests.rs`
- Remove `max_turns` and `max_budget_usd` from all `AgentConfig` constructions (5 sites)

**File:** `crates/rightclaw/src/codegen/telegram.rs` (line 75-76)
**File:** `crates/rightclaw/src/codegen/claude_json.rs` (line 149-150)

### 6. Keep cron budget as-is
**No changes** to:
- `crates/rightclaw/src/cron_spec.rs` — `CronSpec.max_budget_usd` stays
- `crates/bot/src/cron.rs` — `--max-budget-usd` from `spec.max_budget_usd` stays (line 180-181)
- `crates/rightclaw-cli/src/memory_server.rs` — `CronCreateParams.max_budget_usd` stays
- `crates/rightclaw-cli/src/right_backend.rs` — cron create/update stays
- DB schema — `cron_specs.max_budget_usd` column stays

## Verification

1. `cargo check --workspace` — no compile errors
2. `cargo test --workspace` — all tests pass
3. Confirm `cron.rs` still passes `--max-budget-usd` from spec
4. Confirm `cron_delivery.rs` no longer passes budget/turns flags
5. Confirm `worker.rs` no longer passes budget/turns flags to `claude -p`
