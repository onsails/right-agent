# Learned Skills Stage 1 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship foreground learned-skill creation/update with user-visible start/finish receipts, used-skill receipts, and persisted nudge data for future background review.

**Architecture:** Add a built-in `right-learn-skill` skill, expose two learning MCP tools through `RightBackend`, store learning events and nudge signals in the per-agent SQLite DB, and extend foreground structured output with used-skill receipts plus create/update nudge signals. Stage 1 registers learning tools only for foreground invocations, but the invocation kind is modeled so a future background review worker can reuse the same tools without schema churn. Skill package files remain agent-local under `.claude/skills/<skill_name>/`; MCP validates names, verifies successful writes at the derived sandbox/host path, and records metadata but never imports files.

**Tech Stack:** Rust 2024, rusqlite migrations via `right-db`, rmcp tool schemas, Claude Code structured output schemas, Telegram delivery through the existing progress UDS path, standard Agent Skills package layout.

---

## Scope

Stage 1 implements foreground learning and the nudge foundation only.

Included:
- `right-learn-skill` built-in skill.
- `mcp__right__skill_learning_start`.
- `mcp__right__skill_learning_finish`.
- `used_skill_receipts`, `learning_signal`, and `skill_issue_signal` structured output fields.
- SQLite learning event, nudge signal, and nudge counter storage.
- User-visible learning start and successful finish messages.
- Used-skill receipt rendering in the final Telegram reply.
- Runtime validation for nudge signal confidence: one event ref is allowed only for explicit user requests; all other nudge triggers require at least two event refs.
- Sandbox-aware package existence validation for successful learning finish.
- LLM-authored learned/updated receipt text passed as a `message` argument to `skill_learning_finish`.
- Documentation and prompt updates.

Excluded:
- Skill deletion.
- Umbrella skill generation.
- Background review execution.
- Host-side ingestion of sandbox skill files.
- Mutation of core/platform/bundled/codegen-owned skills.

## Assumptions

- Implementation happens in an isolated worktree created by `superpowers:using-git-worktrees`.
- Before writing Rust, load the project-required Rust skill named in `AGENTS.md` if it is available in that execution environment.
- Existing modified files in `crates/right-openshell/` are unrelated and must not be reverted or staged for this feature.
- `docs/research/learned-skills-trigger-flow.html` is a research artifact and is not part of this implementation plan unless the user explicitly asks to commit research docs.

## File Map

- Create `crates/right-codegen/skills/right-learn-skill/SKILL.md`: built-in authoring skill used by foreground agents.
- Modify `crates/right-codegen/src/skills.rs`: embed and install `right-learn-skill`.
- Modify `crates/right-codegen/src/agent_def.rs`: extend normal foreground reply schema.
- Modify `crates/right-codegen/src/agent_def_tests.rs`: schema and prompt contract tests.
- Create `crates/right-agent/src/learned_skills.rs`: shared domain types and SQLite helpers for learning events, nudge signals, counters, and reply-signal selection.
- Modify `crates/right-agent/src/lib.rs`: export `learned_skills`.
- Create `crates/right-db/src/sql/v20_learned_skills.sql`: learning/nudge tables.
- Modify `crates/right-db/src/migrations.rs`: register v20 and test the schema.
- Create `crates/right/src/learning.rs`: MCP parameter structs, skill-name validation, non-core classification, sandbox-aware skill package checks, successful-finish receipt validation, and learning message delivery helper.
- Modify `crates/right/src/main.rs`: add `learning` module.
- Modify `crates/right/src/right_backend.rs`: expose and dispatch learning MCP tools.
- Modify `crates/right/src/right_backend_tests.rs`: backend tool count, validation, and dispatch tests.
- Modify `crates/right/src/progress.rs`: add an unrate-limited learning target lookup for learning start/finish delivery and model future background-review invocation kind.
- Modify `crates/right/src/memory_server.rs`: stdio stubs and `with_instructions()` for tool parity.
- Modify `crates/right/src/aggregator.rs`: `with_instructions()` and tool-list tests.
- Modify `crates/right-mcp/src/internal_client.rs`: constants for full MCP tool names.
- Modify `crates/bot/src/cc/worker_reply.rs`: parse new structured output fields.
- Modify `crates/bot/src/telegram/worker.rs`: persist reply learning metadata and append used-skill receipts.
- Modify `crates/bot/src/cc/invocation.rs`, `crates/bot/src/cron.rs`, `crates/bot/src/reflection.rs`, and `crates/bot/src/cron_delivery.rs`: deny Stage 1 learning tools outside foreground turns.
- Modify `PROMPT_SYSTEM.md`, `docs/architecture/mcp.md`, `docs/architecture/sessions.md`, and `docs/architecture/sandbox.md`: keep docs in sync.

## Verification Cadence

- Start with one narrow baseline in the new worktree.
- For each task, run the targeted tests named in that task.
- Do not run full workspace tests after every task.
- Final verification is mandatory:

```bash
devenv shell -- cargo test --workspace
```

Expected final output: command exits with status 0.

---

### Task 1: Baseline And Worktree Hygiene

**Files:**
- Read: `git status --short`
- Read: `Cargo.toml`
- Read: `AGENTS.rust.md`

- [ ] **Step 1: Confirm worktree and unrelated changes**

Run:

```bash
devenv shell -- git status --short
```

Expected: note any existing changes. Do not stage unrelated `crates/right-openshell/*` edits.

- [ ] **Step 2: Run a narrow baseline**

Run:

```bash
devenv shell -- cargo test -p right-codegen installer_covers_every_builtin_skill_name
```

Expected: PASS. If it fails before changes, record the failure in the task notes and continue only if the failure is unrelated to learned skills.

---

### Task 2: Learned-Skills Database And Domain Helpers

**Files:**
- Create: `crates/right-db/src/sql/v20_learned_skills.sql`
- Modify: `crates/right-db/src/migrations.rs`
- Create: `crates/right-agent/src/learned_skills.rs`
- Modify: `crates/right-agent/src/lib.rs`

- [ ] **Step 1: Write failing migration tests**

In `crates/right-db/src/migrations.rs`, add these tests inside the existing `#[cfg(test)] mod tests`:

```rust
#[test]
fn learned_skills_migration_creates_event_tables() {
    let mut conn = Connection::open_in_memory().unwrap();
    MIGRATIONS.to_latest(&mut conn).unwrap();

    for table in [
        "skill_learning_events",
        "skill_nudge_signals",
        "skill_nudge_state",
    ] {
        let exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
                [table],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(exists, 1, "{table} table must exist");
    }
}

#[test]
fn learned_skills_nudge_state_defaults_are_usable() {
    let mut conn = Connection::open_in_memory().unwrap();
    MIGRATIONS.to_latest(&mut conn).unwrap();

    conn.execute(
        "INSERT INTO skill_nudge_state (agent_name) VALUES ('right')",
        [],
    )
    .unwrap();

    let row: (i64, i64, i64, i64) = conn
        .query_row(
            "SELECT tool_iters_since_review, turns_since_review, skill_issue_hints_since_review, review_running FROM skill_nudge_state WHERE agent_name='right'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .unwrap();
    assert_eq!(row, (0, 0, 0, 0));
}
```

- [ ] **Step 2: Run migration tests and verify failure**

Run:

```bash
devenv shell -- cargo test -p right-db learned_skills
```

Expected: FAIL because v20 tables do not exist yet.

- [ ] **Step 3: Add v20 SQL migration**

Create `crates/right-db/src/sql/v20_learned_skills.sql` with:

```sql
CREATE TABLE IF NOT EXISTS skill_learning_events (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  invocation_id TEXT NOT NULL,
  agent_name TEXT NOT NULL,
  action TEXT NOT NULL CHECK (action IN ('create', 'update')),
  skill_name TEXT NOT NULL,
  phase TEXT NOT NULL CHECK (phase IN ('start', 'finish')),
  status TEXT CHECK (status IS NULL OR status IN ('created', 'updated', 'aborted', 'failed')),
  reason TEXT,
  message TEXT,
  summary TEXT,
  event_refs_json TEXT NOT NULL DEFAULT '[]',
  created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ','now'))
);

CREATE INDEX IF NOT EXISTS idx_skill_learning_events_invocation
  ON skill_learning_events(invocation_id);

CREATE INDEX IF NOT EXISTS idx_skill_learning_events_skill
  ON skill_learning_events(skill_name);

CREATE TABLE IF NOT EXISTS skill_nudge_signals (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  invocation_id TEXT NOT NULL,
  agent_name TEXT NOT NULL,
  root_session_id TEXT,
  chat_id INTEGER,
  thread_id INTEGER,
  signal_kind TEXT NOT NULL CHECK (signal_kind IN ('learning', 'skill_issue')),
  payload_json TEXT NOT NULL,
  accepted_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ','now'))
);

CREATE INDEX IF NOT EXISTS idx_skill_nudge_signals_invocation
  ON skill_nudge_signals(invocation_id);

CREATE TABLE IF NOT EXISTS skill_nudge_state (
  agent_name TEXT PRIMARY KEY,
  tool_iters_since_review INTEGER NOT NULL DEFAULT 0,
  turns_since_review INTEGER NOT NULL DEFAULT 0,
  skill_issue_hints_since_review INTEGER NOT NULL DEFAULT 0,
  last_review_at TEXT,
  review_running INTEGER NOT NULL DEFAULT 0
);
```

- [ ] **Step 4: Register v20 migration**

In `crates/right-db/src/migrations.rs`, add:

```rust
const V20_SCHEMA: &str = include_str!("sql/v20_learned_skills.sql");
```

Then append it after `M::up(V19_SCHEMA),`:

```rust
M::up(V20_SCHEMA),
```

- [ ] **Step 5: Run migration tests and verify pass**

Run:

```bash
devenv shell -- cargo test -p right-db learned_skills
```

Expected: PASS.

- [ ] **Step 6: Write failing domain-helper tests**

