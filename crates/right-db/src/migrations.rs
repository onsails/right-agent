use std::path::Path;

const V1_SCHEMA: &str = include_str!("sql/v1_schema.sql");
const V2_SCHEMA: &str = include_str!("sql/v2_telegram_sessions.sql");
const V3_SCHEMA: &str = include_str!("sql/v3_cron_runs.sql");
const V4_SCHEMA: &str = include_str!("sql/v4_sessions.sql");
const V5_SCHEMA: &str = include_str!("sql/v5_cron_feedback.sql");
const V6_SCHEMA: &str = include_str!("sql/v6_cron_specs.sql");
const V7_SCHEMA: &str = include_str!("sql/v7_cron_trigger.sql");
const V8_SCHEMA: &str = include_str!("sql/v8_mcp_servers.sql");
const V9_SCHEMA: &str = include_str!("sql/v9_mcp_instructions.sql");
const V10_SCHEMA: &str = include_str!("sql/v10_mcp_auth.sql");
const V11_SCHEMA: &str = include_str!("sql/v11_auth_tokens.sql");
#[allow(dead_code)] // Doc-only: actual migration uses Rust hook for idempotency.
const V13_SCHEMA: &str = include_str!("sql/v13_one_shot_cron.sql");
const V14_SCHEMA: &str = include_str!("sql/v14_memory_failure_handling.sql");
const V15_SCHEMA: &str = include_str!("sql/v15_usage_events.sql");
#[allow(dead_code)] // Doc-only: actual migration uses Rust hook for idempotency.
const V16_SCHEMA: &str = include_str!("sql/v16_usage_api_key_source.sql");
#[allow(dead_code)] // Doc-only: actual migration uses Rust hook for idempotency.
const V17_SCHEMA: &str = include_str!("sql/v17_cron_target.sql");
const V19_SCHEMA: &str = include_str!("sql/v19_cron_runs_target_index.sql");
const V20_SCHEMA: &str = include_str!("sql/v20_learned_skills.sql");
const V21_SCHEMA: &str = include_str!("sql/v21_conversation_messages.sql");
const V22_SCHEMA: &str = include_str!("sql/v22_skill_review_reports.sql");
/// Historical v23 async_runs shape, applied by `v23_async_runs` Rust hook.
const V23_SCHEMA: &str = include_str!("sql/v23_async_runs.sql");
const V24_SCHEMA: &str = include_str!("sql/v24_learning_episodes.sql");
const V25_SCHEMA: &str = include_str!("sql/v25_async_runs_delivery_decision.sql");
#[allow(dead_code)] // Doc-only: actual migration uses Rust hook for idempotency.
const V26_SCHEMA: &str = include_str!("sql/v26_skill_nudge_circuit_breaker.sql");
#[allow(dead_code)] // Doc-only: actual migration uses Rust hook for idempotency.
const V27_SCHEMA: &str = include_str!("sql/v27_skill_nudge_signals_source.sql");
#[allow(dead_code)] // Doc-only: actual migration uses Rust hook for idempotency.
const V28_SCHEMA: &str = include_str!("sql/v28_usage_wall_elapsed.sql");
const V29_SCHEMA: &str = include_str!("sql/v29_curator_state.sql");
#[allow(dead_code)] // Doc-only: actual migration uses Rust hook for idempotency.
const V30_SCHEMA: &str = include_str!("sql/v30_skill_learning_hint_outcome.sql");
const V31_SCHEMA: &str = include_str!("sql/v31_skill_learning_events_dashboard_index.sql");
const V32_SCHEMA: &str = include_str!("sql/v32_skill_lifecycle.sql");
const V33_SCHEMA: &str = include_str!("sql/v33_mcp_oauth_resource.sql");

pub const LATEST_SCHEMA_VERSION: u32 = 33;

type MigrationHook = fn(&dyn MigrationConnection) -> Result<(), crate::DbError>;

mod sealed {
    pub trait MigrationConnection {}
    pub trait MigrationTarget {}
}

pub struct Migration {
    pub version: u32,
    pub sql: &'static str,
    pub hook: Option<MigrationHook>,
}

pub struct Migrations {
    migrations: &'static [Migration],
}

#[doc(hidden)]
#[derive(Clone, Copy)]
pub enum MigrationParams<'a> {
    Empty,
    OneText(&'a str),
    TwoText(&'a str, &'a str),
}

impl MigrationParams<'_> {
    fn two_text<'a>(first: &'a str, second: &'a str) -> MigrationParams<'a> {
        MigrationParams::TwoText(first, second)
    }
}

pub trait MigrationConnection: sealed::MigrationConnection {
    fn execute_batch(&self, sql: &str) -> Result<(), crate::DbError>;
    fn query_i64(&self, sql: &str, params: MigrationParams<'_>) -> Result<i64, crate::DbError>;
}

pub trait MigrationTarget: sealed::MigrationTarget {
    fn migration_path(&self) -> &Path;
    fn migration_user_version(&self) -> Result<u32, crate::DbError>;
    fn with_migration_transaction<T>(
        &self,
        f: impl FnOnce(&dyn MigrationConnection) -> Result<T, crate::DbError>,
    ) -> Result<T, crate::DbError>;
}

impl Migrations {
    pub fn to_latest<C: MigrationTarget>(&self, conn: C) -> Result<(), crate::DbError> {
        self.to_version(conn, self.highest_version())
    }

    pub fn to_version<C: MigrationTarget>(
        &self,
        conn: C,
        target_version: u32,
    ) -> Result<(), crate::DbError> {
        let current = conn
            .migration_user_version()
            .map_err(|source| migration_error(&conn, 0, source))?;
        let highest = self.highest_version();

        if current > highest {
            return Err(migration_version_error(
                &conn,
                current,
                format!("database schema is newer than known migrations ({highest})"),
            ));
        }
        if target_version > highest {
            return Err(migration_version_error(
                &conn,
                target_version,
                format!("unknown migration target above highest known version {highest}"),
            ));
        }
        if target_version < current {
            return Err(migration_version_error(
                &conn,
                target_version,
                format!("down migrations are unsupported from current version {current}"),
            ));
        }
        if target_version == current {
            return Ok(());
        }

        let mut active_version = current + 1;
        conn.with_migration_transaction(|tx| {
            for migration in self.migrations {
                if migration.version <= current || migration.version > target_version {
                    continue;
                }
                active_version = migration.version;
                if !migration.sql.trim().is_empty() {
                    tx.execute_batch(migration.sql)?;
                }
                if let Some(hook) = migration.hook {
                    hook(tx)?;
                }
                tx.execute_batch(&format!("PRAGMA user_version = {}", migration.version))?;
            }
            Ok(())
        })
        .map_err(|source| migration_error(&conn, active_version, source))?;

        Ok(())
    }

    fn highest_version(&self) -> u32 {
        self.migrations
            .last()
            .map(|migration| migration.version)
            .unwrap_or(0)
    }
}

fn migration_error<C: MigrationTarget>(
    conn: &C,
    version: u32,
    source: crate::DbError,
) -> crate::DbError {
    crate::DbError::Migration {
        path: conn.migration_path().to_path_buf(),
        version,
        source: Box::new(source),
    }
}

fn migration_version_error<C: MigrationTarget>(
    conn: &C,
    version: u32,
    message: String,
) -> crate::DbError {
    crate::DbError::MigrationVersion {
        path: conn.migration_path().to_path_buf(),
        version,
        message,
    }
}

fn column_exists(
    conn: &dyn MigrationConnection,
    table: &str,
    column: &str,
) -> Result<bool, crate::DbError> {
    let count = conn.query_i64(
        "SELECT COUNT(*) FROM pragma_table_info(?) WHERE name = ?",
        MigrationParams::two_text(table, column),
    )?;
    Ok(count > 0)
}

impl sealed::MigrationConnection for crate::Transaction<'_> {}

impl MigrationConnection for crate::Transaction<'_> {
    fn execute_batch(&self, sql: &str) -> Result<(), crate::DbError> {
        crate::Transaction::execute_batch(self, sql)
    }

    fn query_i64(&self, sql: &str, params: MigrationParams<'_>) -> Result<i64, crate::DbError> {
        match params {
            MigrationParams::Empty => self.query_one(sql, (), |row| row.get(0)),
            MigrationParams::OneText(value) => self.query_one(sql, [value], |row| row.get(0)),
            MigrationParams::TwoText(first, second) => {
                self.query_one(sql, [first, second], |row| row.get(0))
            }
        }
    }
}

impl sealed::MigrationTarget for &crate::Connection {}

impl MigrationTarget for &crate::Connection {
    fn migration_path(&self) -> &Path {
        (*self).path()
    }

    fn migration_user_version(&self) -> Result<u32, crate::DbError> {
        let version: i64 = (*self).query_one("PRAGMA user_version", (), |row| row.get(0))?;
        u32::try_from(version)
            .map_err(|_| crate::DbError::InvalidParameter("negative user_version".into()))
    }

    fn with_migration_transaction<T>(
        &self,
        f: impl FnOnce(&dyn MigrationConnection) -> Result<T, crate::DbError>,
    ) -> Result<T, crate::DbError> {
        (*self).with_immediate_transaction(|tx| f(tx))
    }
}

#[cfg(test)]
impl sealed::MigrationConnection for rusqlite::Connection {}

#[cfg(test)]
impl MigrationConnection for rusqlite::Connection {
    fn execute_batch(&self, sql: &str) -> Result<(), crate::DbError> {
        rusqlite::Connection::execute_batch(self, sql)?;
        Ok(())
    }

    fn query_i64(&self, sql: &str, params: MigrationParams<'_>) -> Result<i64, crate::DbError> {
        let value = match params {
            MigrationParams::Empty => self.query_row(sql, [], |row| row.get(0))?,
            MigrationParams::OneText(value) => self.query_row(sql, [value], |row| row.get(0))?,
            MigrationParams::TwoText(first, second) => {
                self.query_row(sql, [first, second], |row| row.get(0))?
            }
        };
        Ok(value)
    }
}

#[cfg(test)]
impl sealed::MigrationConnection for rusqlite::Transaction<'_> {}

#[cfg(test)]
impl MigrationConnection for rusqlite::Transaction<'_> {
    fn execute_batch(&self, sql: &str) -> Result<(), crate::DbError> {
        rusqlite::Connection::execute_batch(self, sql)?;
        Ok(())
    }

    fn query_i64(&self, sql: &str, params: MigrationParams<'_>) -> Result<i64, crate::DbError> {
        let value = match params {
            MigrationParams::Empty => self.query_row(sql, [], |row| row.get(0))?,
            MigrationParams::OneText(value) => self.query_row(sql, [value], |row| row.get(0))?,
            MigrationParams::TwoText(first, second) => {
                self.query_row(sql, [first, second], |row| row.get(0))?
            }
        };
        Ok(value)
    }
}

