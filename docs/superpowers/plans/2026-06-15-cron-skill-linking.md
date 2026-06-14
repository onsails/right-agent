# Cron ↔ Skill Linking Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let a cron deterministically pull one or more learned `rightx-*` skills — auto-linked from its own runs and agent-linkable via MCP — and let the `right-cron` skill evolve a fat cron prompt toward a thin "what" + skill references.

**Architecture:** A per-agent `cron_skill_links` table (many-to-many) is written by two auto-link seams (inline in `cron.rs`, async in the probe-writer tail) and by agent MCP tools (`skill_names` on `cron_create`, plus `cron_link_skill`/`cron_unlink_skill`). At fire time `compose_run_prompt` names the job's live linked skills as authoritative. Prompt slimming stays exclusively agent-driven via `cron_update` (auditable); the platform never rewrites prompts.

**Tech Stack:** Rust (edition 2024), `right-db` (Turso/SQLite), `thiserror` (lib errors), `anyhow` (handlers/tests), `cargo nextest`, `devenv shell`.

**Spec:** `docs/superpowers/specs/2026-06-15-cron-skill-linking-design.md`. Deferred v2 (proactive nudge): onsails/right-agent#128.

---

## File structure

| File | Responsibility | Action |
|---|---|---|
| `crates/right-db/src/sql/v47_cron_skill_links.sql` | Table + index DDL | Create |
| `crates/right-db/src/migrations.rs` | Register migration 47 | Modify |
| `crates/right-agent/src/cron_skill_link.rs` | Link CRUD + validated link/unlink | Create |
| `crates/right-agent/src/cron_skill_link_tests.rs` | Unit tests for the module | Create |
| `crates/right-agent/src/lib.rs` | `pub mod cron_skill_link;` | Modify |
| `crates/right-agent/src/learned_skills.rs` | `successful_finishes_for_invocation` | Modify |
| `crates/right-agent/src/cron_spec.rs:693` | Fold link cleanup into `delete_spec` | Modify |
| `crates/right-agent/src/cron_spec.rs:725` | `list_specs` adds `linked_skills` | Modify |
| `crates/bot/src/telegram/worker.rs:210,1989` | `ProbeAnchor.origin_cron_job` + foreground site | Modify |
| `crates/bot/src/cron.rs` | anchor site, inline seam, runtime directive | Modify |
| `crates/bot/src/learning_probe_writer.rs:175,328` | async seam | Modify |
| `crates/bot/src/learning_curator.rs:370` | absorb/archive link maintenance | Modify |
| `crates/right/src/memory_server.rs` | params + tools + dispatch + handlers + `with_instructions` | Modify |
| `crates/right/src/right_backend.rs` | tools + dispatch + handlers | Modify |
| `crates/right/src/aggregator.rs` | tool-name test + `with_instructions` | Modify |
| `crates/right-codegen/skills/right-cron/SKILL.md` | link docs + evolution guidance + version bump | Modify |
| `PROMPT_SYSTEM.md`, `ARCHITECTURE.md`, `docs/architecture/learning.md` | docs | Modify |
| `crates/bot/src/cron.rs:3828` | replace `todo!()` with `ci_claude_` test | Modify |

**Baseline (run first, record pre-existing failures):**
```bash
devenv shell -- cargo build -p right-db -p right-agent -p bot -p right
```

---

## Phase 1 — Storage foundation

### Task 1: Migration v47 — `cron_skill_links` table

**Files:**
- Create: `crates/right-db/src/sql/v47_cron_skill_links.sql`
- Modify: `crates/right-db/src/migrations.rs` (const block ~line 38; MIGRATIONS array; tests)

- [ ] **Step 1: Write the SQL file**

`crates/right-db/src/sql/v47_cron_skill_links.sql`:
```sql
-- Explicit many-to-many link between a cron job and the rightx-* skills it
-- should deterministically pull. origin distinguishes platform auto-links from
-- agent-authored links. Per-agent data.db; job_name is unique within an agent.
CREATE TABLE IF NOT EXISTS cron_skill_links (
  job_name   TEXT NOT NULL,
  skill_name TEXT NOT NULL,
  origin     TEXT NOT NULL CHECK (origin IN ('auto', 'agent')),
  created_at TEXT NOT NULL,
  PRIMARY KEY (job_name, skill_name)
);

CREATE INDEX IF NOT EXISTS idx_cron_skill_links_skill
  ON cron_skill_links(skill_name);
```

- [ ] **Step 2: Register the migration**

In `crates/right-db/src/migrations.rs`, after the `V46_NOTICE_TOKEN` const (line 38):
```rust
const V47_CRON_SKILL_LINKS: &str = include_str!("sql/v47_cron_skill_links.sql");
```
Then add to the `MIGRATIONS` array, after the version-46 entry (mirror the plain v44/v46 entries — `hook: None`):
```rust
        Migration {
            version: 47,
            sql: V47_CRON_SKILL_LINKS,
            hook: None,
        },
```

- [ ] **Step 3: Write the idempotency test**

In the `migrations.rs` tests module (mirror `v46_creates_notice_token_table`):
```rust
    #[tokio::test]
    async fn v47_creates_cron_skill_links_table() {
        let (_tmp, conn) = crate::test_support::migrated_connection().await;
        // Table exists and accepts a row.
        conn.execute(
            "INSERT INTO cron_skill_links (job_name, skill_name, origin, created_at) \
             VALUES ('j', 'rightx-a', 'auto', '2026-06-15T00:00:00Z')",
            (),
        )
        .await
        .unwrap();
        // PK is (job_name, skill_name): duplicate INSERT OR IGNORE is a no-op.
        let rows = conn
            .execute(
                "INSERT OR IGNORE INTO cron_skill_links (job_name, skill_name, origin, created_at) \
                 VALUES ('j', 'rightx-a', 'agent', '2026-06-15T00:00:01Z')",
                (),
            )
            .await
            .unwrap();
        assert_eq!(rows, 0, "duplicate PK must not insert");
        // origin CHECK rejects unknown values.
        let bad = conn
            .execute(
                "INSERT INTO cron_skill_links (job_name, skill_name, origin, created_at) \
                 VALUES ('j', 'rightx-b', 'bogus', '2026-06-15T00:00:02Z')",
                (),
            )
            .await;
        assert!(bad.is_err(), "origin CHECK must reject 'bogus'");
    }
```

- [ ] **Step 4: Run the test (expect fail, then pass)**

