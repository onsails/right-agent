# Learning Episode Selector Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build durable episode-based background learned-skill review so multi-turn corrections, async continuations, cron results, tool evidence, and thinking context are reviewed together.

**Architecture:** Persist typed execution evidence and durable `learning_episodes` seeds first. A configurable selector model reads a bounded Rust-built corpus and persists selected refs; the existing report-only reviewer then reads only that selected episode.

**Tech Stack:** Rust 2024, rusqlite migrations, Claude Code stream-json parsing, Telegram worker/cron/background flows, existing `ClaudeInvocation` JSON schema runner.

---

## Assumptions

- Execute from a project worktree under `.worktrees/`.
- Prefix repo commands with `devenv shell --`.
- `learning.episode_selector_model` is optional. If absent, inherit the current per-agent model override.
- Thinking is secondary context: stored, no FTS, never sufficient as sole candidate evidence.

## File Map

- Storage/domain: `crates/right-db/src/sql/v24_learning_episodes.sql`, `crates/right-db/src/migrations.rs`, `crates/right-agent/src/learning_episodes.rs`, `crates/right-agent/src/lib.rs`, `crates/right-agent/src/learned_skills.rs`.
- Config/CLI: `crates/right-agent-config/src/lib.rs`, `crates/right-agent/src/init.rs`, `crates/right-agent/src/agent/types.rs`, `crates/right/src/main.rs`, `crates/right/src/wizard.rs`, `crates/bot/src/config_watcher.rs`.
- Bot runtime: `crates/bot/src/cc/stream.rs`, `crates/bot/src/execution_events.rs`, `crates/bot/src/learning_episode.rs`, `crates/bot/src/lib.rs`, `crates/bot/src/telegram/{handler.rs,dispatch.rs,worker.rs}`, `crates/bot/src/{cron.rs,background.rs,async_delivery.rs,learning_review.rs,learning_review_tests.rs}`.
- Docs: `docs/architecture/sessions.md`, `docs/architecture/mcp.md`, `PROMPT_SYSTEM.md`.

## Verification Cadence

- Baseline once before edits: `devenv shell -- cargo test -p right-db -p right-agent learned`.
- Run narrow package tests after each task.
- Final: `devenv shell -- cargo fmt --check` and `devenv shell -- cargo test --workspace`.

### Task 1: Baseline

**Files:**
- Modify: none

- [ ] **Step 1: Create or enter a worktree**

Run:

```bash
git worktree add .worktrees/learning-episode-selector -b feat/learning-episode-selector
```

Expected: worktree created. If the branch already exists, enter that worktree and run `git status --short`.

- [ ] **Step 2: Run baseline tests**

Run:

```bash
devenv shell -- cargo test -p right-db -p right-agent learned
```

Expected: pass. If a test already fails, record the exact test name before editing.

### Task 2: Database And Domain Storage

**Files:**
- Create: `crates/right-db/src/sql/v24_learning_episodes.sql`
- Modify: `crates/right-db/src/migrations.rs`
- Create: `crates/right-agent/src/learning_episodes.rs`
- Modify: `crates/right-agent/src/lib.rs`
- Modify: `crates/right-agent/src/learned_skills.rs`

- [ ] **Step 1: Write failing migration tests**

Add to `crates/right-db/src/migrations.rs`:

```rust
#[test]
fn learning_episode_tables_exist() {
    let mut conn = rusqlite::Connection::open_in_memory().unwrap();
    MIGRATIONS.to_latest(&mut conn).unwrap();
    let events: String = conn.query_row(
        "SELECT sql FROM sqlite_master WHERE type='table' AND name='execution_events'",
        [],
        |r| r.get(0),
    ).unwrap();
    assert!(events.contains("event_kind"));
    assert!(events.contains("trust_label"));
    let episodes: String = conn.query_row(
        "SELECT sql FROM sqlite_master WHERE type='table' AND name='learning_episodes'",
        [],
        |r| r.get(0),
    ).unwrap();
    assert!(episodes.contains("ready_after"));
    assert!(episodes.contains("episode_hash"));
}

#[test]
fn execution_events_do_not_create_fts() {
    let mut conn = rusqlite::Connection::open_in_memory().unwrap();
    MIGRATIONS.to_latest(&mut conn).unwrap();
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE name LIKE 'execution_events%fts%'",
        [],
        |r| r.get(0),
    ).unwrap();
    assert_eq!(count, 0);
}

#[test]
fn skill_review_reports_links_learning_episode() {
    let mut conn = rusqlite::Connection::open_in_memory().unwrap();
    MIGRATIONS.to_latest(&mut conn).unwrap();
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('skill_review_reports') WHERE name='learning_episode_id'",
        [],
        |r| r.get(0),
    ).unwrap();
    assert_eq!(count, 1);
}
```