#[cfg(test)]
impl sealed::MigrationTarget for &rusqlite::Connection {}

#[cfg(test)]
impl MigrationTarget for &rusqlite::Connection {
    fn migration_path(&self) -> &Path {
        Path::new(":memory:")
    }

    fn migration_user_version(&self) -> Result<u32, crate::DbError> {
        let version: i64 = (*self).query_row("PRAGMA user_version", [], |row| row.get(0))?;
        u32::try_from(version)
            .map_err(|_| crate::DbError::InvalidParameter("negative user_version".into()))
    }

    fn with_migration_transaction<T>(
        &self,
        f: impl FnOnce(&dyn MigrationConnection) -> Result<T, crate::DbError>,
    ) -> Result<T, crate::DbError> {
        rusqlite::Connection::execute_batch(self, "BEGIN IMMEDIATE")?;
        match f(*self) {
            Ok(value) => {
                rusqlite::Connection::execute_batch(self, "COMMIT")?;
                Ok(value)
            }
            Err(err) => {
                rusqlite::Connection::execute_batch(self, "ROLLBACK")?;
                Err(err)
            }
        }
    }
}

#[cfg(test)]
impl sealed::MigrationTarget for &mut rusqlite::Connection {}

#[cfg(test)]
impl MigrationTarget for &mut rusqlite::Connection {
    fn migration_path(&self) -> &Path {
        Path::new(":memory:")
    }

    fn migration_user_version(&self) -> Result<u32, crate::DbError> {
        let version: i64 = (**self).query_row("PRAGMA user_version", [], |row| row.get(0))?;
        u32::try_from(version)
            .map_err(|_| crate::DbError::InvalidParameter("negative user_version".into()))
    }

    fn with_migration_transaction<T>(
        &self,
        f: impl FnOnce(&dyn MigrationConnection) -> Result<T, crate::DbError>,
    ) -> Result<T, crate::DbError> {
        rusqlite::Connection::execute_batch(self, "BEGIN IMMEDIATE")?;
        match f(&**self) {
            Ok(value) => {
                rusqlite::Connection::execute_batch(self, "COMMIT")?;
                Ok(value)
            }
            Err(err) => {
                rusqlite::Connection::execute_batch(self, "ROLLBACK")?;
                Err(err)
            }
        }
    }
}

/// v12: Add delivery_status and no_notify_reason columns to cron_runs,
/// backfill existing rows, and create auto-set trigger.
///
/// Implemented as a Rust hook (not pure SQL) because SQLite lacks
/// `ADD COLUMN IF NOT EXISTS` — the ALTER TABLE would fail with
/// "duplicate column name" if re-run on a database that already has
/// the columns.
fn v12_cron_diagnostics(conn: &dyn MigrationConnection) -> Result<(), crate::DbError> {
    if !column_exists(conn, "cron_runs", "delivery_status")? {
        conn.execute_batch("ALTER TABLE cron_runs ADD COLUMN delivery_status TEXT")?;
    }
    if !column_exists(conn, "cron_runs", "no_notify_reason")? {
        conn.execute_batch("ALTER TABLE cron_runs ADD COLUMN no_notify_reason TEXT")?;
    }

    // Backfill existing rows (idempotent UPDATEs).
    conn.execute_batch(
        "UPDATE cron_runs SET delivery_status = 'delivered'
           WHERE notify_json IS NOT NULL AND delivered_at IS NOT NULL;
         UPDATE cron_runs SET delivery_status = 'pending'
           WHERE notify_json IS NOT NULL AND delivered_at IS NULL;
         UPDATE cron_runs SET delivery_status = 'silent'
           WHERE notify_json IS NULL;",
    )?;

    // Trigger: auto-set delivery_status on INSERT (IF NOT EXISTS is idempotent).
    conn.execute_batch(
        "CREATE TRIGGER IF NOT EXISTS cron_runs_delivery_status_insert
         AFTER INSERT ON cron_runs
         WHEN NEW.delivery_status IS NULL
         BEGIN
           UPDATE cron_runs SET delivery_status =
             CASE
               WHEN NEW.notify_json IS NOT NULL AND NEW.delivered_at IS NOT NULL THEN 'delivered'
               WHEN NEW.notify_json IS NOT NULL AND NEW.delivered_at IS NULL     THEN 'pending'
               ELSE 'silent'
             END
           WHERE id = NEW.id;
         END;",
    )?;

    Ok(())
}

/// v13: Add recurring and run_at columns to cron_specs for one-shot job support.
///
/// Idempotent — checks pragma_table_info before each ALTER.
fn v13_one_shot_cron(conn: &dyn MigrationConnection) -> Result<(), crate::DbError> {
    if !column_exists(conn, "cron_specs", "recurring")? {
        conn.execute_batch(
            "ALTER TABLE cron_specs ADD COLUMN recurring INTEGER NOT NULL DEFAULT 1",
        )?;
    }
    if !column_exists(conn, "cron_specs", "run_at")? {
        conn.execute_batch("ALTER TABLE cron_specs ADD COLUMN run_at TEXT")?;
    }

    Ok(())
}

/// v16: Add api_key_source column to usage_events.
///
/// Idempotent — checks pragma_table_info before ALTER. Column defaults
/// to 'none' which matches the setup-token (subscription) auth mode all
/// current Right Agent deployments use.
fn v16_usage_api_key_source(conn: &dyn MigrationConnection) -> Result<(), crate::DbError> {
    if !column_exists(conn, "usage_events", "api_key_source")? {
        conn.execute_batch(
            "ALTER TABLE usage_events ADD COLUMN api_key_source TEXT NOT NULL DEFAULT 'none'",
        )?;
    }
    Ok(())
}

/// v17: Add target_chat_id and target_thread_id to cron_specs.
///
/// Idempotent — checks pragma_table_info before each ALTER. Both columns
/// are nullable; the MCP layer validates presence on new rows. NULL on
/// existing rows is surfaced by `doctor::check_cron_targets`.
fn v17_cron_target(conn: &dyn MigrationConnection) -> Result<(), crate::DbError> {
    if !column_exists(conn, "cron_specs", "target_chat_id")? {
        conn.execute_batch("ALTER TABLE cron_specs ADD COLUMN target_chat_id INTEGER")?;
    }
    if !column_exists(conn, "cron_specs", "target_thread_id")? {
        conn.execute_batch("ALTER TABLE cron_specs ADD COLUMN target_thread_id INTEGER")?;
    }
    Ok(())
}

/// v18: Add target_chat_id and target_thread_id to cron_runs.
///
/// Snapshot of the spec's delivery target taken at run-insert time. Lets the
/// delivery loop find the recipient even after a one-shot spec auto-deletes.
/// Both columns are nullable. After adding the columns we backfill them from
/// `cron_specs` for any pre-existing run whose spec is still alive (recurring
/// crons): the delivery loop reads the target straight from `cron_runs` (no
/// LEFT JOIN to `cron_specs`), so without this backfill any undelivered run
/// would be permanently `no_target` after upgrade. The UPDATE is idempotent —
/// it filters by `target_chat_id IS NULL` so re-runs are no-ops. Rows whose
/// spec has already been deleted stay NULL and continue to surface as
/// `delivery_status='no_target'` (no recovery path).
fn v18_cron_runs_target(conn: &dyn MigrationConnection) -> Result<(), crate::DbError> {
    if !column_exists(conn, "cron_runs", "target_chat_id")? {
        conn.execute_batch("ALTER TABLE cron_runs ADD COLUMN target_chat_id INTEGER")?;
    }
    if !column_exists(conn, "cron_runs", "target_thread_id")? {
        conn.execute_batch("ALTER TABLE cron_runs ADD COLUMN target_thread_id INTEGER")?;
    }
    // Backfill target columns from cron_specs for runs whose spec still
    // exists. Idempotent — only touches rows where target_chat_id IS NULL.
    conn.execute_batch(
        "UPDATE cron_runs \
           SET target_chat_id = (SELECT target_chat_id FROM cron_specs WHERE cron_specs.job_name = cron_runs.job_name), \
               target_thread_id = (SELECT target_thread_id FROM cron_specs WHERE cron_specs.job_name = cron_runs.job_name) \
           WHERE cron_runs.target_chat_id IS NULL \
             AND EXISTS (SELECT 1 FROM cron_specs WHERE cron_specs.job_name = cron_runs.job_name)",
    )?;
    Ok(())
}

/// v22: Add learned-skill review report storage and review-gate state columns.
///
/// The report table/indexes live in SQL. Column additions are guarded here
/// because SQLite has no `ADD COLUMN IF NOT EXISTS`.
fn v22_skill_review_reports(conn: &dyn MigrationConnection) -> Result<(), crate::DbError> {
    if !column_exists(conn, "skill_nudge_state", "creation_review_interval")? {
        conn.execute_batch(
            "ALTER TABLE skill_nudge_state
             ADD COLUMN creation_review_interval INTEGER NOT NULL DEFAULT 15",
        )?;
    }
    if !column_exists(conn, "skill_nudge_state", "daily_review_count")? {
        conn.execute_batch(
            "ALTER TABLE skill_nudge_state
             ADD COLUMN daily_review_count INTEGER NOT NULL DEFAULT 0",
        )?;
    }
    if !column_exists(conn, "skill_nudge_state", "daily_review_date")? {
        conn.execute_batch("ALTER TABLE skill_nudge_state ADD COLUMN daily_review_date TEXT")?;
    }
    if !column_exists(conn, "skill_nudge_state", "last_review_status")? {
        conn.execute_batch("ALTER TABLE skill_nudge_state ADD COLUMN last_review_status TEXT")?;
    }
    Ok(())
}

