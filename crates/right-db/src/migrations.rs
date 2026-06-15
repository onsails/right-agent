use std::future::Future;
use std::pin::Pin;

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
const V14_SCHEMA: &str = include_str!("sql/v14_memory_failure_handling.sql");
const V15_SCHEMA: &str = include_str!("sql/v15_usage_events.sql");
const V19_SCHEMA: &str = include_str!("sql/v19_cron_runs_target_index.sql");
const V20_SCHEMA: &str = include_str!("sql/v20_learned_skills.sql");
const V21_SCHEMA: &str = include_str!("sql/v21_conversation_messages.sql");
const V22_SCHEMA: &str = include_str!("sql/v22_skill_review_reports.sql");
/// Historical v23 async_runs shape, applied by `v23_async_runs` Rust hook.
const V23_SCHEMA: &str = include_str!("sql/v23_async_runs.sql");
const V24_SCHEMA: &str = include_str!("sql/v24_learning_episodes.sql");
const V25_SCHEMA: &str = include_str!("sql/v25_async_runs_delivery_decision.sql");
const V29_SCHEMA: &str = include_str!("sql/v29_curator_state.sql");
const V30_SCHEMA: &str = include_str!("sql/v30_skill_learning_hint_outcome.sql");
const V31_SCHEMA: &str = include_str!("sql/v31_skill_learning_events_dashboard_index.sql");
const V32_SCHEMA: &str = include_str!("sql/v32_skill_lifecycle.sql");
const V33_SCHEMA: &str = include_str!("sql/v33_mcp_oauth_resource.sql");
const V34_SCHEMA: &str = include_str!("sql/v34_turso_fts_indexes.sql");
const V35_SCHEMA: &str = include_str!("sql/v35_legacy_learning_cleanup.sql");
const V36_SCHEMA: &str = include_str!("sql/v36_mcp_http_headers.sql");
const V38_SCHEMA: &str = include_str!("sql/v38_skill_spend_and_learning_skip.sql");
const V39_SCHEMA: &str = include_str!("sql/v39_error_details.sql");
const V40_SCHEMA: &str = include_str!("sql/v40_forum_topics.sql");
const V43_SCHEMA: &str = include_str!("sql/v43_thread_focus.sql");
const V44_SCHEMA: &str = include_str!("sql/v44_skill_lifecycle_cron.sql");
const V46_NOTICE_TOKEN: &str = include_str!("sql/v46_notice_token.sql");
const V47_CRON_SKILL_LINKS: &str = include_str!("sql/v47_cron_skill_links.sql");
const V48_CURATOR_RUNS: &str = include_str!("sql/v48_curator_runs.sql");

pub const LATEST_SCHEMA_VERSION: u32 = 48;

type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;
type MigrationHook =
    for<'a> fn(&'a dyn MigrationConnection) -> BoxFuture<'a, Result<(), crate::DbError>>;

mod sealed {
    pub trait MigrationConnection {}
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

pub trait MigrationConnection: sealed::MigrationConnection + Sync {
    fn execute_batch<'a>(&'a self, sql: &'a str) -> BoxFuture<'a, Result<(), crate::DbError>>;
    fn query_i64<'a>(
        &'a self,
        sql: &'a str,
        params: MigrationParams<'a>,
    ) -> BoxFuture<'a, Result<i64, crate::DbError>>;
}

impl Migrations {
    pub async fn to_latest(&self, conn: &crate::Connection) -> Result<(), crate::DbError> {
        self.to_version(conn, self.highest_version()).await
    }

    pub async fn to_version(
        &self,
        conn: &crate::Connection,
        target_version: u32,
    ) -> Result<(), crate::DbError> {
        // Fast-path hint: cheap pre-tx read avoids taking the immediate lock
        // when there is genuinely nothing to do. The authoritative read happens
        // inside the transaction below: two cold-boot callers can both read
        // user_version = 0 here, and only one wins the BEGIN IMMEDIATE race.
        let hint_current = migration_user_version(conn)
            .await
            .map_err(|source| migration_error(conn, 0, source))?;
        let highest = self.highest_version();

        if hint_current > highest {
            return Err(migration_version_error(
                conn,
                hint_current,
                format!("database schema is newer than known migrations ({highest})"),
            ));
        }
        if target_version > highest {
            return Err(migration_version_error(
                conn,
                target_version,
                format!("unknown migration target above highest known version {highest}"),
            ));
        }
        if target_version < hint_current {
            return Err(migration_version_error(
                conn,
                target_version,
                format!("down migrations are unsupported from current version {hint_current}"),
            ));
        }
        if target_version == hint_current {
            return Ok(());
        }

        let mut active_version = hint_current + 1;
        let tx = conn.transaction().await?;
        let result = async {
            // Re-read user_version INSIDE the immediate transaction. Two
            // concurrent migrators on the same DB file can both observe
            // user_version = 0 outside the lock. Only one wins
            // BEGIN IMMEDIATE. The loser, once unblocked, must NOT
            // re-apply migrations the winner already committed (v23 is
            // non-idempotent: re-running it crashes with "no such table:
            // cron_runs").
            let current_in_tx_i64 = tx
                .query_i64("PRAGMA user_version", MigrationParams::Empty)
                .await?;
            let current_in_tx = u32::try_from(current_in_tx_i64)
                .map_err(|_| crate::DbError::InvalidParameter("negative user_version".into()))?;
            if current_in_tx >= target_version {
                // The winning migrator already advanced to or past our target.
                // Commit a no-op transaction and return.
                return Ok(());
            }
            for migration in self.migrations {
                if migration.version <= current_in_tx || migration.version > target_version {
                    continue;
                }
                active_version = migration.version;
                if !migration.sql.trim().is_empty() {
                    tx.execute_batch(migration.sql).await?;
                }
                if let Some(hook) = migration.hook {
                    hook(&tx).await?;
                }
                tx.execute_batch(&format!("PRAGMA user_version = {}", migration.version))
                    .await?;
            }
            Ok(())
        }
        .await;

        match result {
            Ok(()) => tx
                .commit()
                .await
                .map_err(|source| migration_error(conn, active_version, source))?,
            Err(source) => {
                if let Err(rollback_err) = tx.rollback().await {
                    tracing::warn!(
                        path = %conn.path().display(),
                        operation_error = format!("{source:#}"),
                        rollback_error = format!("{rollback_err:#}"),
                        "migration transaction rollback failed; returning original operation error",
                    );
                }
                return Err(migration_error(conn, active_version, source));
            }
        }

        Ok(())
    }

    fn highest_version(&self) -> u32 {
        self.migrations
            .last()
            .map(|migration| migration.version)
            .unwrap_or(0)
    }
}

async fn migration_user_version(conn: &crate::Connection) -> Result<u32, crate::DbError> {
    let version: i64 = conn
        .query_one("PRAGMA user_version", (), |row| row.get(0))
        .await?;
    u32::try_from(version)
        .map_err(|_| crate::DbError::InvalidParameter("negative user_version".into()))
}

fn migration_error(
    conn: &crate::Connection,
    version: u32,
    source: crate::DbError,
) -> crate::DbError {
    crate::DbError::Migration {
        path: conn.path().to_path_buf(),
        version,
        source: Box::new(source),
    }
}

fn migration_version_error(
    conn: &crate::Connection,
    version: u32,
    message: String,
) -> crate::DbError {
    crate::DbError::MigrationVersion {
        path: conn.path().to_path_buf(),
        version,
        message,
    }
}

async fn column_exists(
    conn: &dyn MigrationConnection,
    table: &str,
    column: &str,
) -> Result<bool, crate::DbError> {
    let count = conn
        .query_i64(
            "SELECT COUNT(*) FROM pragma_table_info(?) WHERE name = ?",
            MigrationParams::two_text(table, column),
        )
        .await?;
    Ok(count > 0)
}

impl sealed::MigrationConnection for crate::Transaction<'_> {}

impl MigrationConnection for crate::Transaction<'_> {
    fn execute_batch<'a>(&'a self, sql: &'a str) -> BoxFuture<'a, Result<(), crate::DbError>> {
        Box::pin(async move { crate::Transaction::execute_batch(self, sql).await })
    }

    fn query_i64<'a>(
        &'a self,
        sql: &'a str,
        params: MigrationParams<'a>,
    ) -> BoxFuture<'a, Result<i64, crate::DbError>> {
        Box::pin(async move {
            match params {
                MigrationParams::Empty => self.query_one(sql, (), |row| row.get(0)).await,
                MigrationParams::OneText(value) => {
                    self.query_one(sql, [value], |row| row.get(0)).await
                }
                MigrationParams::TwoText(first, second) => {
                    self.query_one(sql, [first, second], |row| row.get(0)).await
                }
            }
        })
    }
}

/// v12: Add delivery_status and no_notify_reason columns to cron_runs,
/// backfill existing rows, and create auto-set trigger.
///
/// Implemented as a Rust hook (not pure SQL) because SQLite lacks
/// `ADD COLUMN IF NOT EXISTS` — the ALTER TABLE would fail with
/// "duplicate column name" if re-run on a database that already has
/// the columns.
fn v12_cron_diagnostics(
    conn: &dyn MigrationConnection,
) -> BoxFuture<'_, Result<(), crate::DbError>> {
    Box::pin(async move {
        if !column_exists(conn, "cron_runs", "delivery_status").await? {
            conn.execute_batch("ALTER TABLE cron_runs ADD COLUMN delivery_status TEXT")
                .await?;
        }
        if !column_exists(conn, "cron_runs", "no_notify_reason").await? {
            conn.execute_batch("ALTER TABLE cron_runs ADD COLUMN no_notify_reason TEXT")
                .await?;
        }

        // Backfill existing rows (idempotent UPDATEs).
        conn.execute_batch(
            "UPDATE cron_runs SET delivery_status = 'delivered'
               WHERE notify_json IS NOT NULL AND delivered_at IS NOT NULL;
             UPDATE cron_runs SET delivery_status = 'pending'
               WHERE notify_json IS NOT NULL AND delivered_at IS NULL;
             UPDATE cron_runs SET delivery_status = 'silent'
               WHERE notify_json IS NULL;",
        )
        .await?;

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
        )
        .await?;

        Ok(())
    })
}

/// v13: Add recurring and run_at columns to cron_specs for one-shot job support.
///
/// Idempotent — checks pragma_table_info before each ALTER.
fn v13_one_shot_cron(conn: &dyn MigrationConnection) -> BoxFuture<'_, Result<(), crate::DbError>> {
    Box::pin(async move {
        if !column_exists(conn, "cron_specs", "recurring").await? {
            conn.execute_batch(
                "ALTER TABLE cron_specs ADD COLUMN recurring INTEGER NOT NULL DEFAULT 1",
            )
            .await?;
        }
        if !column_exists(conn, "cron_specs", "run_at").await? {
            conn.execute_batch("ALTER TABLE cron_specs ADD COLUMN run_at TEXT")
                .await?;
        }

        Ok(())
    })
}

/// v16: Add api_key_source column to usage_events.
///
/// Idempotent — checks pragma_table_info before ALTER. Column defaults
/// to 'none' which matches the setup-token (subscription) auth mode all
/// current Right Agent deployments use.
fn v16_usage_api_key_source(
    conn: &dyn MigrationConnection,
) -> BoxFuture<'_, Result<(), crate::DbError>> {
    Box::pin(async move {
        if !column_exists(conn, "usage_events", "api_key_source").await? {
            conn.execute_batch(
                "ALTER TABLE usage_events ADD COLUMN api_key_source TEXT NOT NULL DEFAULT 'none'",
            )
            .await?;
        }
        Ok(())
    })
}