Run: `devenv shell -- cargo nextest run -p right-db v47_creates_cron_skill_links_table`
Expected before Step 1-2: FAIL (no such table). After: PASS.

- [ ] **Step 5: Run the existing migration suite to confirm no regression**

Run: `devenv shell -- cargo nextest run -p right-db migration`
Expected: PASS (including `registry`/idempotency tests).

- [ ] **Step 6: Commit**
```bash
git add crates/right-db/src/sql/v47_cron_skill_links.sql crates/right-db/src/migrations.rs
git commit -m "feat(right-db): add cron_skill_links table (migration v47)"
```

---

### Task 2: `right_agent::cron_skill_link` module

**Files:**
- Create: `crates/right-agent/src/cron_skill_link.rs`
- Create: `crates/right-agent/src/cron_skill_link_tests.rs`
- Modify: `crates/right-agent/src/lib.rs` (add `pub mod cron_skill_link;`)

- [ ] **Step 1: Add the module declaration**

In `crates/right-agent/src/lib.rs`, alongside the existing `pub mod cron_spec;`:
```rust
pub mod cron_skill_link;
```

- [ ] **Step 2: Write the module**

`crates/right-agent/src/cron_skill_link.rs` (use the same `Connection`/`DbError`/`params!` imports as `cron_spec.rs`/`learned_skills.rs`):
```rust
//! Explicit cron ↔ skill links (`cron_skill_links` table). Forward direction
//! (a cron's skills) is the runtime use; the reverse index serves curator
//! cleanup. Writes that touch 2+ rows run in one immediate transaction.

use right_db::{Connection, DbError, params};

/// Error surface for the validated agent-facing link/unlink operations.
#[derive(Debug, thiserror::Error)]
pub enum LinkError {
    #[error("job '{0}' not found")]
    JobNotFound(String),
    #[error("skill '{0}' not found or archived")]
    SkillNotFound(String),
    #[error(transparent)]
    Db(#[from] DbError),
}

fn now() -> String {
    chrono::Utc::now().to_rfc3339()
}

/// Upsert links with `origin='auto'` (platform). Idempotent; never overwrites an
/// existing agent link. No validation — callers pass skills they just authored.
pub async fn link_auto(
    conn: &Connection,
    job_name: &str,
    skill_names: &[String],
) -> Result<(), DbError> {
    if skill_names.is_empty() {
        return Ok(());
    }
    let now = now();
    let tx = conn.transaction().await?;
    for s in skill_names {
        tx.execute(
            "INSERT OR IGNORE INTO cron_skill_links (job_name, skill_name, origin, created_at) \
             VALUES (?1, ?2, 'auto', ?3)",
            params![job_name, s, &now],
        )
        .await?;
    }
    tx.commit().await?;
    Ok(())
}

/// Validate the job and each skill exist, then upsert `origin='agent'` links.
pub async fn link_agent(
    conn: &Connection,
    job_name: &str,
    skill_names: &[String],
) -> Result<String, LinkError> {
    ensure_job_exists(conn, job_name).await?;
    for s in skill_names {
        ensure_skill_live(conn, s).await?;
    }
    let now = now();
    let tx = conn.transaction().await?;
    for s in skill_names {
        tx.execute(
            "INSERT OR IGNORE INTO cron_skill_links (job_name, skill_name, origin, created_at) \
             VALUES (?1, ?2, 'agent', ?3)",
            params![job_name, s, &now],
        )
        .await?;
    }
    tx.commit().await?;
    Ok(format!(
        "Linked {} skill(s) to cron '{job_name}'.",
        skill_names.len()
    ))
}

/// Remove the given links from a job. Unknown links are ignored.
pub async fn unlink_agent(
    conn: &Connection,
    job_name: &str,
    skill_names: &[String],
) -> Result<String, LinkError> {
    ensure_job_exists(conn, job_name).await?;
    let tx = conn.transaction().await?;
    for s in skill_names {
        tx.execute(
            "DELETE FROM cron_skill_links WHERE job_name = ?1 AND skill_name = ?2",
            params![job_name, s],
        )
        .await?;
    }
    tx.commit().await?;
    Ok(format!(
        "Unlinked {} skill(s) from cron '{job_name}'.",
        skill_names.len()
    ))
}

/// All linked skill names for a job (no liveness filter).
pub async fn list_for_job(conn: &Connection, job_name: &str) -> Result<Vec<String>, DbError> {
    conn.query_all(
        "SELECT skill_name FROM cron_skill_links WHERE job_name = ?1 ORDER BY skill_name",
        params![job_name],
        |row| row.get::<_, String>(0),
    )
    .await
}

/// Linked skill names for a job, excluding archived skills. Used at fire time.
pub async fn list_live_for_job(conn: &Connection, job_name: &str) -> Result<Vec<String>, DbError> {
    conn.query_all(
        "SELECT l.skill_name FROM cron_skill_links l \
         JOIN skill_lifecycle s ON s.skill_name = l.skill_name \
         WHERE l.job_name = ?1 AND s.state != 'archived' \
         ORDER BY l.skill_name",
        params![job_name],
        |row| row.get::<_, String>(0),
    )
    .await
}

/// Jobs linking a given skill (reverse index; curator cleanup).
pub async fn jobs_for_skill(conn: &Connection, skill_name: &str) -> Result<Vec<String>, DbError> {
    conn.query_all(
        "SELECT job_name FROM cron_skill_links WHERE skill_name = ?1 ORDER BY job_name",
        params![skill_name],
        |row| row.get::<_, String>(0),
    )
    .await
}

/// Repoint every link from `old` to `new` (skill absorbed). PK-safe: copy then
/// drop, ignoring rows where the job already links `new`.
pub async fn redirect_skill(conn: &Connection, old: &str, new: &str) -> Result<(), DbError> {
    let now = now();
    let tx = conn.transaction().await?;
    tx.execute(
        "INSERT OR IGNORE INTO cron_skill_links (job_name, skill_name, origin, created_at) \
         SELECT job_name, ?2, origin, ?3 FROM cron_skill_links WHERE skill_name = ?1",
        params![old, new, &now],
    )
    .await?;
    tx.execute(
        "DELETE FROM cron_skill_links WHERE skill_name = ?1",
        params![old],
    )
    .await?;
    tx.commit().await?;
    Ok(())
}

/// Drop all links to a skill (skill retired without a successor).
pub async fn drop_skill(conn: &Connection, skill_name: &str) -> Result<(), DbError> {
    conn.execute(
        "DELETE FROM cron_skill_links WHERE skill_name = ?1",
        params![skill_name],
    )
    .await?;
    Ok(())
}

async fn ensure_job_exists(conn: &Connection, job_name: &str) -> Result<(), LinkError> {
    let n: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM cron_specs WHERE job_name = ?1",
            params![job_name],
            |r| r.get(0),
        )
        .await?;
    if n == 0 {
        return Err(LinkError::JobNotFound(job_name.to_owned()));
    }
    Ok(())
}

async fn ensure_skill_live(conn: &Connection, skill_name: &str) -> Result<(), LinkError> {
    let n: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM skill_lifecycle WHERE skill_name = ?1 AND state != 'archived'",
            params![skill_name],
            |r| r.get(0),
        )
        .await?;
    if n == 0 {
        return Err(LinkError::SkillNotFound(skill_name.to_owned()));
    }
    Ok(())
}

#[cfg(test)]
#[path = "cron_skill_link_tests.rs"]
mod tests;
```