Create `crates/right-agent/src/learned_skills.rs` with the tests first:

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
    fn successful_finish_exists_only_for_created_or_updated() {
        let conn = conn();
        insert_learning_event(
            &conn,
            &LearningEvent {
                invocation_id: "inv-1".to_owned(),
                agent_name: "right".to_owned(),
                action: LearningAction::Create,
                skill_name: "rightx-demo".to_owned(),
                phase: LearningPhase::Finish,
                status: Some(LearningStatus::Failed),
                reason: None,
                message: None,
                summary: Some("write failed".to_owned()),
                event_refs: vec![],
            },
        )
        .unwrap();
        assert!(!successful_finish_exists(&conn, "inv-1").unwrap());

        insert_learning_event(
            &conn,
            &LearningEvent {
                invocation_id: "inv-1".to_owned(),
                agent_name: "right".to_owned(),
                action: LearningAction::Create,
                skill_name: "rightx-demo".to_owned(),
                phase: LearningPhase::Finish,
                status: Some(LearningStatus::Created),
                reason: None,
                message: Some("Learned skill: rightx-demo".to_owned()),
                summary: Some("captured workflow".to_owned()),
                event_refs: vec!["e1".to_owned(), "e2".to_owned()],
            },
        )
        .unwrap();
        assert!(successful_finish_exists(&conn, "inv-1").unwrap());
    }

    #[test]
    fn record_nudge_signal_persists_payload_and_updates_counter() {
        let conn = conn();
        record_nudge_signal(
            &conn,
            &NudgeSignalRecord {
                invocation_id: "inv-2".to_owned(),
                agent_name: "right".to_owned(),
                root_session_id: Some("root-1".to_owned()),
                chat_id: Some(10),
                thread_id: Some(20),
                signal_kind: NudgeSignalKind::SkillIssue,
                payload_json: serde_json::json!({"kind":"update_candidate"}),
            },
        )
        .unwrap();

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM skill_nudge_signals", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);

        let hints: i64 = conn
            .query_row(
                "SELECT skill_issue_hints_since_review FROM skill_nudge_state WHERE agent_name='right'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(hints, 1);
    }
}
```

- [ ] **Step 7: Run domain-helper tests and verify failure**

Run:

```bash
devenv shell -- cargo test -p right-agent learned_skills
```

Expected: FAIL because the public types/functions do not exist yet.

- [ ] **Step 8: Implement domain types and helpers**

In `crates/right-agent/src/learned_skills.rs`, add the implementation above the tests:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LearningAction {
    Create,
    Update,
}

impl LearningAction {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Create => "create",
            Self::Update => "update",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LearningPhase {
    Start,
    Finish,
}

impl LearningPhase {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::Finish => "finish",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LearningStatus {
    Created,
    Updated,
    Aborted,
    Failed,
}

impl LearningStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::Updated => "updated",
            Self::Aborted => "aborted",
            Self::Failed => "failed",
        }
    }

    pub fn is_success(self) -> bool {
        matches!(self, Self::Created | Self::Updated)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LearningEvent {
    pub invocation_id: String,
    pub agent_name: String,
    pub action: LearningAction,
    pub skill_name: String,
    pub phase: LearningPhase,
    pub status: Option<LearningStatus>,
    pub reason: Option<String>,
    pub message: Option<String>,
    pub summary: Option<String>,
    pub event_refs: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NudgeSignalKind {
    Learning,
    SkillIssue,
}

impl NudgeSignalKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Learning => "learning",
            Self::SkillIssue => "skill_issue",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct NudgeSignalRecord {
    pub invocation_id: String,
    pub agent_name: String,
    pub root_session_id: Option<String>,
    pub chat_id: Option<i64>,
    pub thread_id: Option<i64>,
    pub signal_kind: NudgeSignalKind,
    pub payload_json: serde_json::Value,
}

pub fn insert_learning_event(
    conn: &rusqlite::Connection,
    event: &LearningEvent,
) -> Result<(), rusqlite::Error> {
    let event_refs_json = serde_json::to_string(&event.event_refs)
        .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
    conn.execute(
        "INSERT INTO skill_learning_events \
         (invocation_id, agent_name, action, skill_name, phase, status, reason, message, summary, event_refs_json) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        rusqlite::params![
            event.invocation_id,
            event.agent_name,
            event.action.as_str(),
            event.skill_name,
            event.phase.as_str(),
            event.status.map(LearningStatus::as_str),
            event.reason,
            event.message,
            event.summary,
            event_refs_json,
        ],
    )?;
    ensure_nudge_state(conn, &event.agent_name)?;
    Ok(())
}

pub fn successful_finish_exists(
    conn: &rusqlite::Connection,
    invocation_id: &str,
) -> Result<bool, rusqlite::Error> {
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM skill_learning_events \
         WHERE invocation_id=?1 AND phase='finish' AND status IN ('created','updated')",
        [invocation_id],
        |r| r.get(0),
    )?;
    Ok(count > 0)
}

pub fn ensure_nudge_state(conn: &rusqlite::Connection, agent_name: &str) -> Result<(), rusqlite::Error> {
    conn.execute(
        "INSERT OR IGNORE INTO skill_nudge_state (agent_name) VALUES (?1)",
        [agent_name],
    )?;
    Ok(())
}

pub fn increment_turn_nudge_counters(
    conn: &rusqlite::Connection,
    agent_name: &str,
    tool_iters: i64,
) -> Result<(), rusqlite::Error> {
    ensure_nudge_state(conn, agent_name)?;
    conn.execute(
        "UPDATE skill_nudge_state \
         SET turns_since_review = turns_since_review + 1, \
             tool_iters_since_review = tool_iters_since_review + ?2 \
         WHERE agent_name = ?1",
        rusqlite::params![agent_name, tool_iters.max(0)],
    )?;
    Ok(())
}

pub fn record_nudge_signal(
    conn: &rusqlite::Connection,
    record: &NudgeSignalRecord,
) -> Result<(), rusqlite::Error> {
    ensure_nudge_state(conn, &record.agent_name)?;
    let payload = serde_json::to_string(&record.payload_json)
        .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
    conn.execute(
        "INSERT INTO skill_nudge_signals \
         (invocation_id, agent_name, root_session_id, chat_id, thread_id, signal_kind, payload_json) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        rusqlite::params![
            record.invocation_id,
            record.agent_name,
            record.root_session_id,
            record.chat_id,
            record.thread_id,
            record.signal_kind.as_str(),
            payload,
        ],
    )?;
    if matches!(record.signal_kind, NudgeSignalKind::SkillIssue) {
        conn.execute(
            "UPDATE skill_nudge_state \
             SET skill_issue_hints_since_review = skill_issue_hints_since_review + 1 \
             WHERE agent_name = ?1",
            [record.agent_name.as_str()],
        )?;
    }
    Ok(())
}
```

In `crates/right-agent/src/lib.rs`, add:

```rust
pub mod learned_skills;
```

- [ ] **Step 9: Run domain-helper tests and verify pass**

Run:

```bash
devenv shell -- cargo test -p right-agent learned_skills
```

Expected: PASS.

- [ ] **Step 10: Commit database/domain slice**

Run:

```bash
git add crates/right-db/src/sql/v20_learned_skills.sql crates/right-db/src/migrations.rs crates/right-agent/src/learned_skills.rs crates/right-agent/src/lib.rs
git commit -m "feat: add learned skills persistence"
```

Expected: commit succeeds.

---

### Task 3: Built-In `right-learn-skill`

**Files:**
- Create: `crates/right-codegen/skills/right-learn-skill/SKILL.md`
- Modify: `crates/right-codegen/src/skills.rs`

- [ ] **Step 1: Write failing built-in skill tests**

In `crates/right-codegen/src/skills.rs`, add these tests:

```rust
#[test]
fn installs_right_learn_skill() {
    let dir = tempdir().unwrap();
    install_builtin_skills(dir.path(), &MemoryProvider::File).unwrap();
    let path = dir.path().join(".claude/skills/right-learn-skill/SKILL.md");
    assert!(path.exists(), "right-learn-skill/SKILL.md should exist");
}

#[test]
fn right_learn_skill_mentions_protocol_and_boundaries() {
    let dir = tempdir().unwrap();
    install_builtin_skills(dir.path(), &MemoryProvider::File).unwrap();
    let content =
        std::fs::read_to_string(dir.path().join(".claude/skills/right-learn-skill/SKILL.md"))
            .unwrap();

    for needle in [
        "mcp__right__skill_learning_start",
        "mcp__right__skill_learning_finish",
        "rightx-",
        ".claude/skills/",
        "source: \"learned\"",
        "Do not call mcp__right__send_progress just to announce learning",
        "core/platform/bundled/codegen-owned",
        "scripts/",
        "references/",
        "assets/",
    ] {
        assert!(
            content.contains(needle),
            "right-learn-skill must mention {needle:?}"
        );
    }
}
```

Also add `("right-learn-skill", "right-learn-skill"),` to the `all_source_skill_files_are_installed` test data.

- [ ] **Step 2: Run tests and verify failure**

Run:

```bash
devenv shell -- cargo test -p right-codegen right_learn_skill
```

Expected: FAIL because the skill is not embedded or installed yet.

- [ ] **Step 3: Create the skill instructions**

Create `crates/right-codegen/skills/right-learn-skill/SKILL.md` with:

```markdown
---
name: right-learn-skill
description: >-
  Use when real work reveals a reusable workflow, recovered tool/API surprise,
  durable user correction, or problem with a rightx-* learned skill that should
  be captured for future sessions.
version: 0.1.0
compatibility: Uses standard Claude Code Agent Skills in .claude/skills.
---

# /right-learn-skill -- Learn Or Update Skills

Use this skill only when the lesson is reusable across future sessions.

## Create A New Skill

Create a new skill when at least one trigger is true:

- The user explicitly asked you to learn, save, or remember the workflow.
- The task required several non-obvious repeated steps.
- A command, tool, API, or MCP call failed or returned an unexpected shape, and you found a verified reusable path.
- The user corrected your approach and the correction is a durable gotcha.
- You discovered a repeated tool/API usage pattern likely to recur.

New skills created by Right learning must use a `rightx-` package name:

```text
.claude/skills/rightx-<slug>/SKILL.md
```

Use lowercase ASCII letters, digits, and hyphens. Do not use absolute paths.

## Update An Existing Skill

Update an existing `rightx-*` learned skill when it was materially wrong or incomplete:

- missing required step
- stale command or API behavior
- wrong API assumption
- overbroad activation
- broken script
- unsafe instruction

You may update only existing `rightx-*` learned skills. Do not update custom, manually installed, hub-installed, core/platform/bundled, or codegen-owned skills through this learning flow.

## Skip

Do not create a skill for one-off task details, temporary project progress, generic memory facts, unverified workarounds, or failed attempts without a verified path.

## Required Protocol

Before writing or patching any skill package file, call:

```text
mcp__right__skill_learning_start
```

Use `action: "create"` for new `rightx-*` skills and `action: "update"` for existing `rightx-*` skills. Include a short localized message that tells the user what is being learned or updated.

Do not call `mcp__right__send_progress` just to announce learning. The learning start tool sends the user-visible progress message.

After the write succeeds or fails, call:

```text
mcp__right__skill_learning_finish
```

Use `status: "created"` or `status: "updated"` only after the package files are written. Use `status: "failed"` or `status: "aborted"` when the write did not complete.

Successful finish calls send the learned/updated receipt. Failure finish calls record evidence and do not send a success receipt.

## Package Shape

Use the full Agent Skills format:

```text
.claude/skills/<skill_name>/
  SKILL.md
  scripts/
  references/
  assets/
```

Include `scripts/`, `references/`, or `assets/` only when they remove real complexity from future use.

Update `.claude/skills/installed.json` for new learned skills with `source: "learned"` and `path: ".claude/skills/rightx-<slug>"`.

## Skill Quality

Write `description` so the skill loads only for the right future tasks. Prefer concrete triggers over broad categories.

In `SKILL.md`, include:

- when to use the skill
- exact steps that worked
- tool/API gotchas
- verification command or success check
- when not to use it
- that future use of this `rightx-*` learned skill should emit a short localized `used_skill_receipts` message when it materially guides the answer

Do not store secrets. Do not copy large transcripts. Keep references focused.

## Deferred Signal

If the conversation is still evolving or a full-context review is safer, do not write a half-baked skill. Instead, leave at most one hidden structured output signal:

- `learning_signal` for a new `rightx-*` skill candidate
- `skill_issue_signal` for an existing `rightx-*` learned skill problem

Emit no signal after a successful `mcp__right__skill_learning_finish`. Emit at most one signal, never both. Use 1 non-empty event ref for an explicit user request and 2+ non-empty event refs for every other trigger. Do not emit a signal for weak hunches, one-off facts, or unverified failures.
```

- [ ] **Step 4: Embed and install the skill**

In `crates/right-codegen/src/skills.rs`, add:

```rust
const SKILL_RIGHT_LEARN_SKILL: Dir =
    include_dir!("$CARGO_MANIFEST_DIR/skills/right-learn-skill");
```

Add `"right-learn-skill",` to `BUILTIN_SKILL_NAMES`.

Add this match arm in `builtin_skill_dir`:

```rust
"right-learn-skill" => Ok(&SKILL_RIGHT_LEARN_SKILL),
```

- [ ] **Step 5: Run built-in skill tests and verify pass**

Run:

```bash
devenv shell -- cargo test -p right-codegen right_learn_skill
```

Expected: PASS.

- [ ] **Step 6: Commit built-in skill slice**

Run:

```bash
git add crates/right-codegen/skills/right-learn-skill/SKILL.md crates/right-codegen/src/skills.rs
git commit -m "feat: add right learn skill"
```

Expected: commit succeeds.

---

### Task 4: Learning MCP Tools

**Files:**
- Modify: `crates/right-mcp/src/internal_client.rs`
- Create: `crates/right/src/learning.rs`
- Modify: `crates/right/src/main.rs`
- Modify: `crates/right/src/progress.rs`
- Modify: `crates/right/src/right_backend.rs`
- Modify: `crates/right/src/right_backend_tests.rs`
- Modify: `crates/right/src/memory_server.rs`
- Modify: `crates/right/src/aggregator.rs`

- [ ] **Step 1: Add failing RightBackend tests**

In `crates/right/src/right_backend_tests.rs`, update `tools_list_returns_expected_count` expected count from 10 to 12, and add:

```rust
#[test]
fn tools_list_includes_learning_tools() {
    let (backend, _, _tmp) = make_backend();
    let names: Vec<&str> = backend
        .tools_list()
        .iter()
        .map(|t| t.name.as_ref())
        .collect();

    assert!(names.contains(&"skill_learning_start"));
    assert!(names.contains(&"skill_learning_finish"));
}

#[tokio::test]
async fn skill_learning_start_rejects_create_without_learned_prefix() {
    let (backend, agents_dir, _tmp) = make_backend();
    let agent_dir = create_agent_dir(&agents_dir, "test-agent");

    let result = backend
        .tools_call(
            "test-agent",
            &agent_dir,
            "skill_learning_start",
            json!({
                "action": "create",
                "skill_name": "notion-database-filters",
                "reason": "recovered_surprise",
                "event_refs": ["e1", "e2"],
                "message": "Learning a reusable Notion filter skill."
            }),
            crate::progress::ToolCallContext { invocation_id: Some("inv-1".to_owned()) },
        )
        .await
        .expect("tool should return operation error");

    assert_eq!(result.is_error, Some(true));
    let body = extract_error_body(&result);
    assert_eq!(body["error"]["code"], "invalid_argument");
}

#[tokio::test]
async fn skill_learning_start_rejects_core_skill_update() {
    let (backend, agents_dir, _tmp) = make_backend();
    let agent_dir = create_agent_dir(&agents_dir, "test-agent");

    let result = backend
        .tools_call(
            "test-agent",
            &agent_dir,
            "skill_learning_start",
            json!({
                "action": "update",
                "skill_name": "right-cron",
                "reason": "stale_command",
                "event_refs": ["e1", "e2"],
                "message": "Updating right-cron."
            }),
            crate::progress::ToolCallContext { invocation_id: Some("inv-1".to_owned()) },
        )
        .await
        .expect("tool should return operation error");

    assert_eq!(result.is_error, Some(true));
    let body = extract_error_body(&result);
    assert_eq!(body["error"]["code"], "skill_core_readonly");
}

#[tokio::test]
async fn skill_learning_start_rejects_non_learned_update() {
    let (backend, agents_dir, _tmp) = make_backend();
    let agent_dir = create_agent_dir(&agents_dir, "test-agent");
    let skill_dir = agent_dir.join(".claude/skills/custom-skill");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(skill_dir.join("SKILL.md"), "# custom").unwrap();

    let progress = backend.progress_registry();
    progress
        .register(crate::progress::ProgressRegistration {
            invocation_id: "inv-1".to_owned(),
            kind: crate::progress::ProgressInvocationKind::Foreground,
            bot_socket_path: agent_dir.join("missing-bot.sock"),
            bot_send_token: "send-token".to_owned(),
        })
        .await;

    let result = backend
        .tools_call(
            "test-agent",
            &agent_dir,
            "skill_learning_start",
            json!({
                "action": "update",
                "skill_name": "custom-skill",
                "reason": "missing_step",
                "event_refs": ["e1", "e2"],
                "message": "Updating a reusable custom skill."
            }),
            crate::progress::ToolCallContext { invocation_id: Some("inv-1".to_owned()) },
        )
        .await
        .expect("tool should return operation error when bot UDS is missing");

    assert_eq!(result.is_error, Some(true));
    let body = extract_error_body(&result);
    assert_eq!(body["error"]["code"], "invalid_argument");
}

#[tokio::test]
async fn skill_learning_start_rejects_update_when_package_missing() {
    let (backend, agents_dir, _tmp) = make_backend();
    let agent_dir = create_agent_dir(&agents_dir, "test-agent");

    let result = backend
        .tools_call(
            "test-agent",
            &agent_dir,
            "skill_learning_start",
            json!({
                "action": "update",
                "skill_name": "rightx-custom-skill",
                "reason": "missing_step",
                "event_refs": ["e1", "e2"],
                "message": "Updating a reusable learned skill."
            }),
            crate::progress::ToolCallContext { invocation_id: Some("inv-1".to_owned()) },
        )
        .await
        .expect("tool should return operation error");

    assert_eq!(result.is_error, Some(true));
    let body = extract_error_body(&result);
    assert_eq!(body["error"]["code"], "skill_package_missing");
}

#[tokio::test]
async fn skill_learning_finish_requires_receipt_message_for_success() {
    let (backend, agents_dir, _tmp) = make_backend();
    let agent_dir = create_agent_dir(&agents_dir, "test-agent");
    let skill_dir = agent_dir.join(".claude/skills/rightx-demo");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(skill_dir.join("SKILL.md"), "# demo").unwrap();

    let result = backend
        .tools_call(
            "test-agent",
            &agent_dir,
            "skill_learning_finish",
            json!({
                "action": "create",
                "skill_name": "rightx-demo",
                "status": "created",
                "summary": "Captured reusable steps.",
                "event_refs": ["e1", "e2"]
            }),
            crate::progress::ToolCallContext { invocation_id: Some("inv-1".to_owned()) },
        )
        .await
        .expect("tool should return operation error");

    assert_eq!(result.is_error, Some(true));
    let body = extract_error_body(&result);
    assert_eq!(body["error"]["code"], "invalid_argument");
}

#[tokio::test]
async fn skill_learning_finish_rejects_success_when_package_missing() {
    let (backend, agents_dir, _tmp) = make_backend();
    let agent_dir = create_agent_dir(&agents_dir, "test-agent");

    let result = backend
        .tools_call(
            "test-agent",
            &agent_dir,
            "skill_learning_finish",
            json!({
                "action": "create",
                "skill_name": "rightx-demo",
                "status": "created",
                "message": "Learned skill: rightx-demo.",
                "summary": "Captured reusable steps.",
                "event_refs": ["e1", "e2"]
            }),
            crate::progress::ToolCallContext { invocation_id: Some("inv-1".to_owned()) },
        )
        .await
        .expect("tool should return operation error");

    assert_eq!(result.is_error, Some(true));
    let body = extract_error_body(&result);
    assert_eq!(body["error"]["code"], "skill_package_missing");
}
```