Run:

```bash
devenv shell -- cargo test -p right-db learning_episode_tables_exist execution_events_do_not_create_fts skill_review_reports_links_learning_episode
```

Expected: fail because version 24 does not exist.

- [ ] **Step 2: Add schema and migration hook**

Create `crates/right-db/src/sql/v24_learning_episodes.sql`:

```sql
CREATE TABLE IF NOT EXISTS execution_events (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  agent_name TEXT NOT NULL,
  root_session_id TEXT,
  invocation_id TEXT,
  turn_id INTEGER,
  async_run_id TEXT,
  cron_job_name TEXT,
  cron_run_id TEXT,
  seq INTEGER NOT NULL,
  event_kind TEXT NOT NULL CHECK (event_kind IN ('assistant_text','thinking','tool_call','tool_result','tool_error','invocation_result','other')),
  tool_name TEXT,
  content_json TEXT NOT NULL DEFAULT '{}',
  content_text TEXT NOT NULL DEFAULT '',
  trust_label TEXT NOT NULL CHECK (trust_label IN ('primary','secondary','low_trust')),
  created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ','now'))
);
CREATE INDEX IF NOT EXISTS idx_execution_events_agent_session_seq ON execution_events(agent_name, root_session_id, seq);
CREATE INDEX IF NOT EXISTS idx_execution_events_invocation ON execution_events(invocation_id);
CREATE INDEX IF NOT EXISTS idx_execution_events_async_run ON execution_events(async_run_id);
CREATE INDEX IF NOT EXISTS idx_execution_events_cron_run ON execution_events(cron_run_id);

CREATE TABLE IF NOT EXISTS learning_episodes (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  agent_name TEXT NOT NULL,
  kind TEXT NOT NULL CHECK (kind IN ('foreground_thread','async_continuation','cron_run')),
  seed_trigger_kind TEXT NOT NULL CHECK (seed_trigger_kind IN ('learning_signal','skill_issue_signal','effort_threshold','cron','async_result')),
  seed_ref TEXT NOT NULL,
  status TEXT NOT NULL CHECK (status IN ('pending','selecting','selected','reviewing','reviewed','no_episode','insufficient_context','failed')),
  target_chat_id INTEGER,
  target_thread_id INTEGER,
  start_ref TEXT,
  end_ref TEXT,
  message_refs_json TEXT NOT NULL DEFAULT '[]',
  execution_event_refs_json TEXT NOT NULL DEFAULT '[]',
  selector_model TEXT,
  selector_output_json TEXT,
  boundary_rationale TEXT,
  confidence TEXT CHECK (confidence IN ('low','medium','high')),
  context_incomplete INTEGER NOT NULL DEFAULT 0 CHECK (context_incomplete IN (0, 1)),
  episode_hash TEXT,
  ready_after TEXT NOT NULL,
  last_evidence_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ','now')),
  created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ','now')),
  updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ','now'))
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_learning_episodes_hash ON learning_episodes(agent_name, episode_hash) WHERE episode_hash IS NOT NULL;
CREATE UNIQUE INDEX IF NOT EXISTS idx_learning_episodes_seed ON learning_episodes(agent_name, kind, seed_trigger_kind, seed_ref);
CREATE INDEX IF NOT EXISTS idx_learning_episodes_ready ON learning_episodes(agent_name, status, ready_after);
```

In `crates/right-db/src/migrations.rs` add `V24_SCHEMA`, set `LATEST_SCHEMA_VERSION` to 24, and add:

```rust
fn v24_learning_episodes(tx: &Transaction) -> Result<(), HookError> {
    tx.execute_batch(V24_SCHEMA)?;
    let has_column: i64 = tx.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('skill_review_reports') WHERE name=?1",
        ["learning_episode_id"],
        |r| r.get(0),
    )?;
    if has_column == 0 {
        tx.execute_batch("ALTER TABLE skill_review_reports ADD COLUMN learning_episode_id INTEGER")?;
    }
    tx.execute_batch("CREATE INDEX IF NOT EXISTS idx_skill_review_reports_episode ON skill_review_reports(learning_episode_id)")?;
    Ok(())
}
```