> Note: confirm `right_db` re-exports `params`, `Connection`, `DbError` and the `query_all`/`query_row`/`transaction` API match `cron_spec.rs`/`learned_skills.rs` usage; if `transaction()` is named differently, mirror the call used in another 2-write helper. The `#[path]` test split follows AGENTS.rust.md.

- [ ] **Step 3: Write the tests**

`crates/right-agent/src/cron_skill_link_tests.rs`:
```rust
use super::*;
use right_db::Connection;

async fn conn() -> (tempfile::TempDir, Connection) {
    right_db::test_support::migrated_connection().await
}

async fn seed_job(conn: &Connection, job: &str) {
    conn.execute(
        "INSERT INTO cron_specs (job_name, schedule, prompt, max_budget_usd, created_at, updated_at) \
         VALUES (?1, '17 9 * * *', 'do x', 2.0, '2026-06-15T00:00:00Z', '2026-06-15T00:00:00Z')",
        params![job],
    )
    .await
    .unwrap();
}

async fn seed_skill(conn: &Connection, name: &str, state: &str) {
    conn.execute(
        "INSERT INTO skill_lifecycle (skill_name, state, created_by, created_at) \
         VALUES (?1, ?2, 'cron', '2026-06-15T00:00:00Z')",
        params![name, state],
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn link_auto_is_idempotent() {
    let (_t, c) = conn().await;
    seed_job(&c, "j").await;
    let skills = vec!["rightx-a".to_string()];
    link_auto(&c, "j", &skills).await.unwrap();
    link_auto(&c, "j", &skills).await.unwrap();
    assert_eq!(list_for_job(&c, "j").await.unwrap(), vec!["rightx-a"]);
}

#[tokio::test]
async fn link_agent_validates_job_and_skill() {
    let (_t, c) = conn().await;
    seed_job(&c, "j").await;
    // Missing skill rejected.
    let err = link_agent(&c, "j", &["rightx-missing".into()]).await.unwrap_err();
    assert!(matches!(err, LinkError::SkillNotFound(_)));
    // Missing job rejected.
    seed_skill(&c, "rightx-a", "active").await;
    let err = link_agent(&c, "nope", &["rightx-a".into()]).await.unwrap_err();
    assert!(matches!(err, LinkError::JobNotFound(_)));
    // Happy path.
    link_agent(&c, "j", &["rightx-a".into()]).await.unwrap();
    assert_eq!(list_for_job(&c, "j").await.unwrap(), vec!["rightx-a"]);
}

#[tokio::test]
async fn list_live_excludes_archived() {
    let (_t, c) = conn().await;
    seed_job(&c, "j").await;
    seed_skill(&c, "rightx-live", "active").await;
    seed_skill(&c, "rightx-dead", "archived").await;
    link_auto(&c, "j", &["rightx-live".into(), "rightx-dead".into()]).await.unwrap();
    assert_eq!(list_for_job(&c, "j").await.unwrap().len(), 2);
    assert_eq!(list_live_for_job(&c, "j").await.unwrap(), vec!["rightx-live"]);
}

#[tokio::test]
async fn redirect_moves_links_pk_safe() {
    let (_t, c) = conn().await;
    seed_job(&c, "j1").await;
    seed_job(&c, "j2").await;
    link_auto(&c, "j1", &["rightx-old".into()]).await.unwrap();
    // j2 already links the target — redirect must not fail on PK collision.
    link_auto(&c, "j2", &["rightx-old".into(), "rightx-new".into()]).await.unwrap();
    redirect_skill(&c, "rightx-old", "rightx-new").await.unwrap();
    assert_eq!(list_for_job(&c, "j1").await.unwrap(), vec!["rightx-new"]);
    assert_eq!(list_for_job(&c, "j2").await.unwrap(), vec!["rightx-new"]);
    assert!(jobs_for_skill(&c, "rightx-old").await.unwrap().is_empty());
}

#[tokio::test]
async fn unlink_and_drop() {
    let (_t, c) = conn().await;
    seed_job(&c, "j").await;
    link_auto(&c, "j", &["rightx-a".into(), "rightx-b".into()]).await.unwrap();
    unlink_agent(&c, "j", &["rightx-a".into()]).await.unwrap();
    assert_eq!(list_for_job(&c, "j").await.unwrap(), vec!["rightx-b"]);
    drop_skill(&c, "rightx-b").await.unwrap();
    assert!(list_for_job(&c, "j").await.unwrap().is_empty());
}
```

- [ ] **Step 4: Run the tests**

Run: `devenv shell -- cargo nextest run -p right-agent cron_skill_link`
Expected: all PASS. (If `skill_lifecycle` columns differ, align `seed_skill` to the v44 schema.)

- [ ] **Step 5: Commit**
```bash
git add crates/right-agent/src/cron_skill_link.rs crates/right-agent/src/cron_skill_link_tests.rs crates/right-agent/src/lib.rs
git commit -m "feat(right-agent): cron_skill_link module (link CRUD + validated link/unlink)"
```

---

### Task 3: `successful_finishes_for_invocation`

**Files:**
- Modify: `crates/right-agent/src/learned_skills.rs` (after `finish_event_for_invocation`, ~line 132)

- [ ] **Step 1: Write the failing test**