/// v17: Add target_chat_id and target_thread_id to cron_specs.
///
/// Idempotent — checks pragma_table_info before each ALTER. Both columns
/// are nullable; the MCP layer validates presence on new rows. NULL on
/// existing rows is surfaced by `doctor::check_cron_targets`.
fn v17_cron_target(conn: &dyn MigrationConnection) -> BoxFuture<'_, Result<(), crate::DbError>> {
    Box::pin(async move {
        if !column_exists(conn, "cron_specs", "target_chat_id").await? {
            conn.execute_batch("ALTER TABLE cron_specs ADD COLUMN target_chat_id INTEGER")
                .await?;
        }
        if !column_exists(conn, "cron_specs", "target_thread_id").await? {
            conn.execute_batch("ALTER TABLE cron_specs ADD COLUMN target_thread_id INTEGER")
                .await?;
        }
        Ok(())
    })
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
fn v18_cron_runs_target(
    conn: &dyn MigrationConnection,
) -> BoxFuture<'_, Result<(), crate::DbError>> {
    Box::pin(async move {
        if !column_exists(conn, "cron_runs", "target_chat_id").await? {
            conn.execute_batch("ALTER TABLE cron_runs ADD COLUMN target_chat_id INTEGER")
                .await?;
        }
        if !column_exists(conn, "cron_runs", "target_thread_id").await? {
            conn.execute_batch("ALTER TABLE cron_runs ADD COLUMN target_thread_id INTEGER")
                .await?;
        }
        // Backfill target columns from cron_specs for runs whose spec still
        // exists. Idempotent — only touches rows where target_chat_id IS NULL.
        conn.execute_batch(
            "UPDATE cron_runs \
               SET target_chat_id = (SELECT target_chat_id FROM cron_specs WHERE cron_specs.job_name = cron_runs.job_name), \
                   target_thread_id = (SELECT target_thread_id FROM cron_specs WHERE cron_specs.job_name = cron_runs.job_name) \
               WHERE cron_runs.target_chat_id IS NULL \
                 AND EXISTS (SELECT 1 FROM cron_specs WHERE cron_specs.job_name = cron_runs.job_name)",
        )
        .await?;
        Ok(())
    })
}

/// v22: Add learned-skill review report storage and review-gate state columns.
///
/// The report table/indexes live in SQL. Column additions are guarded here
/// because SQLite has no `ADD COLUMN IF NOT EXISTS`.
fn v22_skill_review_reports(
    conn: &dyn MigrationConnection,
) -> BoxFuture<'_, Result<(), crate::DbError>> {
    Box::pin(async move {
        if !column_exists(conn, "skill_nudge_state", "creation_review_interval").await? {
            conn.execute_batch(
                "ALTER TABLE skill_nudge_state
                 ADD COLUMN creation_review_interval INTEGER NOT NULL DEFAULT 15",
            )
            .await?;
        }
        if !column_exists(conn, "skill_nudge_state", "daily_review_count").await? {
            conn.execute_batch(
                "ALTER TABLE skill_nudge_state
                 ADD COLUMN daily_review_count INTEGER NOT NULL DEFAULT 0",
            )
            .await?;
        }
        if !column_exists(conn, "skill_nudge_state", "daily_review_date").await? {
            conn.execute_batch("ALTER TABLE skill_nudge_state ADD COLUMN daily_review_date TEXT")
                .await?;
        }
        if !column_exists(conn, "skill_nudge_state", "last_review_status").await? {
            conn.execute_batch("ALTER TABLE skill_nudge_state ADD COLUMN last_review_status TEXT")
                .await?;
        }
        Ok(())
    })
}

fn v23_async_runs(conn: &dyn MigrationConnection) -> BoxFuture<'_, Result<(), crate::DbError>> {
    Box::pin(async move {
        conn.execute_batch(V23_SCHEMA).await?;
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
        )
        .await?;
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
        )
        .await?;
        conn.execute_batch(
            "DELETE FROM cron_specs
         WHERE schedule LIKE '@bg:%'
            OR (
              schedule = '@immediate'
              AND prompt LIKE 'X-FORK-FROM: %'
              AND instr(prompt, char(10)) > length('X-FORK-FROM: ') + 1
            )",
        )
        .await?;
        conn.execute_batch("DROP TABLE cron_runs").await?;
        Ok(())
    })
}

fn v24_learning_episodes(
    conn: &dyn MigrationConnection,
) -> BoxFuture<'_, Result<(), crate::DbError>> {
    Box::pin(async move {
        conn.execute_batch(V24_SCHEMA).await?;
        if !column_exists(conn, "skill_review_reports", "learning_episode_id").await? {
            conn.execute_batch(
                "ALTER TABLE skill_review_reports ADD COLUMN learning_episode_id INTEGER",
            )
            .await?;
        }
        conn.execute_batch(
            "CREATE INDEX IF NOT EXISTS idx_skill_review_reports_episode ON skill_review_reports(learning_episode_id)",
        )
        .await?;
        Ok(())
    })
}

/// v26: Add circuit-breaker columns to skill_nudge_state.
///
/// Idempotent — checks pragma_table_info before each ALTER. SQLite has no
/// `ADD COLUMN IF NOT EXISTS`.
fn v26_skill_nudge_circuit_breaker(
    conn: &dyn MigrationConnection,
) -> BoxFuture<'_, Result<(), crate::DbError>> {
    Box::pin(async move {
        if !column_exists(conn, "skill_nudge_state", "consecutive_review_failures").await? {
            conn.execute_batch(
                "ALTER TABLE skill_nudge_state ADD COLUMN consecutive_review_failures INTEGER NOT NULL DEFAULT 0",
            )
            .await?;
        }
        if !column_exists(conn, "skill_nudge_state", "review_circuit_open_until").await? {
            conn.execute_batch(
                "ALTER TABLE skill_nudge_state ADD COLUMN review_circuit_open_until TEXT",
            )
            .await?;
        }
        Ok(())
    })
}

/// v27: Add `source` column + index to `skill_nudge_signals`.
///
/// Idempotent — checks pragma_table_info before ALTER. SQLite has no
/// `ADD COLUMN IF NOT EXISTS`. The CREATE INDEX uses `IF NOT EXISTS`.
fn v27_skill_nudge_signals_source(
    conn: &dyn MigrationConnection,
) -> BoxFuture<'_, Result<(), crate::DbError>> {
    Box::pin(async move {
        if !column_exists(conn, "skill_nudge_signals", "source").await? {
            conn.execute_batch(
                "ALTER TABLE skill_nudge_signals ADD COLUMN source TEXT NOT NULL DEFAULT 'reply_field'",
            )
            .await?;
        }
        conn.execute_batch(
            "CREATE INDEX IF NOT EXISTS idx_skill_nudge_signals_source \
             ON skill_nudge_signals(source)",
        )
        .await?;
        Ok(())
    })
}

/// v28: Add nullable `wall_elapsed_ms` column to `usage_events`.
///
/// Idempotent — checks pragma_table_info before ALTER. Foreground worker
/// turns populate this; non-foreground sources leave NULL.
fn v28_usage_wall_elapsed_ms(
    conn: &dyn MigrationConnection,
) -> BoxFuture<'_, Result<(), crate::DbError>> {
    Box::pin(async move {
        if !column_exists(conn, "usage_events", "wall_elapsed_ms").await? {
            conn.execute_batch("ALTER TABLE usage_events ADD COLUMN wall_elapsed_ms INTEGER")
                .await?;
        }
        Ok(())
    })
}

/// v30: Add nullable hint_outcome to skill_learning_events.
///
/// Idempotent — checks pragma_table_info before ALTER. The column carries a
/// CHECK constraint for new writes, but remains nullable for historical rows.
fn v30_skill_learning_hint_outcome(
    conn: &dyn MigrationConnection,
) -> BoxFuture<'_, Result<(), crate::DbError>> {
    Box::pin(async move {
        if !column_exists(conn, "skill_learning_events", "hint_outcome").await? {
            conn.execute_batch(V30_SCHEMA).await?;
        }
        Ok(())
    })
}

fn v34_turso_fts_indexes(
    conn: &dyn MigrationConnection,
) -> BoxFuture<'_, Result<(), crate::DbError>> {
    Box::pin(async move {
        conn.execute_batch(
            "CREATE INDEX IF NOT EXISTS idx_memories_turso_fts
             ON memories USING fts(content);",
        )
        .await?;
        conn.execute_batch(
            "CREATE INDEX IF NOT EXISTS idx_conversation_messages_turso_fts
             ON conversation_messages USING fts(content);",
        )
        .await?;
        Ok(())
    })
}

/// v37: Delete usage rows from the retired learning pipeline.
///
/// `learning_reviewer` and `learning_selector` are dead sources from a
/// removed pipeline; they surface as "unknown usage source" warnings in the
/// dashboard. The hook guards against databases that pre-date `usage_events`
/// (v15) — such databases exist in tests that build a synthetic legacy schema
/// without going through every intermediate migration. The DELETE is idempotent
/// on a re-run: it removes zero rows when no matching source values remain.
fn v37_drop_legacy_usage_sources(
    conn: &dyn MigrationConnection,
) -> BoxFuture<'_, Result<(), crate::DbError>> {
    Box::pin(async move {
        let table_exists = conn
            .query_i64(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='usage_events'",
                MigrationParams::Empty,
            )
            .await?;
        if table_exists > 0 {
            conn.execute_batch(
                "DELETE FROM usage_events WHERE source IN ('learning_reviewer', 'learning_selector')",
            )
            .await?;
        }
        Ok(())
    })
}

/// v41: Force-notify trigger support.
///
/// `cron_specs.trigger_force_notify` is set together with `triggered_at` by a
/// force-notify trigger and cleared together. `async_runs.force_notify` marks a
/// run whose delivery overrides the silent decision and the idle gate.
///
/// Idempotent — checks `pragma_table_info` (no `ADD COLUMN IF NOT EXISTS` in
/// SQLite). Also guards on table existence via `sqlite_master`, like v37: a
/// partially-migrated DB that reaches v41 without `cron_specs`/`async_runs`
/// (e.g. one whose `user_version` advanced without ever running v6/v23) would
/// hit a bare ALTER on a missing table. Real agent DBs always have both tables
/// here, so the guard never short-circuits a genuine upgrade.
fn v41_cron_force_notify(
    conn: &dyn MigrationConnection,
) -> BoxFuture<'_, Result<(), crate::DbError>> {
    Box::pin(async move {
        let cron_specs_exists = conn
            .query_i64(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='cron_specs'",
                MigrationParams::Empty,
            )
            .await?;
        if cron_specs_exists > 0
            && !column_exists(conn, "cron_specs", "trigger_force_notify").await?
        {
            conn.execute_batch(
                "ALTER TABLE cron_specs ADD COLUMN trigger_force_notify INTEGER NOT NULL DEFAULT 0",
            )
            .await?;
        }
        let async_runs_exists = conn
            .query_i64(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='async_runs'",
                MigrationParams::Empty,
            )
            .await?;
        if async_runs_exists > 0 && !column_exists(conn, "async_runs", "force_notify").await? {
            conn.execute_batch(
                "ALTER TABLE async_runs ADD COLUMN force_notify INTEGER NOT NULL DEFAULT 0",
            )
            .await?;
        }
        Ok(())
    })
}

/// v42: Add a per-cron `model` column to `cron_specs`.
///
/// Nullable TEXT holding a CC model alias (`haiku`/`sonnet`/`opus`). NULL =
/// inherit the agent's global `/model` (the prior behavior), so existing rows
/// keep working unchanged. Idempotent — checks `pragma_table_info` before the
/// ALTER. Guards on table existence via `sqlite_master` like v41, because the
/// synthetic legacy-v33 test fixture lacks `cron_specs`.
fn v42_cron_model(conn: &dyn MigrationConnection) -> BoxFuture<'_, Result<(), crate::DbError>> {
    Box::pin(async move {
        let cron_specs_exists = conn
            .query_i64(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='cron_specs'",
                MigrationParams::Empty,
            )
            .await?;
        if cron_specs_exists > 0 && !column_exists(conn, "cron_specs", "model").await? {
            conn.execute_batch("ALTER TABLE cron_specs ADD COLUMN model TEXT")
                .await?;
        }
        Ok(())
    })
}