fn v23_async_runs(conn: &dyn MigrationConnection) -> Result<(), crate::DbError> {
    conn.execute_batch(V23_SCHEMA)?;
    conn.execute_batch(
        "INSERT INTO async_runs (
            id, kind, producer_ref, source_session_id, run_session_id,
            target_chat_id, target_thread_id, status, handoff_state,
            started_at, finished_at, exit_code, log_path, summary,
            notify_json, no_notify_reason, error_json, delivery_required,
            delivery_status, delivery_attempts, delivered_at,
            last_delivery_error, created_at, updated_at
         )
         SELECT
            cr.id,
            CASE
              WHEN (cr.job_name LIKE 'bg-%' AND cs.job_name IS NULL)
                OR cs.schedule LIKE '@bg:%'
                OR (
                  cs.schedule = '@immediate'
                  AND cs.prompt LIKE 'X-FORK-FROM: %'
                  AND instr(cs.prompt, char(10)) > length('X-FORK-FROM: ') + 1
                )
              THEN 'background'
              ELSE 'cron'
            END,
            cr.job_name,
            CASE
              WHEN cs.schedule LIKE '@bg:%' THEN substr(cs.schedule, 5)
              WHEN cs.schedule = '@immediate'
                AND cs.prompt LIKE 'X-FORK-FROM: %'
                AND instr(cs.prompt, char(10)) > length('X-FORK-FROM: ') + 1
              THEN substr(
                cs.prompt,
                length('X-FORK-FROM: ') + 1,
                instr(cs.prompt, char(10)) - length('X-FORK-FROM: ') - 1
              )
              ELSE NULL
            END,
            cr.id,
            COALESCE(cr.target_chat_id, cs.target_chat_id, 0),
            cr.target_thread_id,
            cr.status,
            CASE
              WHEN (cr.job_name LIKE 'bg-%' AND cs.job_name IS NULL)
                OR cs.schedule LIKE '@bg:%'
                OR (
                  cs.schedule = '@immediate'
                  AND cs.prompt LIKE 'X-FORK-FROM: %'
                  AND instr(cs.prompt, char(10)) > length('X-FORK-FROM: ') + 1
                )
              THEN 'spawned'
              ELSE NULL
            END,
            cr.started_at,
            cr.finished_at,
            cr.exit_code,
            cr.log_path,
            cr.summary,
            cr.notify_json,
            cr.no_notify_reason,
            NULL,
            CASE WHEN cr.notify_json IS NULL THEN 0 ELSE 1 END,
            CASE
              WHEN cr.delivery_status = 'silent' THEN 'none'
              WHEN cr.delivery_status IS NULL AND cr.notify_json IS NULL THEN 'none'
              WHEN cr.delivery_status IS NULL AND cr.notify_json IS NOT NULL THEN 'pending'
              ELSE cr.delivery_status
            END,
            0,
            cr.delivered_at,
            NULL,
            cr.started_at,
            COALESCE(cr.finished_at, cr.started_at)
         FROM cron_runs cr
         LEFT JOIN cron_specs cs ON cs.job_name = cr.job_name",
    )?;
    conn.execute_batch(
        "INSERT INTO async_runs (
            id, kind, producer_ref, source_session_id, run_session_id,
            target_chat_id, target_thread_id, status, handoff_state,
            started_at, finished_at, exit_code, log_path, summary,
            notify_json, no_notify_reason, error_json, delivery_required,
            delivery_status, delivery_attempts, delivered_at,
            last_delivery_error, created_at, updated_at
         )
         SELECT
            lower(hex(randomblob(4))) || '-' || lower(hex(randomblob(2))) || '-' ||
            lower(hex(randomblob(2))) || '-' || lower(hex(randomblob(2))) || '-' ||
            lower(hex(randomblob(6))),
            'background',
            cs.job_name,
            CASE
              WHEN cs.schedule LIKE '@bg:%' THEN substr(cs.schedule, 5)
              ELSE substr(
                cs.prompt,
                length('X-FORK-FROM: ') + 1,
                instr(cs.prompt, char(10)) - length('X-FORK-FROM: ') - 1
              )
            END,
            lower(hex(randomblob(4))) || '-' || lower(hex(randomblob(2))) || '-' ||
            lower(hex(randomblob(2))) || '-' || lower(hex(randomblob(2))) || '-' ||
            lower(hex(randomblob(6))),
            COALESCE(cs.target_chat_id, 0),
            cs.target_thread_id,
            'failed',
            'queued',
            COALESCE(cs.triggered_at, cs.created_at),
            strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
            NULL,
            NULL,
            'background handoff interrupted by async_runs migration',
            '{\"content\":\"Background work was interrupted during an upgrade before it could be started.\"}',
            NULL,
            '{\"error\":\"legacy background cron spec removed before execution\"}',
            1,
            'pending',
            0,
            NULL,
            NULL,
            COALESCE(cs.created_at, strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
            strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
         FROM cron_specs cs
         WHERE (
             cs.schedule LIKE '@bg:%'
             OR (
               cs.schedule = '@immediate'
               AND cs.prompt LIKE 'X-FORK-FROM: %'
               AND instr(cs.prompt, char(10)) > length('X-FORK-FROM: ') + 1
             )
           )
           AND NOT EXISTS (SELECT 1 FROM cron_runs cr WHERE cr.job_name = cs.job_name)",
    )?;
    conn.execute_batch(
        "DELETE FROM cron_specs
         WHERE schedule LIKE '@bg:%'
            OR (
              schedule = '@immediate'
              AND prompt LIKE 'X-FORK-FROM: %'
              AND instr(prompt, char(10)) > length('X-FORK-FROM: ') + 1
            )",
    )?;
    conn.execute_batch("DROP TABLE cron_runs")?;
    Ok(())
}

fn v24_learning_episodes(conn: &dyn MigrationConnection) -> Result<(), crate::DbError> {
    conn.execute_batch(V24_SCHEMA)?;
    if !column_exists(conn, "skill_review_reports", "learning_episode_id")? {
        conn.execute_batch(
            "ALTER TABLE skill_review_reports ADD COLUMN learning_episode_id INTEGER",
        )?;
    }
    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_skill_review_reports_episode ON skill_review_reports(learning_episode_id)",
    )?;
    Ok(())
}

/// v26: Add circuit-breaker columns to skill_nudge_state.
///
/// Idempotent — checks pragma_table_info before each ALTER. SQLite has no
/// `ADD COLUMN IF NOT EXISTS`.
fn v26_skill_nudge_circuit_breaker(conn: &dyn MigrationConnection) -> Result<(), crate::DbError> {
    if !column_exists(conn, "skill_nudge_state", "consecutive_review_failures")? {
        conn.execute_batch(
            "ALTER TABLE skill_nudge_state ADD COLUMN consecutive_review_failures INTEGER NOT NULL DEFAULT 0",
        )?;
    }
    if !column_exists(conn, "skill_nudge_state", "review_circuit_open_until")? {
        conn.execute_batch(
            "ALTER TABLE skill_nudge_state ADD COLUMN review_circuit_open_until TEXT",
        )?;
    }
    Ok(())
}

/// v27: Add `source` column + index to `skill_nudge_signals`.
///
/// Idempotent — checks pragma_table_info before ALTER. SQLite has no
/// `ADD COLUMN IF NOT EXISTS`. The CREATE INDEX uses `IF NOT EXISTS`.
fn v27_skill_nudge_signals_source(conn: &dyn MigrationConnection) -> Result<(), crate::DbError> {
    if !column_exists(conn, "skill_nudge_signals", "source")? {
        conn.execute_batch(
            "ALTER TABLE skill_nudge_signals ADD COLUMN source TEXT NOT NULL DEFAULT 'reply_field'",
        )?;
    }
    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_skill_nudge_signals_source \
         ON skill_nudge_signals(source)",
    )?;
    Ok(())
}

/// v28: Add nullable `wall_elapsed_ms` column to `usage_events`.
///
/// Idempotent — checks pragma_table_info before ALTER. Foreground worker
/// turns populate this; non-foreground sources leave NULL.
fn v28_usage_wall_elapsed_ms(conn: &dyn MigrationConnection) -> Result<(), crate::DbError> {
    if !column_exists(conn, "usage_events", "wall_elapsed_ms")? {
        conn.execute_batch("ALTER TABLE usage_events ADD COLUMN wall_elapsed_ms INTEGER")?;
    }
    Ok(())
}

/// v30: Add nullable hint_outcome to skill_learning_events.
///
/// Idempotent — checks pragma_table_info before ALTER. The column carries a
/// CHECK constraint for new writes, but remains nullable for historical rows.
fn v30_skill_learning_hint_outcome(conn: &dyn MigrationConnection) -> Result<(), crate::DbError> {
    if !column_exists(conn, "skill_learning_events", "hint_outcome")? {
        conn.execute_batch(V30_SCHEMA)?;
    }
    Ok(())
}