- [ ] **Step 2: Run backend tests and verify failure**

Run:

```bash
devenv shell -- cargo test -p right skill_learning_start
```

Expected: FAIL because the learning tools are not defined.

- [ ] **Step 3: Add MCP constants**

In `crates/right-mcp/src/internal_client.rs`, add:

```rust
pub const SKILL_LEARNING_START_TOOL: &str = "skill_learning_start";
pub const SKILL_LEARNING_FINISH_TOOL: &str = "skill_learning_finish";
pub const SKILL_LEARNING_START_MCP_TOOL: &str = "mcp__right__skill_learning_start";
pub const SKILL_LEARNING_FINISH_MCP_TOOL: &str = "mcp__right__skill_learning_finish";
```

- [ ] **Step 4: Add unrate-limited learning target lookup**

In `crates/right/src/progress.rs`, expand the invocation kind:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProgressInvocationKind {
    Foreground,
    BackgroundReview,
    #[cfg(test)]
    NonForeground,
}
```

Then add this method to `impl ProgressRegistry`:

```rust
pub(crate) async fn learning_send_target(
    &self,
    invocation_id: &str,
) -> Result<(ProgressInvocationKind, ProgressSendTarget), ProgressError> {
    let inner = self.inner.lock().await;
    let invocation = inner.get(invocation_id).ok_or(ProgressError::Unavailable)?;
    if !matches!(
        invocation.kind,
        ProgressInvocationKind::Foreground | ProgressInvocationKind::BackgroundReview
    ) {
        return Err(ProgressError::Forbidden);
    }
    Ok((
        invocation.kind,
        ProgressSendTarget {
            bot_socket_path: invocation.bot_socket_path.clone(),
            bot_send_token: invocation.bot_send_token.clone(),
        },
    ))
}
```

Add a unit test in the same file:

```rust
#[tokio::test(start_paused = true)]
async fn learning_send_target_does_not_consume_progress_rate_limit() {
    let registry = ProgressRegistry::default();
    registry.register(foreground_registration()).await;

    registry.learning_send_target("inv-1").await.unwrap();
    registry.learning_send_target("inv-1").await.unwrap();
    registry.begin_send("inv-1").await.unwrap();
}
```

In `crates/right-mcp/src/internal_client.rs`, expand `ProgressInvocationKindDto`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProgressInvocationKindDto {
    Foreground,
    BackgroundReview,
}
```

In `crates/right/src/internal_api.rs`, map both DTO values to the internal enum. Stage 1 should still register only `Foreground` from Telegram worker; `BackgroundReview` is reserved for the future background review worker and must not be used by cron/reflection/delivery paths.

- [ ] **Step 5: Implement learning MCP params and validators**

Create `crates/right/src/learning.rs` with:

```rust
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::Context as _;
use right_mcp::internal_client::ProgressSendRequest;
use right_mcp::tool_error::tool_error;
use rmcp::model::{CallToolResult, Content};
use schemars::JsonSchema;
use serde::Deserialize;

const LEARNING_SEND_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum LearningActionParam {
    Create,
    Update,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct SkillLearningStartParams {
    pub(crate) action: LearningActionParam,
    pub(crate) skill_name: String,
    pub(crate) reason: String,
    #[serde(default)]
    pub(crate) event_refs: Vec<String>,
    pub(crate) message: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum LearningFinishStatusParam {
    Created,
    Updated,
    Aborted,
    Failed,
}

impl LearningFinishStatusParam {
    pub(crate) fn is_success(&self) -> bool {
        matches!(
            self,
            LearningFinishStatusParam::Created | LearningFinishStatusParam::Updated
        )
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct SkillLearningFinishParams {
    pub(crate) action: LearningActionParam,
    pub(crate) skill_name: String,
    pub(crate) status: LearningFinishStatusParam,
    pub(crate) message: Option<String>,
    pub(crate) summary: Option<String>,
    #[serde(default)]
    pub(crate) event_refs: Vec<String>,
}

pub(crate) fn validate_skill_name(skill_name: &str) -> Result<(), String> {
    let len = skill_name.chars().count();
    if !(3..=80).contains(&len) {
        return Err("skill_name must be 3 to 80 characters".to_owned());
    }
    if skill_name.starts_with('-') || skill_name.ends_with('-') {
        return Err("skill_name must not start or end with '-'".to_owned());
    }
    if !skill_name
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        return Err("skill_name may contain only lowercase ASCII letters, digits, and hyphens".to_owned());
    }
    Ok(())
}

pub(crate) fn skill_package_dir(agent_dir: &Path, skill_name: &str) -> Result<PathBuf, String> {
    validate_skill_name(skill_name)?;
    let base = agent_dir.join(".claude").join("skills");
    let path = base.join(skill_name);
    if !path.starts_with(&base) {
        return Err("derived skill path escaped .claude/skills".to_owned());
    }
    Ok(path)
}

async fn skill_package_exists(
    agent_name: &str,
    mtls_dir: Option<&Path>,
    agent_dir: &Path,
    skill_name: &str,
) -> Result<bool, anyhow::Error> {
    if let Some(mtls_dir) = mtls_dir {
        let parsed_config = right_agent::agent::parse_agent_config(agent_dir);
        let (sandboxed, explicit_sandbox_name) = match parsed_config {
            Ok(Some(config)) => (
                *config.sandbox_mode() == right_agent::agent::SandboxMode::Openshell,
                config
                    .sandbox
                    .as_ref()
                    .and_then(|sandbox| sandbox.name.as_deref())
                    .map(str::to_owned),
            ),
            Ok(None) | Err(_) => (true, None),
        };
        if sandboxed {
            let sandbox_name = right_openshell::openshell::resolve_sandbox_name(
                agent_name,
                explicit_sandbox_name.as_deref(),
            );
            let mut client = right_openshell::openshell::connect_grpc(mtls_dir)
                .await
                .map_err(|e| anyhow::anyhow!("{e:#}"))
                .context("skill package check: failed to connect to OpenShell gRPC")?;
            let sandbox_id =
                right_openshell::openshell::resolve_sandbox_id(&mut client, &sandbox_name)
                    .await
                    .map_err(|e| anyhow::anyhow!("{e:#}"))
                    .context("skill package check: failed to resolve sandbox ID")?;
            let skill_path = format!("/sandbox/.claude/skills/{skill_name}/SKILL.md");
            let (_, exit_code) = right_openshell::openshell::exec_in_sandbox(
                &mut client,
                &sandbox_id,
                &["test", "-f", &skill_path],
                right_openshell::openshell::DEFAULT_EXEC_TIMEOUT_SECS,
            )
            .await
            .map_err(|e| anyhow::anyhow!("{e:#}"))
            .with_context(|| format!("skill package check: test -f {skill_path} failed"))?;
            return Ok(exit_code == 0);
        }
    }

    Ok(skill_package_dir(agent_dir, skill_name)
        .map_err(|message| anyhow::anyhow!(message))?
        .join("SKILL.md")
        .is_file())
}

pub(crate) fn is_known_core_skill(skill_name: &str) -> bool {
    right_codegen::BUILTIN_SKILL_NAMES.contains(&skill_name)
        || right_codegen::BUILTIN_SKILL_LEGACY_NAMES.contains(&skill_name)
}

pub(crate) fn installed_json_marks_core(agent_dir: &Path, skill_name: &str) -> bool {
    let path = agent_dir.join(".claude/skills/installed.json");
    let Ok(content) = std::fs::read_to_string(path) else {
        return false;
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&content) else {
        return false;
    };
    let Some(entry) = value.get(skill_name) else {
        return false;
    };
    let source = entry
        .get("source")
        .and_then(|v| v.as_str())
        .or_else(|| entry.as_str())
        .unwrap_or("");
    matches!(source, "builtin" | "platform" | "core" | "codegen")
}

pub(crate) fn validate_learning_target(
    agent_dir: &Path,
    action: &LearningActionParam,
    skill_name: &str,
) -> Result<(), CallToolResult> {
    if let Err(message) = validate_skill_name(skill_name) {
        return Err(tool_error("invalid_argument", message, None));
    }
    if matches!(action, LearningActionParam::Create) && !skill_name.starts_with("rightx-") {
        return Err(tool_error(
            "invalid_argument",
            "action=create requires skill_name to start with rightx-",
            None,
        ));
    }
    if is_known_core_skill(skill_name) || installed_json_marks_core(agent_dir, skill_name) {
        return Err(tool_error(
            "skill_core_readonly",
            format!("skill {skill_name:?} is core/platform-owned and cannot be modified by learning"),
            None,
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum SkillPackageExpectation {
    MustExist,
    MustNotExist,
}

pub(crate) async fn validate_skill_package_state(
    agent_name: &str,
    mtls_dir: Option<&Path>,
    agent_dir: &Path,
    skill_name: &str,
    expectation: SkillPackageExpectation,
) -> Result<(), CallToolResult> {
    let exists = skill_package_exists(agent_name, mtls_dir, agent_dir, skill_name)
        .await
        .map_err(|e| {
            tool_error(
                "skill_package_check_failed",
                format!("{e:#}"),
                None,
            )
        })?;
    match (expectation, exists) {
        (SkillPackageExpectation::MustExist, true) => Ok(()),
        (SkillPackageExpectation::MustExist, false) => Err(tool_error(
            "skill_package_missing",
            format!("skill package {skill_name:?} must exist under .claude/skills/{skill_name}/SKILL.md"),
            None,
        )),
        (SkillPackageExpectation::MustNotExist, false) => Ok(()),
        (SkillPackageExpectation::MustNotExist, true) => Err(tool_error(
            "skill_already_exists",
            format!("skill package {skill_name:?} already exists"),
            None,
        )),
    }
}

pub(crate) fn validate_finish_receipt_message<'a>(
    status: &LearningFinishStatusParam,
    message: Option<&'a str>,
) -> Result<Option<&'a str>, CallToolResult> {
    if !status.is_success() {
        return Ok(None);
    }
    let Some(message) = message.map(str::trim).filter(|message| !message.is_empty()) else {
        return Err(tool_error(
            "invalid_argument",
            "successful skill_learning_finish requires an LLM-authored receipt message",
            None,
        ));
    };
    if message.chars().count() > right_mcp::internal_client::PROGRESS_MESSAGE_MAX_CHARS {
        return Err(tool_error(
            "invalid_argument",
            "receipt message must be at most 2000 characters",
            None,
        ));
    }
    Ok(Some(message))
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum LearningMessagePhase {
    Start,
    FinishSuccess,
}

pub(crate) async fn send_learning_message(
    progress: &crate::progress::ProgressRegistry,
    context: &crate::progress::ToolCallContext,
    phase: LearningMessagePhase,
    message: &str,
) -> Result<(), CallToolResult> {
    let Some(invocation_id) = context.invocation_id.as_deref() else {
        return Err(tool_error(
            "learning_unavailable",
            "learning messages require foreground invocation context",
            None,
        ));
    };
    let (kind, target) = progress
        .learning_send_target(invocation_id)
        .await
        .map_err(|_| {
            tool_error(
                "learning_unavailable",
                "learning messages require an active foreground invocation",
                None,
            )
        })?;
    if matches!(kind, crate::progress::ProgressInvocationKind::BackgroundReview)
        && matches!(phase, LearningMessagePhase::Start)
    {
        return Ok(());
    }
    let message = message.trim();
    if message.is_empty() || message.chars().count() > right_mcp::internal_client::PROGRESS_MESSAGE_MAX_CHARS {
        return Err(tool_error(
            "invalid_argument",
            "message must be non-empty and at most 2000 characters",
            None,
        ));
    }
    let req = ProgressSendRequest {
        invocation_id: invocation_id.to_owned(),
        token: target.bot_send_token,
        message: message.to_owned(),
    };
    let client = right_mcp::internal_client::InternalClient::new(target.bot_socket_path);
    let result = tokio::time::timeout(LEARNING_SEND_TIMEOUT, client.progress_send(&req)).await;
    match result {
        Ok(Ok(_)) => Ok(()),
        Ok(Err(e)) => Err(tool_error("learning_send_failed", format!("{e:#}"), None)),
        Err(_) => Err(tool_error(
            "learning_send_failed",
            "learning message send timed out",
            None,
        )),
    }
}

pub(crate) fn success_json(status: &str, skill_name: &str) -> Result<CallToolResult, anyhow::Error> {
    Ok(CallToolResult::success(vec![Content::text(
        serde_json::to_string_pretty(&serde_json::json!({
            "status": status,
            "skill_name": skill_name,
        }))
        .context("serialize learning tool response")?,
    )]))
}
```