/// v45: Add transient trigger columns to `cron_specs`.
///
/// Four nullable columns carrying per-trigger context for cron continuations:
/// `trigger_extra_instruction` (TEXT), `trigger_then_json` (TEXT),
/// `trigger_origin_chat_id` (INTEGER), `trigger_origin_thread_id` (INTEGER).
/// All NULL for existing rows, preserving prior behavior. Idempotent — checks
/// `pragma_table_info` before each ALTER. Guards on table existence via
/// `sqlite_master` like v42, because the synthetic legacy-v33 test fixture
/// lacks `cron_specs`.
fn v45_cron_trigger_transient(
    conn: &dyn MigrationConnection,
) -> BoxFuture<'_, Result<(), crate::DbError>> {
    Box::pin(async move {
        let cron_specs_exists = conn
            .query_i64(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='cron_specs'",
                MigrationParams::Empty,
            )
            .await?;
        if cron_specs_exists == 0 {
            return Ok(());
        }
        for (column, ddl) in [
            (
                "trigger_extra_instruction",
                "ALTER TABLE cron_specs ADD COLUMN trigger_extra_instruction TEXT",
            ),
            (
                "trigger_then_json",
                "ALTER TABLE cron_specs ADD COLUMN trigger_then_json TEXT",
            ),
            (
                "trigger_origin_chat_id",
                "ALTER TABLE cron_specs ADD COLUMN trigger_origin_chat_id INTEGER",
            ),
            (
                "trigger_origin_thread_id",
                "ALTER TABLE cron_specs ADD COLUMN trigger_origin_thread_id INTEGER",
            ),
        ] {
            if !column_exists(conn, "cron_specs", column).await? {
                conn.execute_batch(ddl).await?;
            }
        }
        Ok(())
    })
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
        Migration {
            version: 34,
            sql: V34_SCHEMA,
            hook: Some(v34_turso_fts_indexes),
        },
        Migration {
            version: 35,
            sql: V35_SCHEMA,
            hook: None,
        },
        Migration {
            version: 36,
            sql: V36_SCHEMA,
            hook: None,
        },
        Migration {
            version: 37,
            sql: "",
            hook: Some(v37_drop_legacy_usage_sources),
        },
        Migration {
            version: 38,
            sql: V38_SCHEMA,
            hook: None,
        },
        Migration {
            version: 39,
            sql: V39_SCHEMA,
            hook: None,
        },
        Migration {
            version: 40,
            sql: V40_SCHEMA,
            hook: None,
        },
        Migration {
            version: 41,
            sql: "",
            hook: Some(v41_cron_force_notify),
        },
        Migration {
            version: 42,
            sql: "",
            hook: Some(v42_cron_model),
        },
        Migration {
            version: 43,
            sql: V43_SCHEMA,
            hook: None,
        },
        Migration {
            version: 44,
            sql: V44_SCHEMA,
            hook: None,
        },
        Migration {
            version: 45,
            sql: "",
            hook: Some(v45_cron_trigger_transient),
        },
        Migration {
            version: 46,
            sql: V46_NOTICE_TOKEN,
            hook: None,
        },
        Migration {
            version: 47,
            sql: V47_CRON_SKILL_LINKS,
            hook: None,
        },
        Migration {
            version: 48,
            sql: V48_CURATOR_RUNS,
            hook: None,
        },
    ],
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Connection;

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

    #[tokio::test]
    async fn v42_adds_cron_specs_model_column() {
        let conn = Connection::open_in_memory().await.unwrap();
        MIGRATIONS.to_latest(&conn).await.unwrap();
        conn.execute_batch(
            "INSERT INTO cron_specs (job_name, schedule, prompt, max_budget_usd, recurring, created_at, updated_at) \
             VALUES ('j-null', '17 9 * * *', 'p', 5.0, 1, '2026-06-03T00:00:00Z', '2026-06-03T00:00:00Z'); \
             INSERT INTO cron_specs (job_name, schedule, prompt, max_budget_usd, recurring, model, created_at, updated_at) \
             VALUES ('j-set', '17 9 * * *', 'p', 5.0, 1, 'sonnet', '2026-06-03T00:00:00Z', '2026-06-03T00:00:00Z');",
        )
        .await
        .unwrap();
        let got: Option<String> = conn
            .query_row(
                "SELECT model FROM cron_specs WHERE job_name = 'j-set'",
                (),
                |row| row.get(0),
            )
            .await
            .unwrap();
        assert_eq!(got.as_deref(), Some("sonnet"));
        let null_model: Option<String> = conn
            .query_row(
                "SELECT model FROM cron_specs WHERE job_name = 'j-null'",
                (),
                |row| row.get(0),
            )
            .await
            .unwrap();
        assert_eq!(null_model, None);
    }

    #[tokio::test]
    async fn v45_adds_cron_trigger_transient_columns() {
        let conn = Connection::open_in_memory().await.unwrap();
        MIGRATIONS.to_latest(&conn).await.unwrap();

        for column in [
            "trigger_extra_instruction",
            "trigger_then_json",
            "trigger_origin_chat_id",
            "trigger_origin_thread_id",
        ] {
            let present: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM pragma_table_info('cron_specs') WHERE name = ?",
                    [column],
                    |row| row.get(0),
                )
                .await
                .unwrap();
            assert_eq!(present, 1, "column {column} should exist on cron_specs");
        }
    }

    #[tokio::test]
    async fn v41_adds_force_notify_columns() {
        let conn = Connection::open_in_memory().await.unwrap();
        MIGRATIONS.to_latest(&conn).await.unwrap();

        let spec_col: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('cron_specs') WHERE name = 'trigger_force_notify'",
                [],
                |row| row.get(0),
            )
            .await
            .unwrap();
        assert_eq!(spec_col, 1, "cron_specs.trigger_force_notify must exist");

        let run_col: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('async_runs') WHERE name = 'force_notify'",
                [],
                |row| row.get(0),
            )
            .await
            .unwrap();
        assert_eq!(run_col, 1, "async_runs.force_notify must exist");

        // Idempotent: re-running to_latest is a no-op, not an error.
        MIGRATIONS.to_latest(&conn).await.unwrap();
    }

    #[tokio::test]
    async fn migration_runner_semantics_latest_rejects_future_user_version() {
        let conn = Connection::open_in_memory().await.unwrap();
        conn.execute_batch(&format!(
            "PRAGMA user_version = {}",
            LATEST_SCHEMA_VERSION + 1
        ))
        .await
        .unwrap();

        let err = MIGRATIONS
            .to_latest(&conn)
            .await
            .expect_err("future user_version must be rejected");

        assert!(err.to_string().contains("newer than known migrations"));
    }

    #[tokio::test]
    async fn migration_runner_semantics_rolls_back_all_pending_migrations_on_later_failure() {
        let conn = Connection::open_in_memory().await.unwrap();

        let err = FAILING_MIGRATIONS
            .to_latest(&conn)
            .await
            .expect_err("second migration should fail");

        assert!(
            err.to_string().contains("migration 2"),
            "expected migration 2 context, got {err:?}",
        );
        let user_version: i64 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .await
            .unwrap();
        assert_eq!(user_version, 0);
        let table_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='synthetic_probe'",
                [],
                |row| row.get(0),
            )
            .await
            .unwrap();
        assert_eq!(table_count, 0);
    }

    #[tokio::test]
    async fn cold_boot_concurrent_migrators_do_not_double_apply_v23() {
        // Two cold-boot callers (bot + aggregator) racing to migrate the same
        // per-agent data.db. Before the in-tx user_version recheck, the loser
        // of the BEGIN IMMEDIATE race would re-run v23, whose hook does
        // `INSERT INTO async_runs ... FROM cron_runs` then `DROP TABLE
        // cron_runs`. On the second run cron_runs is already gone; the hook
        // crashes with "no such table: cron_runs", the tx rolls back, and
        // open_connection(_, true) returns Err. Process-compose then restarts
        // both processes and the agent never starts.
        let dir = tempfile::tempdir().unwrap();
        let agent_dir = dir.path().to_path_buf();

        // Both connections target the same data.db file. open_connection
        // joins "data.db" onto the path internally.
        let conn1 = crate::open_connection(&agent_dir, false).await.unwrap();
        let conn2 = crate::open_connection(&agent_dir, false).await.unwrap();

        // Run both migration futures concurrently. The async API exposes the
        // same contention directly without a sync runtime bridge.
        let (r1, r2) = tokio::join!(MIGRATIONS.to_latest(&conn1), MIGRATIONS.to_latest(&conn2));

        r1.expect("first migrator must succeed");
        r2.expect("second migrator must succeed (no double-apply of v23)");

        // Both connections see the same final user_version, equal to LATEST.
        let v1: i64 = conn1
            .query_one("PRAGMA user_version", (), |row| row.get(0))
            .await
            .unwrap();
        let v2: i64 = conn2
            .query_one("PRAGMA user_version", (), |row| row.get(0))
            .await
            .unwrap();
        assert_eq!(v1, v2, "both migrators must converge on the same version");
        assert_eq!(
            v1,
            i64::from(LATEST_SCHEMA_VERSION),
            "final user_version must equal LATEST_SCHEMA_VERSION",
        );

        // Schema sanity: async_runs (created by v23) exists; cron_runs
        // (dropped by v23) does not.
        let async_runs_exists: i64 = conn1
            .query_one(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='async_runs'",
                (),
                |r| r.get(0),
            )
            .await
            .unwrap();
        assert_eq!(async_runs_exists, 1, "async_runs must exist post-v23");
        let cron_runs_exists: i64 = conn1
            .query_one(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='cron_runs'",
                (),
                |r| r.get(0),
            )
            .await
            .unwrap();
        assert_eq!(cron_runs_exists, 0, "cron_runs must be dropped post-v23");
    }

    #[tokio::test]
    async fn migration_runner_semantics_local_db_rolls_back_all_pending_migrations_on_later_failure()
     {
        let dir = tempfile::tempdir().unwrap();
        let conn = crate::open_connection(dir.path(), false).await.unwrap();

        let err = FAILING_MIGRATIONS
            .to_latest(&conn)
            .await
            .expect_err("second migration should fail");

        assert!(
            err.to_string().contains("migration 2"),
            "expected migration 2 context, got {err:?}",
        );
        let user_version: i64 = conn
            .query_one("PRAGMA user_version", (), |row| row.get(0))
            .await
            .unwrap();
        assert_eq!(user_version, 0);
        let table_count: i64 = conn
            .query_one(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='synthetic_probe'",
                (),
                |row| row.get(0),
            )
            .await
            .unwrap();
        assert_eq!(table_count, 0);
    }

    #[tokio::test]
    async fn v34_drops_legacy_fts_triggers_and_creates_turso_indexes() {
        let mut conn = Connection::open_in_memory().await.unwrap();
        MIGRATIONS.to_version(&mut conn, 33).await.unwrap();

        conn.execute_batch(
            "DROP INDEX IF EXISTS idx_memories_turso_fts;
             DROP INDEX IF EXISTS idx_conversation_messages_turso_fts;

             CREATE TABLE IF NOT EXISTS memories_fts (
                 rowid INTEGER PRIMARY KEY,
                 content TEXT NOT NULL
             );
             CREATE TRIGGER IF NOT EXISTS memories_ai
             AFTER INSERT ON memories
             BEGIN
                 INSERT INTO memories_fts(rowid, content)
                 VALUES (new.id, new.content);
             END;
             CREATE TRIGGER IF NOT EXISTS memories_ad
             AFTER DELETE ON memories
             BEGIN
                 DELETE FROM memories_fts WHERE rowid = old.id;
             END;
             CREATE TRIGGER IF NOT EXISTS memories_au
             AFTER UPDATE OF content ON memories
             BEGIN
                 UPDATE memories_fts SET content = new.content WHERE rowid = old.id;
             END;

             CREATE TABLE IF NOT EXISTS conversation_messages_fts (
                 rowid INTEGER PRIMARY KEY,
                 content TEXT NOT NULL
             );
             CREATE TRIGGER IF NOT EXISTS conversation_messages_ai
             AFTER INSERT ON conversation_messages
             BEGIN
                 INSERT INTO conversation_messages_fts(rowid, content)
                 VALUES (new.id, new.content);
             END;
             CREATE TRIGGER IF NOT EXISTS conversation_messages_ad
             AFTER DELETE ON conversation_messages
             BEGIN
                 DELETE FROM conversation_messages_fts WHERE rowid = old.id;
             END;
             CREATE TRIGGER IF NOT EXISTS conversation_messages_au
             AFTER UPDATE OF content ON conversation_messages
             BEGIN
                 UPDATE conversation_messages_fts
                 SET content = new.content
                 WHERE rowid = old.id;
             END;",
        )
        .await
        .unwrap();

        for trigger_name in [
            "memories_ai",
            "memories_ad",
            "memories_au",
            "conversation_messages_ai",
            "conversation_messages_ad",
            "conversation_messages_au",
        ] {
            let trigger_count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='trigger' AND name=?1",
                    [trigger_name],
                    |row| row.get(0),
                )
                .await
                .unwrap();
            assert_eq!(trigger_count, 1, "{trigger_name} fixture trigger missing");
        }

        conn.execute(
            "INSERT INTO memories (content) VALUES ('legacy memory needle')",
            (),
        )
        .await
        .unwrap();
        conn.execute(
            "INSERT INTO conversation_messages (chat_id, thread_id, role, content)
             VALUES (1, 0, 'user', 'legacy conversation needle')",
            (),
        )
        .await
        .unwrap();

        MIGRATIONS.to_latest(&mut conn).await.unwrap();

        for trigger_name in [
            "memories_ai",
            "memories_ad",
            "memories_au",
            "conversation_messages_ai",
            "conversation_messages_ad",
            "conversation_messages_au",
        ] {
            let trigger_count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='trigger' AND name=?1",
                    [trigger_name],
                    |row| row.get(0),
                )
                .await
                .unwrap();
            assert_eq!(trigger_count, 0, "{trigger_name} trigger must be removed");
        }

        for table_name in ["memories_fts", "conversation_messages_fts"] {
            let table_count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
                    [table_name],
                    |row| row.get(0),
                )
                .await
                .unwrap();
            assert_eq!(table_count, 0, "{table_name} table must be removed");
        }

        for index_name in [
            "idx_memories_turso_fts",
            "idx_conversation_messages_turso_fts",
        ] {
            let index_count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name=?1",
                    [index_name],
                    |row| row.get(0),
                )
                .await
                .unwrap();
            assert_eq!(index_count, 1, "{index_name} index must exist");
        }

        let memory_match_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM memories WHERE content MATCH 'memory'",
                (),
                |row| row.get(0),
            )
            .await
            .unwrap();
        assert_eq!(memory_match_count, 1);

        let conversation_match_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM conversation_messages WHERE content MATCH 'conversation'",
                (),
                |row| row.get(0),
            )
            .await
            .unwrap();
        assert_eq!(conversation_match_count, 1);
    }

    #[tokio::test]
    async fn migration_runner_semantics_rejects_target_after_highest_known_version() {
        let conn = Connection::open_in_memory().await.unwrap();

        let err = FAILING_MIGRATIONS
            .to_version(&conn, 3)
            .await
            .expect_err("target past registry end must be rejected");

        assert!(err.to_string().contains("unknown migration target"));
    }

    #[tokio::test]
    async fn migration_runner_semantics_rejects_down_migrations() {
        let conn = Connection::open_in_memory().await.unwrap();
        conn.execute_batch("PRAGMA user_version = 2").await.unwrap();

        let err = FAILING_MIGRATIONS
            .to_version(&conn, 1)
            .await
            .expect_err("down migrations must be rejected");

        assert!(err.to_string().contains("down migrations are unsupported"));
    }

    #[tokio::test]
    async fn migration_runner_semantics_allows_current_version_noop() {
        let conn = Connection::open_in_memory().await.unwrap();
        conn.execute_batch("PRAGMA user_version = 2").await.unwrap();

        FAILING_MIGRATIONS.to_version(&conn, 2).await.unwrap();

        let user_version: i64 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .await
            .unwrap();
        assert_eq!(user_version, 2);
    }

    #[tokio::test]
    async fn migration_runner_semantics_latest_schema_version_matches_highest_migration() {
        assert_eq!(
            MIGRATIONS
                .migrations
                .last()
                .expect("migration registry should not be empty")
                .version,
            LATEST_SCHEMA_VERSION,
        );
    }

    #[tokio::test]
    async fn migrations_apply_cleanly_to_v4() {
        let mut conn = Connection::open_in_memory().await.unwrap();
        MIGRATIONS.to_latest(&mut conn).await.unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('sessions') WHERE name IN ('id','chat_id','thread_id','root_session_id','label','is_active','created_at','last_used_at')",
                [],
                |r| r.get(0),
            )
            .await
            .unwrap();
        assert_eq!(count, 8, "sessions table should have all 8 columns");
        let old_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='telegram_sessions'",
                [],
                |r| r.get(0),
            )
            .await
            .unwrap();
        assert_eq!(old_count, 0, "telegram_sessions should be dropped");
    }

    #[tokio::test]
    async fn sessions_partial_unique_index_enforces_single_active() {
        let mut conn = Connection::open_in_memory().await.unwrap();
        MIGRATIONS.to_latest(&mut conn).await.unwrap();
        conn.execute(
            "INSERT INTO sessions (chat_id, thread_id, root_session_id, is_active) VALUES (1, 0, 'aaa', 1)",
            [],
        )
        .await
        .unwrap();
        let result = conn.execute(
            "INSERT INTO sessions (chat_id, thread_id, root_session_id, is_active) VALUES (1, 0, 'bbb', 1)",
            [],
        )
        .await;
        assert!(
            result.is_err(),
            "partial unique index should prevent two active sessions"
        );
    }

    #[tokio::test]
    async fn v46_creates_notice_token_table() {
        let mut conn = Connection::open_in_memory().await.unwrap();
        MIGRATIONS.to_version(&mut conn, 46).await.unwrap();
        conn.execute("INSERT INTO notice_token (token) VALUES ('abc')", [])
            .await
            .unwrap();
        let t: String = conn
            .query_one("SELECT token FROM notice_token LIMIT 1", (), |r| r.get(0))
            .await
            .unwrap();
        assert_eq!(t, "abc");
    }

    #[tokio::test]
    async fn v47_creates_cron_skill_links_table() {
        let mut conn = Connection::open_in_memory().await.unwrap();
        MIGRATIONS.to_latest(&mut conn).await.unwrap();
        conn.execute(
            "INSERT INTO cron_skill_links (job_name, skill_name, origin, created_at) \
             VALUES ('j', 'rightx-a', 'auto', '2026-06-15T00:00:00Z')",
            [],
        )
        .await
        .unwrap();
        let rows = conn
            .execute(
                "INSERT OR IGNORE INTO cron_skill_links (job_name, skill_name, origin, created_at) \
                 VALUES ('j', 'rightx-a', 'agent', '2026-06-15T00:00:01Z')",
                [],
            )
            .await
            .unwrap();
        assert_eq!(rows, 0, "duplicate PK must not insert");
        let bad = conn
            .execute(
                "INSERT INTO cron_skill_links (job_name, skill_name, origin, created_at) \
                 VALUES ('j', 'rightx-b', 'bogus', '2026-06-15T00:00:02Z')",
                [],
            )
            .await;
        assert!(bad.is_err(), "origin CHECK must reject 'bogus'");
    }

    #[tokio::test]
    async fn sessions_allows_multiple_inactive() {
        let mut conn = Connection::open_in_memory().await.unwrap();
        MIGRATIONS.to_latest(&mut conn).await.unwrap();
        conn.execute(
            "INSERT INTO sessions (chat_id, thread_id, root_session_id, is_active) VALUES (1, 0, 'aaa', 0)",
            [],
        )
        .await
        .unwrap();
        conn.execute(
            "INSERT INTO sessions (chat_id, thread_id, root_session_id, is_active) VALUES (1, 0, 'bbb', 0)",
            [],
        )
        .await
        .unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM sessions WHERE chat_id=1", [], |r| {
                r.get(0)
            })
            .await
            .unwrap();
        assert_eq!(count, 2);
    }

    #[tokio::test]
    async fn async_runs_has_delivery_decision_columns() {
        let mut conn = Connection::open_in_memory().await.unwrap();
        MIGRATIONS.to_latest(&mut conn).await.unwrap();
        let cols: Vec<String> = conn
            .prepare("SELECT name FROM pragma_table_info('async_runs')")
            .unwrap()
            .query_map([], |r| r.get(0))
            .await
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

    #[tokio::test]
    async fn v25_loses_old_pending_delivery_payloads() {
        let mut conn = Connection::open_in_memory().await.unwrap();
        MIGRATIONS.to_version(&mut conn, 24).await.unwrap();
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
        .await
        .unwrap();

        MIGRATIONS.to_latest(&mut conn).await.unwrap();

        let row: (Option<String>, Option<String>, i64, String) = conn
            .query_row(
                "SELECT run_note, delivery_json, delivery_required, delivery_status
             FROM async_runs WHERE id = 'run-1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .await
            .unwrap();
        assert_eq!(row, (Some("old summary".into()), None, 0, "none".into()));
    }

    #[tokio::test]
    async fn learning_episode_tables_exist() {
        let mut conn = Connection::open_in_memory().await.unwrap();
        MIGRATIONS.to_version(&mut conn, 24).await.unwrap();
        let events: String = conn
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type='table' AND name='execution_events'",
                [],
                |r| r.get(0),
            )
            .await
            .unwrap();
        assert!(events.contains("event_kind"));
        assert!(events.contains("trust_label"));
        let episodes: String = conn
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type='table' AND name='learning_episodes'",
                [],
                |r| r.get(0),
            )
            .await
            .unwrap();
        assert!(episodes.contains("ready_after"));
        assert!(episodes.contains("episode_hash"));
    }

    #[tokio::test]
    async fn legacy_learning_cleanup_drops_deprecated_tables() {
        let legacy_tables = [
            "learning_episodes",
            "skill_nudge_signals",
            "skill_nudge_state",
            "skill_review_reports",
            "execution_events",
        ];

        let mut v34_conn = Connection::open_in_memory().await.unwrap();
        MIGRATIONS.to_version(&mut v34_conn, 34).await.unwrap();

        for table in legacy_tables {
            let exists: i64 = v34_conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
                    [table],
                    |r| r.get(0),
                )
                .await
                .unwrap();
            assert_eq!(exists, 1, "{table} must exist through v34");
        }

        let mut latest_conn = Connection::open_in_memory().await.unwrap();
        MIGRATIONS.to_latest(&mut latest_conn).await.unwrap();

        for table in legacy_tables {
            let exists: i64 = latest_conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
                    [table],
                    |r| r.get(0),
                )
                .await
                .unwrap();
            assert_eq!(exists, 0, "{table} must be dropped by latest migration");
        }
    }

    #[tokio::test]
    async fn execution_events_do_not_create_fts() {
        let mut conn = Connection::open_in_memory().await.unwrap();
        MIGRATIONS.to_latest(&mut conn).await.unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE name LIKE 'execution_events%fts%'",
                [],
                |r| r.get(0),
            )
            .await
            .unwrap();
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn skill_review_reports_links_learning_episode() {
        let mut conn = Connection::open_in_memory().await.unwrap();
        MIGRATIONS.to_version(&mut conn, 24).await.unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('skill_review_reports') WHERE name='learning_episode_id'",
                [],
                |r| r.get(0),
            )
            .await
            .unwrap();
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn migrations_apply_cleanly_to_v7() {
        let mut conn = Connection::open_in_memory().await.unwrap();
        MIGRATIONS.to_latest(&mut conn).await.unwrap();
        let cols: Vec<String> = conn
            .prepare("SELECT name FROM pragma_table_info('cron_specs')")
            .unwrap()
            .query_map([], |r| r.get(0))
            .await
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        assert!(
            cols.contains(&"triggered_at".to_string()),
            "triggered_at column missing"
        );
    }

    #[tokio::test]
    async fn v8_mcp_servers_table() {
        let mut conn = Connection::open_in_memory().await.unwrap();
        MIGRATIONS.to_latest(&mut conn).await.unwrap();

        conn.execute(
            "INSERT INTO mcp_servers (name, url) VALUES (?1, ?2)",
            ("notion", "https://mcp.notion.com/mcp"),
        )
        .await
        .unwrap();

        let url: String = conn
            .query_row(
                "SELECT url FROM mcp_servers WHERE name = ?1",
                ["notion"],
                |row| row.get(0),
            )
            .await
            .unwrap();
        assert_eq!(url, "https://mcp.notion.com/mcp");

        // Test upsert
        conn.execute(
            "INSERT OR REPLACE INTO mcp_servers (name, url) VALUES (?1, ?2)",
            ("notion", "https://new-url.com/mcp"),
        )
        .await
        .unwrap();
        let url: String = conn
            .query_row(
                "SELECT url FROM mcp_servers WHERE name = ?1",
                ["notion"],
                |row| row.get(0),
            )
            .await
            .unwrap();
        assert_eq!(url, "https://new-url.com/mcp");
    }

    #[tokio::test]
    async fn v9_mcp_servers_has_instructions_column() {
        let mut conn = Connection::open_in_memory().await.unwrap();
        MIGRATIONS.to_latest(&mut conn).await.unwrap();

        conn.execute(
            "INSERT INTO mcp_servers (name, url) VALUES (?1, ?2)",
            ("test-server", "https://example.com/mcp"),
        )
        .await
        .unwrap();

        let instructions: Option<String> = conn
            .query_row(
                "SELECT instructions FROM mcp_servers WHERE name = ?1",
                ["test-server"],
                |row| row.get(0),
            )
            .await
            .unwrap();
        assert!(
            instructions.is_none(),
            "instructions should be NULL by default"
        );
    }

    #[tokio::test]
    async fn v10_mcp_servers_has_auth_columns() {
        let mut conn = Connection::open_in_memory().await.unwrap();
        MIGRATIONS.to_latest(&mut conn).await.unwrap();

        conn.execute(
            "INSERT INTO mcp_servers (name, url, auth_type, auth_token) VALUES (?1, ?2, ?3, ?4)",
            ("test", "https://example.com/mcp", "bearer", "sk-123"),
        )
        .await
        .unwrap();

        let auth_type: Option<String> = conn
            .query_row(
                "SELECT auth_type FROM mcp_servers WHERE name = ?1",
                ["test"],
                |row| row.get(0),
            )
            .await
            .unwrap();
        assert_eq!(auth_type.as_deref(), Some("bearer"));

        // Verify all new columns exist
        let cols: Vec<String> = conn
            .prepare("SELECT name FROM pragma_table_info('mcp_servers')")
            .unwrap()
            .query_map([], |r| r.get(0))
            .await
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

    #[tokio::test]
    async fn v33_mcp_servers_has_oauth_resource_column() {
        let mut conn = Connection::open_in_memory().await.unwrap();
        MIGRATIONS.to_latest(&mut conn).await.unwrap();

        let cols: Vec<String> = conn
            .prepare("SELECT name FROM pragma_table_info('mcp_servers')")
            .unwrap()
            .query_map([], |r| r.get(0))
            .await
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();

        assert!(
            cols.contains(&"oauth_resource".to_string()),
            "oauth_resource column missing"
        );
    }

    #[tokio::test]
    async fn v36_mcp_http_headers_table() {
        let mut conn = Connection::open_in_memory().await.unwrap();
        MIGRATIONS.to_latest(&mut conn).await.unwrap();

        conn.execute(
            "INSERT INTO mcp_servers (name, url) VALUES (?1, ?2)",
            ("nango", "https://api.nango.dev/mcp"),
        )
        .await
        .unwrap();
        conn.execute(
            "INSERT INTO mcp_http_headers (server_name, header_name, header_value)
             VALUES (?1, ?2, ?3)",
            ("nango", "connection-id", "conn_123"),
        )
        .await
        .unwrap();
        let duplicate_case = conn
            .execute(
                "INSERT INTO mcp_http_headers (server_name, header_name, header_value)
             VALUES (?1, ?2, ?3)",
                ("nango", "Connection-ID", "conn_duplicate"),
            )
            .await;
        assert!(
            duplicate_case.is_err(),
            "header_name uniqueness must be case-insensitive"
        );

        let value: String = conn
            .query_one(
                "SELECT header_value FROM mcp_http_headers WHERE server_name = ?1 AND header_name = ?2",
                ("nango", "connection-id"),
                |row| row.get(0),
            )
            .await
            .unwrap();
        assert_eq!(value, "conn_123");

        conn.execute("DELETE FROM mcp_servers WHERE name = ?1", ["nango"])
            .await
            .unwrap();
        let count: i64 = conn
            .query_one("SELECT COUNT(*) FROM mcp_http_headers", (), |row| {
                row.get(0)
            })
            .await
            .unwrap();
        assert_eq!(count, 0, "headers must be deleted with their MCP server");
    }

    #[tokio::test]
    async fn v12_cron_diagnostics_columns() {
        let mut conn = Connection::open_in_memory().await.unwrap();
        MIGRATIONS.to_version(&mut conn, 21).await.unwrap();
        let cols: Vec<String> = conn
            .prepare("SELECT name FROM pragma_table_info('cron_runs')")
            .unwrap()
            .query_map([], |r| r.get(0))
            .await
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

    #[tokio::test]
    async fn v12_backfill_delivery_status() {
        let mut conn = Connection::open_in_memory().await.unwrap();
        MIGRATIONS.to_version(&mut conn, 21).await.unwrap();

        // Insert a delivered run (has notify_json + delivered_at)
        conn.execute(
            "INSERT INTO cron_runs (id, job_name, started_at, status, log_path, notify_json, delivered_at) \
             VALUES ('d1', 'j1', '2026-01-01T00:00:00Z', 'success', '/log', '{\"content\":\"hi\"}', '2026-01-01T00:05:00Z')",
            [],
        )
        .await
        .unwrap();
        // Insert a pending run (has notify_json, no delivered_at)
        conn.execute(
            "INSERT INTO cron_runs (id, job_name, started_at, status, log_path, notify_json) \
             VALUES ('p1', 'j1', '2026-01-01T01:00:00Z', 'success', '/log', '{\"content\":\"pending\"}')",
            [],
        )
        .await
        .unwrap();
        // Insert a silent run (no notify_json)
        conn.execute(
            "INSERT INTO cron_runs (id, job_name, started_at, status, log_path, summary) \
             VALUES ('s1', 'j1', '2026-01-01T02:00:00Z', 'success', '/log', 'quiet')",
            [],
        )
        .await
        .unwrap();

        async fn status_of(conn: &Connection, id: &str) -> Option<String> {
            conn.query_row(
                "SELECT delivery_status FROM cron_runs WHERE id = ?1",
                [id],
                |r| r.get(0),
            )
            .await
            .unwrap()
        }
        assert_eq!(status_of(&conn, "d1").await.as_deref(), Some("delivered"));
        assert_eq!(status_of(&conn, "p1").await.as_deref(), Some("pending"));
        assert_eq!(status_of(&conn, "s1").await.as_deref(), Some("silent"));
    }

    #[tokio::test]
    async fn v12_idempotent_when_columns_already_exist() {
        let mut conn = Connection::open_in_memory().await.unwrap();
        // Apply up to v11 (version index is 1-based in to_version).
        MIGRATIONS.to_version(&mut conn, 11).await.unwrap();

        // Manually add the columns that v12 would create.
        conn.execute_batch("ALTER TABLE cron_runs ADD COLUMN delivery_status TEXT")
            .await
            .unwrap();
        conn.execute_batch("ALTER TABLE cron_runs ADD COLUMN no_notify_reason TEXT")
            .await
            .unwrap();

        // v12 must not fail even though columns already exist.
        MIGRATIONS.to_version(&mut conn, 21).await.unwrap();

        let cols: Vec<String> = conn
            .prepare("SELECT name FROM pragma_table_info('cron_runs')")
            .unwrap()
            .query_map([], |r| r.get(0))
            .await
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        assert!(cols.contains(&"delivery_status".to_string()));
        assert!(cols.contains(&"no_notify_reason".to_string()));
    }

    #[tokio::test]
    async fn migrations_apply_cleanly_to_v6() {
        let mut conn = Connection::open_in_memory().await.unwrap();
        MIGRATIONS.to_latest(&mut conn).await.unwrap();
        let cols: Vec<String> = conn
            .prepare("SELECT name FROM pragma_table_info('cron_specs')")
            .unwrap()
            .query_map([], |r| r.get(0))
            .await
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

    #[tokio::test]
    async fn v13_one_shot_cron_columns() {
        let mut conn = Connection::open_in_memory().await.unwrap();
        MIGRATIONS.to_latest(&mut conn).await.unwrap();
        let cols: Vec<String> = conn
            .prepare("SELECT name FROM pragma_table_info('cron_specs')")
            .unwrap()
            .query_map([], |r| r.get(0))
            .await
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

    #[tokio::test]
    async fn v13_idempotent_when_columns_already_exist() {
        let mut conn = Connection::open_in_memory().await.unwrap();
        MIGRATIONS.to_version(&mut conn, 12).await.unwrap();
        conn.execute_batch(
            "ALTER TABLE cron_specs ADD COLUMN recurring INTEGER NOT NULL DEFAULT 1",
        )
        .await
        .unwrap();
        conn.execute_batch("ALTER TABLE cron_specs ADD COLUMN run_at TEXT")
            .await
            .unwrap();
        MIGRATIONS.to_latest(&mut conn).await.unwrap();
        let cols: Vec<String> = conn
            .prepare("SELECT name FROM pragma_table_info('cron_specs')")
            .unwrap()
            .query_map([], |r| r.get(0))
            .await
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        assert!(cols.contains(&"recurring".to_string()));
        assert!(cols.contains(&"run_at".to_string()));
    }

    #[tokio::test]
    async fn v13_existing_specs_get_recurring_true() {
        let mut conn = Connection::open_in_memory().await.unwrap();
        MIGRATIONS.to_version(&mut conn, 12).await.unwrap();
        conn.execute(
            "INSERT INTO cron_specs (job_name, schedule, prompt, max_budget_usd, created_at, updated_at) \
             VALUES ('old-job', '*/5 * * * *', 'do stuff', 1.0, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
            [],
        )
        .await
        .unwrap();
        MIGRATIONS.to_latest(&mut conn).await.unwrap();
        let recurring: i64 = conn
            .query_row(
                "SELECT recurring FROM cron_specs WHERE job_name = 'old-job'",
                [],
                |r| r.get(0),
            )
            .await
            .unwrap();
        assert_eq!(recurring, 1, "existing specs must default to recurring=1");
        let run_at: Option<String> = conn
            .query_row(
                "SELECT run_at FROM cron_specs WHERE job_name = 'old-job'",
                [],
                |r| r.get(0),
            )
            .await
            .unwrap();
        assert!(run_at.is_none(), "existing specs must have run_at=NULL");
    }

    #[tokio::test]
    async fn v14_pending_retains_table_exists() {
        let mut conn = Connection::open_in_memory().await.unwrap();
        MIGRATIONS.to_latest(&mut conn).await.unwrap();
        let cols: Vec<String> = conn
            .prepare("SELECT name FROM pragma_table_info('pending_retains')")
            .unwrap()
            .query_map([], |r| r.get(0))
            .await
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

    #[tokio::test]
    async fn v14_memory_alerts_table_exists() {
        let mut conn = Connection::open_in_memory().await.unwrap();
        MIGRATIONS.to_latest(&mut conn).await.unwrap();
        let cols: Vec<String> = conn
            .prepare("SELECT name FROM pragma_table_info('memory_alerts')")
            .unwrap()
            .query_map([], |r| r.get(0))
            .await
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        for col in ["alert_type", "first_sent_at"] {
            assert!(cols.contains(&col.to_string()), "{col} column missing");
        }
    }

    #[tokio::test]
    async fn v14_pending_retains_created_index_exists() {
        let mut conn = Connection::open_in_memory().await.unwrap();
        MIGRATIONS.to_latest(&mut conn).await.unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='index' \
                 AND name='idx_pending_retains_created'",
                [],
                |r| r.get(0),
            )
            .await
            .unwrap();
        assert_eq!(count, 1, "idx_pending_retains_created should exist");
    }

    #[tokio::test]
    async fn v14_idempotent_when_tables_already_exist() {
        let mut conn = Connection::open_in_memory().await.unwrap();
        // Apply up to v13 (version index is 1-based in to_version).
        MIGRATIONS.to_version(&mut conn, 13).await.unwrap();

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
        .await
        .unwrap();

        // v14 must not fail even though tables already exist.
        MIGRATIONS.to_latest(&mut conn).await.unwrap();

        let pending_cols: Vec<String> = conn
            .prepare("SELECT name FROM pragma_table_info('pending_retains')")
            .unwrap()
            .query_map([], |r| r.get(0))
            .await
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
            .await
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
            .await
            .unwrap();
        assert_eq!(
            idx_count, 1,
            "idx_pending_retains_created should exist after idempotent v14"
        );
    }

    #[tokio::test]
    async fn v15_creates_usage_events_table_with_indexes() {
        let mut conn = Connection::open_in_memory().await.unwrap();
        MIGRATIONS.to_latest(&mut conn).await.unwrap();

        // Table exists and is writable.
        conn.execute_batch(
            "INSERT INTO usage_events (
                ts, source, session_uuid, total_cost_usd, num_turns,
                model_usage_json
             ) VALUES (
                '2026-04-20T00:00:00Z', 'interactive', 'test-uuid', 0.05, 3, '{}'
             );",
        )
        .await
        .unwrap();

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM usage_events", [], |r| r.get(0))
            .await
            .unwrap();
        assert_eq!(count, 1);

        // Indexes present.
        let indexes: Vec<String> = conn
            .prepare(
                "SELECT name FROM sqlite_master WHERE type='index' AND tbl_name='usage_events'",
            )
            .unwrap()
            .query_map([], |r| r.get::<_, String>(0))
            .await
            .unwrap()
            .filter_map(Result::ok)
            .collect();
        assert!(indexes.iter().any(|n| n == "idx_usage_events_ts"));
        assert!(indexes.iter().any(|n| n == "idx_usage_events_source_ts"));
    }

    #[tokio::test]
    async fn v15_migration_is_idempotent() {
        let mut conn = Connection::open_in_memory().await.unwrap();
        MIGRATIONS.to_latest(&mut conn).await.unwrap();
        // Second call must be a no-op.
        MIGRATIONS.to_latest(&mut conn).await.unwrap();
    }

    #[tokio::test]
    async fn v16_usage_events_has_api_key_source() {
        let mut conn = Connection::open_in_memory().await.unwrap();
        MIGRATIONS.to_latest(&mut conn).await.unwrap();
        let cols: Vec<String> = conn
            .prepare("SELECT name FROM pragma_table_info('usage_events')")
            .unwrap()
            .query_map([], |r| r.get(0))
            .await
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        assert!(
            cols.contains(&"api_key_source".to_string()),
            "api_key_source column missing"
        );
    }

    #[tokio::test]
    async fn v16_backfills_existing_rows_to_none() {
        let mut conn = Connection::open_in_memory().await.unwrap();
        // Apply up to v15, insert a row without api_key_source.
        MIGRATIONS.to_version(&mut conn, 15).await.unwrap();
        conn.execute(
            "INSERT INTO usage_events (
                ts, source, session_uuid, total_cost_usd, num_turns, model_usage_json
             ) VALUES ('2026-04-20T00:00:00Z','interactive','s',0.0,1,'{}')",
            [],
        )
        .await
        .unwrap();
        // Apply v16.
        MIGRATIONS.to_latest(&mut conn).await.unwrap();
        let src: String = conn
            .query_row("SELECT api_key_source FROM usage_events LIMIT 1", [], |r| {
                r.get(0)
            })
            .await
            .unwrap();
        assert_eq!(src, "none");
    }

    #[tokio::test]
    async fn v16_idempotent_when_column_already_exists() {
        let mut conn = Connection::open_in_memory().await.unwrap();
        MIGRATIONS.to_version(&mut conn, 15).await.unwrap();
        conn.execute_batch(
            "ALTER TABLE usage_events ADD COLUMN api_key_source TEXT NOT NULL DEFAULT 'none'",
        )
        .await
        .unwrap();
        // v16 must succeed even though the column already exists.
        MIGRATIONS.to_latest(&mut conn).await.unwrap();
        let cols: Vec<String> = conn
            .prepare("SELECT name FROM pragma_table_info('usage_events')")
            .unwrap()
            .query_map([], |r| r.get(0))
            .await
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        assert!(cols.contains(&"api_key_source".to_string()));
    }

    #[tokio::test]
    async fn v17_adds_cron_target_columns() {
        let mut conn = Connection::open_in_memory().await.unwrap();
        MIGRATIONS.to_latest(&mut conn).await.unwrap();
        let chat_present: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('cron_specs') WHERE name = 'target_chat_id'",
                [],
                |r| r.get(0),
            )
            .await
            .unwrap();
        assert_eq!(chat_present, 1, "target_chat_id column missing");
        let thread_present: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('cron_specs') WHERE name = 'target_thread_id'",
                [],
                |r| r.get(0),
            )
            .await
            .unwrap();
        assert_eq!(thread_present, 1, "target_thread_id column missing");
    }

    #[tokio::test]
    async fn v17_is_idempotent_on_rerun() {
        let mut conn = Connection::open_in_memory().await.unwrap();
        MIGRATIONS.to_latest(&mut conn).await.unwrap();
        // Manually re-run the v17 hook; it must not error.
        let tx = conn.transaction().await.unwrap();
        super::v17_cron_target(&tx).await.unwrap();
        tx.commit().await.unwrap();
    }

    #[tokio::test]
    async fn v17_existing_rows_get_null_target() {
        let mut conn = Connection::open_in_memory().await.unwrap();
        // Stop one version before v17 so the legacy row is inserted into a table
        // without target_chat_id / target_thread_id columns.
        MIGRATIONS.to_version(&mut conn, 16).await.unwrap();
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO cron_specs (job_name, schedule, prompt, max_budget_usd, created_at, updated_at) \
             VALUES ('legacy', '*/5 * * * *', 'p', 1.0, ?1, ?1)",
            [&now],
        )
        .await
        .unwrap();
        // Apply v17 — this is what we're actually testing.
        MIGRATIONS.to_latest(&mut conn).await.unwrap();
        let target: Option<i64> = conn
            .query_row(
                "SELECT target_chat_id FROM cron_specs WHERE job_name = 'legacy'",
                [],
                |r| r.get(0),
            )
            .await
            .unwrap();
        assert!(
            target.is_none(),
            "legacy row should have NULL target_chat_id"
        );
    }

    #[tokio::test]
    async fn v18_cron_runs_has_target_columns() {
        let mut conn = Connection::open_in_memory().await.unwrap();
        MIGRATIONS.to_version(&mut conn, 21).await.unwrap();
        let cols: Vec<String> = conn
            .prepare("SELECT name FROM pragma_table_info('cron_runs')")
            .unwrap()
            .query_map([], |r| r.get(0))
            .await
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

    #[tokio::test]
    async fn v18_is_idempotent() {
        // Apply once.
        let mut conn = Connection::open_in_memory().await.unwrap();
        MIGRATIONS.to_latest(&mut conn).await.unwrap();
        // Apply again — must not error (re-running should be a no-op).
        MIGRATIONS.to_latest(&mut conn).await.unwrap();
    }

    #[tokio::test]
    async fn v18_backfills_target_from_cron_specs_for_pending_undelivered_runs() {
        // Stop one version before v18 so the legacy run is inserted into a
        // cron_runs table that does not yet have target_* columns.
        let mut conn = Connection::open_in_memory().await.unwrap();
        MIGRATIONS.to_version(&mut conn, 17).await.unwrap();
        let now = chrono::Utc::now().to_rfc3339();
        // Spec with target_chat_id set, target_thread_id NULL.
        conn.execute(
            "INSERT INTO cron_specs (job_name, schedule, prompt, max_budget_usd, created_at, updated_at, target_chat_id) \
             VALUES ('legacy-recurring', '*/5 * * * *', 'p', 1.0, ?1, ?1, 12345)",
            [&now],
        )
        .await
        .unwrap();
        // Pre-v18 cron_runs row: status=success, notify_json present,
        // delivered_at NULL — i.e. an undelivered run waiting for the
        // delivery loop to pick it up.
        conn.execute(
            "INSERT INTO cron_runs (id, job_name, started_at, finished_at, exit_code, status, log_path, summary, notify_json, delivered_at) \
             VALUES ('run-1', 'legacy-recurring', ?1, ?1, 0, 'success', '/tmp/x.ndjson', 's', '{\"text\":\"hi\"}', NULL)",
            [&now],
        )
        .await
        .unwrap();
        // Apply through v21 so cron_runs still exists while checking v18 behavior.
        MIGRATIONS.to_version(&mut conn, 21).await.unwrap();
        let chat: Option<i64> = conn
            .query_row(
                "SELECT target_chat_id FROM cron_runs WHERE id = 'run-1'",
                [],
                |r| r.get(0),
            )
            .await
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
            .await
            .unwrap();
        assert!(
            thread.is_none(),
            "target_thread_id should remain NULL when spec's thread is NULL"
        );
    }

    #[tokio::test]
    async fn v23_creates_async_runs_and_drops_cron_runs() {
        let mut conn = Connection::open_in_memory().await.unwrap();
        MIGRATIONS.to_latest(&mut conn).await.unwrap();

        let async_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='async_runs'",
                [],
                |r| r.get(0),
            )
            .await
            .unwrap();
        assert_eq!(async_count, 1, "async_runs table should exist");

        let cron_runs_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='cron_runs'",
                [],
                |r| r.get(0),
            )
            .await
            .unwrap();
        assert_eq!(cron_runs_count, 0, "cron_runs must be dropped after v22");
    }

    #[tokio::test]
    #[allow(clippy::type_complexity)]
    async fn v23_migrates_cron_runs_to_async_runs() {
        let mut conn = Connection::open_in_memory().await.unwrap();
        MIGRATIONS.to_version(&mut conn, 21).await.unwrap();

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
        .await
        .unwrap();

        MIGRATIONS.to_latest(&mut conn).await.unwrap();

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
            .await
            .unwrap();
        assert_eq!(row.0, "cron");
        assert_eq!(row.1, "morning");
        assert_eq!(row.2, "none");
        assert_eq!(row.3.as_deref(), Some("summary"));
        assert!(row.4.is_none());
        assert_eq!(row.5, Some(-100));
        assert_eq!(row.6, Some(7));
    }

    #[tokio::test]
    async fn v23_migrates_background_run_detected_by_schedule() {
        let mut conn = Connection::open_in_memory().await.unwrap();
        MIGRATIONS.to_version(&mut conn, 21).await.unwrap();

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
        .await
        .unwrap();
        conn.execute(
            "INSERT INTO cron_runs (id, job_name, started_at, status, log_path, delivery_status)
             VALUES (
                'schedule-bg-1', 'continuation-run', '2026-05-18T02:00:00Z',
                'success', '/log/schedule-bg-1.ndjson', 'silent'
             )",
            [],
        )
        .await
        .unwrap();

        MIGRATIONS.to_latest(&mut conn).await.unwrap();

        let row: (String, Option<String>, Option<String>) = conn
            .query_row(
                "SELECT kind, source_session_id, handoff_state
                 FROM async_runs WHERE id = 'schedule-bg-1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .await
            .unwrap();
        assert_eq!(row.0, "background");
        assert_eq!(
            row.1.as_deref(),
            Some("123e4567-e89b-12d3-a456-426614174000")
        );
        assert_eq!(row.2.as_deref(), Some("spawned"));
    }

    #[tokio::test]
    async fn v23_migrates_immediate_background_run_with_source_header() {
        let mut conn = Connection::open_in_memory().await.unwrap();
        MIGRATIONS.to_version(&mut conn, 21).await.unwrap();

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
        .await
        .unwrap();
        conn.execute(
            "INSERT INTO cron_runs (id, job_name, started_at, status, log_path, delivery_status)
             VALUES (
                'started-immediate-run', 'started-immediate-bg', '2026-05-18T02:00:00Z',
                'success', '/log/started-immediate-run.ndjson', 'silent'
             )",
            [],
        )
        .await
        .unwrap();

        MIGRATIONS.to_latest(&mut conn).await.unwrap();

        let row: (String, Option<String>, Option<String>) = conn
            .query_row(
                "SELECT kind, source_session_id, handoff_state
                 FROM async_runs WHERE id = 'started-immediate-run'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .await
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
            .await
            .unwrap();
        assert_eq!(spec_count, 0, "legacy immediate fork spec must be deleted");
    }

    #[tokio::test]
    async fn v23_preserves_copied_run_null_thread_snapshot() {
        let mut conn = Connection::open_in_memory().await.unwrap();
        MIGRATIONS.to_version(&mut conn, 21).await.unwrap();

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
        .await
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
        .await
        .unwrap();

        MIGRATIONS.to_latest(&mut conn).await.unwrap();

        let thread_id: Option<i64> = conn
            .query_row(
                "SELECT target_thread_id FROM async_runs WHERE id = 'root-topic-run'",
                [],
                |r| r.get(0),
            )
            .await
            .unwrap();
        assert!(
            thread_id.is_none(),
            "copied run must preserve NULL target_thread_id snapshot"
        );
    }

    #[tokio::test]
    async fn v23_detects_background_run_by_bg_job_name_when_spec_is_missing() {
        // Orphaned cron_runs row (cron_specs row absent) with a `bg-` prefixed
        // job_name is the legacy shape we still want to classify as background.
        let mut conn = Connection::open_in_memory().await.unwrap();
        MIGRATIONS.to_version(&mut conn, 21).await.unwrap();

        conn.execute(
            "INSERT INTO cron_runs (id, job_name, started_at, status, log_path, delivery_status)
             VALUES (
                'bg-name-run', 'bg-legacy', '2026-05-18T02:00:00Z',
                'success', '/log/bg-name-run.ndjson', 'silent'
             )",
            [],
        )
        .await
        .unwrap();

        MIGRATIONS.to_latest(&mut conn).await.unwrap();

        let row: (String, Option<String>) = conn
            .query_row(
                "SELECT kind, handoff_state FROM async_runs WHERE id = 'bg-name-run'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .await
            .unwrap();
        assert_eq!(row.0, "background");
        assert_eq!(row.1.as_deref(), Some("spawned"));
    }

    #[tokio::test]
    async fn v23_keeps_user_cron_with_bg_prefix_classified_as_cron() {
        // A user-created recurring cron job whose name happens to start with
        // `bg-` (allowed by validate_job_name) and whose spec is still present
        // must NOT be reclassified as background. Only the `bg-` + orphaned-
        // spec combination survives as a background heuristic.
        let mut conn = Connection::open_in_memory().await.unwrap();
        MIGRATIONS.to_version(&mut conn, 21).await.unwrap();

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
        .await
        .unwrap();
        conn.execute(
            "INSERT INTO cron_runs (id, job_name, started_at, status, log_path, delivery_status)
             VALUES (
                'bg-status-check-run', 'bg-status-check', '2026-05-18T09:00:00Z',
                'success', '/log/bg-status-check-run.ndjson', 'pending'
             )",
            [],
        )
        .await
        .unwrap();

        MIGRATIONS.to_latest(&mut conn).await.unwrap();

        let row: (String, Option<String>) = conn
            .query_row(
                "SELECT kind, handoff_state FROM async_runs WHERE id = 'bg-status-check-run'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .await
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

    #[tokio::test]
    async fn v23_synthesizes_failed_background_run_for_pending_legacy_bg_spec() {
        let mut conn = Connection::open_in_memory().await.unwrap();
        MIGRATIONS.to_version(&mut conn, 21).await.unwrap();

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
        .await
        .unwrap();

        MIGRATIONS.to_latest(&mut conn).await.unwrap();

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
            .await
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
            .await
            .unwrap();
        assert_eq!(spec_count, 0, "legacy @bg cron spec must be deleted");
    }

    #[tokio::test]
    async fn v23_synthesizes_failed_background_run_for_immediate_fork_spec() {
        let mut conn = Connection::open_in_memory().await.unwrap();
        MIGRATIONS.to_version(&mut conn, 21).await.unwrap();

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
        .await
        .unwrap();

        MIGRATIONS.to_latest(&mut conn).await.unwrap();

        let async_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM async_runs WHERE producer_ref = 'immediate-bg'",
                [],
                |r| r.get(0),
            )
            .await
            .unwrap();
        assert_eq!(async_count, 1);

        let row: (String, String, String, Option<String>, Option<i64>) = conn
            .query_row(
                "SELECT kind, status, delivery_status, source_session_id, target_thread_id
                 FROM async_runs WHERE producer_ref = 'immediate-bg'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .await
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
            .await
            .unwrap();
        assert_eq!(spec_count, 0, "legacy immediate fork spec must be deleted");
    }

    #[tokio::test]
    async fn v23_maps_silent_delivery_status_to_none() {
        let mut conn = Connection::open_in_memory().await.unwrap();
        MIGRATIONS.to_version(&mut conn, 21).await.unwrap();

        conn.execute(
            "INSERT INTO cron_runs (id, job_name, started_at, status, log_path, delivery_status)
         VALUES ('silent-1', 'quiet', '2026-05-18T02:00:00Z', 'success', '/log', 'silent')",
            [],
        )
        .await
        .unwrap();

        MIGRATIONS.to_latest(&mut conn).await.unwrap();

        let delivery: (i64, String) = conn
            .query_row(
                "SELECT delivery_required, delivery_status FROM async_runs WHERE id = 'silent-1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .await
            .unwrap();
        assert_eq!(delivery, (0, "none".to_string()));
    }

    #[tokio::test]
    async fn learned_skills_migration_creates_event_tables() {
        let mut conn = Connection::open_in_memory().await.unwrap();
        MIGRATIONS.to_version(&mut conn, 20).await.unwrap();

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
                .await
                .unwrap();
            assert_eq!(exists, 1, "{table} table must exist");
        }
    }

    #[tokio::test]
    async fn learned_skills_nudge_state_defaults_are_usable() {
        let mut conn = Connection::open_in_memory().await.unwrap();
        MIGRATIONS.to_version(&mut conn, 20).await.unwrap();

        conn.execute(
            "INSERT INTO skill_nudge_state (agent_name) VALUES ('right')",
            [],
        )
        .await
        .unwrap();

        let row: (i64, i64, i64, i64) = conn
            .query_row(
                "SELECT tool_iters_since_review, turns_since_review, skill_issue_hints_since_review, review_running FROM skill_nudge_state WHERE agent_name='right'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .await
            .unwrap();
        assert_eq!(row, (0, 0, 0, 0));
    }

    #[tokio::test]
    async fn conversation_messages_schema_exists() {
        let mut conn = Connection::open_in_memory().await.unwrap();
        MIGRATIONS.to_latest(&mut conn).await.unwrap();

        let table_exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='conversation_messages'",
                [],
                |r| r.get(0),
            )
            .await
            .unwrap();
        assert_eq!(table_exists, 1, "conversation_messages table must exist");

        let index_exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master
                 WHERE type='index' AND name='idx_conversation_messages_turso_fts'",
                [],
                |r| r.get(0),
            )
            .await
            .unwrap();
        assert_eq!(
            index_exists, 1,
            "idx_conversation_messages_turso_fts index must exist"
        );
    }

    #[tokio::test]
    async fn skill_review_reports_migration_creates_report_table() {
        let mut conn = Connection::open_in_memory().await.unwrap();
        MIGRATIONS.to_version(&mut conn, 22).await.unwrap();

        let exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='skill_review_reports'",
                [],
                |r| r.get(0),
            )
            .await
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
                .await
                .unwrap();
            assert_eq!(count, 1, "{column} column must exist");
        }
    }

    #[tokio::test]
    async fn conversation_messages_unique_inbound_message() {
        let mut conn = Connection::open_in_memory().await.unwrap();
        MIGRATIONS.to_latest(&mut conn).await.unwrap();

        conn.execute(
            "INSERT INTO conversation_messages (
                platform, chat_id, thread_id, message_id, role, content
             ) VALUES ('telegram', 10, 0, 25, 'user', 'hello')",
            [],
        )
        .await
        .unwrap();
        let result = conn
            .execute(
                "INSERT INTO conversation_messages (
                platform, chat_id, thread_id, message_id, role, content
             ) VALUES ('telegram', 10, 0, 25, 'user', 'duplicate')",
                [],
            )
            .await;

        assert!(
            result.is_err(),
            "same platform/chat/message/role inbound row must be unique"
        );
    }

    #[tokio::test]
    async fn conversation_messages_fts_tracks_updates() {
        let mut conn = Connection::open_in_memory().await.unwrap();
        MIGRATIONS.to_latest(&mut conn).await.unwrap();

        conn.execute(
            "INSERT INTO conversation_messages (
                platform, chat_id, thread_id, message_id, role, content
             ) VALUES ('telegram', 10, 0, 25, 'user', 'original term')",
            [],
        )
        .await
        .unwrap();

        let original_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM conversation_messages
                 WHERE content MATCH 'original'",
                [],
                |r| r.get(0),
            )
            .await
            .unwrap();
        assert_eq!(original_count, 1);

        conn.execute(
            "UPDATE conversation_messages SET content = 'replacement term' WHERE id = 1",
            [],
        )
        .await
        .unwrap();

        let original_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM conversation_messages
                 WHERE content MATCH 'original'",
                [],
                |r| r.get(0),
            )
            .await
            .unwrap();
        let replacement_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM conversation_messages
                 WHERE content MATCH 'replacement'",
                [],
                |r| r.get(0),
            )
            .await
            .unwrap();

        assert_eq!(original_count, 0, "old FTS term must be removed");
        assert_eq!(replacement_count, 1, "new FTS term must be indexed");
    }

    #[tokio::test]
    async fn skill_nudge_state_has_review_gate_defaults() {
        let mut conn = Connection::open_in_memory().await.unwrap();
        MIGRATIONS.to_version(&mut conn, 22).await.unwrap();

        conn.execute(
            "INSERT INTO skill_nudge_state (agent_name) VALUES ('right')",
            [],
        )
        .await
        .unwrap();

        let row: (i64, i64, Option<String>, Option<String>) = conn
            .query_row(
                "SELECT creation_review_interval, daily_review_count, daily_review_date, last_review_status \
             FROM skill_nudge_state WHERE agent_name='right'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .await
            .unwrap();
        assert_eq!(row, (15, 0, None, None));
    }

    #[tokio::test]
    async fn skill_nudge_state_existing_v21_rows_get_review_gate_defaults() {
        let mut conn = Connection::open_in_memory().await.unwrap();
        MIGRATIONS.to_version(&mut conn, 21).await.unwrap();

        conn.execute(
            "INSERT INTO skill_nudge_state (agent_name) VALUES ('right')",
            [],
        )
        .await
        .unwrap();

        MIGRATIONS.to_version(&mut conn, 22).await.unwrap();

        let row: (i64, i64, Option<String>, Option<String>) = conn
            .query_row(
                "SELECT creation_review_interval, daily_review_count, daily_review_date, last_review_status \
             FROM skill_nudge_state WHERE agent_name='right'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .await
            .unwrap();
        assert_eq!(row, (15, 0, None, None));
    }

    #[tokio::test]
    async fn skill_nudge_state_review_gate_migration_tolerates_existing_columns() {
        let mut conn = Connection::open_in_memory().await.unwrap();
        MIGRATIONS.to_version(&mut conn, 21).await.unwrap();

        conn.execute_batch(
            "ALTER TABLE skill_nudge_state
             ADD COLUMN creation_review_interval INTEGER NOT NULL DEFAULT 15;",
        )
        .await
        .unwrap();
        conn.execute(
            "INSERT INTO skill_nudge_state (agent_name) VALUES ('right')",
            [],
        )
        .await
        .unwrap();

        MIGRATIONS
            .to_version(&mut conn, 22)
            .await
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
                .await
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
            .await
            .unwrap();
        assert_eq!(row, (15, 0, None, None));
    }

    #[tokio::test]
    async fn migration_v26_adds_circuit_breaker_columns_idempotently() {
        let mut conn = Connection::open_in_memory().await.unwrap();
        MIGRATIONS.to_version(&mut conn, 25).await.unwrap();
        let pre_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('skill_nudge_state') \
                 WHERE name IN ('consecutive_review_failures', 'review_circuit_open_until')",
                [],
                |r| r.get(0),
            )
            .await
            .unwrap();
        assert_eq!(pre_count, 0, "preconditions: columns not yet present");

        MIGRATIONS.to_version(&mut conn, 26).await.unwrap();
        let post_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('skill_nudge_state') \
                 WHERE name IN ('consecutive_review_failures', 'review_circuit_open_until')",
                [],
                |r| r.get(0),
            )
            .await
            .unwrap();
        assert_eq!(post_count, 2, "both columns present after v26");

        // Re-running v26 is a no-op — verifies idempotency.
        MIGRATIONS.to_version(&mut conn, 26).await.unwrap();
        let post_count_again: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('skill_nudge_state') \
                 WHERE name IN ('consecutive_review_failures', 'review_circuit_open_until')",
                [],
                |r| r.get(0),
            )
            .await
            .unwrap();
        assert_eq!(post_count_again, 2);
    }

    #[tokio::test]
    async fn v27_adds_source_column_to_skill_nudge_signals() {
        let mut conn = Connection::open_in_memory().await.unwrap();
        MIGRATIONS.to_version(&mut conn, 27).await.unwrap();
        let has_column: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('skill_nudge_signals') WHERE name = ?1",
                ["source"],
                |r| r.get(0),
            )
            .await
            .unwrap();
        assert_eq!(has_column, 1, "source column must exist");
        let not_null: i64 = conn
            .query_row(
                "SELECT \"notnull\" FROM pragma_table_info('skill_nudge_signals') WHERE name = ?1",
                ["source"],
                |r| r.get(0),
            )
            .await
            .unwrap();
        assert_eq!(not_null, 1, "source column must be NOT NULL");
    }

    #[tokio::test]
    async fn v27_is_idempotent_on_databases_already_at_v27() {
        let mut conn = Connection::open_in_memory().await.unwrap();
        MIGRATIONS.to_latest(&mut conn).await.unwrap();
        // Re-run by calling the migration registry again should not error.
        MIGRATIONS.to_latest(&mut conn).await.unwrap();
    }

    #[tokio::test]
    async fn v27_index_on_source_column_exists() {
        let mut conn = Connection::open_in_memory().await.unwrap();
        MIGRATIONS.to_version(&mut conn, 27).await.unwrap();
        let exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master \
                 WHERE type='index' AND name='idx_skill_nudge_signals_source'",
                [],
                |r| r.get(0),
            )
            .await
            .unwrap();
        assert_eq!(exists, 1, "idx_skill_nudge_signals_source must exist");
    }

    #[tokio::test]
    async fn v28_adds_wall_elapsed_ms_column_idempotently() {
        let mut conn = Connection::open_in_memory().await.unwrap();
        MIGRATIONS.to_version(&mut conn, 27).await.unwrap();
        let pre: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('usage_events') WHERE name = ?1",
                ["wall_elapsed_ms"],
                |r| r.get(0),
            )
            .await
            .unwrap();
        assert_eq!(pre, 0, "wall_elapsed_ms must not exist at v27");

        MIGRATIONS.to_version(&mut conn, 28).await.unwrap();
        let post: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('usage_events') WHERE name = ?1",
                ["wall_elapsed_ms"],
                |r| r.get(0),
            )
            .await
            .unwrap();
        assert_eq!(post, 1, "wall_elapsed_ms must exist at v28");

        let notnull: i64 = conn
            .query_row(
                "SELECT \"notnull\" FROM pragma_table_info('usage_events') WHERE name = ?1",
                ["wall_elapsed_ms"],
                |r| r.get(0),
            )
            .await
            .unwrap();
        assert_eq!(notnull, 0, "wall_elapsed_ms must be nullable");

        // Re-run is no-op.
        MIGRATIONS.to_version(&mut conn, 28).await.unwrap();
    }

    #[tokio::test]
    async fn v29_creates_curator_state_singleton_table_idempotently() {
        let mut conn = Connection::open_in_memory().await.unwrap();
        MIGRATIONS.to_version(&mut conn, 28).await.unwrap();
        let pre: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='curator_state'",
                [],
                |r| r.get(0),
            )
            .await
            .unwrap();
        assert_eq!(pre, 0);

        MIGRATIONS.to_version(&mut conn, 29).await.unwrap();
        let post: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='curator_state'",
                [],
                |r| r.get(0),
            )
            .await
            .unwrap();
        assert_eq!(post, 1);

        // Singleton CHECK constraint: id=2 must fail.
        let err = conn
            .execute(
                "INSERT INTO curator_state (agent_singleton_id, last_run_at) VALUES (2, NULL)",
                [],
            )
            .await;
        assert!(err.is_err(), "CHECK constraint must reject id != 1");

        // id=1 must succeed.
        conn.execute(
            "INSERT INTO curator_state (agent_singleton_id, last_run_at) VALUES (1, '2026-05-22T00:00:00Z')",
            [],
        )
        .await
        .unwrap();

        // Re-run is no-op.
        MIGRATIONS.to_version(&mut conn, 29).await.unwrap();
    }

    #[tokio::test]
    async fn v30_adds_skill_learning_event_hint_outcome_idempotently() {
        let mut conn = Connection::open_in_memory().await.unwrap();
        MIGRATIONS.to_version(&mut conn, 29).await.unwrap();
        let pre: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('skill_learning_events') WHERE name = ?1",
                ["hint_outcome"],
                |r| r.get(0),
            )
            .await
            .unwrap();
        assert_eq!(pre, 0);

        MIGRATIONS.to_version(&mut conn, 30).await.unwrap();
        let post: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('skill_learning_events') WHERE name = ?1",
                ["hint_outcome"],
                |r| r.get(0),
            )
            .await
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
        .await
        .unwrap();

        let invalid = conn
            .execute(
                "INSERT INTO skill_learning_events (
                invocation_id, agent_name, action, skill_name, phase, status,
                event_refs_json, hint_outcome
             ) VALUES (
                'inv-2', 'alpha', 'create', 'rightx-demo', 'finish', 'aborted',
                '[]', 'bogus'
             )",
                [],
            )
            .await;
        assert!(invalid.is_err(), "invalid hint_outcome must be rejected");

        // Re-run is no-op.
        MIGRATIONS.to_version(&mut conn, 30).await.unwrap();
    }

    #[tokio::test]
    async fn skill_lifecycle_schema_constraints_and_defaults() {
        let mut conn = Connection::open_in_memory().await.unwrap();
        MIGRATIONS.to_latest(&mut conn).await.unwrap();

        let table_exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='skill_lifecycle'",
                [],
                |r| r.get(0),
            )
            .await
            .unwrap();
        assert_eq!(table_exists, 1, "skill_lifecycle table must exist");

        let skill_name_pk: i64 = conn
            .query_row(
                "SELECT pk FROM pragma_table_info('skill_lifecycle') WHERE name = 'skill_name'",
                [],
                |r| r.get(0),
            )
            .await
            .unwrap();
        assert_eq!(skill_name_pk, 1, "skill_name must be the primary key");

        conn.execute(
            "INSERT INTO skill_lifecycle (skill_name) VALUES ('default-row')",
            [],
        )
        .await
        .unwrap();
        let defaults: (i64, i64, i64) = conn
            .query_row(
                "SELECT pinned, use_count, patch_count FROM skill_lifecycle WHERE skill_name = 'default-row'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .await
            .unwrap();
        assert_eq!(defaults, (0, 0, 0));

        for state in ["active", "stale", "archived"] {
            conn.execute(
                "INSERT INTO skill_lifecycle (skill_name, state) VALUES (?1, ?2)",
                [format!("state-{state}"), state.to_string()],
            )
            .await
            .unwrap();
        }
        let invalid_state = conn.execute(
            "INSERT INTO skill_lifecycle (skill_name, state) VALUES ('state-invalid', 'retired')",
            [],
        )
        .await;
        assert!(invalid_state.is_err(), "invalid state must be rejected");

        for created_by in ["foreground", "probe_writer", "curator", "bundled", "cron"] {
            conn.execute(
                "INSERT INTO skill_lifecycle (skill_name, created_by) VALUES (?1, ?2)",
                [format!("created-by-{created_by}"), created_by.to_string()],
            )
            .await
            .unwrap();
        }
        let invalid_created_by = conn.execute(
            "INSERT INTO skill_lifecycle (skill_name, created_by) VALUES ('created-by-invalid', 'unknown')",
            [],
        )
        .await;
        assert!(
            invalid_created_by.is_err(),
            "invalid created_by must be rejected"
        );
    }

    #[tokio::test]
    async fn v37_deletes_legacy_learning_usage_sources() {
        let mut conn = Connection::open_in_memory().await.unwrap();
        // Bring schema up to v36 so usage_events exists (created at v15,
        // extended by v16/v28). Stops just before v37.
        MIGRATIONS.to_version(&mut conn, 36).await.unwrap();

        conn.execute_batch(
            "INSERT INTO usage_events (session_uuid, source, ts, total_cost_usd, num_turns, \
             input_tokens, output_tokens, cache_creation_tokens, cache_read_tokens, \
             web_search_requests, web_fetch_requests, model_usage_json) VALUES \
             ('s1','learning_reviewer','2026-01-01T00:00:00Z',0.10,1,0,0,0,0,0,0,'{}'), \
             ('s2','learning_selector','2026-01-01T00:00:00Z',0.20,1,0,0,0,0,0,0,'{}'), \
             ('s3','interactive','2026-01-01T00:00:00Z',0.30,1,0,0,0,0,0,0,'{}');",
        )
        .await
        .unwrap();

        MIGRATIONS.to_latest(&mut conn).await.unwrap();

        let legacy: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM usage_events WHERE source IN ('learning_reviewer','learning_selector')",
                [],
                |r| r.get(0),
            )
            .await
            .unwrap();
        assert_eq!(legacy, 0, "legacy learning usage rows must be deleted");

        let kept: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM usage_events WHERE source = 'interactive'",
                [],
                |r| r.get(0),
            )
            .await
            .unwrap();
        assert_eq!(kept, 1, "non-legacy rows must be preserved");

        // Idempotent: re-running to_latest is a no-op and does not error.
        MIGRATIONS.to_latest(&mut conn).await.unwrap();
    }

    #[tokio::test]
    async fn v38_creates_skill_spend_and_learning_skip_idempotently() {
        let mut conn = Connection::open_in_memory().await.unwrap();
        MIGRATIONS.to_version(&mut conn, 37).await.unwrap();
        MIGRATIONS.to_latest(&mut conn).await.unwrap();

        for table in ["skill_spend", "learning_skip"] {
            let n: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
                    [table],
                    |r| r.get(0),
                )
                .await
                .unwrap();
            assert_eq!(n, 1, "{table} must exist after v38");
        }

        // Insert + read back one row of each to confirm columns/CHECK.
        conn.execute(
            "INSERT INTO skill_spend (skill_name, kind, cost_usd, cache_read, cache_creation, invocation_id)              VALUES ('rightx-x','create',0.5,10,20,'inv1')",
            (),
        )
        .await
        .unwrap();
        conn.execute(
            "INSERT INTO learning_skip (reason, intended_kind, chat_id, thread_id)              VALUES ('budget', NULL, 7, 0)",
            (),
        )
        .await
        .unwrap();

        // Idempotent: re-running to_latest is a no-op and does not error.
        MIGRATIONS.to_latest(&mut conn).await.unwrap();
    }

    #[tokio::test]
    async fn v39_creates_error_details_table_and_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        // First open runs migrations to LATEST.
        crate::open_connection(dir.path(), true).await.unwrap();
        // Second open must be a no-op (CREATE TABLE IF NOT EXISTS), not error.
        let conn = crate::open_connection(dir.path(), true).await.unwrap();

        let table_exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='error_details'",
                [],
                |row| row.get(0),
            )
            .await
            .unwrap();
        assert_eq!(table_exists, 1, "error_details table must exist after v39");

        let index_exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name='idx_error_details_created_at'",
                [],
                |row| row.get(0),
            )
            .await
            .unwrap();
        assert_eq!(
            index_exists, 1,
            "idx_error_details_created_at index must exist after v39"
        );

        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .await
            .unwrap();
        assert_eq!(version, i64::from(crate::migrations::LATEST_SCHEMA_VERSION));
    }

    #[tokio::test]
    async fn migration_v40_creates_forum_topics_table() {
        let conn = Connection::open_in_memory().await.unwrap();
        MIGRATIONS.to_latest(&conn).await.unwrap();
        let count: i64 = conn
            .query_one(
                "SELECT COUNT(*) FROM pragma_table_info('forum_topics') \
                 WHERE name IN ('chat_id','message_thread_id','name','icon_color','icon_custom_emoji_id','state','updated_at')",
                (),
                |r| r.get(0),
            )
            .await
            .unwrap();
        assert_eq!(count, 7, "forum_topics must have all 7 columns");
    }

    #[tokio::test]
    async fn v48_curator_runs_round_trip_and_idempotent() {
        let conn = Connection::open_in_memory().await.unwrap();
        MIGRATIONS.to_latest(&conn).await.unwrap();

        conn.execute(
            "INSERT INTO curator_runs (run_at, trigger, mode, status) \
             VALUES ('2026-06-15T00:00:00Z','time_fallback','apply','success')",
            (),
        )
        .await
        .unwrap();

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM curator_runs", [], |r| r.get(0))
            .await
            .unwrap();
        assert_eq!(count, 1, "curator_runs must have exactly one row");

        // Re-running to_latest is a no-op — the CREATE TABLE IF NOT EXISTS is
        // idempotent and the existing row is not disturbed.
        MIGRATIONS.to_latest(&conn).await.unwrap();

        let count2: i64 = conn
            .query_row("SELECT COUNT(*) FROM curator_runs", [], |r| r.get(0))
            .await
            .unwrap();
        assert_eq!(count2, 1, "row count must be unchanged after re-migration");
    }
}