Append `M::up_with_hook("", v24_learning_episodes)` to `MIGRATIONS`.

- [ ] **Step 3: Write failing domain tests**

Create `crates/right-agent/src/learning_episodes.rs` with:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn conn() -> rusqlite::Connection {
        let mut conn = rusqlite::Connection::open_in_memory().unwrap();
        right_db::MIGRATIONS.to_latest(&mut conn).unwrap();
        conn
    }

    #[test]
    fn execution_event_insert_round_trips_thinking_as_secondary() {
        let conn = conn();
        let id = insert_execution_event(&conn, &NewExecutionEvent {
            agent_name: "right".to_owned(),
            root_session_id: Some("session-1".to_owned()),
            invocation_id: Some("inv-1".to_owned()),
            turn_id: Some(7),
            async_run_id: None,
            cron_job_name: None,
            cron_run_id: None,
            seq: 3,
            event_kind: ExecutionEventKind::Thinking,
            tool_name: None,
            content_json: serde_json::json!({"text":"considering route"}),
            content_text: "considering route".to_owned(),
            trust_label: TrustLabel::Secondary,
        }).unwrap();
        let row: (String, String) = conn.query_row(
            "SELECT event_kind, trust_label FROM execution_events WHERE id=?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        ).unwrap();
        assert_eq!(row, ("thinking".to_owned(), "secondary".to_owned()));
    }

    #[test]
    fn claim_ready_episode_moves_pending_to_selecting() {
        let conn = conn();
        let id = insert_pending_episode(&conn, &NewLearningEpisodeSeed {
            agent_name: "right".to_owned(),
            kind: LearningEpisodeKind::ForegroundThread,
            seed_trigger_kind: EpisodeSeedTriggerKind::LearningSignal,
            seed_ref: "inv:inv-1".to_owned(),
            target_chat_id: Some(10),
            target_thread_id: Some(20),
            ready_after: "2026-05-19T00:00:00Z".to_owned(),
        }).unwrap();
        let claimed = claim_ready_episode(&conn, "right", "2026-05-19T00:00:01Z").unwrap();
        assert_eq!(claimed.map(|e| e.id), Some(id));
    }
}
```

Run:

```bash
devenv shell -- cargo test -p right-agent learning_episodes
```

Expected: compile failure.

- [ ] **Step 4: Implement domain types and helpers**

Add enums `ExecutionEventKind`, `TrustLabel`, `LearningEpisodeKind`, `EpisodeSeedTriggerKind`, `LearningEpisodeStatus` with `as_str()` methods. Add structs `NewExecutionEvent`, `NewLearningEpisodeSeed`, `LearningEpisodeRow`. Add helpers:

```rust
pub fn insert_execution_event(conn: &rusqlite::Connection, event: &NewExecutionEvent) -> Result<i64, rusqlite::Error>;
pub fn insert_pending_episode(conn: &rusqlite::Connection, seed: &NewLearningEpisodeSeed) -> Result<i64, rusqlite::Error>;
pub fn claim_ready_episode(conn: &rusqlite::Connection, agent_name: &str, now: &str) -> Result<Option<LearningEpisodeRow>, rusqlite::Error>;
pub fn mark_episode_selected(conn: &rusqlite::Connection, episode_id: i64, selection: &SelectedEpisodeUpdate) -> Result<(), rusqlite::Error>;
pub fn mark_episode_terminal(conn: &rusqlite::Connection, episode_id: i64, status: LearningEpisodeStatus, output_json: &serde_json::Value) -> Result<(), rusqlite::Error>;
pub fn mark_episode_failed(conn: &rusqlite::Connection, episode_id: i64, reason: &str) -> Result<(), rusqlite::Error>;
```

Export in `crates/right-agent/src/lib.rs`:

```rust
pub mod learning_episodes;
```

- [ ] **Step 5: Link reports and remove cooldown gate**

In `SkillReviewReport`, add:

```rust
pub learning_episode_id: Option<i64>,
```

Insert it into `skill_review_reports` and set it to `None` in existing tests.

Change `ReviewGateInput`:

```rust
pub struct ReviewGateInput<'a> {
    pub signal_trigger: Option<ReviewTriggerKind>,
    pub today: &'a str,
    pub daily_limit: i64,
}
```

Delete cooldown skip logic. Keep `review_running`, daily limit, and effort threshold.

- [ ] **Step 6: Verify and commit**

Run:

```bash
devenv shell -- cargo test -p right-db learning_episode
devenv shell -- cargo test -p right-agent learned_skills learning_episodes
```

Expected: pass.

Commit:

```bash
git add crates/right-db/src/migrations.rs crates/right-db/src/sql/v24_learning_episodes.sql crates/right-agent/src/lib.rs crates/right-agent/src/learning_episodes.rs crates/right-agent/src/learned_skills.rs
git commit -m "feat(db): add learning episode storage"
```

### Task 3: Learning Config

**Files:**
- Modify: `crates/right-agent-config/src/lib.rs`
- Modify: `crates/right-agent/src/init.rs`
- Modify: `crates/right-agent/src/agent/types.rs`
- Modify: `crates/right/src/main.rs`
- Modify: `crates/right/src/wizard.rs`
- Modify: `crates/bot/src/config_watcher.rs`
- Modify: `crates/bot/src/telegram/handler.rs`
- Modify: `crates/bot/src/telegram/dispatch.rs`
- Modify: `crates/bot/src/telegram/worker.rs`

- [ ] **Step 1: Write failing config tests**

Add to `crates/right-agent/src/agent/types.rs` tests:

```rust
#[test]
fn learning_config_defaults_when_missing() {
    let cfg: AgentConfig = serde_saphyr::from_str("restart: never\n").unwrap();
    assert_eq!(cfg.learning.episode_selector_model, None);
    assert_eq!(cfg.learning.episode_selector_max_budget_usd, 0.10);
    assert_eq!(cfg.learning.episode_settle_seconds, 90);
}