- [ ] **Step 6: Add module declaration**

In `crates/right/src/main.rs`, add:

```rust
pub(crate) mod learning;
```

- [ ] **Step 7: Expose and dispatch tools in RightBackend**

In `crates/right/src/right_backend.rs`, import:

```rust
use crate::learning::{SkillLearningFinishParams, SkillLearningStartParams};
```

Add two `Tool::new(...)` entries after `send_progress`:

```rust
Tool::new(
    right_mcp::internal_client::SKILL_LEARNING_START_TOOL,
    "Announce and record that the foreground agent is starting to create or update a learned skill package. Metadata/progress only; accepts skill names, never paths. action=create and action=update both require rightx-* skill names.",
    schema_for_type::<SkillLearningStartParams>(),
),
Tool::new(
    right_mcp::internal_client::SKILL_LEARNING_FINISH_TOOL,
    "Record the result of a skill create/update attempt. Successful statuses require an LLM-authored receipt message, verify .claude/skills/<skill_name>/SKILL.md exists, and send the learned/updated receipt. Metadata/receipt only; does not move skill files.",
    schema_for_type::<SkillLearningFinishParams>(),
),
```

Add dispatch arms:

```rust
right_mcp::internal_client::SKILL_LEARNING_START_TOOL => {
    self.call_skill_learning_start(agent_name, agent_dir, context, &args).await
}
right_mcp::internal_client::SKILL_LEARNING_FINISH_TOOL => {
    self.call_skill_learning_finish(agent_name, agent_dir, context, &args).await
}
```

Add these methods in `impl RightBackend`:

```rust
async fn call_skill_learning_start(
    &self,
    agent_name: &str,
    agent_dir: &Path,
    context: crate::progress::ToolCallContext,
    args: &serde_json::Value,
) -> Result<CallToolResult, anyhow::Error> {
    let params: SkillLearningStartParams =
        serde_json::from_value(args.clone()).context("invalid skill_learning_start params")?;
    if let Err(err) =
        crate::learning::validate_learning_target(agent_dir, &params.action, &params.skill_name)
    {
        return Ok(err);
    }
    let package_expectation = match params.action {
        crate::learning::LearningActionParam::Create => {
            crate::learning::SkillPackageExpectation::MustNotExist
        }
        crate::learning::LearningActionParam::Update => {
            crate::learning::SkillPackageExpectation::MustExist
        }
    };
    if let Err(err) = crate::learning::validate_skill_package_state(
        agent_name,
        self.mtls_dir.as_deref(),
        agent_dir,
        &params.skill_name,
        package_expectation,
    )
    .await
    {
        return Ok(err);
    }
    let Some(invocation_id) = context.invocation_id.clone() else {
        return Ok(tool_error(
            "learning_unavailable",
            "skill_learning_start requires foreground invocation context",
            None,
        ));
    };
    let conn_arc = self.get_conn(agent_name)?;
    {
        let conn = Self::lock_conn(&conn_arc)?;
        right_agent::learned_skills::insert_learning_event(
            &conn,
            &right_agent::learned_skills::LearningEvent {
                invocation_id: invocation_id.clone(),
                agent_name: agent_name.to_owned(),
                action: match params.action {
                    crate::learning::LearningActionParam::Create => {
                        right_agent::learned_skills::LearningAction::Create
                    }
                    crate::learning::LearningActionParam::Update => {
                        right_agent::learned_skills::LearningAction::Update
                    }
                },
                skill_name: params.skill_name.clone(),
                phase: right_agent::learned_skills::LearningPhase::Start,
                status: None,
                reason: Some(params.reason.clone()),
                message: Some(params.message.clone()),
                summary: None,
                event_refs: params.event_refs.clone(),
            },
        )?;
    }
    if let Err(err) = crate::learning::send_learning_message(
        &self.progress,
        &context,
        crate::learning::LearningMessagePhase::Start,
        &params.message,
    )
    .await
    {
        return Ok(err);
    }
    crate::learning::success_json("started", &params.skill_name)
}

async fn call_skill_learning_finish(
    &self,
    agent_name: &str,
    agent_dir: &Path,
    context: crate::progress::ToolCallContext,
    args: &serde_json::Value,
) -> Result<CallToolResult, anyhow::Error> {
    let params: SkillLearningFinishParams =
        serde_json::from_value(args.clone()).context("invalid skill_learning_finish params")?;
    if let Err(err) =
        crate::learning::validate_learning_target(agent_dir, &params.action, &params.skill_name)
    {
        return Ok(err);
    }
    let receipt_message =
        crate::learning::validate_finish_receipt_message(&params.status, params.message.as_deref());
    let receipt_message = match receipt_message {
        Ok(message) => message.map(str::to_owned),
        Err(err) => return Ok(err),
    };
    let Some(invocation_id) = context.invocation_id.clone() else {
        return Ok(tool_error(
            "learning_unavailable",
            "skill_learning_finish requires foreground invocation context",
            None,
        ));
    };
    let status = match params.status {
        crate::learning::LearningFinishStatusParam::Created => {
            right_agent::learned_skills::LearningStatus::Created
        }
        crate::learning::LearningFinishStatusParam::Updated => {
            right_agent::learned_skills::LearningStatus::Updated
        }
        crate::learning::LearningFinishStatusParam::Aborted => {
            right_agent::learned_skills::LearningStatus::Aborted
        }
        crate::learning::LearningFinishStatusParam::Failed => {
            right_agent::learned_skills::LearningStatus::Failed
        }
    };
    if status.is_success()
        && let Err(err) = crate::learning::validate_skill_package_state(
            agent_name,
            self.mtls_dir.as_deref(),
            agent_dir,
            &params.skill_name,
            crate::learning::SkillPackageExpectation::MustExist,
        )
        .await
    {
        return Ok(err);
    }
    let conn_arc = self.get_conn(agent_name)?;
    {
        let conn = Self::lock_conn(&conn_arc)?;
        right_agent::learned_skills::insert_learning_event(
            &conn,
            &right_agent::learned_skills::LearningEvent {
                invocation_id: invocation_id.clone(),
                agent_name: agent_name.to_owned(),
                action: match params.action {
                    crate::learning::LearningActionParam::Create => {
                        right_agent::learned_skills::LearningAction::Create
                    }
                    crate::learning::LearningActionParam::Update => {
                        right_agent::learned_skills::LearningAction::Update
                    }
                },
                skill_name: params.skill_name.clone(),
                phase: right_agent::learned_skills::LearningPhase::Finish,
                status: Some(status),
                reason: None,
                message: params.message.clone(),
                summary: params.summary.clone(),
                event_refs: params.event_refs.clone(),
            },
        )?;
    }
    if status.is_success()
        && let Some(message) = receipt_message.as_deref()
        && let Err(err) = crate::learning::send_learning_message(
            &self.progress,
            &context,
            crate::learning::LearningMessagePhase::FinishSuccess,
            message,
        )
        .await
    {
        return Ok(err);
    }
    crate::learning::success_json(status.as_str(), &params.skill_name)
}
```