In the `learned_skills.rs` tests module:
```rust
    #[tokio::test]
    async fn successful_finishes_returns_created_and_updated() {
        let (_t, c) = conn().await;
        // Two successful finishes + one aborted under the same invocation.
        for (skill, status) in [("rightx-a", "created"), ("rightx-b", "updated"), ("rightx-c", "aborted")] {
            c.execute(
                "INSERT INTO skill_learning_events (invocation_id, agent_name, action, skill_name, phase, status, created_at) \
                 VALUES ('inv1', 'a', 'create', ?1, 'finish', ?2, '2026-06-15T00:00:00Z')",
                right_db::params![skill, status],
            ).await.unwrap();
        }
        let mut got = successful_finishes_for_invocation(&c, "inv1").await.unwrap();
        got.sort();
        assert_eq!(got, vec![
            ("rightx-a".to_string(), "created".to_string()),
            ("rightx-b".to_string(), "updated".to_string()),
        ]);
    }
```

- [ ] **Step 2: Run it (expect fail)**

Run: `devenv shell -- cargo nextest run -p right-agent successful_finishes_returns`
Expected: FAIL (function not found). (Align the `skill_learning_events` columns to `v20`/`v30` if the INSERT errors.)

- [ ] **Step 3: Implement (mirror `successful_finish_exists`)**
```rust
/// All (skill_name, status) finish rows with a create/patch status for an
/// invocation. Plural sibling of `successful_finish_exists`.
pub async fn successful_finishes_for_invocation(
    conn: &Connection,
    invocation_id: &str,
) -> Result<Vec<(String, String)>, DbError> {
    conn.query_all(
        "SELECT skill_name, status FROM skill_learning_events \
         WHERE invocation_id = ?1 AND phase = 'finish' AND status IN ('created','updated') \
         ORDER BY id",
        [invocation_id],
        |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
    )
    .await
}
```

- [ ] **Step 4: Run it (expect pass)**

Run: `devenv shell -- cargo nextest run -p right-agent successful_finishes_returns`
Expected: PASS.

- [ ] **Step 5: Commit**
```bash
git add crates/right-agent/src/learned_skills.rs
git commit -m "feat(right-agent): successful_finishes_for_invocation helper"
```

---

## Phase 2 — Auto-link

### Task 4: `ProbeAnchor.origin_cron_job` field

**Files:**
- Modify: `crates/bot/src/telegram/worker.rs:210` (struct), `:1989` (foreground build)
- Modify: `crates/bot/src/cron.rs:1256` (cron build)

- [ ] **Step 1: Add the field to the struct**

`worker.rs`, in `pub(crate) struct ProbeAnchor` (after `learning_invocation_id`):
```rust
    /// The originating recurring-cron `job_name` when this anchor came from a
    /// cron run; `None` for foreground turns. Drives auto-linking the learned
    /// skill to the cron.
    pub origin_cron_job: Option<String>,
```

- [ ] **Step 2: Set it at both construction sites**

Foreground (`worker.rs:1989` `ProbeAnchor { ... }`): add `origin_cron_job: None,`.
Cron (`cron.rs:1256` `ProbeAnchor { ... }`): add `origin_cron_job: Some(job_name.to_owned()),`.

- [ ] **Step 3: Compile (expect pass)**

Run: `devenv shell -- cargo build -p bot`
Expected: builds (struct-literal exhaustiveness forces both sites; no other consumers set it).

- [ ] **Step 4: Commit**
```bash
git add crates/bot/src/telegram/worker.rs crates/bot/src/cron.rs
git commit -m "feat(bot): thread origin_cron_job through ProbeAnchor"
```

---

### Task 5: Async seam — link probe-writer-authored skills

**Files:**
- Modify: `crates/bot/src/learning_probe_writer.rs:175` (capture), `:328-343` (link after finish lookup)

- [ ] **Step 1: Capture `origin_cron_job` before the detached task**

In `run(...)`, alongside the other pre-`tokio::spawn` captures (near `let invocation_id = ...;`, line 292):
```rust
    let origin_cron_job = anchor.origin_cron_job.clone();
```
Add `let origin_cron_job = origin_cron_job;` is unnecessary — capture by move into the `async move` block (it is `Option<String>`).

- [ ] **Step 2: Write the link after the spend record**

Inside the detached `tokio::spawn` block, in the `if let Some(result_line) = ... { ... }` body, right after `record_probe_writer_spend(&conn, &agent_name, &invocation_id, &b).await;` (line 339):
```rust
            if let Some(job) = origin_cron_job.as_deref() {
                let authored: Vec<String> =
                    match right_agent::learned_skills::successful_finishes_for_invocation(
                        &conn,
                        &invocation_id,
                    )
                    .await
                    {
                        Ok(rows) => rows.into_iter().map(|(name, _status)| name).collect(),
                        Err(e) => {
                            tracing::warn!(agent = %agent_name, "cron link lookup failed: {e:#}");
                            Vec::new()
                        }
                    };
                if let Err(e) = right_agent::cron_skill_link::link_auto(&conn, job, &authored).await {
                    tracing::warn!(agent = %agent_name, job = %job, "cron auto-link failed: {e:#}");
                }
            }
```

> Note: this reuses the `conn` already opened at line 330. `link_auto` ignores an empty slice, so a non-create/patch finish links nothing.

- [ ] **Step 3: Unit-test the link logic via a seam helper (optional extraction)**

The fork is not unit-testable end to end. Add a thin tested helper in `learning_probe_writer.rs` and call it from Step 2 instead of inlining:
```rust
/// Auto-link the skills authored under `invocation_id` to the originating cron.
pub(crate) async fn link_cron_authored(
    conn: &right_db::Connection,
    job: &str,
    invocation_id: &str,
) -> Result<usize, right_db::DbError> {
    let authored: Vec<String> =
        right_agent::learned_skills::successful_finishes_for_invocation(conn, invocation_id)
            .await?
            .into_iter()
            .map(|(name, _status)| name)
            .collect();
    let n = authored.len();
    right_agent::cron_skill_link::link_auto(conn, job, &authored).await?;
    Ok(n)
}
```
Test (tests module of `learning_probe_writer.rs`):
```rust
    #[tokio::test]
    async fn link_cron_authored_links_created_and_patched() {
        let (_t, c) = right_db::test_support::migrated_connection().await;
        c.execute("INSERT INTO cron_specs (job_name, schedule, prompt, max_budget_usd, created_at, updated_at) VALUES ('j','17 9 * * *','x',2.0,'t','t')", ()).await.unwrap();
        c.execute("INSERT INTO skill_learning_events (invocation_id, agent_name, action, skill_name, phase, status, created_at) VALUES ('inv','a','create','rightx-a','finish','created','t')", ()).await.unwrap();
        let n = super::link_cron_authored(&c, "j", "inv").await.unwrap();
        assert_eq!(n, 1);
        assert_eq!(right_agent::cron_skill_link::list_for_job(&c, "j").await.unwrap(), vec!["rightx-a"]);
    }
```
Then Step 2 becomes: `if let Some(job) = origin_cron_job.as_deref() { let _ = link_cron_authored(&conn, job, &invocation_id).await.map_err(|e| tracing::warn!(...)); }`.

