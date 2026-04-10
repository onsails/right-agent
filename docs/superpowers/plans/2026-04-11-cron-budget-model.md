# Cron Budget Controls & Model Passthrough — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Pass `--model` from AgentConfig to both Telegram worker and cron invocations, replace `max_turns` with `max_budget_usd` in CronSpec, add round-minutes warning, and update the rightcron SKILL.md.

**Architecture:** Four independent changes: (1) thread `model` through the DI/dispatch pipeline into WorkerContext and claude args, (2) refactor CronSpec to drop `max_turns` and add `max_budget_usd`, thread `model` into cron execute_job, (3) add `warn_round_minutes` helper in cron.rs, (4) update SKILL.md docs.

**Tech Stack:** Rust, serde, teloxide DI (dptree), tokio::process

---

### Task 1: Add `--model` to Telegram worker pipeline

**Files:**
- Modify: `crates/bot/src/telegram/handler.rs:57-65` (add ModelOverride newtype)
- Modify: `crates/bot/src/telegram/handler.rs:152-167` (pass model into WorkerContext)
- Modify: `crates/bot/src/telegram/worker.rs:52-78` (add model field to WorkerContext)
- Modify: `crates/bot/src/telegram/worker.rs:814-820` (emit --model arg)
- Modify: `crates/bot/src/telegram/dispatch.rs:55-67` (accept model param)
- Modify: `crates/bot/src/telegram/dispatch.rs:88-141` (wrap in Arc, inject into deps)
- Modify: `crates/bot/src/lib.rs:383-395` (pass config.model to run_telegram)
- Modify: `crates/bot/src/telegram/mod.rs:85-95` (add model to test default config)

- [ ] **Step 1: Add ModelOverride newtype in handler.rs**

In `crates/bot/src/telegram/handler.rs`, after line 65 (`pub struct ShowThinking(pub bool);`), add:

```rust
/// Claude model override from agent.yaml (e.g. "sonnet", "opus").
#[derive(Clone)]
pub struct ModelOverride(pub Option<String>);
```

- [ ] **Step 2: Add `model` field to WorkerContext**

In `crates/bot/src/telegram/worker.rs`, after line 75 (`pub show_thinking: bool,`), add:

```rust
    /// Claude model override (passed as --model). None = inherit CLI default.
    pub model: Option<String>,
```

- [ ] **Step 3: Pass model into WorkerContext construction in handler.rs**

In `crates/bot/src/telegram/handler.rs`, add `model: Arc<ModelOverride>,` to the `handle_message` function signature (line 81-94), after `show_thinking: Arc<ShowThinking>,` (line 92):

```rust
    model: Arc<ModelOverride>,
```

Then in the `WorkerContext { ... }` block at line 152-167, add after `show_thinking: show_thinking.0,`:

```rust
                    model: model.0.clone(),
```

- [ ] **Step 4: Emit `--model` in worker.rs claude args**

In `crates/bot/src/telegram/worker.rs`, after line 820 (`claude_args.push(format!("{:.2}", ctx.max_budget_usd));`), add:

```rust
    if let Some(ref model) = ctx.model {
        claude_args.push("--model".into());
        claude_args.push(model.clone());
    }
```

- [ ] **Step 5: Thread model through dispatch.rs**

In `crates/bot/src/telegram/dispatch.rs`:

1. Add `model: Option<String>` parameter to `run_telegram` after `show_thinking: bool` (line 66):

```rust
    model: Option<String>,
```

2. After line 92 (`let show_thinking_arc: ...`), add:

```rust
    let model_arc: Arc<ModelOverride> = Arc::new(ModelOverride(model));
```

3. In the `dptree::deps![...]` block (line 127-141), add after `Arc::clone(&show_thinking_arc),`:

```rust
            Arc::clone(&model_arc),
```

- [ ] **Step 6: Pass config.model from lib.rs to run_telegram**

In `crates/bot/src/lib.rs`, the `run_telegram` call at line 383-396. Add `config.model.clone()` after `config.show_thinking,` (line 394):

```rust
            config.model.clone(),
```

- [ ] **Step 7: Update test default config in telegram/mod.rs**

In `crates/bot/src/telegram/mod.rs` line 93, the test helper builds a default AgentConfig. No change needed here — `model` is already `Option<String>` defaulting to `None` in AgentConfig.