pub static MIGRATIONS: Migrations = Migrations {
    migrations: &[
        Migration {
            version: 1,
            sql: V1_SCHEMA,
            hook: None,
        },
        Migration {
            version: 2,
            sql: V2_SCHEMA,
            hook: None,
        },
        Migration {
            version: 3,
            sql: V3_SCHEMA,
            hook: None,
        },
        Migration {
            version: 4,
            sql: V4_SCHEMA,
            hook: None,
        },
        Migration {
            version: 5,
            sql: V5_SCHEMA,
            hook: None,
        },
        Migration {
            version: 6,
            sql: V6_SCHEMA,
            hook: None,
        },
        Migration {
            version: 7,
            sql: V7_SCHEMA,
            hook: None,
        },
        Migration {
            version: 8,
            sql: V8_SCHEMA,
            hook: None,
        },
        Migration {
            version: 9,
            sql: V9_SCHEMA,
            hook: None,
        },
        Migration {
            version: 10,
            sql: V10_SCHEMA,
            hook: None,
        },
        Migration {
            version: 11,
            sql: V11_SCHEMA,
            hook: None,
        },
        Migration {
            version: 12,
            sql: "",
            hook: Some(v12_cron_diagnostics),
        },
        Migration {
            version: 13,
            sql: "",
            hook: Some(v13_one_shot_cron),
        },
        Migration {
            version: 14,
            sql: V14_SCHEMA,
            hook: None,
        },
        Migration {
            version: 15,
            sql: V15_SCHEMA,
            hook: None,
        },
        Migration {
            version: 16,
            sql: "",
            hook: Some(v16_usage_api_key_source),
        },
        Migration {
            version: 17,
            sql: "",
            hook: Some(v17_cron_target),
        },
        Migration {
            version: 18,
            sql: "",
            hook: Some(v18_cron_runs_target),
        },
        Migration {
            version: 19,
            sql: V19_SCHEMA,
            hook: None,
        },
        Migration {
            version: 20,
            sql: V20_SCHEMA,
            hook: None,
        },
        Migration {
            version: 21,
            sql: V21_SCHEMA,
            hook: None,
        },
        Migration {
            version: 22,
            sql: V22_SCHEMA,
            hook: Some(v22_skill_review_reports),
        },
        Migration {
            version: 23,
            sql: "",
            hook: Some(v23_async_runs),
        },
        Migration {
            version: 24,
            sql: "",
            hook: Some(v24_learning_episodes),
        },
        Migration {
            version: 25,
            sql: V25_SCHEMA,
            hook: None,
        },
        Migration {
            version: 26,
            sql: "",
            hook: Some(v26_skill_nudge_circuit_breaker),
        },
        Migration {
            version: 27,
            sql: "",
            hook: Some(v27_skill_nudge_signals_source),
        },
        Migration {
            version: 28,
            sql: "",
            hook: Some(v28_usage_wall_elapsed_ms),
        },
        Migration {
            version: 29,
            sql: V29_SCHEMA,
            hook: None,
        },
        Migration {
            version: 30,
            sql: "",
            hook: Some(v30_skill_learning_hint_outcome),
        },
        Migration {
            version: 31,
            sql: V31_SCHEMA,
            hook: None,
        },
        Migration {
            version: 32,
            sql: V32_SCHEMA,
            hook: None,
        },
        Migration {
            version: 33,
            sql: V33_SCHEMA,
            hook: None,
        },
    ],
};

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    static FAILING_MIGRATIONS: Migrations = Migrations {
        migrations: &[
            Migration {
                version: 1,
                sql: "CREATE TABLE synthetic_probe (id INTEGER PRIMARY KEY)",
                hook: None,
            },
            Migration {
                version: 2,
                sql: "CREATE TABLE synthetic_probe (id INTEGER PRIMARY KEY)",
                hook: None,
            },
        ],
    };

    #[test]
    fn migration_runner_semantics_latest_rejects_future_user_version() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(&format!(
            "PRAGMA user_version = {}",
            LATEST_SCHEMA_VERSION + 1
        ))
        .unwrap();

        let err = MIGRATIONS
            .to_latest(&conn)
            .expect_err("future user_version must be rejected");

        assert!(err.to_string().contains("newer than known migrations"));
    }

    #[test]
    fn migration_runner_semantics_rolls_back_all_pending_migrations_on_later_failure() {
        let conn = Connection::open_in_memory().unwrap();

        let err = FAILING_MIGRATIONS
            .to_latest(&conn)
            .expect_err("second migration should fail");

        assert!(
            err.to_string().contains("migration 2"),
            "expected migration 2 context, got {err:?}",
        );
        let user_version: i64 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(user_version, 0);
        let table_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='synthetic_probe'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(table_count, 0);
    }

    #[test]
    fn migration_runner_semantics_rejects_target_after_highest_known_version() {
        let conn = Connection::open_in_memory().unwrap();

        let err = FAILING_MIGRATIONS
            .to_version(&conn, 3)
            .expect_err("target past registry end must be rejected");

        assert!(err.to_string().contains("unknown migration target"));
    }

    #[test]
    fn migration_runner_semantics_rejects_down_migrations() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA user_version = 2").unwrap();

        let err = FAILING_MIGRATIONS
            .to_version(&conn, 1)
            .expect_err("down migrations must be rejected");

        assert!(err.to_string().contains("down migrations are unsupported"));
    }

    #[test]
    fn migration_runner_semantics_allows_current_version_noop() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA user_version = 2").unwrap();

        FAILING_MIGRATIONS.to_version(&conn, 2).unwrap();

        let user_version: i64 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(user_version, 2);
    }

    #[test]
    fn migration_runner_semantics_latest_schema_version_matches_highest_migration() {
        assert_eq!(
            MIGRATIONS
                .migrations
                .last()
                .expect("migration registry should not be empty")
                .version,
            LATEST_SCHEMA_VERSION,
        );
    }

    #[test]
    fn migrations_apply_cleanly_to_v4() {
        let mut conn = Connection::open_in_memory().unwrap();
        MIGRATIONS.to_latest(&mut conn).unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('sessions') WHERE name IN ('id','chat_id','thread_id','root_session_id','label','is_active','created_at','last_used_at')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 8, "sessions table should have all 8 columns");
        let old_exists: bool = conn
            .prepare("SELECT 1 FROM telegram_sessions LIMIT 1")
            .is_ok();
        assert!(!old_exists, "telegram_sessions should be dropped");
    }

    #[test]
    fn sessions_partial_unique_index_enforces_single_active() {
        let mut conn = Connection::open_in_memory().unwrap();
        MIGRATIONS.to_latest(&mut conn).unwrap();
        conn.execute(
            "INSERT INTO sessions (chat_id, thread_id, root_session_id, is_active) VALUES (1, 0, 'aaa', 1)",
            [],
        )
        .unwrap();
        let result = conn.execute(
            "INSERT INTO sessions (chat_id, thread_id, root_session_id, is_active) VALUES (1, 0, 'bbb', 1)",
            [],
        );
        assert!(
            result.is_err(),
            "partial unique index should prevent two active sessions"
        );
    }

    #[test]
    fn sessions_allows_multiple_inactive() {
        let mut conn = Connection::open_in_memory().unwrap();
        MIGRATIONS.to_latest(&mut conn).unwrap();
        conn.execute(
            "INSERT INTO sessions (chat_id, thread_id, root_session_id, is_active) VALUES (1, 0, 'aaa', 0)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO sessions (chat_id, thread_id, root_session_id, is_active) VALUES (1, 0, 'bbb', 0)",
            [],
        )
        .unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM sessions WHERE chat_id=1", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(count, 2);
    }

    #[test]
    fn async_runs_has_delivery_decision_columns() {
        let mut conn = Connection::open_in_memory().unwrap();
        MIGRATIONS.to_latest(&mut conn).unwrap();
        let cols: Vec<String> = conn
            .prepare("SELECT name FROM pragma_table_info('async_runs')")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();

        for col in [
            "run_note",
            "delivery_json",
            "delivery_required",
            "delivery_status",
        ] {
            assert!(cols.contains(&col.to_string()), "{col} column missing");
        }
        for col in ["summary", "notify_json", "no_notify_reason"] {
            assert!(
                !cols.contains(&col.to_string()),
                "{col} column should be removed"
            );
        }
    }

    #[test]
    fn v25_loses_old_pending_delivery_payloads() {
        let mut conn = Connection::open_in_memory().unwrap();
        MIGRATIONS.to_version(&mut conn, 24).unwrap();
        conn.execute(
            "INSERT INTO async_runs (
            id, kind, producer_ref, run_session_id, target_chat_id,
            status, started_at, finished_at, summary, notify_json,
            no_notify_reason, delivery_required, delivery_status,
            created_at, updated_at
         ) VALUES (
            'run-1', 'cron', 'ping', 'run-1', -100,
            'success', '2026-05-21T10:00:00Z', '2026-05-21T10:00:05Z',
            'old summary', '{\"content\":\"old payload\"}', NULL,
            1, 'pending', '2026-05-21T10:00:00Z', '2026-05-21T10:00:05Z'
         )",
            [],
        )
        .unwrap();

        MIGRATIONS.to_latest(&mut conn).unwrap();

        let row: (Option<String>, Option<String>, i64, String) = conn
            .query_row(
                "SELECT run_note, delivery_json, delivery_required, delivery_status
             FROM async_runs WHERE id = 'run-1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!(row, (Some("old summary".into()), None, 0, "none".into()));
    }

    #[test]
    fn learning_episode_tables_exist() {
        let mut conn = rusqlite::Connection::open_in_memory().unwrap();
        MIGRATIONS.to_latest(&mut conn).unwrap();
        let events: String = conn
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type='table' AND name='execution_events'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(events.contains("event_kind"));
        assert!(events.contains("trust_label"));
        let episodes: String = conn
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type='table' AND name='learning_episodes'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(episodes.contains("ready_after"));
        assert!(episodes.contains("episode_hash"));
    }

    #[test]
    fn execution_events_do_not_create_fts() {
        let mut conn = rusqlite::Connection::open_in_memory().unwrap();
        MIGRATIONS.to_latest(&mut conn).unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE name LIKE 'execution_events%fts%'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn skill_review_reports_links_learning_episode() {
        let mut conn = rusqlite::Connection::open_in_memory().unwrap();
        MIGRATIONS.to_latest(&mut conn).unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('skill_review_reports') WHERE name='learning_episode_id'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn migrations_apply_cleanly_to_v7() {
        let mut conn = Connection::open_in_memory().unwrap();
        MIGRATIONS.to_latest(&mut conn).unwrap();
        let cols: Vec<String> = conn
            .prepare("SELECT name FROM pragma_table_info('cron_specs')")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        assert!(
            cols.contains(&"triggered_at".to_string()),
            "triggered_at column missing"
        );
    }

    #[test]
    fn v8_mcp_servers_table() {
        let mut conn = Connection::open_in_memory().unwrap();
        MIGRATIONS.to_latest(&mut conn).unwrap();

        conn.execute(
            "INSERT INTO mcp_servers (name, url) VALUES (?1, ?2)",
            ("notion", "https://mcp.notion.com/mcp"),
        )
        .unwrap();

        let url: String = conn
            .query_row(
                "SELECT url FROM mcp_servers WHERE name = ?1",
                ["notion"],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(url, "https://mcp.notion.com/mcp");

        // Test upsert
        conn.execute(
            "INSERT OR REPLACE INTO mcp_servers (name, url) VALUES (?1, ?2)",
            ("notion", "https://new-url.com/mcp"),
        )
        .unwrap();
        let url: String = conn
            .query_row(
                "SELECT url FROM mcp_servers WHERE name = ?1",
                ["notion"],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(url, "https://new-url.com/mcp");
    }

    #[test]
    fn v9_mcp_servers_has_instructions_column() {
        let mut conn = Connection::open_in_memory().unwrap();
        MIGRATIONS.to_latest(&mut conn).unwrap();

        conn.execute(
            "INSERT INTO mcp_servers (name, url) VALUES (?1, ?2)",
            ("test-server", "https://example.com/mcp"),
        )
        .unwrap();

        let instructions: Option<String> = conn
            .query_row(
                "SELECT instructions FROM mcp_servers WHERE name = ?1",
                ["test-server"],
                |row| row.get(0),
            )
            .unwrap();
        assert!(
            instructions.is_none(),
            "instructions should be NULL by default"
        );
    }

    #[test]
    fn v10_mcp_servers_has_auth_columns() {
        let mut conn = Connection::open_in_memory().unwrap();
        MIGRATIONS.to_latest(&mut conn).unwrap();

        conn.execute(
            "INSERT INTO mcp_servers (name, url, auth_type, auth_token) VALUES (?1, ?2, ?3, ?4)",
            ("test", "https://example.com/mcp", "bearer", "sk-123"),
        )
        .unwrap();

        let auth_type: Option<String> = conn
            .query_row(
                "SELECT auth_type FROM mcp_servers WHERE name = ?1",
                ["test"],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(auth_type.as_deref(), Some("bearer"));

        // Verify all new columns exist
        let cols: Vec<String> = conn
            .prepare("SELECT name FROM pragma_table_info('mcp_servers')")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        for col in [
            "auth_type",
            "auth_header",
            "auth_token",
            "refresh_token",
            "token_endpoint",
            "client_id",
            "client_secret",
            "expires_at",
        ] {
            assert!(cols.contains(&col.to_string()), "{col} column missing");
        }
    }

    #[test]
    fn v33_mcp_servers_has_oauth_resource_column() {
        let mut conn = Connection::open_in_memory().unwrap();
        MIGRATIONS.to_latest(&mut conn).unwrap();

        let cols: Vec<String> = conn
            .prepare("SELECT name FROM pragma_table_info('mcp_servers')")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();

        assert!(
            cols.contains(&"oauth_resource".to_string()),
            "oauth_resource column missing"
        );
    }

    #[test]
    fn v12_cron_diagnostics_columns() {
        let mut conn = Connection::open_in_memory().unwrap();
        MIGRATIONS.to_version(&mut conn, 21).unwrap();
        let cols: Vec<String> = conn
            .prepare("SELECT name FROM pragma_table_info('cron_runs')")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        assert!(
            cols.contains(&"delivery_status".to_string()),
            "delivery_status column missing"
        );
        assert!(
            cols.contains(&"no_notify_reason".to_string()),
            "no_notify_reason column missing"
        );
    }

    #[test]
    fn v12_backfill_delivery_status() {
        let mut conn = Connection::open_in_memory().unwrap();
        MIGRATIONS.to_version(&mut conn, 21).unwrap();

        // Insert a delivered run (has notify_json + delivered_at)
        conn.execute(
            "INSERT INTO cron_runs (id, job_name, started_at, status, log_path, notify_json, delivered_at) \
             VALUES ('d1', 'j1', '2026-01-01T00:00:00Z', 'success', '/log', '{\"content\":\"hi\"}', '2026-01-01T00:05:00Z')",
            [],
        ).unwrap();
        // Insert a pending run (has notify_json, no delivered_at)
        conn.execute(
            "INSERT INTO cron_runs (id, job_name, started_at, status, log_path, notify_json) \
             VALUES ('p1', 'j1', '2026-01-01T01:00:00Z', 'success', '/log', '{\"content\":\"pending\"}')",
            [],
        ).unwrap();
        // Insert a silent run (no notify_json)
        conn.execute(
            "INSERT INTO cron_runs (id, job_name, started_at, status, log_path, summary) \
             VALUES ('s1', 'j1', '2026-01-01T02:00:00Z', 'success', '/log', 'quiet')",
            [],
        )
        .unwrap();

        let status_of = |id: &str| -> Option<String> {
            conn.query_row(
                "SELECT delivery_status FROM cron_runs WHERE id = ?1",
                [id],
                |r| r.get(0),
            )
            .unwrap()
        };
        assert_eq!(status_of("d1").as_deref(), Some("delivered"));
        assert_eq!(status_of("p1").as_deref(), Some("pending"));
        assert_eq!(status_of("s1").as_deref(), Some("silent"));
    }

    #[test]
    fn v12_idempotent_when_columns_already_exist() {
        let mut conn = Connection::open_in_memory().unwrap();
        // Apply up to v11 (version index is 1-based in to_version).
        MIGRATIONS.to_version(&mut conn, 11).unwrap();

        // Manually add the columns that v12 would create.
        conn.execute_batch("ALTER TABLE cron_runs ADD COLUMN delivery_status TEXT")
            .unwrap();
        conn.execute_batch("ALTER TABLE cron_runs ADD COLUMN no_notify_reason TEXT")
            .unwrap();

        // v12 must not fail even though columns already exist.
        MIGRATIONS.to_version(&mut conn, 21).unwrap();

        let cols: Vec<String> = conn
            .prepare("SELECT name FROM pragma_table_info('cron_runs')")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        assert!(cols.contains(&"delivery_status".to_string()));
        assert!(cols.contains(&"no_notify_reason".to_string()));
    }

    #[test]
    fn migrations_apply_cleanly_to_v6() {
        let mut conn = Connection::open_in_memory().unwrap();
        MIGRATIONS.to_latest(&mut conn).unwrap();
        let cols: Vec<String> = conn
            .prepare("SELECT name FROM pragma_table_info('cron_specs')")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        assert!(
            cols.contains(&"job_name".to_string()),
            "job_name column missing"
        );
        assert!(
            cols.contains(&"schedule".to_string()),
            "schedule column missing"
        );
        assert!(
            cols.contains(&"prompt".to_string()),
            "prompt column missing"
        );
        assert!(
            cols.contains(&"lock_ttl".to_string()),
            "lock_ttl column missing"
        );
        assert!(
            cols.contains(&"max_budget_usd".to_string()),
            "max_budget_usd column missing"
        );
        assert!(
            cols.contains(&"created_at".to_string()),
            "created_at column missing"
        );
        assert!(
            cols.contains(&"updated_at".to_string()),
            "updated_at column missing"
        );
    }

    #[test]
    fn v13_one_shot_cron_columns() {
        let mut conn = Connection::open_in_memory().unwrap();
        MIGRATIONS.to_latest(&mut conn).unwrap();
        let cols: Vec<String> = conn
            .prepare("SELECT name FROM pragma_table_info('cron_specs')")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        assert!(
            cols.contains(&"recurring".to_string()),
            "recurring column missing"
        );
        assert!(
            cols.contains(&"run_at".to_string()),
            "run_at column missing"
        );
    }

    #[test]
    fn v13_idempotent_when_columns_already_exist() {
        let mut conn = Connection::open_in_memory().unwrap();
        MIGRATIONS.to_version(&mut conn, 12).unwrap();
        conn.execute_batch(
            "ALTER TABLE cron_specs ADD COLUMN recurring INTEGER NOT NULL DEFAULT 1",
        )
        .unwrap();
        conn.execute_batch("ALTER TABLE cron_specs ADD COLUMN run_at TEXT")
            .unwrap();
        MIGRATIONS.to_latest(&mut conn).unwrap();
        let cols: Vec<String> = conn
            .prepare("SELECT name FROM pragma_table_info('cron_specs')")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        assert!(cols.contains(&"recurring".to_string()));
        assert!(cols.contains(&"run_at".to_string()));
    }

    #[test]
    fn v13_existing_specs_get_recurring_true() {
        let mut conn = Connection::open_in_memory().unwrap();
        MIGRATIONS.to_version(&mut conn, 12).unwrap();
        conn.execute(
            "INSERT INTO cron_specs (job_name, schedule, prompt, max_budget_usd, created_at, updated_at) \
             VALUES ('old-job', '*/5 * * * *', 'do stuff', 1.0, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
            [],
        )
        .unwrap();
        MIGRATIONS.to_latest(&mut conn).unwrap();
        let recurring: i64 = conn
            .query_row(
                "SELECT recurring FROM cron_specs WHERE job_name = 'old-job'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(recurring, 1, "existing specs must default to recurring=1");
        let run_at: Option<String> = conn
            .query_row(
                "SELECT run_at FROM cron_specs WHERE job_name = 'old-job'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(run_at.is_none(), "existing specs must have run_at=NULL");
    }

    #[test]
    fn v14_pending_retains_table_exists() {
        let mut conn = Connection::open_in_memory().unwrap();
        MIGRATIONS.to_latest(&mut conn).unwrap();
        let cols: Vec<String> = conn
            .prepare("SELECT name FROM pragma_table_info('pending_retains')")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        for col in [
            "id",
            "content",
            "context",
            "document_id",
            "update_mode",
            "tags_json",
            "created_at",
            "attempts",
            "last_attempt_at",
            "last_error",
            "source",
        ] {
            assert!(cols.contains(&col.to_string()), "{col} column missing");
        }
    }

    #[test]
    fn v14_memory_alerts_table_exists() {
        let mut conn = Connection::open_in_memory().unwrap();
        MIGRATIONS.to_latest(&mut conn).unwrap();
        let cols: Vec<String> = conn
            .prepare("SELECT name FROM pragma_table_info('memory_alerts')")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        for col in ["alert_type", "first_sent_at"] {
            assert!(cols.contains(&col.to_string()), "{col} column missing");
        }
    }

    #[test]
    fn v14_pending_retains_created_index_exists() {
        let mut conn = Connection::open_in_memory().unwrap();
        MIGRATIONS.to_latest(&mut conn).unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='index' \
                 AND name='idx_pending_retains_created'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1, "idx_pending_retains_created should exist");
    }

    #[test]
    fn v14_idempotent_when_tables_already_exist() {
        let mut conn = Connection::open_in_memory().unwrap();
        // Apply up to v13 (version index is 1-based in to_version).
        MIGRATIONS.to_version(&mut conn, 13).unwrap();

        // Manually pre-create the tables that v14 would create, matching the
        // schema in sql/v14_memory_failure_handling.sql.
        conn.execute_batch(
            "CREATE TABLE pending_retains (
                 id              INTEGER PRIMARY KEY AUTOINCREMENT,
                 content         TEXT NOT NULL,
                 context         TEXT,
                 document_id     TEXT,
                 update_mode     TEXT,
                 tags_json       TEXT,
                 created_at      TEXT NOT NULL,
                 attempts        INTEGER NOT NULL DEFAULT 0,
                 last_attempt_at TEXT,
                 last_error      TEXT,
                 source          TEXT NOT NULL
             );
             CREATE TABLE memory_alerts (
                 alert_type    TEXT PRIMARY KEY,
                 first_sent_at TEXT NOT NULL
             );",
        )
        .unwrap();

        // v14 must not fail even though tables already exist.
        MIGRATIONS.to_latest(&mut conn).unwrap();

        let pending_cols: Vec<String> = conn
            .prepare("SELECT name FROM pragma_table_info('pending_retains')")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        for col in [
            "id",
            "content",
            "context",
            "document_id",
            "update_mode",
            "tags_json",
            "created_at",
            "attempts",
            "last_attempt_at",
            "last_error",
            "source",
        ] {
            assert!(
                pending_cols.contains(&col.to_string()),
                "{col} column missing from pending_retains"
            );
        }

        let alert_cols: Vec<String> = conn
            .prepare("SELECT name FROM pragma_table_info('memory_alerts')")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        for col in ["alert_type", "first_sent_at"] {
            assert!(
                alert_cols.contains(&col.to_string()),
                "{col} column missing from memory_alerts"
            );
        }

        let idx_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='index' \
                 AND name='idx_pending_retains_created'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            idx_count, 1,
            "idx_pending_retains_created should exist after idempotent v14"
        );
    }

    #[test]
    fn v15_creates_usage_events_table_with_indexes() {
        let mut conn = Connection::open_in_memory().unwrap();
        MIGRATIONS.to_latest(&mut conn).unwrap();

        // Table exists and is writable.
        conn.execute_batch(
            "INSERT INTO usage_events (
                ts, source, session_uuid, total_cost_usd, num_turns,
                model_usage_json
             ) VALUES (
                '2026-04-20T00:00:00Z', 'interactive', 'test-uuid', 0.05, 3, '{}'
             );",
        )
        .unwrap();

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM usage_events", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);

        // Indexes present.
        let indexes: Vec<String> = conn
            .prepare(
                "SELECT name FROM sqlite_master WHERE type='index' AND tbl_name='usage_events'",
            )
            .unwrap()
            .query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .filter_map(Result::ok)
            .collect();
        assert!(indexes.iter().any(|n| n == "idx_usage_events_ts"));
        assert!(indexes.iter().any(|n| n == "idx_usage_events_source_ts"));
    }

    #[test]
    fn v15_migration_is_idempotent() {
        let mut conn = Connection::open_in_memory().unwrap();
        MIGRATIONS.to_latest(&mut conn).unwrap();
        // Second call must be a no-op.
        MIGRATIONS.to_latest(&mut conn).unwrap();
    }

    #[test]
    fn v16_usage_events_has_api_key_source() {
        let mut conn = Connection::open_in_memory().unwrap();
        MIGRATIONS.to_latest(&mut conn).unwrap();
        let cols: Vec<String> = conn
            .prepare("SELECT name FROM pragma_table_info('usage_events')")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        assert!(
            cols.contains(&"api_key_source".to_string()),
            "api_key_source column missing"
        );
    }

    #[test]
    fn v16_backfills_existing_rows_to_none() {
        let mut conn = Connection::open_in_memory().unwrap();
        // Apply up to v15, insert a row without api_key_source.
        MIGRATIONS.to_version(&mut conn, 15).unwrap();
        conn.execute(
            "INSERT INTO usage_events (
                ts, source, session_uuid, total_cost_usd, num_turns, model_usage_json
             ) VALUES ('2026-04-20T00:00:00Z','interactive','s',0.0,1,'{}')",
            [],
        )
        .unwrap();
        // Apply v16.
        MIGRATIONS.to_latest(&mut conn).unwrap();
        let src: String = conn
            .query_row("SELECT api_key_source FROM usage_events LIMIT 1", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(src, "none");
    }

    #[test]
    fn v16_idempotent_when_column_already_exists() {
        let mut conn = Connection::open_in_memory().unwrap();
        MIGRATIONS.to_version(&mut conn, 15).unwrap();
        conn.execute_batch(
            "ALTER TABLE usage_events ADD COLUMN api_key_source TEXT NOT NULL DEFAULT 'none'",
        )
        .unwrap();
        // v16 must succeed even though the column already exists.
        MIGRATIONS.to_latest(&mut conn).unwrap();
        let cols: Vec<String> = conn
            .prepare("SELECT name FROM pragma_table_info('usage_events')")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        assert!(cols.contains(&"api_key_source".to_string()));
    }

    #[test]
    fn v17_adds_cron_target_columns() {
        let mut conn = Connection::open_in_memory().unwrap();
        MIGRATIONS.to_latest(&mut conn).unwrap();
        let chat_present: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('cron_specs') WHERE name = 'target_chat_id'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(chat_present, 1, "target_chat_id column missing");
        let thread_present: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('cron_specs') WHERE name = 'target_thread_id'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(thread_present, 1, "target_thread_id column missing");
    }

    #[test]
    fn v17_is_idempotent_on_rerun() {
        let mut conn = Connection::open_in_memory().unwrap();
        MIGRATIONS.to_latest(&mut conn).unwrap();
        // Manually re-run the v17 hook; it must not error.
        let tx = conn.transaction().unwrap();
        super::v17_cron_target(&tx).unwrap();
        tx.commit().unwrap();
    }

    #[test]
    fn v17_existing_rows_get_null_target() {
        let mut conn = Connection::open_in_memory().unwrap();
        // Stop one version before v17 so the legacy row is inserted into a table
        // without target_chat_id / target_thread_id columns.
        MIGRATIONS.to_version(&mut conn, 16).unwrap();
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO cron_specs (job_name, schedule, prompt, max_budget_usd, created_at, updated_at) \
             VALUES ('legacy', '*/5 * * * *', 'p', 1.0, ?1, ?1)",
            [&now],
        )
        .unwrap();
        // Apply v17 — this is what we're actually testing.
        MIGRATIONS.to_latest(&mut conn).unwrap();
        let target: Option<i64> = conn
            .query_row(
                "SELECT target_chat_id FROM cron_specs WHERE job_name = 'legacy'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(
            target.is_none(),
            "legacy row should have NULL target_chat_id"
        );
    }

    #[test]
    fn v18_cron_runs_has_target_columns() {
        let mut conn = Connection::open_in_memory().unwrap();
        MIGRATIONS.to_version(&mut conn, 21).unwrap();
        let cols: Vec<String> = conn
            .prepare("SELECT name FROM pragma_table_info('cron_runs')")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        assert!(
            cols.contains(&"target_chat_id".to_string()),
            "cron_runs.target_chat_id column missing"
        );
        assert!(
            cols.contains(&"target_thread_id".to_string()),
            "cron_runs.target_thread_id column missing"
        );
    }

    #[test]
    fn v18_is_idempotent() {
        // Apply once.
        let mut conn = Connection::open_in_memory().unwrap();
        MIGRATIONS.to_latest(&mut conn).unwrap();
        // Apply again — must not error (re-running should be a no-op).
        MIGRATIONS.to_latest(&mut conn).unwrap();
    }

    #[test]
    fn v18_backfills_target_from_cron_specs_for_pending_undelivered_runs() {
        // Stop one version before v18 so the legacy run is inserted into a
        // cron_runs table that does not yet have target_* columns.
        let mut conn = Connection::open_in_memory().unwrap();
        MIGRATIONS.to_version(&mut conn, 17).unwrap();
        let now = chrono::Utc::now().to_rfc3339();
        // Spec with target_chat_id set, target_thread_id NULL.
        conn.execute(
            "INSERT INTO cron_specs (job_name, schedule, prompt, max_budget_usd, created_at, updated_at, target_chat_id) \
             VALUES ('legacy-recurring', '*/5 * * * *', 'p', 1.0, ?1, ?1, 12345)",
            [&now],
        )
        .unwrap();
        // Pre-v18 cron_runs row: status=success, notify_json present,
        // delivered_at NULL — i.e. an undelivered run waiting for the
        // delivery loop to pick it up.
        conn.execute(
            "INSERT INTO cron_runs (id, job_name, started_at, finished_at, exit_code, status, log_path, summary, notify_json, delivered_at) \
             VALUES ('run-1', 'legacy-recurring', ?1, ?1, 0, 'success', '/tmp/x.ndjson', 's', '{\"text\":\"hi\"}', NULL)",
            [&now],
        )
        .unwrap();
        // Apply through v21 so cron_runs still exists while checking v18 behavior.
        MIGRATIONS.to_version(&mut conn, 21).unwrap();
        let chat: Option<i64> = conn
            .query_row(
                "SELECT target_chat_id FROM cron_runs WHERE id = 'run-1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            chat,
            Some(12345),
            "v18 must backfill target_chat_id from cron_specs"
        );
        let thread: Option<i64> = conn
            .query_row(
                "SELECT target_thread_id FROM cron_runs WHERE id = 'run-1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(
            thread.is_none(),
            "target_thread_id should remain NULL when spec's thread is NULL"
        );
    }

    #[test]
    fn v23_creates_async_runs_and_drops_cron_runs() {
        let mut conn = Connection::open_in_memory().unwrap();
        MIGRATIONS.to_latest(&mut conn).unwrap();

        let async_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='async_runs'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(async_count, 1, "async_runs table should exist");

        let cron_runs_exists = conn.prepare("SELECT 1 FROM cron_runs LIMIT 1").is_ok();
        assert!(!cron_runs_exists, "cron_runs must be dropped after v22");
    }

    #[test]
    #[allow(clippy::type_complexity)]
    fn v23_migrates_cron_runs_to_async_runs() {
        let mut conn = Connection::open_in_memory().unwrap();
        MIGRATIONS.to_version(&mut conn, 21).unwrap();

        conn.execute(
            "INSERT INTO cron_runs (
            id, job_name, started_at, finished_at, exit_code, status, log_path,
            summary, notify_json, delivered_at, delivery_status, no_notify_reason,
            target_chat_id, target_thread_id
         ) VALUES (
            'run-1', 'morning', '2026-05-18T01:00:00Z', '2026-05-18T01:01:00Z',
            0, 'success', '/log/run-1.ndjson', 'summary', '{\"content\":\"hi\"}',
            NULL, 'pending', NULL, -100, 7
         )",
            [],
        )
        .unwrap();

        MIGRATIONS.to_latest(&mut conn).unwrap();

        let row: (
            String,
            String,
            String,
            Option<String>,
            Option<String>,
            Option<i64>,
            Option<i64>,
        ) = conn
            .query_row(
                "SELECT kind, producer_ref, delivery_status, run_note, delivery_json, target_chat_id, target_thread_id
             FROM async_runs WHERE id = 'run-1'",
                [],
                |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                        r.get(6)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(row.0, "cron");
        assert_eq!(row.1, "morning");
        assert_eq!(row.2, "none");
        assert_eq!(row.3.as_deref(), Some("summary"));
        assert!(row.4.is_none());
        assert_eq!(row.5, Some(-100));
        assert_eq!(row.6, Some(7));
    }

    #[test]
    fn v23_migrates_background_run_detected_by_schedule() {
        let mut conn = Connection::open_in_memory().unwrap();
        MIGRATIONS.to_version(&mut conn, 21).unwrap();

        conn.execute(
            "INSERT INTO cron_specs (
                job_name, schedule, prompt, max_budget_usd, created_at, updated_at,
                target_chat_id
             ) VALUES (
                'continuation-run', '@bg:123e4567-e89b-12d3-a456-426614174000',
                'continue', 1.0, '2026-05-18T00:00:00Z', '2026-05-18T00:00:00Z',
                -100
             )",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO cron_runs (id, job_name, started_at, status, log_path, delivery_status)
             VALUES (
                'schedule-bg-1', 'continuation-run', '2026-05-18T02:00:00Z',
                'success', '/log/schedule-bg-1.ndjson', 'silent'
             )",
            [],
        )
        .unwrap();

        MIGRATIONS.to_latest(&mut conn).unwrap();

        let row: (String, Option<String>, Option<String>) = conn
            .query_row(
                "SELECT kind, source_session_id, handoff_state
                 FROM async_runs WHERE id = 'schedule-bg-1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(row.0, "background");
        assert_eq!(
            row.1.as_deref(),
            Some("123e4567-e89b-12d3-a456-426614174000")
        );
        assert_eq!(row.2.as_deref(), Some("spawned"));
    }

    #[test]
    fn v23_migrates_immediate_background_run_with_source_header() {
        let mut conn = Connection::open_in_memory().unwrap();
        MIGRATIONS.to_version(&mut conn, 21).unwrap();

        conn.execute(
            "INSERT INTO cron_specs (
                job_name, schedule, prompt, max_budget_usd, created_at, updated_at,
                target_chat_id
             ) VALUES (
                'started-immediate-bg', '@immediate',
                'X-FORK-FROM: 123e4567-e89b-12d3-a456-426614174003
continue started background work',
                1.0, '2026-05-18T00:00:00Z', '2026-05-18T00:00:00Z',
                -100
             )",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO cron_runs (id, job_name, started_at, status, log_path, delivery_status)
             VALUES (
                'started-immediate-run', 'started-immediate-bg', '2026-05-18T02:00:00Z',
                'success', '/log/started-immediate-run.ndjson', 'silent'
             )",
            [],
        )
        .unwrap();

        MIGRATIONS.to_latest(&mut conn).unwrap();

        let row: (String, Option<String>, Option<String>) = conn
            .query_row(
                "SELECT kind, source_session_id, handoff_state
                 FROM async_runs WHERE id = 'started-immediate-run'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(row.0, "background");
        assert_eq!(
            row.1.as_deref(),
            Some("123e4567-e89b-12d3-a456-426614174003")
        );
        assert_eq!(row.2.as_deref(), Some("spawned"));

        let spec_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM cron_specs WHERE job_name = 'started-immediate-bg'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(spec_count, 0, "legacy immediate fork spec must be deleted");
    }

    #[test]
    fn v23_preserves_copied_run_null_thread_snapshot() {
        let mut conn = Connection::open_in_memory().unwrap();
        MIGRATIONS.to_version(&mut conn, 21).unwrap();

        conn.execute(
            "INSERT INTO cron_specs (
                job_name, schedule, prompt, max_budget_usd, created_at, updated_at,
                target_chat_id, target_thread_id
             ) VALUES (
                'threaded-spec', '*/15 * * * *', 'run', 1.0,
                '2026-05-18T00:00:00Z', '2026-05-18T00:00:00Z',
                -100, 77
             )",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO cron_runs (
                id, job_name, started_at, status, log_path, delivery_status,
                target_chat_id, target_thread_id
             ) VALUES (
                'root-topic-run', 'threaded-spec', '2026-05-18T02:00:00Z',
                'success', '/log/root-topic-run.ndjson', 'silent', -100, NULL
             )",
            [],
        )
        .unwrap();

        MIGRATIONS.to_latest(&mut conn).unwrap();

        let thread_id: Option<i64> = conn
            .query_row(
                "SELECT target_thread_id FROM async_runs WHERE id = 'root-topic-run'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(
            thread_id.is_none(),
            "copied run must preserve NULL target_thread_id snapshot"
        );
    }

    #[test]
    fn v23_detects_background_run_by_bg_job_name_when_spec_is_missing() {
        // Orphaned cron_runs row (cron_specs row absent) with a `bg-` prefixed
        // job_name is the legacy shape we still want to classify as background.
        let mut conn = Connection::open_in_memory().unwrap();
        MIGRATIONS.to_version(&mut conn, 21).unwrap();

        conn.execute(
            "INSERT INTO cron_runs (id, job_name, started_at, status, log_path, delivery_status)
             VALUES (
                'bg-name-run', 'bg-legacy', '2026-05-18T02:00:00Z',
                'success', '/log/bg-name-run.ndjson', 'silent'
             )",
            [],
        )
        .unwrap();

        MIGRATIONS.to_latest(&mut conn).unwrap();

        let row: (String, Option<String>) = conn
            .query_row(
                "SELECT kind, handoff_state FROM async_runs WHERE id = 'bg-name-run'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(row.0, "background");
        assert_eq!(row.1.as_deref(), Some("spawned"));
    }

    #[test]
    fn v23_keeps_user_cron_with_bg_prefix_classified_as_cron() {
        // A user-created recurring cron job whose name happens to start with
        // `bg-` (allowed by validate_job_name) and whose spec is still present
        // must NOT be reclassified as background. Only the `bg-` + orphaned-
        // spec combination survives as a background heuristic.
        let mut conn = Connection::open_in_memory().unwrap();
        MIGRATIONS.to_version(&mut conn, 21).unwrap();

        conn.execute(
            "INSERT INTO cron_specs (
                job_name, schedule, prompt, max_budget_usd, created_at, updated_at,
                target_chat_id
             ) VALUES (
                'bg-status-check', '0 9 * * *', 'check status', 1.0,
                '2026-05-18T00:00:00Z', '2026-05-18T00:00:00Z',
                -100
             )",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO cron_runs (id, job_name, started_at, status, log_path, delivery_status)
             VALUES (
                'bg-status-check-run', 'bg-status-check', '2026-05-18T09:00:00Z',
                'success', '/log/bg-status-check-run.ndjson', 'pending'
             )",
            [],
        )
        .unwrap();

        MIGRATIONS.to_latest(&mut conn).unwrap();

        let row: (String, Option<String>) = conn
            .query_row(
                "SELECT kind, handoff_state FROM async_runs WHERE id = 'bg-status-check-run'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            row.0, "cron",
            "user cron job with `bg-` prefix and live spec must remain kind='cron'"
        );
        assert!(
            row.1.is_none(),
            "user cron job must not be marked as a spawned handoff"
        );
    }

    #[test]
    fn v23_synthesizes_failed_background_run_for_pending_legacy_bg_spec() {
        let mut conn = Connection::open_in_memory().unwrap();
        MIGRATIONS.to_version(&mut conn, 21).unwrap();

        conn.execute(
            "INSERT INTO cron_specs (
                job_name, schedule, prompt, max_budget_usd, created_at, updated_at,
                triggered_at, target_chat_id, target_thread_id
             ) VALUES (
                'queued-bg', '@bg:123e4567-e89b-12d3-a456-426614174001',
                'continue', 1.0, '2026-05-18T00:00:00Z', '2026-05-18T00:00:00Z',
                '2026-05-18T02:00:00Z', -100, 42
             )",
            [],
        )
        .unwrap();

        MIGRATIONS.to_latest(&mut conn).unwrap();

        let row: (i64, String, String, String, Option<String>, Option<i64>) = conn
            .query_row(
                "SELECT COUNT(*), kind, status, delivery_status, source_session_id, target_thread_id
                 FROM async_runs WHERE producer_ref = 'queued-bg'",
                [],
                |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(row.0, 1);
        assert_eq!(row.1, "background");
        assert_eq!(row.2, "failed");
        assert_eq!(row.3, "none");
        assert_eq!(
            row.4.as_deref(),
            Some("123e4567-e89b-12d3-a456-426614174001")
        );
        assert_eq!(row.5, Some(42));

        let spec_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM cron_specs WHERE job_name = 'queued-bg'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(spec_count, 0, "legacy @bg cron spec must be deleted");
    }

    #[test]
    fn v23_synthesizes_failed_background_run_for_immediate_fork_spec() {
        let mut conn = Connection::open_in_memory().unwrap();
        MIGRATIONS.to_version(&mut conn, 21).unwrap();

        conn.execute(
            "INSERT INTO cron_specs (
                job_name, schedule, prompt, max_budget_usd, created_at, updated_at,
                triggered_at, target_chat_id, target_thread_id
             ) VALUES (
                'immediate-bg', '@immediate',
                'X-FORK-FROM: 123e4567-e89b-12d3-a456-426614174002
continue background work',
                1.0, '2026-05-18T00:00:00Z', '2026-05-18T00:00:00Z',
                '2026-05-18T02:00:00Z', -100, 43
             )",
            [],
        )
        .unwrap();

        MIGRATIONS.to_latest(&mut conn).unwrap();

        let async_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM async_runs WHERE producer_ref = 'immediate-bg'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(async_count, 1);

        let row: (String, String, String, Option<String>, Option<i64>) = conn
            .query_row(
                "SELECT kind, status, delivery_status, source_session_id, target_thread_id
                 FROM async_runs WHERE producer_ref = 'immediate-bg'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .unwrap();
        assert_eq!(row.0, "background");
        assert_eq!(row.1, "failed");
        assert_eq!(row.2, "none");
        assert_eq!(
            row.3.as_deref(),
            Some("123e4567-e89b-12d3-a456-426614174002")
        );
        assert_eq!(row.4, Some(43));

        let spec_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM cron_specs WHERE job_name = 'immediate-bg'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(spec_count, 0, "legacy immediate fork spec must be deleted");
    }

    #[test]
    fn v23_maps_silent_delivery_status_to_none() {
        let mut conn = Connection::open_in_memory().unwrap();
        MIGRATIONS.to_version(&mut conn, 21).unwrap();

        conn.execute(
            "INSERT INTO cron_runs (id, job_name, started_at, status, log_path, delivery_status)
         VALUES ('silent-1', 'quiet', '2026-05-18T02:00:00Z', 'success', '/log', 'silent')",
            [],
        )
        .unwrap();

        MIGRATIONS.to_latest(&mut conn).unwrap();

        let delivery: (i64, String) = conn
            .query_row(
                "SELECT delivery_required, delivery_status FROM async_runs WHERE id = 'silent-1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(delivery, (0, "none".to_string()));
    }

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

    #[test]
    fn conversation_messages_schema_exists() {
        let mut conn = Connection::open_in_memory().unwrap();
        MIGRATIONS.to_latest(&mut conn).unwrap();

        for table in ["conversation_messages", "conversation_messages_fts"] {
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
    fn skill_review_reports_migration_creates_report_table() {
        let mut conn = Connection::open_in_memory().unwrap();
        MIGRATIONS.to_latest(&mut conn).unwrap();

        let exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='skill_review_reports'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(exists, 1, "skill_review_reports table must exist");

        for column in [
            "agent_name",
            "source_invocation_id",
            "trigger_kind",
            "status",
            "confidence",
            "candidate_skill_name",
            "review_output_json",
            "telegram_notified",
        ] {
            let count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM pragma_table_info('skill_review_reports') WHERE name = ?1",
                    [column],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(count, 1, "{column} column must exist");
        }
    }

    #[test]
    fn conversation_messages_unique_inbound_message() {
        let mut conn = Connection::open_in_memory().unwrap();
        MIGRATIONS.to_latest(&mut conn).unwrap();

        conn.execute(
            "INSERT INTO conversation_messages (
                platform, chat_id, thread_id, message_id, role, content
             ) VALUES ('telegram', 10, 0, 25, 'user', 'hello')",
            [],
        )
        .unwrap();
        let result = conn.execute(
            "INSERT INTO conversation_messages (
                platform, chat_id, thread_id, message_id, role, content
             ) VALUES ('telegram', 10, 0, 25, 'user', 'duplicate')",
            [],
        );

        assert!(
            result.is_err(),
            "same platform/chat/message/role inbound row must be unique"
        );
    }

    #[test]
    fn conversation_messages_fts_tracks_updates() {
        let mut conn = Connection::open_in_memory().unwrap();
        MIGRATIONS.to_latest(&mut conn).unwrap();

        conn.execute(
            "INSERT INTO conversation_messages (
                platform, chat_id, thread_id, message_id, role, content
             ) VALUES ('telegram', 10, 0, 25, 'user', 'original term')",
            [],
        )
        .unwrap();

        let original_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM conversation_messages_fts
                 WHERE conversation_messages_fts MATCH 'original'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(original_count, 1);

        conn.execute(
            "UPDATE conversation_messages SET content = 'replacement term' WHERE id = 1",
            [],
        )
        .unwrap();

        let original_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM conversation_messages_fts
                 WHERE conversation_messages_fts MATCH 'original'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let replacement_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM conversation_messages_fts
                 WHERE conversation_messages_fts MATCH 'replacement'",
                [],
                |r| r.get(0),
            )
            .unwrap();

        assert_eq!(original_count, 0, "old FTS term must be removed");
        assert_eq!(replacement_count, 1, "new FTS term must be indexed");
    }

    #[test]
    fn skill_nudge_state_has_review_gate_defaults() {
        let mut conn = Connection::open_in_memory().unwrap();
        MIGRATIONS.to_latest(&mut conn).unwrap();

        conn.execute(
            "INSERT INTO skill_nudge_state (agent_name) VALUES ('right')",
            [],
        )
        .unwrap();

        let row: (i64, i64, Option<String>, Option<String>) = conn
            .query_row(
                "SELECT creation_review_interval, daily_review_count, daily_review_date, last_review_status \
             FROM skill_nudge_state WHERE agent_name='right'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!(row, (15, 0, None, None));
    }

    #[test]
    fn skill_nudge_state_existing_v21_rows_get_review_gate_defaults() {
        let mut conn = Connection::open_in_memory().unwrap();
        MIGRATIONS.to_version(&mut conn, 21).unwrap();

        conn.execute(
            "INSERT INTO skill_nudge_state (agent_name) VALUES ('right')",
            [],
        )
        .unwrap();

        MIGRATIONS.to_latest(&mut conn).unwrap();

        let row: (i64, i64, Option<String>, Option<String>) = conn
            .query_row(
                "SELECT creation_review_interval, daily_review_count, daily_review_date, last_review_status \
             FROM skill_nudge_state WHERE agent_name='right'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!(row, (15, 0, None, None));
    }

    #[test]
    fn skill_nudge_state_review_gate_migration_tolerates_existing_columns() {
        let mut conn = Connection::open_in_memory().unwrap();
        MIGRATIONS.to_version(&mut conn, 21).unwrap();

        conn.execute_batch(
            "ALTER TABLE skill_nudge_state
             ADD COLUMN creation_review_interval INTEGER NOT NULL DEFAULT 15;",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO skill_nudge_state (agent_name) VALUES ('right')",
            [],
        )
        .unwrap();

        MIGRATIONS
            .to_latest(&mut conn)
            .expect("v22 migration should tolerate pre-existing review columns");

        for column in [
            "creation_review_interval",
            "daily_review_count",
            "daily_review_date",
            "last_review_status",
        ] {
            let count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM pragma_table_info('skill_nudge_state') WHERE name = ?1",
                    [column],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(count, 1, "{column} column must exist");
        }

        let row: (i64, i64, Option<String>, Option<String>) = conn
            .query_row(
                "SELECT creation_review_interval, daily_review_count, daily_review_date, last_review_status \
             FROM skill_nudge_state WHERE agent_name='right'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!(row, (15, 0, None, None));
    }

    #[test]
    fn migration_v26_adds_circuit_breaker_columns_idempotently() {
        let mut conn = rusqlite::Connection::open_in_memory().unwrap();
        MIGRATIONS.to_version(&mut conn, 25).unwrap();
        let pre_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('skill_nudge_state') \
                 WHERE name IN ('consecutive_review_failures', 'review_circuit_open_until')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(pre_count, 0, "preconditions: columns not yet present");

        MIGRATIONS.to_version(&mut conn, 26).unwrap();
        let post_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('skill_nudge_state') \
                 WHERE name IN ('consecutive_review_failures', 'review_circuit_open_until')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(post_count, 2, "both columns present after v26");

        // Re-running v26 is a no-op — verifies idempotency.
        MIGRATIONS.to_version(&mut conn, 26).unwrap();
        let post_count_again: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('skill_nudge_state') \
                 WHERE name IN ('consecutive_review_failures', 'review_circuit_open_until')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(post_count_again, 2);
    }

    #[test]
    fn v27_adds_source_column_to_skill_nudge_signals() {
        let mut conn = Connection::open_in_memory().unwrap();
        MIGRATIONS.to_latest(&mut conn).unwrap();
        let has_column: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('skill_nudge_signals') WHERE name = ?1",
                ["source"],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(has_column, 1, "source column must exist");
        let not_null: i64 = conn
            .query_row(
                "SELECT \"notnull\" FROM pragma_table_info('skill_nudge_signals') WHERE name = ?1",
                ["source"],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(not_null, 1, "source column must be NOT NULL");
    }

    #[test]
    fn v27_is_idempotent_on_databases_already_at_v27() {
        let mut conn = Connection::open_in_memory().unwrap();
        MIGRATIONS.to_latest(&mut conn).unwrap();
        // Re-run by calling the migration registry again should not error.
        MIGRATIONS.to_latest(&mut conn).unwrap();
    }

    #[test]
    fn v27_index_on_source_column_exists() {
        let mut conn = Connection::open_in_memory().unwrap();
        MIGRATIONS.to_latest(&mut conn).unwrap();
        let exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master \
                 WHERE type='index' AND name='idx_skill_nudge_signals_source'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(exists, 1, "idx_skill_nudge_signals_source must exist");
    }

    #[test]
    fn v28_adds_wall_elapsed_ms_column_idempotently() {
        let mut conn = Connection::open_in_memory().unwrap();
        MIGRATIONS.to_version(&mut conn, 27).unwrap();
        let pre: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('usage_events') WHERE name = ?1",
                ["wall_elapsed_ms"],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(pre, 0, "wall_elapsed_ms must not exist at v27");

        MIGRATIONS.to_version(&mut conn, 28).unwrap();
        let post: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('usage_events') WHERE name = ?1",
                ["wall_elapsed_ms"],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(post, 1, "wall_elapsed_ms must exist at v28");

        let notnull: i64 = conn
            .query_row(
                "SELECT \"notnull\" FROM pragma_table_info('usage_events') WHERE name = ?1",
                ["wall_elapsed_ms"],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(notnull, 0, "wall_elapsed_ms must be nullable");

        // Re-run is no-op.
        MIGRATIONS.to_version(&mut conn, 28).unwrap();
    }

    #[test]
    fn v29_creates_curator_state_singleton_table_idempotently() {
        let mut conn = Connection::open_in_memory().unwrap();
        MIGRATIONS.to_version(&mut conn, 28).unwrap();
        let pre: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='curator_state'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(pre, 0);

        MIGRATIONS.to_version(&mut conn, 29).unwrap();
        let post: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='curator_state'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(post, 1);

        // Singleton CHECK constraint: id=2 must fail.
        let err = conn.execute(
            "INSERT INTO curator_state (agent_singleton_id, last_run_at) VALUES (2, NULL)",
            [],
        );
        assert!(err.is_err(), "CHECK constraint must reject id != 1");

        // id=1 must succeed.
        conn.execute(
            "INSERT INTO curator_state (agent_singleton_id, last_run_at) VALUES (1, '2026-05-22T00:00:00Z')",
            [],
        )
        .unwrap();

        // Re-run is no-op.
        MIGRATIONS.to_version(&mut conn, 29).unwrap();
    }

    #[test]
    fn v30_adds_skill_learning_event_hint_outcome_idempotently() {
        let mut conn = Connection::open_in_memory().unwrap();
        MIGRATIONS.to_version(&mut conn, 29).unwrap();
        let pre: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('skill_learning_events') WHERE name = ?1",
                ["hint_outcome"],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(pre, 0);

        MIGRATIONS.to_version(&mut conn, 30).unwrap();
        let post: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('skill_learning_events') WHERE name = ?1",
                ["hint_outcome"],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(post, 1);

        conn.execute(
            "INSERT INTO skill_learning_events (
                invocation_id, agent_name, action, skill_name, phase, status,
                event_refs_json, hint_outcome
             ) VALUES (
                'inv-1', 'alpha', 'create', 'rightx-demo', 'finish', 'aborted',
                '[]', 'refused'
             )",
            [],
        )
        .unwrap();

        let invalid = conn.execute(
            "INSERT INTO skill_learning_events (
                invocation_id, agent_name, action, skill_name, phase, status,
                event_refs_json, hint_outcome
             ) VALUES (
                'inv-2', 'alpha', 'create', 'rightx-demo', 'finish', 'aborted',
                '[]', 'bogus'
             )",
            [],
        );
        assert!(invalid.is_err(), "invalid hint_outcome must be rejected");

        // Re-run is no-op.
        MIGRATIONS.to_version(&mut conn, 30).unwrap();
    }

    #[test]
    fn skill_lifecycle_schema_constraints_and_defaults() {
        let mut conn = Connection::open_in_memory().unwrap();
        MIGRATIONS.to_latest(&mut conn).unwrap();

        let table_exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='skill_lifecycle'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(table_exists, 1, "skill_lifecycle table must exist");

        let skill_name_pk: i64 = conn
            .query_row(
                "SELECT pk FROM pragma_table_info('skill_lifecycle') WHERE name = 'skill_name'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(skill_name_pk, 1, "skill_name must be the primary key");

        conn.execute(
            "INSERT INTO skill_lifecycle (skill_name) VALUES ('default-row')",
            [],
        )
        .unwrap();
        let defaults: (i64, i64, i64) = conn
            .query_row(
                "SELECT pinned, use_count, patch_count FROM skill_lifecycle WHERE skill_name = 'default-row'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(defaults, (0, 0, 0));

        for state in ["active", "stale", "archived"] {
            conn.execute(
                "INSERT INTO skill_lifecycle (skill_name, state) VALUES (?1, ?2)",
                [format!("state-{state}"), state.to_string()],
            )
            .unwrap();
        }
        let invalid_state = conn.execute(
            "INSERT INTO skill_lifecycle (skill_name, state) VALUES ('state-invalid', 'retired')",
            [],
        );
        assert!(invalid_state.is_err(), "invalid state must be rejected");

        for created_by in ["foreground", "probe_writer", "curator", "bundled"] {
            conn.execute(
                "INSERT INTO skill_lifecycle (skill_name, created_by) VALUES (?1, ?2)",
                [format!("created-by-{created_by}"), created_by.to_string()],
            )
            .unwrap();
        }
        let invalid_created_by = conn.execute(
            "INSERT INTO skill_lifecycle (skill_name, created_by) VALUES ('created-by-invalid', 'unknown')",
            [],
        );
        assert!(
            invalid_created_by.is_err(),
            "invalid created_by must be rejected"
        );
    }
}