- [ ] **Step 8: Add stdio stubs and instructions**

In `crates/right/src/memory_server.rs`, add stub tool methods next to `send_progress` that return `learning_unavailable`:

```rust
#[tool(
    description = "DO NOT CALL in stdio mode. In HTTP foreground mode this records and announces the start of a skill create/update attempt. Accepts skill names only; action=create and action=update both require rightx-*."
)]
async fn skill_learning_start(
    &self,
    Parameters(_params): Parameters<crate::learning::SkillLearningStartParams>,
) -> Result<CallToolResult, McpError> {
    Ok(tool_error(
        "learning_unavailable",
        "skill_learning_start requires foreground HTTP aggregator context",
        None,
    ))
}

#[tool(
    description = "DO NOT CALL in stdio mode. In HTTP foreground mode this records a skill create/update result and sends successful learned/updated receipts."
)]
async fn skill_learning_finish(
    &self,
    Parameters(_params): Parameters<crate::learning::SkillLearningFinishParams>,
) -> Result<CallToolResult, McpError> {
    Ok(tool_error(
        "learning_unavailable",
        "skill_learning_finish requires foreground HTTP aggregator context",
        None,
    ))
}
```

Update `with_instructions()` in both `memory_server.rs` and `aggregator.rs` with:

```text
## Learning
- mcp__right__skill_learning_start: Stage 1 foreground metadata/progress for learned skill create/update. Call before writing or patching skill package files. action=create and action=update both require rightx-* skill names. Accepts skill names only, never paths.
- mcp__right__skill_learning_finish: Stage 1 foreground metadata/receipt for skill create/update completion. Successful statuses require a non-empty LLM-authored message argument, verify the skill package exists at .claude/skills/<skill_name>/SKILL.md, and send learned/updated receipts. Does not move files.
```

- [ ] **Step 9: Run targeted MCP tests and verify pass**

Run:

```bash
devenv shell -- cargo test -p right skill_learning
```

Expected: PASS.

Run:

```bash
devenv shell -- cargo test -p right all_tools_have_valid_input_schema
```

Expected: PASS.

- [ ] **Step 10: Commit MCP tool slice**

Run:

```bash
git add crates/right-mcp/src/internal_client.rs crates/right/src/learning.rs crates/right/src/main.rs crates/right/src/progress.rs crates/right/src/right_backend.rs crates/right/src/right_backend_tests.rs crates/right/src/memory_server.rs crates/right/src/aggregator.rs
git commit -m "feat: expose learned skill MCP tools"
```

Expected: commit succeeds.

---

### Task 5: Structured Output, Reply Parsing, And Nudge Persistence

**Files:**
- Modify: `crates/right-codegen/src/agent_def.rs`
- Modify: `crates/right-codegen/src/agent_def_tests.rs`
- Modify: `crates/bot/src/cc/worker_reply.rs`
- Modify: `crates/bot/src/telegram/worker.rs`
- Modify: `crates/right-agent/src/learned_skills.rs`

- [ ] **Step 1: Write failing schema tests**

In `crates/right-codegen/src/agent_def_tests.rs`, add:

```rust
#[test]
fn reply_schema_contains_learned_skill_fields() {
    let parsed: serde_json::Value = serde_json::from_str(REPLY_SCHEMA_JSON).unwrap();
    let props = parsed.get("properties").unwrap();
    for field in [
        "used_skill_receipts",
        "learning_signal",
        "skill_issue_signal",
    ] {
        assert!(props.get(field).is_some(), "missing {field}");
    }
    let required = parsed.get("required").unwrap().as_array().unwrap();
    assert!(
        required.iter().any(|v| v == "content"),
        "content remains required"
    );
    assert!(
        !required.iter().any(|v| v == "used_skill_receipts"),
        "used_skill_receipts must be optional"
    );
}
```

- [ ] **Step 2: Run schema test and verify failure**

Run:

```bash
devenv shell -- cargo test -p right-codegen reply_schema_contains_learned_skill_fields
```

Expected: FAIL because the schema does not contain the fields.

- [ ] **Step 3: Replace normal reply schema JSON**

In `crates/right-codegen/src/agent_def.rs`, replace `REPLY_SCHEMA_JSON` with this minified schema:

```rust
pub const REPLY_SCHEMA_JSON: &str = r#"{"type":"object","properties":{"content":{"type":["string","null"]},"reply_to_message_id":{"type":["integer","null"]},"attachments":{"type":["array","null"],"items":{"type":"object","properties":{"type":{"enum":["photo","document","video","audio","voice","video_note","sticker","animation"]},"path":{"type":"string"},"filename":{"type":["string","null"]},"caption":{"type":["string","null"]},"media_group_id":{"type":["string","null"]}},"required":["type","path"]}},"used_skill_receipts":{"type":["array","null"],"items":{"type":"object","properties":{"package_name":{"type":"string"},"message":{"type":"string"}},"required":["package_name","message"]}},"learning_signal":{"type":["object","null"],"properties":{"kind":{"const":"create_candidate"},"package_name_hint":{"type":"string"},"trigger":{"enum":["explicit_user_request","multi_step_workflow","recovered_surprise","user_correction","repeated_tool_pattern"]},"reason_not_written":{"enum":["conversation_still_evolving","needs_full_context_review","write_or_publish_failed","needs_existing_skill_diff"]},"event_refs":{"type":"array","items":{"type":"string"},"minItems":1},"summary":{"type":"string"}},"required":["kind","package_name_hint","trigger","reason_not_written","event_refs","summary"]},"skill_issue_signal":{"type":["object","null"],"properties":{"kind":{"const":"update_candidate"},"skill_name":{"type":"string"},"issue":{"enum":["missing_step","stale_command","wrong_api_assumption","overbroad_activation","broken_script","unsafe_instruction"]},"reason_not_patched":{"enum":["conversation_still_evolving","needs_full_context_review","write_or_publish_failed","needs_existing_skill_diff"]},"observed_effect":{"enum":["retry_after_tool_error","retry_after_user_correction","manual_override","verified_alternative"]},"event_refs":{"type":"array","items":{"type":"string"},"minItems":1},"patch_hint":{"type":"string"}},"required":["kind","skill_name","issue","reason_not_patched","observed_effect","event_refs","patch_hint"]}},"required":["content"]}"#;
```

Keep schema `minItems: 1` because `explicit_user_request` can be backed by a single event. The worker-side `select_reply_signal` runtime validation must reject one-event signals for all non-explicit triggers.

- [ ] **Step 4: Run schema tests and verify pass**

Run:

```bash
devenv shell -- cargo test -p right-codegen reply_schema
```

Expected: PASS.

- [ ] **Step 5: Write failing reply parser tests**

In `crates/bot/src/cc/worker_reply.rs`, add:

```rust
#[test]
fn parse_reply_output_accepts_used_skill_receipts() {
    let json = r#"{"result":{"content":"Done","used_skill_receipts":[{"package_name":"rightx-demo","message":"Used learned skill: rightx-demo"}],"learning_signal":null,"skill_issue_signal":null}}"#;
    let (output, _) = parse_reply_output(json).unwrap();
    let receipts = output.used_skill_receipts.unwrap();
    assert_eq!(receipts[0].package_name, "rightx-demo");
    assert_eq!(receipts[0].message, "Used learned skill: rightx-demo");
}

#[test]
fn parse_reply_output_accepts_learning_signal() {
    let json = r#"{"result":{"content":"Done","learning_signal":{"kind":"create_candidate","package_name_hint":"rightx-demo","trigger":"recovered_surprise","reason_not_written":"needs_full_context_review","event_refs":["e1","e2"],"summary":"Reusable gotcha."},"skill_issue_signal":null}}"#;
    let (output, _) = parse_reply_output(json).unwrap();
    assert!(output.learning_signal.is_some());
    assert!(output.skill_issue_signal.is_none());
}

#[test]
fn parse_reply_output_keeps_skill_fields_optional() {
    let json = r#"{"result":{"content":"Done"}}"#;
    let (output, _) = parse_reply_output(json).unwrap();
    assert!(output.used_skill_receipts.is_none());
    assert!(output.learning_signal.is_none());
    assert!(output.skill_issue_signal.is_none());
}
```

- [ ] **Step 6: Run parser tests and verify failure**

Run:

```bash
devenv shell -- cargo test -p right-bot parse_reply_output_accepts_used_skill_receipts
```

Expected: FAIL because `ReplyOutput` does not have those fields.

- [ ] **Step 7: Add parser structs**

In `crates/bot/src/cc/worker_reply.rs`, add:

```rust
#[derive(Debug, serde::Deserialize, Clone)]
pub struct UsedSkillReceipt {
    pub package_name: String,
    pub message: String,
}

#[derive(Debug, serde::Deserialize, Clone)]
pub struct LearningSignal {
    pub kind: String,
    pub package_name_hint: String,
    pub trigger: String,
    pub reason_not_written: String,
    pub event_refs: Vec<String>,
    pub summary: String,
}

#[derive(Debug, serde::Deserialize, Clone)]
pub struct SkillIssueSignal {
    pub kind: String,
    pub skill_name: String,
    pub issue: String,
    pub reason_not_patched: String,
    pub observed_effect: String,
    pub event_refs: Vec<String>,
    pub patch_hint: String,
}
```

Add fields to `ReplyOutput`:

```rust
pub used_skill_receipts: Option<Vec<UsedSkillReceipt>>,
pub learning_signal: Option<LearningSignal>,
pub skill_issue_signal: Option<SkillIssueSignal>,
```

