# Cron Budget Controls & Model Passthrough

**Date:** 2026-04-11
**Status:** Approved

## Problem

1. `AgentConfig.model` (e.g. `sonnet`) is parsed and logged but never passed as `--model` to `claude -p` — neither in Telegram worker nor cron. Claude defaults to Opus, making cron jobs unexpectedly expensive ($1+ for simple tasks).
2. `--max-budget-usd` is not passed in cron invocations — only in Telegram worker. Cron jobs use Claude CLI's internal default.
3. `CronSpec.max_turns` is a blunt instrument for cron cost control. Budget is the real constraint.

## Solution

### 1. Pass `--model` from AgentConfig

**worker.rs**: Add `model: Option<String>` to `WorkerContext`. After `--max-budget-usd` args, emit `--model <value>` when present.

**cron.rs**: Thread `model: Option<String>` through `run_cron_task` -> `reconcile_jobs` -> `run_job_loop` -> `execute_job`. Emit `--model <value>` when present.

Source of truth: `AgentConfig.model` field in `agent.yaml`.

### 2. CronSpec: replace `max_turns` with `max_budget_usd`

Remove `max_turns` from `CronSpec`. Add `max_budget_usd` with default $1.00.

```rust
#[derive(Debug, Clone, serde::Deserialize, PartialEq)]
pub struct CronSpec {
    pub schedule: String,
    pub prompt: String,
    pub lock_ttl: Option<String>,
    #[serde(default = "default_cron_max_budget_usd")]
    pub max_budget_usd: f64,  // default 1.0
}

fn default_cron_max_budget_usd() -> f64 { 1.0 }
```

In `execute_job`: remove `--max-turns` arg, add `--max-budget-usd` from `spec.max_budget_usd`.

### 3. SKILL.md updates

- Remove `max_turns` from the YAML spec table.
- Add `max_budget_usd` (optional, default $1.00).
- Add **Schedule Guidelines** section: when user doesn't specify exact minutes, avoid `:00` and `:30` — use odd minutes like `:17`, `:43`, `:07` to spread API load and avoid rate limit spikes.
- Update examples to use `max_budget_usd` instead of `max_turns`, and offset minutes in schedules.

### 4. Runtime warning for round minutes

In `load_specs` (or `reconcile_jobs`), after parsing each spec, check if the minute field of the schedule is `0`, `00`, or `30`. Emit `tracing::warn` suggesting an offset.

```rust
fn warn_round_minutes(job_name: &str, schedule: &str) {
    let minute_field = schedule.split_whitespace().next().unwrap_or("");
    if matches!(minute_field, "0" | "00" | "30") {
        tracing::warn!(
            job = %job_name,
            schedule,
            "cron schedule uses :00 or :30 minutes - consider offset to avoid API rate limits"
        );
    }
}
```

## Files Changed

| File | Change |
|------|--------|
| `crates/bot/src/telegram/worker.rs` | Add `model` to WorkerContext, pass `--model` to claude args |
| `crates/bot/src/cron.rs` | Remove `max_turns` from CronSpec, add `max_budget_usd`, pass `--model` and `--max-budget-usd`, add round-minutes warning |
| `crates/bot/src/lib.rs` | Thread `config.model` into WorkerContext and cron task |
| `skills/cronsync/SKILL.md` | Update spec table, examples, add schedule guidelines |

## Not Changed

- `AgentConfig` struct (already has all needed fields)
- Worker budget/turns defaults (remain as-is)
- No `deny_unknown_fields` on CronSpec