- [ ] **Step 8: Build and fix compilation**

Run: `cargo build --workspace` via rust-builder subagent.
Expected: compiles cleanly. The dptree injection order must match — `ModelOverride` must appear in `handle_message` params in the same order it's added to `dptree::deps!`.

- [ ] **Step 9: Commit**

```bash
git add crates/bot/src/telegram/handler.rs crates/bot/src/telegram/worker.rs crates/bot/src/telegram/dispatch.rs crates/bot/src/lib.rs
git commit -m "feat: pass --model from agent.yaml to Telegram worker"
```

---

### Task 2: Refactor CronSpec and pass `--model` + `--max-budget-usd` to cron

**Files:**
- Modify: `crates/bot/src/cron.rs:8-15` (CronSpec struct)
- Modify: `crates/bot/src/cron.rs:122-133` (execute_job signature)
- Modify: `crates/bot/src/cron.rs:210-225` (command building)
- Modify: `crates/bot/src/cron.rs:352-364` (run_cron_task signature)
- Modify: `crates/bot/src/cron.rs:393-399` (reconcile_jobs signature)
- Modify: `crates/bot/src/cron.rs:437-444` (run_job_loop signature)
- Modify: `crates/bot/src/cron.rs:565-585` (test_load_specs_valid_yaml)
- Modify: `crates/bot/src/lib.rs:189-194` (cron spawn site)

- [ ] **Step 1: Write failing test for new CronSpec fields**

In `crates/bot/src/cron.rs`, replace the existing `test_load_specs_valid_yaml` test (line 565-585) with:

```rust
    #[test]
    fn test_load_specs_valid_yaml() {
        let dir = tempdir().unwrap();
        let crons_dir = dir.path().join("crons");
        std::fs::create_dir_all(&crons_dir).unwrap();

        let yaml = r#"
schedule: "*/5 * * * *"
prompt: "Check system health"
lock_ttl: "1h"
max_budget_usd: 0.50
"#;
        std::fs::write(crons_dir.join("health-check.yaml"), yaml).unwrap();

        let specs = load_specs(dir.path());
        assert_eq!(specs.len(), 1);
        let spec = specs.get("health-check").expect("health-check spec should exist");
        assert_eq!(spec.schedule, "*/5 * * * *");
        assert_eq!(spec.prompt, "Check system health");
        assert_eq!(spec.lock_ttl.as_deref(), Some("1h"));
        assert_eq!(spec.max_budget_usd, 0.50);
    }

    #[test]
    fn test_load_specs_default_budget() {
        let dir = tempdir().unwrap();
        let crons_dir = dir.path().join("crons");
        std::fs::create_dir_all(&crons_dir).unwrap();

        let yaml = r#"
schedule: "17 9 * * *"
prompt: "Do stuff"
"#;
        std::fs::write(crons_dir.join("simple.yaml"), yaml).unwrap();

        let specs = load_specs(dir.path());
        let spec = specs.get("simple").unwrap();
        assert_eq!(spec.max_budget_usd, 1.0, "default budget should be 1.0");
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p rightclaw-bot test_load_specs`
Expected: FAIL — `max_turns` field removed, `max_budget_usd` not yet on CronSpec.

- [ ] **Step 3: Update CronSpec struct**

Replace lines 8-15 in `crates/bot/src/cron.rs`:

```rust
/// Deserialized from crons/*.yaml
#[derive(Debug, Clone, serde::Deserialize, PartialEq)]
pub struct CronSpec {
    pub schedule: String,
    pub prompt: String,
    pub lock_ttl: Option<String>, // default "30m"
    #[serde(default = "default_cron_max_budget_usd")]
    pub max_budget_usd: f64,
}

fn default_cron_max_budget_usd() -> f64 {
    1.0
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p rightclaw-bot test_load_specs`
Expected: PASS

- [ ] **Step 5: Update execute_job signature — add model param**

Change `execute_job` signature (line 126-133) to:

```rust
async fn execute_job(
    job_name: &str,
    spec: &CronSpec,
    agent_dir: &std::path::Path,
    agent_name: &str,
    model: Option<&str>,
    bot: &BotType,
    notify_chat_ids: &[i64],
) {
```