When wrapping a plain string result, initialize those fields to `None`.

- [ ] **Step 8: Run parser tests and verify pass**

Run:

```bash
devenv shell -- cargo test -p right-bot parse_reply_output
```

Expected: PASS.

- [ ] **Step 9: Add nudge selection tests**

In `crates/right-agent/src/learned_skills.rs`, add tests:

```rust
#[test]
fn nudge_signal_is_dropped_when_successful_finish_exists() {
    let conn = conn();
    insert_learning_event(
        &conn,
        &LearningEvent {
            invocation_id: "inv-3".to_owned(),
            agent_name: "right".to_owned(),
            action: LearningAction::Create,
            skill_name: "rightx-demo".to_owned(),
            phase: LearningPhase::Finish,
            status: Some(LearningStatus::Created),
            reason: None,
            message: Some("Learned skill: rightx-demo".to_owned()),
            summary: Some("captured".to_owned()),
            event_refs: vec![],
        },
    )
    .unwrap();

    let selected = select_reply_signal(
        &conn,
        "inv-3",
        Some(serde_json::json!({"kind":"create_candidate","event_refs":["e1","e2"]})),
        None,
    )
    .unwrap();
    assert!(selected.is_none());
}

#[test]
fn nudge_signal_is_dropped_when_both_signals_present() {
    let conn = conn();
    let selected = select_reply_signal(
        &conn,
        "inv-4",
        Some(serde_json::json!({"kind":"create_candidate","event_refs":["e1","e2"]})),
        Some(serde_json::json!({"kind":"update_candidate","event_refs":["e3","e4"]})),
    )
    .unwrap();
    assert!(selected.is_none());
}

#[test]
fn nudge_signal_requires_two_event_refs_unless_explicit_user_request() {
    let conn = conn();
    let selected = select_reply_signal(
        &conn,
        "inv-5",
        Some(serde_json::json!({
            "kind": "create_candidate",
            "package_name_hint": "rightx-demo",
            "trigger": "recovered_surprise",
            "reason_not_written": "needs_full_context_review",
            "event_refs": ["e1"],
            "summary": "Recovered a reusable surprise."
        })),
        None,
    )
    .unwrap();
    assert!(selected.is_none());

    let accepted = select_reply_signal(
        &conn,
        "inv-6",
        Some(serde_json::json!({
            "kind": "create_candidate",
            "package_name_hint": "rightx-demo",
            "trigger": "explicit_user_request",
            "reason_not_written": "conversation_still_evolving",
            "event_refs": ["e1"],
            "summary": "User explicitly asked to learn this."
        })),
        None,
    )
    .unwrap();
    assert!(accepted.is_some());
}
```

- [ ] **Step 10: Implement nudge selector**

In `crates/right-agent/src/learned_skills.rs`, add:

```rust
pub fn select_reply_signal(
    conn: &rusqlite::Connection,
    invocation_id: &str,
    learning_signal: Option<serde_json::Value>,
    skill_issue_signal: Option<serde_json::Value>,
) -> Result<Option<(NudgeSignalKind, serde_json::Value)>, rusqlite::Error> {
    if successful_finish_exists(conn, invocation_id)? {
        return Ok(None);
    }
    match (learning_signal, skill_issue_signal) {
        (Some(_), Some(_)) => Ok(None),
        (Some(signal), None) if validate_nudge_signal(&signal) => {
            Ok(Some((NudgeSignalKind::Learning, signal)))
        }
        (None, Some(signal)) if validate_nudge_signal(&signal) => {
            Ok(Some((NudgeSignalKind::SkillIssue, signal)))
        }
        (Some(_), None) | (None, Some(_)) => Ok(None),
        (None, None) => Ok(None),
    }
}

fn validate_nudge_signal(signal: &serde_json::Value) -> bool {
    let event_count = signal
        .get("event_refs")
        .and_then(|value| value.as_array())
        .map_or(0, Vec::len);
    let explicit = signal
        .get("trigger")
        .and_then(|value| value.as_str())
        .is_some_and(|trigger| trigger == "explicit_user_request");
    if event_count < 2 && !explicit {
        return false;
    }
    let summary_ok = signal
        .get("summary")
        .or_else(|| signal.get("patch_hint"))
        .and_then(|value| value.as_str())
        .is_some_and(|text| !text.trim().is_empty());
    summary_ok
}
```

- [ ] **Step 11: Persist reply metadata in worker**

In `crates/bot/src/telegram/worker.rs`, before `finish_progress_invocation(ctx, active).await`, capture the invocation id:

```rust
let learning_invocation_id = active_progress
    .as_ref()
    .map(|active| active.invocation_id.clone());
```

After `parse_reply_output(&stdout_str)` succeeds and before returning `CcReply`, add:

```rust
if let Some(invocation_id) = learning_invocation_id.as_deref() {
    if let Err(e) = right_agent::learned_skills::increment_turn_nudge_counters(
        &conn,
        &ctx.agent_name,
        usage.num_turns as i64,
    ) {
        tracing::warn!(?chat_id, "failed to update learned-skill nudge counters: {e:#}");
    }

    let learning_signal = reply_output
        .learning_signal
        .as_ref()
        .and_then(|signal| serde_json::to_value(signal).ok());
    let skill_issue_signal = reply_output
        .skill_issue_signal
        .as_ref()
        .and_then(|signal| serde_json::to_value(signal).ok());

    match right_agent::learned_skills::select_reply_signal(
        &conn,
        invocation_id,
        learning_signal,
        skill_issue_signal,
    ) {
        Ok(Some((signal_kind, payload_json))) => {
            if let Err(e) = right_agent::learned_skills::record_nudge_signal(
                &conn,
                &right_agent::learned_skills::NudgeSignalRecord {
                    invocation_id: invocation_id.to_owned(),
                    agent_name: ctx.agent_name.clone(),
                    root_session_id: Some(session_uuid.clone()),
                    chat_id: Some(chat_id),
                    thread_id: Some(eff_thread_id),
                    signal_kind,
                    payload_json,
                },
            ) {
                tracing::warn!(?chat_id, "failed to record learned-skill nudge signal: {e:#}");
            }
        }
        Ok(None) => {}
        Err(e) => tracing::warn!(?chat_id, "failed to select learned-skill nudge signal: {e:#}"),
    }
}
```

- [ ] **Step 12: Append used-skill receipts to final content**

In `crates/bot/src/cc/worker_reply.rs`, add:

```rust
pub fn append_used_skill_receipts(
    content: Option<String>,
    receipts: Option<&[UsedSkillReceipt]>,
) -> Option<String> {
    let Some(receipts) = receipts.filter(|items| !items.is_empty()) else {
        return content;
    };
    let mut base = content.unwrap_or_default();
    if !base.trim().is_empty() {
        base.push_str("\n\n");
    }
    for (idx, receipt) in receipts.iter().enumerate() {
        if idx > 0 {
            base.push('\n');
        }
        base.push_str(&receipt.message);
    }
    Some(base)
}
```

Add tests:

```rust
#[test]
fn append_used_skill_receipts_adds_messages_after_content() {
    let content = Some("Done.".to_owned());
    let receipts = vec![UsedSkillReceipt {
        package_name: "rightx-demo".to_owned(),
        message: "Used learned skill: rightx-demo".to_owned(),
    }];
    assert_eq!(
        append_used_skill_receipts(content, Some(&receipts)).as_deref(),
        Some("Done.\n\nUsed learned skill: rightx-demo")
    );
}
```

In `crates/bot/src/telegram/worker.rs`, before sending content, replace:

```rust
if let Some(content) = output.content {
```

with:

```rust
let content_with_receipts = crate::cc::worker_reply::append_used_skill_receipts(
    output.content,
    output.used_skill_receipts.as_deref(),
);

if let Some(content) = content_with_receipts {
```

- [ ] **Step 13: Run targeted structured-output tests**

Run:

```bash
devenv shell -- cargo test -p right-codegen reply_schema_contains_learned_skill_fields
```

Expected: PASS.

Run:

```bash
devenv shell -- cargo test -p right-bot parse_reply_output
```

Expected: PASS.

Run:

```bash
devenv shell -- cargo test -p right-agent learned_skills
```

Expected: PASS.

- [ ] **Step 14: Commit structured-output slice**

Run:

```bash
git add crates/right-codegen/src/agent_def.rs crates/right-codegen/src/agent_def_tests.rs crates/bot/src/cc/worker_reply.rs crates/bot/src/telegram/worker.rs crates/right-agent/src/learned_skills.rs
git commit -m "feat: record learned skill reply signals"
```

Expected: commit succeeds.

---

### Task 6: Deny Stage 1 Learning Tools Outside Foreground

**Files:**
- Modify: `crates/bot/src/cc/invocation.rs`
- Modify: `crates/bot/src/cron.rs`
- Modify: `crates/bot/src/reflection.rs`
- Modify: `crates/bot/src/cron_delivery.rs`

- [ ] **Step 1: Write failing disallowed-tool tests**

Stage 1 permits only foreground registration. The shared `BackgroundReview` invocation kind is reserved for the future review worker; cron, reflection, and delivery prompts must still deny the learning tools.

In `crates/bot/src/cc/invocation.rs`, add tests:

```rust
#[test]
fn disallow_learning_tools_adds_full_mcp_tool_names() {
    let tools = disallow_learning_tools(vec!["Agent".to_owned()]);

    assert!(
        tools
            .iter()
            .any(|tool| tool == right_mcp::internal_client::SKILL_LEARNING_START_MCP_TOOL)
    );
    assert!(
        tools
            .iter()
            .any(|tool| tool == right_mcp::internal_client::SKILL_LEARNING_FINISH_MCP_TOOL)
    );
    assert!(tools.iter().any(|tool| tool == "Agent"));
}

#[test]
fn disallow_foreground_only_tools_is_idempotent() {
    let tools = disallow_foreground_only_tools(disallow_foreground_only_tools(Vec::new()));
    for needle in [
        SEND_PROGRESS_MCP_TOOL,
        right_mcp::internal_client::SKILL_LEARNING_START_MCP_TOOL,
        right_mcp::internal_client::SKILL_LEARNING_FINISH_MCP_TOOL,
    ] {
        let count = tools.iter().filter(|tool| tool.as_str() == needle).count();
        assert_eq!(count, 1, "{needle} should appear once");
    }
}
```

