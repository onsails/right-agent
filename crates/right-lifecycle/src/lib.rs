use chrono::{DateTime, Duration, Utc};
use right_db::{Connection, DbError, OptionalExtension};
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub type Result<T> = std::result::Result<T, LifecycleError>;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LifecycleState {
    Active,
    Stale,
    Archived,
}

impl LifecycleState {
    pub fn as_db_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Stale => "stale",
            Self::Archived => "archived",
        }
    }

    pub fn from_db_str(value: &str) -> Result<Self> {
        match value {
            "active" => Ok(Self::Active),
            "stale" => Ok(Self::Stale),
            "archived" => Ok(Self::Archived),
            other => Err(LifecycleError::InvalidState(other.to_string())),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CreatedBy {
    Foreground,
    ProbeWriter,
    Curator,
    Bundled,
    Cron,
}

impl CreatedBy {
    pub fn as_db_str(self) -> &'static str {
        match self {
            Self::Foreground => "foreground",
            Self::ProbeWriter => "probe_writer",
            Self::Curator => "curator",
            Self::Bundled => "bundled",
            Self::Cron => "cron",
        }
    }

    pub fn from_db_str(value: &str) -> Result<Self> {
        match value {
            "foreground" => Ok(Self::Foreground),
            "probe_writer" => Ok(Self::ProbeWriter),
            "curator" => Ok(Self::Curator),
            "bundled" => Ok(Self::Bundled),
            "cron" => Ok(Self::Cron),
            other => Err(LifecycleError::InvalidCreatedBy(other.to_string())),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SkillLifecycleRow {
    pub skill_name: String,
    pub state: LifecycleState,
    pub pinned: bool,
    pub created_by: CreatedBy,
    pub use_count: i64,
    pub patch_count: i64,
    pub created_at: Option<DateTime<Utc>>,
    pub last_used_at: Option<DateTime<Utc>>,
    pub last_patched_at: Option<DateTime<Utc>>,
    pub archived_at: Option<DateTime<Utc>>,
    pub absorbed_into: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TransitionConfig {
    pub stale_after: Duration,
    pub archive_after: Duration,
}

impl Default for TransitionConfig {
    fn default() -> Self {
        Self {
            stale_after: Duration::days(30),
            archive_after: Duration::days(90),
        }
    }
}

#[derive(Debug, Error)]
pub enum LifecycleError {
    #[error("invalid lifecycle state from database: {0}")]
    InvalidState(String),

    #[error("invalid lifecycle created_by from database: {0}")]
    InvalidCreatedBy(String),

    #[error("invalid lifecycle timestamp in {column}: {value}")]
    InvalidTimestamp {
        column: &'static str,
        value: String,
        #[source]
        source: chrono::ParseError,
    },

    #[error("database error: {0}")]
    Database(#[from] DbError),
}

struct RawSkillLifecycleRow {
    skill_name: String,
    state: String,
    pinned: i64,
    created_by: String,
    use_count: i64,
    patch_count: i64,
    created_at: Option<String>,
    last_used_at: Option<String>,
    last_patched_at: Option<String>,
    archived_at: Option<String>,
    absorbed_into: Option<String>,
}

pub async fn mark_created(
    conn: &Connection,
    skill_name: &str,
    created_by: CreatedBy,
    now_utc: DateTime<Utc>,
) -> Result<()> {
    let now = now_utc.to_rfc3339();
    conn.execute(
        "INSERT INTO skill_lifecycle (
            skill_name, state, created_by, created_at, archived_at, absorbed_into
         ) VALUES (?1, ?2, ?3, ?4, NULL, NULL)
         ON CONFLICT(skill_name) DO UPDATE SET
            state = excluded.state,
            created_by = excluded.created_by,
            created_at = excluded.created_at,
            archived_at = NULL,
            absorbed_into = NULL",
        (
            skill_name,
            LifecycleState::Active.as_db_str(),
            created_by.as_db_str(),
            now,
        ),
    )
    .await?;
    Ok(())
}

pub async fn bump_patch(
    conn: &Connection,
    skill_name: &str,
    created_by: CreatedBy,
    now_utc: DateTime<Utc>,
) -> Result<()> {
    let now = now_utc.to_rfc3339();
    conn.execute(
        "INSERT INTO skill_lifecycle (
            skill_name, state, created_by, patch_count, created_at, last_patched_at
         ) VALUES (?1, ?2, ?3, 1, ?4, ?4)
         ON CONFLICT(skill_name) DO UPDATE SET
            state = excluded.state,
            patch_count = skill_lifecycle.patch_count + 1,
            last_patched_at = excluded.last_patched_at,
            archived_at = NULL,
            absorbed_into = NULL",
        (
            skill_name,
            LifecycleState::Active.as_db_str(),
            created_by.as_db_str(),
            now,
        ),
    )
    .await?;
    Ok(())
}

pub async fn bump_use(conn: &Connection, skill_name: &str, now_utc: DateTime<Utc>) -> Result<()> {
    let now = now_utc.to_rfc3339();
    bump_use_stmt(conn, skill_name, &now).await?;
    Ok(())
}

/// Bump usage counters for each name in one transaction. Empty iterators
/// commit a no-op transaction (cheap).
pub async fn bump_use_many<I, S>(
    conn: &Connection,
    skill_names: I,
    now_utc: DateTime<Utc>,
) -> Result<()>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let now = now_utc.to_rfc3339();
    let tx = conn.transaction().await?;
    for name in skill_names {
        tx.execute(
            BUMP_USE_SQL,
            (
                name.as_ref(),
                LifecycleState::Active.as_db_str(),
                CreatedBy::Foreground.as_db_str(),
                now.as_str(),
            ),
        )
        .await?;
    }
    tx.commit().await?;
    Ok(())
}

const BUMP_USE_SQL: &str = "INSERT INTO skill_lifecycle (
            skill_name, state, created_by, use_count, created_at, last_used_at
         ) VALUES (?1, ?2, ?3, 1, ?4, ?4)
         ON CONFLICT(skill_name) DO UPDATE SET
            state = excluded.state,
            use_count = skill_lifecycle.use_count + 1,
            last_used_at = excluded.last_used_at,
            archived_at = NULL,
            absorbed_into = NULL";

async fn bump_use_stmt(conn: &Connection, skill_name: &str, now_rfc3339: &str) -> Result<()> {
    conn.execute(
        BUMP_USE_SQL,
        (
            skill_name,
            LifecycleState::Active.as_db_str(),
            CreatedBy::Foreground.as_db_str(),
            now_rfc3339,
        ),
    )
    .await?;
    Ok(())
}

pub async fn set_pinned(conn: &Connection, skill_name: &str, pinned: bool) -> Result<bool> {
    let pinned_value = i64::from(pinned);
    let changed = conn
        .execute(
            "UPDATE skill_lifecycle
         SET pinned = ?2
         WHERE skill_name = ?1 AND pinned != ?2",
            (skill_name, pinned_value),
        )
        .await?;
    Ok(changed > 0)
}

pub async fn get(conn: &Connection, skill_name: &str) -> Result<Option<SkillLifecycleRow>> {
    let raw = conn
        .query_one(
            "SELECT
                skill_name, state, pinned, created_by, use_count, patch_count,
                created_at, last_used_at, last_patched_at, archived_at, absorbed_into
             FROM skill_lifecycle
             WHERE skill_name = ?1",
            [skill_name],
            raw_row_from_sql,
        )
        .await
        .optional()?;
    raw.map(row_from_raw).transpose()
}

pub async fn list(conn: &Connection) -> Result<Vec<SkillLifecycleRow>> {
    list_where(
        conn,
        "SELECT
            skill_name, state, pinned, created_by, use_count, patch_count,
            created_at, last_used_at, last_patched_at, archived_at, absorbed_into
         FROM skill_lifecycle
         ORDER BY skill_name",
    )
    .await
}

pub async fn list_curator_candidates(conn: &Connection) -> Result<Vec<SkillLifecycleRow>> {
    list_where(
        conn,
        "SELECT
            skill_name, state, pinned, created_by, use_count, patch_count,
            created_at, last_used_at, last_patched_at, archived_at, absorbed_into
         FROM skill_lifecycle
         WHERE state = 'stale'
           AND pinned = 0
           AND created_by IN ('probe_writer', 'curator', 'foreground', 'cron')
         ORDER BY skill_name",
    )
    .await
}

pub async fn apply_automatic_transitions(
    conn: &Connection,
    now_utc: DateTime<Utc>,
    config: TransitionConfig,
) -> Result<usize> {
    let stale_cutoff = (now_utc - config.stale_after).to_rfc3339();
    let archive_cutoff = (now_utc - config.archive_after).to_rfc3339();
    let now = now_utc.to_rfc3339();
    let tx = conn.transaction().await?;

    // Archive first: any unpinned learned (probe-writer/curator/foreground/cron) row
    // (active OR stale) whose latest activity is older than the archive cutoff.
    // Running archive before stale prevents an active-then-stale double hop in one call.
    let archived = tx
        .execute(
            &format!(
                "UPDATE skill_lifecycle
             SET state = ?1, archived_at = ?2
             WHERE state IN (?3, ?4)
               AND pinned = 0
               AND created_by IN ('probe_writer', 'curator', 'foreground', 'cron')
               AND {}",
                activity_before_cutoff("?5"),
            ),
            (
                LifecycleState::Archived.as_db_str(),
                now.as_str(),
                LifecycleState::Active.as_db_str(),
                LifecycleState::Stale.as_db_str(),
                archive_cutoff.as_str(),
            ),
        )
        .await?;

    let staled = tx
        .execute(
            &format!(
                "UPDATE skill_lifecycle
             SET state = ?1
             WHERE state = ?2
               AND pinned = 0
               AND created_by IN ('probe_writer', 'curator', 'foreground', 'cron')
               AND {}",
                activity_before_cutoff("?3"),
            ),
            (
                LifecycleState::Stale.as_db_str(),
                LifecycleState::Active.as_db_str(),
                stale_cutoff.as_str(),
            ),
        )
        .await?;

    tx.commit().await?;
    Ok(archived + staled)
}

/// SQL fragment: true when `MAX(last_used_at, last_patched_at)` (treating
/// NULLs as missing) is strictly before the cutoff parameter. Compares via
/// `julianday` to normalize across RFC3339 offsets; rows with both timestamps
/// NULL never match. Pass the cutoff placeholder (e.g. `"?7"`) so the caller
/// keeps the surrounding positional indexes consistent.
fn activity_before_cutoff(cutoff_placeholder: &str) -> String {
    format!(
        "julianday({cutoff_placeholder}) > COALESCE(
            MAX(julianday(last_used_at), julianday(last_patched_at)),
            julianday(last_used_at),
            julianday(last_patched_at)
        )"
    )
}

/// Count rows whose `created_at` OR `last_patched_at` is strictly after
/// `since`. Aggregate query — no row materialization. Used by the curator's
/// skill-change-count trigger.
pub async fn count_changes_since(conn: &Connection, since: DateTime<Utc>) -> Result<u32> {
    let since = since.to_rfc3339();
    let count: i64 = conn
        .query_one(
            "SELECT COUNT(*) FROM skill_lifecycle
         WHERE (created_at IS NOT NULL AND julianday(created_at) > julianday(?1))
            OR (last_patched_at IS NOT NULL AND julianday(last_patched_at) > julianday(?1))",
            [&since],
            |row| row.get(0),
        )
        .await?;
    Ok(u32::try_from(count.max(0)).unwrap_or(u32::MAX))
}

async fn list_where(conn: &Connection, sql: &str) -> Result<Vec<SkillLifecycleRow>> {
    let rows = conn.query_all(sql, (), raw_row_from_sql).await?;
    rows.into_iter().map(row_from_raw).collect()
}

fn raw_row_from_sql(
    row: &right_db::row::Row<'_>,
) -> std::result::Result<RawSkillLifecycleRow, DbError> {
    Ok(RawSkillLifecycleRow {
        skill_name: row.get(0)?,
        state: row.get(1)?,
        pinned: row.get(2)?,
        created_by: row.get(3)?,
        use_count: row.get(4)?,
        patch_count: row.get(5)?,
        created_at: row.get(6)?,
        last_used_at: row.get(7)?,
        last_patched_at: row.get(8)?,
        archived_at: row.get(9)?,
        absorbed_into: row.get(10)?,
    })
}

fn row_from_raw(raw: RawSkillLifecycleRow) -> Result<SkillLifecycleRow> {
    Ok(SkillLifecycleRow {
        skill_name: raw.skill_name,
        state: LifecycleState::from_db_str(&raw.state)?,
        pinned: raw.pinned != 0,
        created_by: CreatedBy::from_db_str(&raw.created_by)?,
        use_count: raw.use_count,
        patch_count: raw.patch_count,
        created_at: parse_timestamp("created_at", raw.created_at)?,
        last_used_at: parse_timestamp("last_used_at", raw.last_used_at)?,
        last_patched_at: parse_timestamp("last_patched_at", raw.last_patched_at)?,
        archived_at: parse_timestamp("archived_at", raw.archived_at)?,
        absorbed_into: raw.absorbed_into,
    })
}

fn parse_timestamp(column: &'static str, value: Option<String>) -> Result<Option<DateTime<Utc>>> {
    value
        .map(|value| {
            DateTime::parse_from_rfc3339(&value)
                .map(|dt| dt.with_timezone(&Utc))
                .map_err(|source| LifecycleError::InvalidTimestamp {
                    column,
                    value,
                    source,
                })
        })
        .transpose()
}

pub fn latest_activity_at(row: &SkillLifecycleRow) -> Option<DateTime<Utc>> {
    match (row.last_used_at, row.last_patched_at) {
        (Some(used), Some(patched)) => Some(used.max(patched)),
        (Some(used), None) => Some(used),
        (None, Some(patched)) => Some(patched),
        (None, None) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeDelta;

    async fn migrated_conn() -> (tempfile::TempDir, Connection) {
        right_db::test_support::migrated_connection().await
    }

    fn utc(value: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(value)
            .unwrap()
            .with_timezone(&Utc)
    }

    #[test]
    fn created_by_cron_round_trips_db_str() {
        assert_eq!(CreatedBy::Cron.as_db_str(), "cron");
        assert_eq!(CreatedBy::from_db_str("cron").unwrap(), CreatedBy::Cron);
    }

    #[tokio::test]
    async fn lifecycle_enums_serialize_as_snake_case() {
        assert_eq!(
            serde_json::to_string(&LifecycleState::Active).unwrap(),
            "\"active\""
        );
        assert_eq!(
            serde_json::to_string(&CreatedBy::ProbeWriter).unwrap(),
            "\"probe_writer\""
        );
    }

    struct LifecycleRowFixture<'a> {
        skill_name: &'a str,
        state: LifecycleState,
        pinned: bool,
        created_by: CreatedBy,
        use_count: i64,
        patch_count: i64,
        last_used_at: Option<DateTime<Utc>>,
        last_patched_at: Option<DateTime<Utc>>,
    }

    impl<'a> LifecycleRowFixture<'a> {
        fn new(skill_name: &'a str) -> Self {
            Self {
                skill_name,
                state: LifecycleState::Active,
                pinned: false,
                created_by: CreatedBy::Curator,
                use_count: 0,
                patch_count: 0,
                last_used_at: None,
                last_patched_at: None,
            }
        }

        fn state(mut self, state: LifecycleState) -> Self {
            self.state = state;
            self
        }

        fn pinned(mut self, pinned: bool) -> Self {
            self.pinned = pinned;
            self
        }

        fn created_by(mut self, created_by: CreatedBy) -> Self {
            self.created_by = created_by;
            self
        }

        fn patch_count(mut self, patch_count: i64) -> Self {
            self.patch_count = patch_count;
            self
        }

        fn last_used_at(mut self, last_used_at: DateTime<Utc>) -> Self {
            self.last_used_at = Some(last_used_at);
            self
        }

        fn last_patched_at(mut self, last_patched_at: DateTime<Utc>) -> Self {
            self.last_patched_at = Some(last_patched_at);
            self
        }

        async fn insert(self, conn: &Connection) {
            conn.execute(
                "INSERT INTO skill_lifecycle (
                    skill_name, state, pinned, created_by, use_count, patch_count,
                    created_at, last_used_at, last_patched_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                (
                    self.skill_name,
                    self.state.as_db_str(),
                    i64::from(self.pinned),
                    self.created_by.as_db_str(),
                    self.use_count,
                    self.patch_count,
                    utc("2026-04-01T00:00:00Z").to_rfc3339(),
                    self.last_used_at.map(|t| t.to_rfc3339()),
                    self.last_patched_at.map(|t| t.to_rfc3339()),
                ),
            )
            .await
            .unwrap();
        }
    }

    #[tokio::test]
    async fn mark_created_inserts_active_row_with_provenance() {
        let (_dir, conn) = migrated_conn().await;
        let now = utc("2026-05-23T12:00:00Z");

        mark_created(&conn, "rightx-demo", CreatedBy::ProbeWriter, now)
            .await
            .unwrap();

        let row = get(&conn, "rightx-demo").await.unwrap().unwrap();
        assert_eq!(row.skill_name, "rightx-demo");
        assert_eq!(row.state, LifecycleState::Active);
        assert!(!row.pinned);
        assert_eq!(row.created_by, CreatedBy::ProbeWriter);
        assert_eq!(row.use_count, 0);
        assert_eq!(row.patch_count, 0);
        assert_eq!(row.created_at, Some(now));
        assert_eq!(row.archived_at, None);
        assert_eq!(row.absorbed_into, None);
    }

    #[tokio::test]
    async fn bump_patch_preserves_existing_created_by() {
        let (_dir, conn) = migrated_conn().await;
        let now = utc("2026-05-23T12:00:00Z");
        LifecycleRowFixture::new("rightx-demo")
            .state(LifecycleState::Stale)
            .created_by(CreatedBy::Curator)
            .patch_count(2)
            .insert(&conn)
            .await;

        bump_patch(&conn, "rightx-demo", CreatedBy::ProbeWriter, now)
            .await
            .unwrap();

        let row = get(&conn, "rightx-demo").await.unwrap().unwrap();
        assert_eq!(row.state, LifecycleState::Active);
        assert_eq!(row.created_by, CreatedBy::Curator);
        assert_eq!(row.patch_count, 3);
        assert_eq!(row.last_patched_at, Some(now));
    }

    #[tokio::test]
    async fn bump_use_creates_foreground_row_when_missing() {
        let (_dir, conn) = migrated_conn().await;
        let now = utc("2026-05-23T12:00:00Z");

        bump_use(&conn, "rightx-demo", now).await.unwrap();

        let row = get(&conn, "rightx-demo").await.unwrap().unwrap();
        assert_eq!(row.state, LifecycleState::Active);
        assert_eq!(row.created_by, CreatedBy::Foreground);
        assert_eq!(row.use_count, 1);
        assert_eq!(row.patch_count, 0);
        assert_eq!(row.created_at, Some(now));
        assert_eq!(row.last_used_at, Some(now));
    }

    #[tokio::test]
    async fn set_pinned_toggles_existing_row() {
        let (_dir, conn) = migrated_conn().await;
        LifecycleRowFixture::new("rightx-demo")
            .created_by(CreatedBy::Curator)
            .insert(&conn)
            .await;

        assert!(set_pinned(&conn, "rightx-demo", true).await.unwrap());
        assert!(get(&conn, "rightx-demo").await.unwrap().unwrap().pinned);
        assert!(set_pinned(&conn, "rightx-demo", false).await.unwrap());
        assert!(!get(&conn, "rightx-demo").await.unwrap().unwrap().pinned);

        assert!(!set_pinned(&conn, "missing", true).await.unwrap());
        assert!(get(&conn, "missing").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn automatic_transitions_skip_pinned_and_bundled_rows() {
        let (_dir, conn) = migrated_conn().await;
        let now = utc("2026-05-23T12:00:00Z");
        let old = now - TimeDelta::days(45);
        let stale_old = now - TimeDelta::days(20);
        let recent = now - TimeDelta::days(2);
        let config = TransitionConfig {
            stale_after: TimeDelta::days(7),
            archive_after: TimeDelta::days(30),
        };

        LifecycleRowFixture::new("candidate-probe")
            .created_by(CreatedBy::ProbeWriter)
            .last_used_at(stale_old)
            .insert(&conn)
            .await;
        LifecycleRowFixture::new("old-stale-curator")
            .state(LifecycleState::Stale)
            .created_by(CreatedBy::Curator)
            .last_used_at(old)
            .insert(&conn)
            .await;
        LifecycleRowFixture::new("pinned-probe")
            .pinned(true)
            .created_by(CreatedBy::ProbeWriter)
            .last_used_at(old)
            .insert(&conn)
            .await;
        // Unpinned foreground row old enough to archive — foreground is now auto-managed.
        LifecycleRowFixture::new("foreground-old")
            .created_by(CreatedBy::Foreground)
            .last_used_at(old)
            .insert(&conn)
            .await;
        // Bundled rows are never touched regardless of age.
        LifecycleRowFixture::new("bundled-old")
            .created_by(CreatedBy::Bundled)
            .last_used_at(old)
            .insert(&conn)
            .await;
        LifecycleRowFixture::new("recent-patch-wins")
            .created_by(CreatedBy::Curator)
            .last_used_at(old)
            .last_patched_at(recent)
            .insert(&conn)
            .await;

        let updated = apply_automatic_transitions(&conn, now, config)
            .await
            .unwrap();

        // candidate-probe → Stale, old-stale-curator → Archived, foreground-old → Archived
        assert_eq!(updated, 3);
        assert_eq!(
            get(&conn, "candidate-probe").await.unwrap().unwrap().state,
            LifecycleState::Stale
        );
        let archived_curator = get(&conn, "old-stale-curator").await.unwrap().unwrap();
        assert_eq!(archived_curator.state, LifecycleState::Archived);
        assert_eq!(archived_curator.archived_at, Some(now));
        let archived_fg = get(&conn, "foreground-old").await.unwrap().unwrap();
        assert_eq!(
            archived_fg.state,
            LifecycleState::Archived,
            "unpinned foreground row must be archived when old enough"
        );
        assert_eq!(archived_fg.archived_at, Some(now));
        for skill_name in ["pinned-probe", "bundled-old", "recent-patch-wins"] {
            assert_eq!(
                get(&conn, skill_name).await.unwrap().unwrap().state,
                LifecycleState::Active,
                "{skill_name} must not transition"
            );
        }
    }

    #[tokio::test]
    async fn automatic_transitions_leave_rows_without_activity_unchanged() {
        let (_dir, conn) = migrated_conn().await;
        let now = utc("2026-05-23T12:00:00Z");
        let config = TransitionConfig {
            stale_after: TimeDelta::days(7),
            archive_after: TimeDelta::days(30),
        };
        LifecycleRowFixture::new("created-only-probe")
            .created_by(CreatedBy::ProbeWriter)
            .insert(&conn)
            .await;

        let updated = apply_automatic_transitions(&conn, now, config)
            .await
            .unwrap();

        assert_eq!(updated, 0);
        assert_eq!(
            get(&conn, "created-only-probe")
                .await
                .unwrap()
                .unwrap()
                .state,
            LifecycleState::Active
        );
    }

    #[tokio::test]
    async fn active_rows_older_than_archive_after_archive_directly() {
        let (_dir, conn) = migrated_conn().await;
        let now = utc("2026-05-23T12:00:00Z");
        let old = now - TimeDelta::days(45);
        let config = TransitionConfig {
            stale_after: TimeDelta::days(7),
            archive_after: TimeDelta::days(30),
        };
        LifecycleRowFixture::new("ancient-active-curator")
            .created_by(CreatedBy::Curator)
            .last_used_at(old)
            .insert(&conn)
            .await;

        let updated = apply_automatic_transitions(&conn, now, config)
            .await
            .unwrap();

        assert_eq!(updated, 1);
        let row = get(&conn, "ancient-active-curator").await.unwrap().unwrap();
        assert_eq!(row.state, LifecycleState::Archived);
        assert_eq!(row.archived_at, Some(now));
    }

    #[tokio::test]
    async fn automatic_transitions_do_not_fire_at_exact_thresholds() {
        let (_dir, conn) = migrated_conn().await;
        let now = utc("2026-05-23T12:00:00Z");
        let config = TransitionConfig {
            stale_after: TimeDelta::days(7),
            archive_after: TimeDelta::days(30),
        };
        LifecycleRowFixture::new("active-at-stale-boundary")
            .created_by(CreatedBy::Curator)
            .last_used_at(now - config.stale_after)
            .insert(&conn)
            .await;
        LifecycleRowFixture::new("active-at-archive-boundary")
            .created_by(CreatedBy::Curator)
            .last_used_at(now - config.archive_after)
            .insert(&conn)
            .await;
        LifecycleRowFixture::new("stale-at-archive-boundary")
            .state(LifecycleState::Stale)
            .created_by(CreatedBy::ProbeWriter)
            .last_used_at(now - config.archive_after)
            .insert(&conn)
            .await;

        let updated = apply_automatic_transitions(&conn, now, config)
            .await
            .unwrap();

        assert_eq!(updated, 1);
        assert_eq!(
            get(&conn, "active-at-stale-boundary")
                .await
                .unwrap()
                .unwrap()
                .state,
            LifecycleState::Active
        );
        assert_eq!(
            get(&conn, "active-at-archive-boundary")
                .await
                .unwrap()
                .unwrap()
                .state,
            LifecycleState::Stale
        );
        assert_eq!(
            get(&conn, "stale-at-archive-boundary")
                .await
                .unwrap()
                .unwrap()
                .state,
            LifecycleState::Stale
        );
    }

    #[tokio::test]
    async fn automatic_transitions_skip_rows_pinned_or_used_between_selection_and_update() {
        let (_dir, conn) = migrated_conn().await;
        let now = utc("2026-05-23T12:00:00Z");
        let old = now - TimeDelta::days(20);
        let config = TransitionConfig {
            stale_after: TimeDelta::days(7),
            archive_after: TimeDelta::days(30),
        };

        LifecycleRowFixture::new("pinned-after-selection")
            .created_by(CreatedBy::ProbeWriter)
            .last_used_at(old)
            .insert(&conn)
            .await;
        LifecycleRowFixture::new("used-after-selection")
            .created_by(CreatedBy::Curator)
            .last_used_at(old)
            .insert(&conn)
            .await;

        // Simulate a race: pin + foreground use happen between gate decision
        // and UPDATE. The single-statement UPDATE re-evaluates the WHERE
        // clause at execution time, so both rows must be skipped.
        set_pinned(&conn, "pinned-after-selection", true)
            .await
            .unwrap();
        bump_use(&conn, "used-after-selection", now).await.unwrap();

        let updated = apply_automatic_transitions(&conn, now, config)
            .await
            .unwrap();

        assert_eq!(updated, 0);
        assert_eq!(
            get(&conn, "pinned-after-selection")
                .await
                .unwrap()
                .unwrap()
                .state,
            LifecycleState::Active
        );
        assert_eq!(
            get(&conn, "used-after-selection")
                .await
                .unwrap()
                .unwrap()
                .state,
            LifecycleState::Active
        );
    }

    #[tokio::test]
    async fn stale_transition_predicate_compares_rfc3339_instants_across_offsets() {
        let (_dir, conn) = migrated_conn().await;
        let now = utc("2026-05-30T12:00:00Z");
        let config = TransitionConfig {
            stale_after: TimeDelta::days(7),
            archive_after: TimeDelta::days(30),
        };
        // last_used_at is `now - 7 days` written with a +01:00 offset; the
        // stale predicate must compare it as the same instant as the cutoff
        // and not transition (boundary: `<`, not `<=`).
        conn.execute(
            "INSERT INTO skill_lifecycle (
                skill_name, state, pinned, created_by, created_at, last_used_at
             ) VALUES (
                'offset-equivalent', 'active', 0, 'curator',
                '2026-04-01T00:00:00Z', '2026-05-23T13:00:00+01:00'
             )",
            [],
        )
        .await
        .unwrap();
        // last_used_at is `now - 8 days` with a +01:00 offset → must stale.
        conn.execute(
            "INSERT INTO skill_lifecycle (
                skill_name, state, pinned, created_by, created_at, last_used_at
             ) VALUES (
                'offset-stale', 'active', 0, 'curator',
                '2026-04-01T00:00:00Z', '2026-05-22T13:00:00+01:00'
             )",
            [],
        )
        .await
        .unwrap();

        let updated = apply_automatic_transitions(&conn, now, config)
            .await
            .unwrap();

        assert_eq!(updated, 1);
        assert_eq!(
            get(&conn, "offset-equivalent")
                .await
                .unwrap()
                .unwrap()
                .state,
            LifecycleState::Active
        );
        assert_eq!(
            get(&conn, "offset-stale").await.unwrap().unwrap().state,
            LifecycleState::Stale
        );
    }

    #[tokio::test]
    async fn count_changes_since_counts_rows_created_or_patched_after_cutoff() {
        let (_dir, conn) = migrated_conn().await;
        let since = utc("2026-05-21T00:00:00Z");

        LifecycleRowFixture::new("rightx-old")
            .created_by(CreatedBy::ProbeWriter)
            .insert(&conn)
            .await;
        mark_created(
            &conn,
            "rightx-new",
            CreatedBy::ProbeWriter,
            utc("2026-05-22T00:00:00Z"),
        )
        .await
        .unwrap();
        bump_patch(
            &conn,
            "rightx-patched-old",
            CreatedBy::Curator,
            utc("2026-05-22T00:00:00Z"),
        )
        .await
        .unwrap();

        assert_eq!(count_changes_since(&conn, since).await.unwrap(), 2);
    }

    #[tokio::test]
    async fn foreground_and_cron_rows_become_curator_candidates() {
        let (_dir, conn) = migrated_conn().await;
        let now = utc("2026-05-23T12:00:00Z");
        let old = now - TimeDelta::days(20);

        LifecycleRowFixture::new("rightx-fg")
            .state(LifecycleState::Stale)
            .created_by(CreatedBy::Foreground)
            .last_used_at(old)
            .insert(&conn)
            .await;
        LifecycleRowFixture::new("rightx-cron")
            .state(LifecycleState::Stale)
            .created_by(CreatedBy::Cron)
            .last_used_at(old)
            .insert(&conn)
            .await;
        LifecycleRowFixture::new("rightx-probe")
            .state(LifecycleState::Stale)
            .created_by(CreatedBy::ProbeWriter)
            .last_used_at(old)
            .insert(&conn)
            .await;

        let candidates = list_curator_candidates(&conn).await.unwrap();
        let names: Vec<&str> = candidates.iter().map(|r| r.skill_name.as_str()).collect();
        assert!(
            names.contains(&"rightx-fg"),
            "foreground row must be a curator candidate"
        );
        assert!(
            names.contains(&"rightx-cron"),
            "cron row must be a curator candidate"
        );
        assert!(
            names.contains(&"rightx-probe"),
            "probe_writer row must be a curator candidate"
        );
    }

    #[tokio::test]
    async fn unused_foreground_row_archives_but_pinned_does_not() {
        let (_dir, conn) = migrated_conn().await;
        let now = utc("2026-05-23T12:00:00Z");
        let old = now - TimeDelta::days(45);
        let config = TransitionConfig {
            stale_after: TimeDelta::days(7),
            archive_after: TimeDelta::days(30),
        };

        LifecycleRowFixture::new("fg-unpinned")
            .created_by(CreatedBy::Foreground)
            .last_used_at(old)
            .insert(&conn)
            .await;
        LifecycleRowFixture::new("fg-pinned")
            .created_by(CreatedBy::Foreground)
            .pinned(true)
            .last_used_at(old)
            .insert(&conn)
            .await;

        apply_automatic_transitions(&conn, now, config)
            .await
            .unwrap();

        assert_eq!(
            get(&conn, "fg-unpinned").await.unwrap().unwrap().state,
            LifecycleState::Archived,
            "unpinned foreground row must be archived"
        );
        assert_eq!(
            get(&conn, "fg-pinned").await.unwrap().unwrap().state,
            LifecycleState::Active,
            "pinned foreground row must stay active"
        );
    }

    #[tokio::test]
    async fn bump_use_many_increments_in_a_single_transaction() {
        let (_dir, conn) = migrated_conn().await;
        let now = utc("2026-05-23T12:00:00Z");

        bump_use_many(&conn, ["rightx-a", "rightx-b", "rightx-a"], now)
            .await
            .unwrap();

        assert_eq!(
            get(&conn, "rightx-a").await.unwrap().unwrap().use_count,
            2,
            "duplicate names increment independently"
        );
        assert_eq!(get(&conn, "rightx-b").await.unwrap().unwrap().use_count, 1);
    }
}