- [ ] **Step 6: Replace --max-turns with --model + --max-budget-usd in command building**

Replace lines 214-217 (the `max_turns` block) in `execute_job`:

```rust
    if let Some(model) = model {
        cmd.arg("--model").arg(model);
    }
    cmd.arg("--max-budget-usd").arg(format!("{:.2}", spec.max_budget_usd));
```

- [ ] **Step 7: Thread model through run_cron_task → reconcile_jobs → run_job_loop → execute_job**

Update `run_cron_task` (line 358-364):

```rust
pub async fn run_cron_task(
    agent_dir: std::path::PathBuf,
    agent_name: String,
    model: Option<String>,
    bot: BotType,
    notify_chat_ids: Vec<i64>,
    shutdown: CancellationToken,
) {
```

Update the two `reconcile_jobs` calls inside (lines 371, 376) to pass `&model`.

Update `reconcile_jobs` (line 393-399):

```rust
async fn reconcile_jobs(
    handles: &mut HashMap<String, (CronSpec, JoinHandle<()>)>,
    agent_dir: &std::path::Path,
    agent_name: &str,
    model: &Option<String>,
    bot: &BotType,
    notify_chat_ids: &[i64],
) {
```

In the spawn block (line 428-431), clone model and pass it:

```rust
        let job_model = model.clone();
        let handle = tokio::spawn(async move {
            run_job_loop(job_name, job_spec, job_agent_dir, job_agent_name, job_model, job_bot, job_chat_ids)
                .await;
        });
```

Update `run_job_loop` (line 438-444):

```rust
async fn run_job_loop(
    job_name: String,
    spec: CronSpec,
    agent_dir: std::path::PathBuf,
    agent_name: String,
    model: Option<String>,
    bot: BotType,
    notify_chat_ids: Vec<i64>,
) {
```

In the inner spawn (line 482-484):

```rust
        let md = model.clone();
        tokio::spawn(async move {
            execute_job(&jn, &sp, &ad, &an, md.as_deref(), &bt, &nc).await;
        });
```

- [ ] **Step 8: Update cron spawn site in lib.rs**

In `crates/bot/src/lib.rs` line 194, add `config.model.clone()` to the `run_cron_task` call:

```rust
        cron::run_cron_task(cron_agent_dir, cron_agent_name, config.model.clone(), cron_bot, cron_chat_ids, cron_shutdown).await;
```

- [ ] **Step 9: Build and fix compilation**

Run: `cargo build --workspace` via rust-builder subagent.
Expected: compiles cleanly.

- [ ] **Step 10: Run all cron tests**

Run: `cargo test -p rightclaw-bot cron`
Expected: all PASS (the `max_turns` references in old tests are already replaced in step 1).

- [ ] **Step 11: Commit**

```bash
git add crates/bot/src/cron.rs crates/bot/src/lib.rs
git commit -m "feat: cron budget control + model passthrough

Replace max_turns with max_budget_usd in CronSpec (default $1).
Thread --model from AgentConfig into cron execute_job."
```

---

### Task 3: Add round-minutes warning

**Files:**
- Modify: `crates/bot/src/cron.rs` (add warn_round_minutes helper, call from load_specs)

- [ ] **Step 1: Write test for warn_round_minutes**

Add to the `#[cfg(test)] mod tests` in `crates/bot/src/cron.rs`:

```rust
    #[test]
    fn test_is_round_minutes_detects_zero() {
        assert!(is_round_minutes("0 9 * * *"));
        assert!(is_round_minutes("00 9 * * *"));
    }

    #[test]
    fn test_is_round_minutes_detects_thirty() {
        assert!(is_round_minutes("30 9 * * *"));
    }

    #[test]
    fn test_is_round_minutes_allows_offset() {
        assert!(!is_round_minutes("17 9 * * *"));
        assert!(!is_round_minutes("*/5 * * * *"));
        assert!(!is_round_minutes("43 */8 * * *"));
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p rightclaw-bot is_round_minutes`
Expected: FAIL — function doesn't exist yet.

- [ ] **Step 3: Implement is_round_minutes + warn in load_specs**

Add the helper function in `crates/bot/src/cron.rs` (after `parse_lock_ttl`, before `is_lock_fresh`):