- [ ] **Step 2: Run tests and verify failure**

Run:

```bash
devenv shell -- cargo test -p right-bot disallow_learning_tools
```

Expected: FAIL because helpers do not exist yet.

- [ ] **Step 3: Add helper functions**

In `crates/bot/src/cc/invocation.rs`, add:

```rust
pub(crate) fn disallow_learning_tools(mut tools: Vec<String>) -> Vec<String> {
    for tool in [
        right_mcp::internal_client::SKILL_LEARNING_START_MCP_TOOL,
        right_mcp::internal_client::SKILL_LEARNING_FINISH_MCP_TOOL,
    ] {
        if !tools.iter().any(|existing| existing == tool) {
            tools.push(tool.to_owned());
        }
    }
    tools
}

pub(crate) fn disallow_foreground_only_tools(tools: Vec<String>) -> Vec<String> {
    disallow_learning_tools(disallow_send_progress(tools))
}
```

- [ ] **Step 4: Replace non-foreground callsites**

In `crates/bot/src/cron.rs`, replace:

```rust
let disallowed_tools = crate::cc::invocation::disallow_send_progress(
    crate::cc::invocation::baseline_disallowed_tools(),
);
```

with:

```rust
let disallowed_tools = crate::cc::invocation::disallow_foreground_only_tools(
    crate::cc::invocation::baseline_disallowed_tools(),
);
```

In `crates/bot/src/reflection.rs`, replace the `disallow_send_progress(d)` call with:

```rust
crate::cc::invocation::disallow_foreground_only_tools(d)
```

In `crates/bot/src/cron_delivery.rs`, replace `disallow_send_progress(...)` with:

```rust
crate::cc::invocation::disallow_foreground_only_tools(...)
```

- [ ] **Step 5: Run targeted disallowed-tool tests**

Run:

```bash
devenv shell -- cargo test -p right-bot disallow
```

Expected: PASS.

- [ ] **Step 6: Commit tool-deny slice**

Run:

```bash
git add crates/bot/src/cc/invocation.rs crates/bot/src/cron.rs crates/bot/src/reflection.rs crates/bot/src/cron_delivery.rs
git commit -m "feat: deny learned skill tools outside foreground"
```

Expected: commit succeeds.

---

### Task 7: Prompt, Architecture Docs, And System Prompt Snapshot

**Files:**
- Modify: `crates/right-codegen/templates/right/prompt/OPERATING_INSTRUCTIONS.md`
- Modify: `PROMPT_SYSTEM.md`
- Modify: `docs/architecture/mcp.md`
- Modify: `docs/architecture/sessions.md`
- Modify: `docs/architecture/sandbox.md`

- [ ] **Step 1: Add failing prompt/doc tests**

In `crates/right-codegen/src/agent_def_tests.rs`, add:

```rust
#[test]
fn operating_instructions_route_reusable_workflows_to_right_learn_skill() {
    let ops = crate::OPERATING_INSTRUCTIONS;
    for needle in [
        "/right-learn-skill",
        "Procedures and reusable workflows",
        "save as skills, not memory",
    ] {
        assert!(ops.contains(needle), "OPERATING_INSTRUCTIONS must mention {needle:?}");
    }
}
```

- [ ] **Step 2: Run prompt test and verify failure**

Run:

```bash
devenv shell -- cargo test -p right-codegen operating_instructions_route_reusable_workflows_to_right_learn_skill
```

Expected: FAIL until operating instructions mention the new skill.

- [ ] **Step 3: Update operating instructions**

In `crates/right-codegen/templates/right/prompt/OPERATING_INSTRUCTIONS.md`, in the Memory section after `Procedures and reusable workflows -- save as skills, not memory`, add:

```markdown
When you discover a reusable procedure, recovered tool/API surprise, user correction that should change future behavior, or a `rightx-*` learned skill that needs repair, use the `/right-learn-skill` skill. It decides whether to create or update a `rightx-*` learned skill, or leave a nudge signal.
```

- [ ] **Step 4: Update PROMPT_SYSTEM.md**

In `PROMPT_SYSTEM.md`:
- Add `right-learn-skill` to the built-in skills/platform store sections.
- Update the normal `reply-schema.json` section to list `used_skill_receipts`, `learning_signal`, and `skill_issue_signal` as optional fields.
- Update the MCP Server Instructions section to include `mcp__right__skill_learning_start` and `mcp__right__skill_learning_finish`.
- State that learning tools are metadata/progress/receipt tools and do not move files from sandbox to host.

- [ ] **Step 5: Update architecture docs**

In `docs/architecture/mcp.md`, add a "Learned Skill MCP Tools" section:

```markdown
## Learned Skill MCP Tools

`mcp__right__skill_learning_start` and
`mcp__right__skill_learning_finish` are built-in RightBackend tools. They are
metadata/progress/receipt tools: the active agent writes skill package files
directly under `.claude/skills/<skill_name>/`; MCP validates the skill name,
records learning events in `data.db`, verifies successful finishes by checking
`.claude/skills/<skill_name>/SKILL.md`, and sends foreground learning messages
through the existing bot UDS delivery path. In OpenShell mode that existence
check runs inside the sandbox; in `sandbox: none` mode it checks the host agent
directory. The receipt text is authored by the LLM and passed as the
`message` argument to `mcp__right__skill_learning_finish`.

Create and update both require `rightx-*`. The learning flow never patches
custom/manual/hub/core/platform/bundled/codegen-owned non-`rightx-*` skills.
```

In `docs/architecture/sessions.md`, extend the foreground progress paragraph:

```markdown
Foreground turns may also call learned-skill start/finish tools. These use the
same per-invocation `X-Right-Invocation` registration as progress, but they are
not generic progress calls: start sends the learning/update notice, successful
finish sends the learned/updated receipt, and both calls persist provenance.
```

In `docs/architecture/sandbox.md`, add `right-learn-skill.<hash>/` to the platform store skill list and add:

```markdown
Learned skill packages are agent-owned directories under
`/sandbox/.claude/skills/rightx-*`. The learning MCP tools do not patch
non-`rightx-*` skill directories and do not copy skill files from sandbox to
host.
```

- [ ] **Step 6: Run prompt/doc tests**

Run:

```bash
devenv shell -- cargo test -p right-codegen operating_instructions_route_reusable_workflows_to_right_learn_skill
```

Expected: PASS.

- [ ] **Step 7: Commit docs/prompt slice**

Run:

```bash
git add crates/right-codegen/templates/right/prompt/OPERATING_INSTRUCTIONS.md crates/right-codegen/src/agent_def_tests.rs PROMPT_SYSTEM.md docs/architecture/mcp.md docs/architecture/sessions.md docs/architecture/sandbox.md
git commit -m "docs: document learned skill flow"
```

Expected: commit succeeds.

---

### Task 8: Integration Checks And Final Verification

**Files:**
- Read: full worktree

- [ ] **Step 1: Run package-level targeted suite**

Run:

```bash
devenv shell -- cargo test -p right-codegen right_learn_skill
```

Expected: PASS.

Run:

```bash
devenv shell -- cargo test -p right skill_learning
```

Expected: PASS.

Run:

```bash
devenv shell -- cargo test -p right-bot parse_reply_output
```

Expected: PASS.

Run:

```bash
devenv shell -- cargo test -p right-agent learned_skills
```

Expected: PASS.

- [ ] **Step 2: Check prompt and MCP names**

Run:

```bash
devenv shell -- rg -n "skill_learning_start|skill_learning_finish|right-learn-skill|skill_receipt|send_progress just to announce learning" crates PROMPT_SYSTEM.md docs/architecture
```

Expected:
- `skill_learning_start`, `skill_learning_finish`, and `right-learn-skill` appear in implementation, prompts, docs, and tests.
- No `skill_receipt` remains in agent-facing schema or learned-skill instructions.
- `send_progress just to announce learning` appears only as a prohibition.

- [ ] **Step 3: Run final workspace tests**

Run:

```bash
devenv shell -- cargo test --workspace
```

Expected: PASS.

- [ ] **Step 4: Inspect git status**

Run:

```bash
devenv shell -- git status --short
```

Expected:
- Only intentional learned-skills files are modified or committed.
- Pre-existing unrelated `crates/right-openshell/*` changes remain untouched.
- No untracked `docs/superpowers/` files remain.

- [ ] **Step 5: Verify task commits are present**

Run:

```bash
devenv shell -- git log --oneline -n 6
```

Expected: the recent commits include the task commits from this plan:
`feat: add learned skills persistence`, `feat: add right learn skill`,
`feat: expose learned skill MCP tools`, `feat: record learned skill reply signals`,
`feat: deny learned skill tools outside foreground`, and
`docs: document learned skill flow`.

---

## Handoff Notes

- Foreground learning must not depend on background review execution.
- Start/finish MCP tools must never accept absolute paths.
- MCP validates skill package names before deriving paths: lowercase ASCII letters, digits, hyphens, 3-80 chars, no leading/trailing hyphen.
- New learned skills must use `rightx-*`.
- Updating existing `rightx-*` learned skills is allowed.
- Custom/manual/hub/core/platform/bundled/codegen-owned non-`rightx-*` skills are read-only to learning flows.
- Successful `skill_learning_finish` requires an LLM-authored `message` argument and must verify `.claude/skills/<skill_name>/SKILL.md` exists in the sandbox or host agent directory before recording/sending success.
- Successful finish calls suppress nudge signals for the same invocation.
- Background review remains disabled; data contracts and counters are ready for a future worker.