- [ ] **Step 4: Run the test + build**

Run: `devenv shell -- cargo nextest run -p bot link_cron_authored && devenv shell -- cargo build -p bot`
Expected: PASS + builds.

- [ ] **Step 5: Commit**
```bash
git add crates/bot/src/learning_probe_writer.rs
git commit -m "feat(bot): auto-link probe-writer-authored skills to originating cron"
```

---

### Task 6: Inline seam — link skills authored during the cron turn

**Files:**
- Modify: `crates/bot/src/cron.rs` learning block (`:1247-1309`, after `cron_invocation_id` is known and the run succeeded)

- [ ] **Step 1: Add the inline link write**

In the recurring-success learning block, after `cron_invocation_id` is resolved and before/after the `run_post_turn` spawn (the inline skills are already persisted by the time the run finished), reusing the block's `conn`:
```rust
    // Inline authoring runs under cron_invocation_id (created_by='cron') and is
    // skipped by the async pipeline (authored_skill_this_turn). Link those here;
    // probe-writer-authored skills are linked in the async seam.
    if let Some(inv) = cron_invocation_id.as_deref() {
        match right_agent::learned_skills::successful_finishes_for_invocation(&conn, inv).await {
            Ok(rows) => {
                let skills: Vec<String> = rows.into_iter().map(|(n, _)| n).collect();
                if let Err(e) = right_agent::cron_skill_link::link_auto(&conn, job_name, &skills).await {
                    tracing::warn!(job = %job_name, "cron inline auto-link failed: {e:#}");
                }
            }
            Err(e) => tracing::warn!(job = %job_name, "cron inline link lookup failed: {e:#}"),
        }
    }
```