#[test]
fn learning_config_explicit_yaml_roundtrip() {
    let yaml = "learning:\n  episode_selector_model: \"claude-sonnet-4-6\"\n  episode_selector_max_budget_usd: 0.25\n  episode_settle_seconds: 180\n";
    let cfg: AgentConfig = serde_saphyr::from_str(yaml).unwrap();
    assert_eq!(cfg.learning.episode_selector_model.as_deref(), Some("claude-sonnet-4-6"));
    assert_eq!(cfg.learning.episode_selector_max_budget_usd, 0.25);
    assert_eq!(cfg.learning.episode_settle_seconds, 180);
}
```

Run:

```bash
devenv shell -- cargo test -p right-agent learning_config
```

Expected: compile failure.

- [ ] **Step 2: Add `LearningConfig` and pass it through**

Add to `crates/right-agent-config/src/lib.rs`:

```rust
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LearningConfig {
    pub episode_selector_model: Option<String>,
    #[serde(default = "default_episode_selector_max_budget_usd")]
    pub episode_selector_max_budget_usd: f64,
    #[serde(default = "default_episode_settle_seconds")]
    pub episode_settle_seconds: u64,
}
```

Use defaults `0.10` and `90`. Add `pub learning: LearningConfig` to `AgentConfig`.

Add `learning: LearningConfig` to `InitOverrides`, preserve it in `crates/right/src/main.rs`, and emit a non-default `learning:` block in `init_agent`.

Add `learning: right_agent::agent::types::LearningConfig` to `AgentSettings` and `WorkerContext`, then pass it from dispatch to handler to worker.

- [ ] **Step 3: Expose in `right agent config`**

In `crates/right/src/wizard.rs`, add a `learning:` menu entry that writes:

```yaml
learning:
  episode_selector_model: "<model>"
  episode_selector_max_budget_usd: 0.10
  episode_settle_seconds: 90