```rust
/// Check if a cron schedule's minute field is exactly 0, 00, or 30.
/// These fire at popular intervals and risk API rate limit spikes.
pub fn is_round_minutes(schedule: &str) -> bool {
    let minute_field = schedule.split_whitespace().next().unwrap_or("");
    matches!(minute_field, "0" | "00" | "30")
}
```

In `load_specs`, after inserting into the map (after `map.insert(stem.to_string(), spec);` around line 114), add:

```rust
                if is_round_minutes(&spec.schedule) {
                    tracing::warn!(
                        job = %stem,
                        schedule = %spec.schedule,
                        "cron schedule uses :00 or :30 minutes — consider offset to avoid API rate limits"
                    );
                }
```

Note: move `spec` usage — you'll need to reference `spec.schedule` before `map.insert` consumes it. Reorder: warn first, then insert. Or clone the schedule string. Simplest: call `is_round_minutes` on the schedule before insert:

```rust
            Ok(spec) => {
                if is_round_minutes(&spec.schedule) {
                    tracing::warn!(
                        job = %stem,
                        schedule = %spec.schedule,
                        "cron schedule uses :00 or :30 minutes — consider offset to avoid API rate limits"
                    );
                }
                map.insert(stem.to_string(), spec);
            }
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p rightclaw-bot is_round_minutes`
Expected: PASS

- [ ] **Step 5: Build workspace**

Run: `cargo build --workspace` via rust-builder subagent.

- [ ] **Step 6: Commit**

```bash
git add crates/bot/src/cron.rs
git commit -m "feat: warn when cron schedule uses :00 or :30 minutes"
```

---

### Task 4: Update rightcron SKILL.md

**Files:**
- Modify: `skills/cronsync/SKILL.md`

- [ ] **Step 1: Update YAML Spec Format table**

Replace the table in `skills/cronsync/SKILL.md` (lines 44-49):

```markdown
| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `schedule` | string | Yes | - | Standard 5-field cron expression (minute hour day-of-month month day-of-week). Evaluated in **UTC** by the Rust runtime. |
| `lock_ttl` | string | No | `30m` | Duration after which a lock is considered stale (e.g., `10m`, `1h`, `30m`). |
| `max_budget_usd` | number | No | `1.0` | Maximum dollar spend for this cron job invocation. Claude stops gracefully when budget is reached. |
| `prompt` | string | Yes | - | The task prompt text that Claude executes when the cron fires. |
```

- [ ] **Step 2: Add Schedule Guidelines section**

After the "YAML Spec Format" section and before "Example specs:", add:

```markdown
### Schedule Guidelines

When the user doesn't specify exact minutes, **avoid :00 and :30** — these are peak times when many automated jobs fire simultaneously, causing API rate limit spikes. Use odd minutes like `:17`, `:43`, `:07`, `:53` to spread load.

The runtime emits a warning when it detects `:00` or `:30` in the minute field.
```

- [ ] **Step 3: Update example specs**

Replace the example specs block:

```markdown
**Example specs:**

```yaml
# crons/deploy-check.yaml
schedule: "*/5 * * * *"
lock_ttl: 10m
max_budget_usd: 0.25
prompt: "Check CI status for all open PRs, post comment if broken"
```

```yaml
# crons/morning-briefing.yaml
schedule: "7 9 * * 1-5"  # 09:07 UTC weekdays (avoid :00 rate limit spikes)
lock_ttl: 30m
max_budget_usd: 0.50
prompt: "Gather open PRs, failing tests, pending reviews. Post summary to Slack."
```
```

- [ ] **Step 4: Commit**

```bash
git add skills/cronsync/SKILL.md
git commit -m "docs: update rightcron skill — max_budget_usd, schedule guidelines"
```

---

### Task 5: Final verification

- [ ] **Step 1: Full workspace build**

Run: `cargo build --workspace` via rust-builder subagent.

- [ ] **Step 2: Run all tests**

Run: `cargo test --workspace` via rust-builder subagent.

- [ ] **Step 3: Run clippy**

Run: `cargo clippy --workspace` via rust-builder subagent.

- [ ] **Step 4: Verify no remaining max_turns in CronSpec-related code**

Run: `rg 'max_turns' crates/bot/src/cron.rs`
Expected: no matches (only worker.rs should still reference max_turns for Telegram).