> Note: place this where `conn` (the run's connection) and `job_name` are in scope. It is a no-op when no inline authoring happened (empty `skills`). The probe-writer uses a different `invocation_id`, so no double-linking.

- [ ] **Step 2: Build**

Run: `devenv shell -- cargo build -p bot`
Expected: builds.

- [ ] **Step 3: Commit**
```bash
git add crates/bot/src/cron.rs
git commit -m "feat(bot): auto-link inline-authored cron skills"
```

---

## Phase 3 — Agent MCP surface

### Task 7: `skill_names` on `cron_create`

**Files:**
- Modify: `crates/right/src/memory_server.rs:32` (`CronCreateParams`); `call_cron_create` handler in **both** `memory_server.rs:369` and `right_backend.rs:408`

- [ ] **Step 1: Add the param**

In `CronCreateParams` (`memory_server.rs:32`):
```rust
    #[schemars(description = "Optional rightx-* skill names to link to this cron at creation. The cron will deterministically pull these at fire time. The skills must already exist.")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skill_names: Option<Vec<String>>,
```

- [ ] **Step 2: Link after create in both handlers**

In `right_backend.rs::call_cron_create`, after the `create_spec_v2(...)?` success and before building the result:
```rust
        if let Some(skills) = params.skill_names.as_deref()
            && !skills.is_empty()
            && let Err(e) = right_agent::cron_skill_link::link_agent(&conn, &params.job_name, skills).await
        {
            return Ok(tool_error("cron_link_failed", &format!("{e:#}"), None));
        }
```
Apply the equivalent in `memory_server.rs::cron_create` (mirror its existing error-return style — it returns `CallToolResult`/`anyhow` like the surrounding handler).

> Validation happens before any link write; the spec row is already committed, so a link failure leaves a cron with no links (recoverable via `cron_link_skill`) — acceptable per spec.

- [ ] **Step 3: Test (right_backend)**

Add to `right_backend_tests.rs` (mirror an existing `cron_create` test that builds args + calls the tool):
```rust
    #[tokio::test]
    async fn cron_create_links_skill_names() {
        // ... set up backend + agent (copy an existing cron_create test's scaffolding) ...
        // seed skill_lifecycle row 'rightx-a' (state 'active') in the agent db.
        // call cron_create with args including "skill_names": ["rightx-a"].
        // assert cron_skill_link::list_for_job(conn, "<job>") == ["rightx-a"].
    }
```
Fill the scaffolding by copying the nearest existing `cron_create` test (e.g. around `right_backend_tests.rs:2093`) and seeding `skill_lifecycle` as in Task 2.

- [ ] **Step 4: Run + build**

Run: `devenv shell -- cargo nextest run -p right cron_create_links_skill_names && devenv shell -- cargo build -p right`
Expected: PASS + builds.

- [ ] **Step 5: Commit**
```bash
git add crates/right/src/memory_server.rs crates/right/src/right_backend.rs crates/right/src/right_backend_tests.rs
git commit -m "feat(right): link skill_names on cron_create"
```

---

### Task 8: `cron_link_skill` / `cron_unlink_skill` tools

**Files:**
- Modify: `memory_server.rs` (params ~line 109 area; `with_instructions` ~line 661; tool list; dispatch; handlers)
- Modify: `right_backend.rs` (tool defs ~line 158 area; dispatch ~line 288; handlers)
- Modify: `aggregator.rs` (`with_instructions`; tool-name test ~line 875)

- [ ] **Step 1: Param structs (`memory_server.rs`)**
```rust
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct CronLinkSkillParams {
    #[schemars(description = "The cron job_name to link skills to.")]
    pub job_name: String,
    #[schemars(description = "rightx-* skill names to link. Each must already exist.")]
    pub skill_names: Vec<String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct CronUnlinkSkillParams {
    #[schemars(description = "The cron job_name to unlink skills from.")]
    pub job_name: String,
    #[schemars(description = "rightx-* skill names to unlink.")]
    pub skill_names: Vec<String>,
}
```
(Match the exact derive attributes used by the existing `CronDeleteParams`.)

- [ ] **Step 2: Tool definitions (`right_backend.rs`, after the `cron_delete` `Tool::new`)**
```rust
            Tool::new(
                "cron_link_skill",
                "Link one or more existing rightx-* skills to a cron job. At fire time the cron deterministically pulls its linked skills. Use after capturing a procedure as a skill, or to attach skills a cron should rely on.",
                schema_for_type::<CronLinkSkillParams>(),
            ),
            Tool::new(
                "cron_unlink_skill",
                "Unlink one or more rightx-* skills from a cron job. Note: a skill the cron's own runs re-learn may be auto-linked again.",
                schema_for_type::<CronUnlinkSkillParams>(),
            ),
```

- [ ] **Step 3: Dispatch arms (`right_backend.rs:288` area)**
```rust
            "cron_link_skill" => self.call_cron_link_skill(agent_name, &args).await,
            "cron_unlink_skill" => self.call_cron_unlink_skill(agent_name, &args).await,
```

- [ ] **Step 4: Handlers (`right_backend.rs`, near `call_cron_delete`)**
```rust
    async fn call_cron_link_skill(
        &self,
        agent_name: &str,
        args: &serde_json::Value,
    ) -> Result<CallToolResult, anyhow::Error> {
        let params: CronLinkSkillParams =
            serde_json::from_value(args.clone()).context("invalid cron_link_skill params")?;
        let conn_arc = self.get_conn(agent_name).await?;
        let conn = conn_arc.lock().await;
        match right_agent::cron_skill_link::link_agent(&conn, &params.job_name, &params.skill_names).await {
            Ok(msg) => Ok(CallToolResult::success(vec![Content::text(msg)])),
            Err(e) => Ok(tool_error("cron_link_failed", &format!("{e:#}"), None)),
        }
    }

    async fn call_cron_unlink_skill(
        &self,
        agent_name: &str,
        args: &serde_json::Value,
    ) -> Result<CallToolResult, anyhow::Error> {
        let params: CronUnlinkSkillParams =
            serde_json::from_value(args.clone()).context("invalid cron_unlink_skill params")?;
        let conn_arc = self.get_conn(agent_name).await?;
        let conn = conn_arc.lock().await;
        match right_agent::cron_skill_link::unlink_agent(&conn, &params.job_name, &params.skill_names).await {
            Ok(msg) => Ok(CallToolResult::success(vec![Content::text(msg)])),
            Err(e) => Ok(tool_error("cron_unlink_failed", &format!("{e:#}"), None)),
        }
    }
```

- [ ] **Step 5: Mirror in `memory_server.rs`** — add the same two `Tool::new` entries to its tool list, the same two dispatch arms, and the same two handler methods (using `memory_server.rs`'s own `tool_error`/`Content` imports and `get_conn` equivalent).

- [ ] **Step 6: Update `with_instructions` (both `memory_server.rs:661` and `aggregator.rs`)** — add the two lines to the cron-tools block:
```
                 - mcp__right__cron_link_skill: Link rightx-* skills to a cron (deterministic pull at fire time)\n\
                 - mcp__right__cron_unlink_skill: Unlink rightx-* skills from a cron\n\
```

- [ ] **Step 7: Update the tool-name test (`aggregator.rs:875` area)** — add assertions:
```rust
        assert!(names.contains(&"cron_link_skill"), "missing cron_link_skill");
        assert!(names.contains(&"cron_unlink_skill"), "missing cron_unlink_skill");
```

- [ ] **Step 8: Test the handlers** — add `right_backend_tests.rs` cases mirroring `cron_create_links_skill_names`: link then `list_for_job` shows the skill; link unknown skill → `tool_error` with `cron_link_failed`; unlink removes it.

- [ ] **Step 9: Run + build**

Run: `devenv shell -- cargo nextest run -p right cron_link && devenv shell -- cargo build -p right`
Expected: PASS + builds.

- [ ] **Step 10: Commit**
```bash
git add crates/right/src/memory_server.rs crates/right/src/right_backend.rs crates/right/src/aggregator.rs crates/right/src/right_backend_tests.rs
git commit -m "feat(right): cron_link_skill / cron_unlink_skill MCP tools"
```

---

### Task 9: `cron_list` returns `linked_skills`

**Files:**
- Modify: `crates/right-agent/src/cron_spec.rs:725` (`list_specs`)

- [ ] **Step 1: Add `linked_skills` per job**

In `list_specs`, after the row query, enrich each JSON object with its links. Simplest correct approach: after building the `Vec<serde_json::Value>`, for each entry read `job_name` and attach `list_for_job`:
```rust
    let mut rows: Vec<serde_json::Value> = /* existing query_all result */;
    for entry in rows.iter_mut() {
        if let Some(job) = entry.get("job_name").and_then(|v| v.as_str()) {
            let links = crate::cron_skill_link::list_for_job(conn, job)
                .await
                .unwrap_or_default();
            if let Some(obj) = entry.as_object_mut() {
                obj.insert("linked_skills".into(), serde_json::json!(links));
            }
        }
    }
    serde_json::to_string(&rows).map_err(|e| format!("serialize failed: {e:#}"))
```
Adjust to the function's existing return-construction (it currently builds and serializes the JSON array).

- [ ] **Step 2: Test**

Add to `cron_spec` tests: seed a job + a link, call `list_specs`, assert the parsed JSON contains `linked_skills: ["rightx-a"]` for that job.

- [ ] **Step 3: Run + build**

Run: `devenv shell -- cargo nextest run -p right-agent list_specs && devenv shell -- cargo build -p right-agent`
Expected: PASS + builds.

- [ ] **Step 4: Commit**
```bash
git add crates/right-agent/src/cron_spec.rs
git commit -m "feat(right-agent): cron_list includes linked_skills"
```

---

## Phase 4 — Runtime + lifecycle integrity

### Task 10: Runtime directive in `compose_run_prompt`

**Files:**
- Modify: `crates/bot/src/cron.rs:192` (`compose_run_prompt` signature + body), `:787` (caller fetches links)

- [ ] **Step 1: Add a `linked_skills` parameter**

Change `compose_run_prompt` to accept `linked_skills: &[String]` and append the block when non-empty. In the body, after the stored prompt is pushed:
```rust
    if !linked_skills.is_empty() {
        out.push_str("\n\n## Linked skills\nLinked skills for this job — use them via the Skill tool as appropriate: ");
        out.push_str(&linked_skills.join(", "));
        out.push('\n');
    }
```

- [ ] **Step 2: Fetch links at the caller (`cron.rs:787`)**

Before calling `compose_run_prompt`, using the run's `conn` and `job_name`:
```rust
    let linked_skills = right_agent::cron_skill_link::list_live_for_job(&conn, job_name)
        .await
        .unwrap_or_default();
```
Pass `&linked_skills` into `compose_run_prompt(...)`.

- [ ] **Step 3: Update the existing unit test**

`compose_run_prompt_orders_force_notify_then_extra_then_prompt` (`cron.rs:2850`) must pass `&[]` for the new arg. Add a new test:
```rust
    #[test]
    fn compose_run_prompt_appends_linked_skills_when_present() {
        let out = compose_run_prompt("do x", false, None, "tok", &["rightx-a".to_string(), "rightx-b".to_string()]);
        assert!(out.contains("## Linked skills"));
        assert!(out.contains("rightx-a, rightx-b"));
        let none = compose_run_prompt("do x", false, None, "tok", &[]);
        assert!(!none.contains("## Linked skills"));
    }
```
(Match the real `compose_run_prompt` argument order/types.)

- [ ] **Step 4: Run + build**

Run: `devenv shell -- cargo nextest run -p bot compose_run_prompt && devenv shell -- cargo build -p bot`
Expected: PASS + builds.

- [ ] **Step 5: Commit**
```bash
git add crates/bot/src/cron.rs
git commit -m "feat(bot): name live linked skills in the cron run prompt"
```

---

### Task 11: Cron deletion cascade (fold into `delete_spec`)

**Files:**
- Modify: `crates/right-agent/src/cron_spec.rs:693` (`delete_spec`)

- [ ] **Step 1: Wrap the spec delete + link delete in one transaction**

Rewrite the DB portion of `delete_spec`:
```rust
    let tx = conn
        .transaction()
        .await
        .map_err(|e| format!("delete failed: {e:#}"))?;
    let rows = tx
        .execute("DELETE FROM cron_specs WHERE job_name = ?1", params![job_name])
        .await
        .map_err(|e| format!("delete failed: {e:#}"))?;
    if rows == 0 {
        tx.rollback().await.ok();
        return Err(format!("job '{job_name}' not found"));
    }
    tx.execute(
        "DELETE FROM cron_skill_links WHERE job_name = ?1",
        params![job_name],
    )
    .await
    .map_err(|e| format!("delete failed: {e:#}"))?;
    tx.commit().await.map_err(|e| format!("delete failed: {e:#}"))?;
```
Keep the lock-file removal after the commit, unchanged.

- [ ] **Step 2: Test**

Add to `cron_spec` tests: seed job + link, `delete_spec`, assert `cron_skill_link::list_for_job` is empty and a second `delete_spec` returns "not found".

- [ ] **Step 3: Run + build**

Run: `devenv shell -- cargo nextest run -p right-agent delete_spec && devenv shell -- cargo build -p right-agent`
Expected: PASS + builds.

- [ ] **Step 4: Commit**
```bash
git add crates/right-agent/src/cron_spec.rs
git commit -m "feat(right-agent): cascade link deletion in delete_spec (tool + one-shot)"
```

---

### Task 12: Curator link maintenance

**Files:**
- Modify: `crates/bot/src/learning_curator.rs:370` (after `archived_skill_names` is computed)

- [ ] **Step 1: Redirect or drop links for archived skills**

After `archived_skill_names` is built, query each archived skill's `absorbed_into` and maintain links (reuse the curator's `conn`):
```rust
    for skill in &archived_skill_names {
        let absorbed_into: Option<String> = conn
            .query_row(
                "SELECT absorbed_into FROM skill_lifecycle WHERE skill_name = ?1",
                right_db::params![skill],
                |r| r.get::<_, Option<String>>(0),
            )
            .await
            .ok()
            .flatten();
        let res = match absorbed_into {
            Some(target) => right_agent::cron_skill_link::redirect_skill(&conn, skill, &target).await,
            None => right_agent::cron_skill_link::drop_skill(&conn, skill).await,
        };
        if let Err(e) = res {
            tracing::warn!(skill = %skill, "cron link maintenance failed: {e:#}");
        }
    }
```

> The runtime `state != 'archived'` filter (Task 2 `list_live_for_job`) is the correctness backstop; this hook keeps the table tidy and redirects absorbed skills to their successor.

- [ ] **Step 2: Build (logic is covered by Task 2 unit tests for redirect/drop)**

Run: `devenv shell -- cargo build -p bot`
Expected: builds. (If `learning_curator.rs` has a focused test harness, add a case asserting an archived linked skill is dropped and an absorbed one is redirected.)

- [ ] **Step 3: Commit**
```bash
git add crates/bot/src/learning_curator.rs
git commit -m "feat(bot): curator redirects/drops cron links on absorb/archive"
```

---

## Phase 5 — Skill doc + platform docs

### Task 13: `right-cron` SKILL.md

**Files:**
- Modify: `crates/right-codegen/skills/right-cron/SKILL.md`

- [ ] **Step 1: Bump version** — frontmatter `version: 3.6.0` → `version: 3.7.0`.

- [ ] **Step 2: Add a "Linking skills to a cron" section** (after "What, not how", line 108):
```markdown
## Linking Skills to a Cron

A cron can have one or more `rightx-*` skills **linked** to it. At fire time the
runtime names the job's live linked skills as authoritative, so the cron pulls
them deterministically instead of relying on description-matching.

- **Automatic:** any skill a recurring cron's own runs create or patch is linked
  to that cron automatically. You do not need to link those by hand.
- **At creation:** pass `skill_names` to `cron_create` to link existing skills up
  front (capture the skill first, then create the cron that uses it).
- **For existing crons:** `mcp__right__cron_link_skill(job_name, skill_names=[...])`
  links several at once; `mcp__right__cron_unlink_skill(job_name, skill_names=[...])`
  removes them. (A skill the cron re-learns may be auto-linked again.)

`mcp__right__cron_list` shows each job's `linked_skills`.
```

- [ ] **Step 3: Add prompt-evolution guidance** to "Editing a Cron Job" (after line 134):
```markdown
**Evolve the prompt toward its skills.** Before `cron_update`-ing a prompt, check
the job's `linked_skills` (`cron_list`). If the prompt still spells out "how" that
a linked skill now covers, slim the prompt to the "what" and rely on the link —
migrating a fat cron prompt toward a thin goal + skill references. Tell the user
what you simplified; never rewrite silently.
```

- [ ] **Step 4: Add `skill_names` to the Parameters table** (after the `prompt` row, line 246):
```markdown
| `skill_names` | string[] | No | - | `cron_create` only. rightx-* skills to link at creation; pulled deterministically at fire time. Skills must already exist. |
```

- [ ] **Step 5: Verify codegen tests still pass** (skill content/version may be referenced):

Run: `devenv shell -- cargo nextest run -p right-codegen`
Expected: PASS. If a skill-hash/index test fails, regenerate per the failing test's instructions and re-run.

- [ ] **Step 6: Commit**
```bash
git add crates/right-codegen/skills/right-cron/SKILL.md
git commit -m "docs(right-cron): document skill linking + prompt evolution (v3.7.0)"
```

---

### Task 14: Platform docs

**Files:**
- Modify: `PROMPT_SYSTEM.md`, `ARCHITECTURE.md`, `docs/architecture/learning.md`

- [ ] **Step 1: `docs/architecture/learning.md`** — add a "Cron ↔ skill linking" subsection: the `cron_skill_links` table, the two auto-link seams (inline `cron.rs`, async probe-writer tail), agent tools, the runtime directive in `compose_run_prompt` (live-state filter), curator redirect/drop, and that prompt evolution is agent-driven only. Reference the spec by plain path.

- [ ] **Step 2: `PROMPT_SYSTEM.md`** — document the per-run `## Linked skills` block (when ≥1 live link) and the `right-cron` prompt-evolution guidance.

- [ ] **Step 3: `ARCHITECTURE.md`** — minimal prescriptive lines only (respect the 40k budget): under the cron/learning area note that (a) `cron_skill_links` is the only reliable cron→skill provenance (`created_by` cannot express it), (b) link tools resolve job/skill scope server-side per-agent, (c) link writes that touch 2+ rows share one transaction; cron deletion cascades via `delete_spec`. Link the satellite by plain path. Verify the file stays < 40k chars:
```bash
wc -c ARCHITECTURE.md
```

- [ ] **Step 4: Commit**
```bash
git add PROMPT_SYSTEM.md ARCHITECTURE.md docs/architecture/learning.md
git commit -m "docs: cron↔skill linking (architecture + prompt system)"
```

---

## Phase 6 — Integration + final verification

### Task 15: `ci_claude_` create-then-reuse integration test

**Files:**
- Modify: `crates/bot/src/cron.rs:3828` (replace the `todo!()` runbook stub)

- [ ] **Step 1: Replace the stub with a real ignored integration test**

Per AGENTS.rust.md (`ci-claude:` reason + `ci_claude_` prefix), write a test that: provisions a recurring cron in a live sandbox, runs it so the learning pipeline authors a `rightx-*` skill, asserts a `cron_skill_links` row appears for the job (auto-link), and asserts the next `compose_run_prompt`/run names that skill. Shape:
```rust
    #[tokio::test]
    #[ignore = "ci-claude: live sandbox + Claude Code skill-learning round trip"]
    async fn ci_claude_cron_learns_and_links_skill_then_reuses() {
        // 1. TestSandbox + agent db (right_openshell::test_support).
        // 2. Insert a recurring CronSpec; run it via the same path execute_job uses.
        // 3. Await the learning pipeline; poll cron_skill_links for the job.
        // 4. assert !cron_skill_link::list_live_for_job(conn, job).is_empty().
        // 5. Build the next run prompt; assert it contains "## Linked skills".
    }
```
Fill the scaffolding from the existing live cron test harness referenced around `cron.rs:3814-3920` (it already documents a recurring `CronSpec` in a temp home).

- [ ] **Step 2: Confirm it compiles and is ignored by default**

Run: `devenv shell -- cargo nextest run -p bot ci_claude_cron_learns_and_links`
Expected: the test is listed as skipped/ignored (no live run locally). Compilation must succeed.

- [ ] **Step 3: Verify the ignored-contract gate passes**

Run: `devenv shell -- cargo nextest run -p right ci_ignored_contract`
Expected: PASS (the new `ci_claude_` test conforms to reason/prefix rules).

- [ ] **Step 4: Commit**
```bash
git add crates/bot/src/cron.rs
git commit -m "test(bot): ci-claude cron learns+links skill then reuses it"
```

---

### Task 16: Final workspace verification

- [ ] **Step 1: Full test suite (mandatory)**

Run: `devenv shell -- cargo nextest run --workspace`
Expected: PASS. Re-run any flaky cc/invocation or dashboard warn-count tests in isolation before attributing a failure to this work (known parallel-load flakes).

- [ ] **Step 2: Doctests**

Run: `devenv shell -- cargo test --doc --workspace`
Expected: PASS.

- [ ] **Step 3: Debug build**

Run: `devenv shell -- cargo build --workspace`
Expected: builds clean.

- [ ] **Step 4: Rust review pass**

Dispatch `rust-dev:review-rust-code` over the diff; turn findings into TODOs and fix one by one (FAIL FAST error handling, no swallowed errors, `format!("{:#}", e)` for error chains).

- [ ] **Step 5: Final commit (if review produced fixes)**
```bash
git add -A
git commit -m "chore(cron-skill-linking): address review findings"
```

---

## Self-review notes (author)

- **Spec coverage:** table+helpers (T1-2), provenance helper (T3), auto-link both seams (T4-6), agent MCP create-time + tools (T7-8), introspection (T9), runtime directive (T10), deletion cascade (T11), curator integrity (T12), skill doc + evolution (T13), platform docs (T14), create-then-reuse test (T15), final verify (T16). All spec sections mapped.
- **Type consistency:** module API names (`link_auto`, `link_agent`, `unlink_agent`, `list_for_job`, `list_live_for_job`, `jobs_for_skill`, `redirect_skill`, `drop_skill`) and `successful_finishes_for_invocation` are used identically across T5-12.
- **Grounding caveats to confirm during execution (not placeholders — exact call sites are given):** `right_db` transaction method name and `params`/`query_all` re-exports; exact `skill_lifecycle`/`skill_learning_events`/`cron_specs` column lists for test seeds; `memory_server.rs` handler error-return idiom; the live cron test harness around `cron.rs:3814`.