```

An empty model input removes `episode_selector_model` so the runtime inherits the agent model.

- [ ] **Step 4: Keep watcher restart semantics**

Add test in `crates/bot/src/config_watcher.rs`:

```rust
#[test]
fn diff_learning_change_requires_restart() {
    let old = "restart: never\nlearning:\n  episode_settle_seconds: 90\n";
    let new = "restart: never\nlearning:\n  episode_settle_seconds: 180\n";
    assert!(matches!(classify(old, new), ChangeKind::RestartRequired));
}
```

- [ ] **Step 5: Verify and commit**

Run:

```bash
devenv shell -- cargo test -p right-agent learning_config
devenv shell -- cargo test -p right-bot diff_learning_change_requires_restart
```

Expected: pass.

Commit:

```bash
git add crates/right-agent-config/src/lib.rs crates/right-agent/src/init.rs crates/right-agent/src/agent/types.rs crates/right/src/main.rs crates/right/src/wizard.rs crates/bot/src/config_watcher.rs crates/bot/src/telegram/handler.rs crates/bot/src/telegram/dispatch.rs crates/bot/src/telegram/worker.rs
git commit -m "feat(config): add learning episode settings"
```

### Task 4: Typed Execution Events From Streams

**Files:**
- Modify: `crates/bot/src/cc/stream.rs`
- Create: `crates/bot/src/execution_events.rs`
- Modify: `crates/bot/src/lib.rs`
- Modify: `crates/bot/src/telegram/worker.rs`
- Modify: `crates/bot/src/cron.rs`
- Modify: `crates/bot/src/background.rs`

- [ ] **Step 1: Write failing parser and redaction tests**

Add tests for `parse_persisted_stream_event` in `crates/bot/src/cc/stream.rs`:

```rust
#[test]
fn persisted_event_parses_thinking_text() {
    let line = r#"{"type":"assistant","message":{"content":[{"type":"thinking","thinking":"Need check Notion first"}]}}"#;
    let event = parse_persisted_stream_event(line).unwrap();
    assert_eq!(event.kind, PersistedStreamEventKind::Thinking);
    assert_eq!(event.content_text, "Need check Notion first");
}

#[test]
fn persisted_event_parses_tool_result_error() {
    let line = r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"toolu_1","is_error":true,"content":"permission denied"}]}}"#;
    let event = parse_persisted_stream_event(line).unwrap();
    assert_eq!(event.kind, PersistedStreamEventKind::ToolError);
}
```

Create a redaction test in `crates/bot/src/execution_events.rs`:

```rust
#[test]
fn sensitive_json_keys_are_redacted() {
    let input = serde_json::json!({"api_key":"abc","nested":{"refresh_token":"secret"},"safe":"visible"});
    let redacted = redact_sensitive_json(input);
    assert_eq!(redacted["api_key"], "[redacted]");
    assert_eq!(redacted["nested"]["refresh_token"], "[redacted]");
    assert_eq!(redacted["safe"], "visible");
}
```

Run:

```bash
devenv shell -- cargo test -p right-bot persisted_event_parses sensitive_json_keys_are_redacted
```

Expected: compile failure.

- [ ] **Step 2: Add persisted parser and DB insert helper**

In `crates/bot/src/cc/stream.rs`, add `PersistedStreamEventKind`, `PersistedStreamEvent`, and `parse_persisted_stream_event(line)` for assistant text, thinking, tool use, user tool result/error, and invocation result.

In `crates/bot/src/execution_events.rs`, add:

```rust
pub(crate) struct ExecutionEventScope<'a> {
    pub(crate) agent_name: &'a str,
    pub(crate) root_session_id: Option<&'a str>,
    pub(crate) invocation_id: Option<&'a str>,
    pub(crate) turn_id: Option<i64>,
    pub(crate) async_run_id: Option<&'a str>,
    pub(crate) cron_job_name: Option<&'a str>,
    pub(crate) cron_run_id: Option<&'a str>,
}

pub(crate) fn persist_stream_line(conn: &rusqlite::Connection, scope: &ExecutionEventScope<'_>, seq: i64, line: &str) -> Result<Option<i64>, rusqlite::Error>;
```

`persist_stream_line` must call `right_agent::learning_episodes::insert_execution_event`, use `TrustLabel::Secondary` for thinking, redact sensitive JSON keys, and bound `content_text` to 2,000 chars.

Export in `crates/bot/src/lib.rs`:

```rust
pub(crate) mod execution_events;
```

- [ ] **Step 3: Persist stream lines in runtime paths**

Foreground stdout loop scope:

```rust
ExecutionEventScope {
    agent_name: &ctx.agent_name,
    root_session_id: Some(&session_uuid),
    invocation_id: learning_invocation_id.as_deref(),
    turn_id: Some(i64::from(turn_id)),
    async_run_id: None,
    cron_job_name: None,
    cron_run_id: None,
}
```

Cron scope uses `root_session_id: Some(&run_id)`, `async_run_id: Some(&run_id)`, `cron_job_name: Some(job_name)`, `cron_run_id: Some(&run_id)`.

Background scope uses `root_session_id: Some(&request.run_id)` and `async_run_id: Some(&request.run_id)`.

- [ ] **Step 4: Verify and commit**

Run:

```bash
devenv shell -- cargo test -p right-bot persisted_event_parses sensitive_json_keys_are_redacted
```

Expected: pass.

Commit:

```bash
git add crates/bot/src/cc/stream.rs crates/bot/src/execution_events.rs crates/bot/src/lib.rs crates/bot/src/telegram/worker.rs crates/bot/src/cron.rs crates/bot/src/background.rs
git commit -m "feat(bot): persist typed execution events"
```

### Task 5: Episode Capture, Selector, And Drain

**Files:**
- Create: `crates/bot/src/learning_episode.rs`
- Modify: `crates/bot/src/lib.rs`
- Modify: `crates/bot/src/telegram/worker.rs`
- Modify: `crates/bot/src/cron.rs`
- Modify: `crates/bot/src/background.rs`
- Modify: `crates/bot/src/async_delivery.rs`

- [ ] **Step 1: Write failing seed and selector tests**

Create `crates/bot/src/learning_episode.rs` tests:

```rust
#[test]
fn accepted_signal_creates_pending_seed_without_cooldown() {
    let conn = tests::conn();
    capture_episode_seed(&conn, EpisodeSeedInput {
        agent_name: "right",
        kind: LearningEpisodeKind::ForegroundThread,
        seed_trigger_kind: EpisodeSeedTriggerKind::LearningSignal,
        seed_ref: "inv:inv-1",
        target_chat_id: Some(10),
        target_thread_id: Some(20),
        settle_seconds: 90,
        now: "2026-05-19T10:00:00Z",
    }).unwrap();
    let row: (String, String) = conn.query_row(
        "SELECT status, ready_after FROM learning_episodes WHERE seed_ref='inv:inv-1'",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    ).unwrap();
    assert_eq!(row, ("pending".to_owned(), "2026-05-19T10:01:30Z".to_owned()));
}

#[test]
fn selector_rejects_refs_outside_corpus() {
    let corpus = SelectorCorpus::for_test(vec!["msg:1"], vec![]);
    let output = EpisodeSelectorOutput::for_test_selected(vec!["msg:2"], vec![]);
    assert!(validate_selector_output(&corpus, &output).is_err());
}

#[test]
fn selector_rejects_thinking_only_episode() {
    let corpus = SelectorCorpus::for_test(vec![], vec![("exec:10", ExecutionEventKind::Thinking)]);
    let output = EpisodeSelectorOutput::for_test_selected(vec![], vec!["exec:10"]);
    assert!(validate_selector_output(&corpus, &output).is_err());
}
```

Run:

```bash
devenv shell -- cargo test -p right-bot accepted_signal_creates_pending_seed_without_cooldown selector_rejects
```

Expected: compile failure.

- [ ] **Step 2: Implement seed capture**

Add `EpisodeSeedInput` and:

```rust
pub(crate) fn capture_episode_seed(conn: &rusqlite::Connection, input: EpisodeSeedInput<'_>) -> Result<i64, rusqlite::Error> {
    let now = chrono::DateTime::parse_from_rfc3339(input.now)
        .map(|dt| dt.with_timezone(&chrono::Utc))
        .unwrap_or_else(|_| chrono::Utc::now());
    let ready_after = now + chrono::Duration::seconds(input.settle_seconds as i64);
    right_agent::learning_episodes::insert_pending_episode(conn, &NewLearningEpisodeSeed {
        agent_name: input.agent_name.to_owned(),
        kind: input.kind,
        seed_trigger_kind: input.seed_trigger_kind,
        seed_ref: input.seed_ref.to_owned(),
        target_chat_id: input.target_chat_id,
        target_thread_id: input.target_thread_id,
        ready_after: ready_after.format("%Y-%m-%dT%H:%M:%SZ").to_string(),
    })
}
```

Export in `crates/bot/src/lib.rs`:

```rust
pub(crate) mod learning_episode;
```

- [ ] **Step 3: Add selector schema, corpus, and validation**

Add `EPISODE_SELECTOR_SCHEMA_JSON` with required fields `status`, `kind`, `start_ref`, `end_ref`, `message_refs`, `execution_event_refs`, `boundary_rationale`, `confidence`, and `context_incomplete`.

Implement `SelectorCorpus`, `CorpusMessage`, `CorpusExecutionEvent`, and `EpisodeSelectorOutput`. Corpus queries:

```sql
SELECT id, role, content, addressed_to_bot, routed_to_agent, created_at
FROM conversation_messages
WHERE platform='telegram' AND chat_id=?1 AND thread_id=?2
ORDER BY created_at DESC
LIMIT 40
```

```sql
SELECT id, event_kind, trust_label, content_text
FROM execution_events
WHERE agent_name=?1 AND (root_session_id=?2 OR invocation_id=?3 OR async_run_id=?4 OR cron_run_id=?5)
ORDER BY seq
LIMIT 120
```

`validate_selector_output` rejects refs outside corpus and rejects selected output with no message ref and no observable non-thinking execution ref.

- [ ] **Step 4: Add ready-episode drain**

Add `LearningEpisodeRuntime` and `drain_ready_learning_episodes_once(runtime)`. It must:

1. Claim one ready pending episode.
2. Call `try_mark_review_started` with no cooldown field.
3. Requeue on `AlreadyRunning` or `DailyLimit`.
4. Build corpus.
5. Run selector through `ClaudeInvocation` with JSON schema, max 3 turns, and selector budget.
6. Validate output.
7. Mark terminal statuses or selected status.
8. Call the reviewer bridge from Task 6 after selection.

- [ ] **Step 5: Capture seeds from all sources**

Foreground success seed: `kind=ForegroundThread`, trigger from accepted signal or effort threshold, `seed_ref=inv:<source_invocation_id>`.

Cron completion seed: `kind=CronRun`, `seed_trigger_kind=Cron`, `seed_ref=cron:<run_id>`.

Background completion and async delivery seed: `kind=AsyncContinuation`, `seed_trigger_kind=AsyncResult`, `seed_ref=async:<run_id>`.

After capture, spawn a delayed one-shot drain:

```rust
std::mem::drop(tokio::spawn(async move {
    tokio::time::sleep(std::time::Duration::from_secs(runtime.learning.episode_settle_seconds)).await;
    crate::learning_episode::drain_ready_learning_episodes_once(runtime).await;
}));
```

- [ ] **Step 6: Verify and commit**

Run:

```bash
devenv shell -- cargo test -p right-bot accepted_signal_creates_pending_seed_without_cooldown selector_rejects learning_episode
```

Expected: pass.

Commit:

```bash
git add crates/bot/src/learning_episode.rs crates/bot/src/lib.rs crates/bot/src/telegram/worker.rs crates/bot/src/cron.rs crates/bot/src/background.rs crates/bot/src/async_delivery.rs
git commit -m "feat(bot): select learning episodes"
```

### Task 6: Reviewer Reads Selected Episode

**Files:**
- Modify: `crates/bot/src/learning_review.rs`
- Modify: `crates/bot/src/learning_review_tests.rs`
- Modify: `crates/bot/src/learning_episode.rs`
- Modify: `crates/bot/src/telegram/worker.rs`

- [ ] **Step 1: Write failing reviewer tests**

Add to `crates/bot/src/learning_review_tests.rs`:

```rust
#[test]
fn review_prompt_marks_thinking_secondary() {
    let bundle = ReviewBundle::for_test_with_execution_event("exec:3", "thinking", "secondary");
    let prompt = build_review_prompt(&bundle);
    assert!(prompt.contains("secondary context"));
    assert!(prompt.contains("cannot be the only evidence"));
}

#[test]
fn candidate_with_only_thinking_evidence_is_rejected() {
    let raw = serde_json::json!({
        "status":"create_candidate",
        "confidence":"high",
        "candidate_skill_name":"rightx-context-window",
        "candidate_summary":"Use context",
        "evidence_refs":["exec:3"],
        "user_notice":null
    });
    let output = ReviewOutput::parse(raw).unwrap();
    let refs = EpisodeEvidenceIndex::from_pairs(vec![("exec:3".to_owned(), EvidenceKind::Thinking)]);
    assert!(output.validate_candidate_evidence(&refs).is_err());
}
```

Run:

```bash
devenv shell -- cargo test -p right-bot review_prompt_marks_thinking_secondary candidate_with_only_thinking_evidence_is_rejected
```

Expected: compile failure.

- [ ] **Step 2: Replace review bundle shape**

In `crates/bot/src/learning_review.rs`, replace old event timeline with:

```rust
pub(crate) learning_episode_id: Option<i64>,
pub(crate) episode_messages: Vec<ReviewMessage>,
pub(crate) episode_execution_events: Vec<ReviewExecutionEvent>,
```

Add `ReviewMessage` and `ReviewExecutionEvent` structs with `ref_id`, trust label, and content. Prompt text must state:

```text
Thinking events are secondary context. They can guide wording, but candidate evidence_refs must include at least one observable ref: msg:* or non-thinking exec:*.
```

- [ ] **Step 3: Add evidence validation and report link**

Add `EvidenceKind`, `EpisodeEvidenceIndex`, and:

```rust
impl ReviewOutput {
    pub(crate) fn validate_candidate_evidence(&self, index: &EpisodeEvidenceIndex) -> Result<(), String>;
}
```

For create/update candidates, reject output unless at least one evidence ref is `Message` or `ObservableExecution`.

Add `learning_episode_id` to `ReviewReportContext` and `ReviewOutput::to_report`.

- [ ] **Step 4: Build episode review bridge**

In `crates/bot/src/learning_episode.rs`, implement `run_episode_reviewer(runtime, episode_id)`:

1. Load selected message refs from `conversation_messages`.
2. Load selected execution refs from `execution_events`.
3. Build `ReviewBundle`.
4. Run reviewer invocation with the existing report-only tool boundary.
5. Parse output and validate candidate evidence.
6. Insert `SkillReviewReport` with `learning_episode_id: Some(episode_id)`.
7. Mark episode `reviewed`; on process, parse, or validation failure mark `failed`.
8. Call `mark_review_finished` so `review_running` is released.

- [ ] **Step 5: Verify and commit**

Run:

```bash
devenv shell -- cargo test -p right-bot learning_review
```

Expected: pass.

Commit:

```bash
git add crates/bot/src/learning_review.rs crates/bot/src/learning_review_tests.rs crates/bot/src/learning_episode.rs crates/bot/src/telegram/worker.rs
git commit -m "feat(bot): review selected learning episodes"
```

### Task 7: Docs And Final Verification

**Files:**
- Modify: `docs/architecture/sessions.md`
- Modify: `docs/architecture/mcp.md`
- Modify: `PROMPT_SYSTEM.md`
- Modify: `ARCHITECTURE.md` only if a prescriptive rule changes

- [ ] **Step 1: Update architecture docs**

In `docs/architecture/sessions.md`, replace old background-review text with:

```markdown
Background learned-skill review is episode-based. Trigger sources create durable
`learning_episodes` rows immediately, then a short settle delay lets nearby
user corrections or async feedback arrive before selection. The old fixed
review cooldown no longer drops evidence. The selector reads a bounded
Rust-built corpus from `conversation_messages`, typed `execution_events`,
signals, async run metadata, and cron run metadata, then persists selected refs.
The report-only reviewer receives only that selected episode plus the current
`rightx-*` skill index.
```

Also state that foreground, background, and cron stream-json lines are normalized into `execution_events`; thinking is secondary and has no FTS.

In `docs/architecture/mcp.md`, state that Stage 2 runs after `learning_episodes` selection and candidate evidence must cite at least one observable `msg:*` or non-thinking `exec:*` ref.

In `PROMPT_SYSTEM.md`, update the background learned-skill review exception so it describes selector plus reviewer, selected episode refs, typed execution events, and no fixed cooldown drop.

- [ ] **Step 2: Run final checks**

Run:

```bash
devenv shell -- rg -n "TB[D]|TO[D]O|FIXM[E]|mayb[e]|or equivalen[t]|similar t[o]|appropriat[e]|etc[.]" docs/superpowers/plans/2026-05-19-learning-episode-selector.md docs/superpowers/specs/2026-05-19-learning-episode-selector-design.md docs/architecture/sessions.md docs/architecture/mcp.md PROMPT_SYSTEM.md
devenv shell -- git diff --check
devenv shell -- cargo fmt --check
devenv shell -- cargo test --workspace
```

Expected: no placeholder matches in touched docs, no whitespace errors, formatting passes, workspace tests pass.

- [ ] **Step 3: Review invariants**

Check these in code:

```text
- fixed cooldown no longer prevents seed capture
- execution_events has no FTS table
- thinking events use trust_label='secondary'
- selector cannot pick refs outside corpus
- reviewer cannot accept thinking-only candidate evidence
- cron/background/delivery paths create episode seeds
- skill_review_reports.source_invocation_id remains populated
- skill_review_reports.learning_episode_id is populated for episode reviews
- reviewer tool boundary remains report-only
```

- [ ] **Step 4: Commit docs and final fixes**

Run:

```bash
git add docs/architecture/sessions.md docs/architecture/mcp.md PROMPT_SYSTEM.md ARCHITECTURE.md
git commit -m "docs: describe learning episode review"
```

If review fixes were required:

```bash
git add <changed-files>
git commit -m "fix(bot): harden learning episode review"
```
