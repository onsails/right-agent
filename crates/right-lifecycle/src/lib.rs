use chrono::{DateTime, Duration, Utc};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub type Result<T> = std::result::Result<T, LifecycleError>;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
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
pub enum CreatedBy {
    Foreground,
    ProbeWriter,
    Curator,
    Bundled,
}

impl CreatedBy {
    pub fn as_db_str(self) -> &'static str {
        match self {
            Self::Foreground => "foreground",
            Self::ProbeWriter => "probe_writer",
            Self::Curator => "curator",
            Self::Bundled => "bundled",
        }
    }

    pub fn from_db_str(value: &str) -> Result<Self> {
        match value {
            "foreground" => Ok(Self::Foreground),
            "probe_writer" => Ok(Self::ProbeWriter),
            "curator" => Ok(Self::Curator),
            "bundled" => Ok(Self::Bundled),
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
    Database(#[from] rusqlite::Error),
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

pub fn mark_created(
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
        params![
            skill_name,
            LifecycleState::Active.as_db_str(),
            created_by.as_db_str(),
            now
        ],
    )?;
    Ok(())
}

pub fn bump_patch(
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
        params![
            skill_name,
            LifecycleState::Active.as_db_str(),
            created_by.as_db_str(),
            now
        ],
    )?;
    Ok(())
}

pub fn bump_use(conn: &Connection, skill_name: &str, now_utc: DateTime<Utc>) -> Result<()> {
    let now = now_utc.to_rfc3339();
    conn.execute(
        "INSERT INTO skill_lifecycle (
            skill_name, state, created_by, use_count, created_at, last_used_at
         ) VALUES (?1, ?2, ?3, 1, ?4, ?4)
         ON CONFLICT(skill_name) DO UPDATE SET
            state = excluded.state,
            use_count = skill_lifecycle.use_count + 1,
            last_used_at = excluded.last_used_at,
            archived_at = NULL,
            absorbed_into = NULL",
        params![
            skill_name,
            LifecycleState::Active.as_db_str(),
            CreatedBy::Foreground.as_db_str(),
            now
        ],
    )?;
    Ok(())
}

pub fn set_pinned(conn: &Connection, skill_name: &str, pinned: bool) -> Result<bool> {
    let pinned_value = i64::from(pinned);
    let changed = conn.execute(
        "UPDATE skill_lifecycle
         SET pinned = ?2
         WHERE skill_name = ?1 AND pinned != ?2",
        params![skill_name, pinned_value],
    )?;
    Ok(changed > 0)
}

pub fn get(conn: &Connection, skill_name: &str) -> Result<Option<SkillLifecycleRow>> {
    let raw = conn
        .query_row(
            "SELECT
                skill_name, state, pinned, created_by, use_count, patch_count,
                created_at, last_used_at, last_patched_at, archived_at, absorbed_into
             FROM skill_lifecycle
             WHERE skill_name = ?1",
            [skill_name],
            raw_row_from_sql,
        )
        .optional()?;
    raw.map(row_from_raw).transpose()
}

pub fn list(conn: &Connection) -> Result<Vec<SkillLifecycleRow>> {
    list_where(
        conn,
        "SELECT
            skill_name, state, pinned, created_by, use_count, patch_count,
            created_at, last_used_at, last_patched_at, archived_at, absorbed_into
         FROM skill_lifecycle
         ORDER BY skill_name",
    )
}

pub fn list_curator_candidates(conn: &Connection) -> Result<Vec<SkillLifecycleRow>> {
    list_where(
        conn,
        "SELECT
            skill_name, state, pinned, created_by, use_count, patch_count,
            created_at, last_used_at, last_patched_at, archived_at, absorbed_into
         FROM skill_lifecycle
         WHERE state = 'stale'
           AND pinned = 0
           AND created_by IN ('probe_writer', 'curator')
         ORDER BY skill_name",
    )
}

pub fn apply_automatic_transitions(
    conn: &Connection,
    now_utc: DateTime<Utc>,
    config: TransitionConfig,
) -> Result<usize> {
    let stale_cutoff = now_utc - config.stale_after;
    let archive_cutoff = now_utc - config.archive_after;
    let archived_at = now_utc.to_rfc3339();
    let mut changed = 0;

    for row in list(conn)? {
        if should_skip_automatic_transition(&row) {
            continue;
        }

        let Some(activity_at) = latest_activity_at(&row) else {
            continue;
        };

        match row.state {
            LifecycleState::Active if activity_at <= stale_cutoff => {
                changed += conn.execute(
                    "UPDATE skill_lifecycle
                     SET state = ?2
                     WHERE skill_name = ?1 AND state = ?3",
                    params![
                        row.skill_name,
                        LifecycleState::Stale.as_db_str(),
                        LifecycleState::Active.as_db_str()
                    ],
                )?;
            }
            LifecycleState::Stale if activity_at <= archive_cutoff => {
                changed += conn.execute(
                    "UPDATE skill_lifecycle
                     SET state = ?2, archived_at = ?3
                     WHERE skill_name = ?1 AND state = ?4",
                    params![
                        row.skill_name,
                        LifecycleState::Archived.as_db_str(),
                        archived_at,
                        LifecycleState::Stale.as_db_str()
                    ],
                )?;
            }
            _ => {}
        }
    }

    Ok(changed)
}

fn list_where(conn: &Connection, sql: &str) -> Result<Vec<SkillLifecycleRow>> {
    let mut stmt = conn.prepare(sql)?;
    let mut rows = stmt.query([])?;
    let mut lifecycle_rows = Vec::new();
    while let Some(row) = rows.next()? {
        lifecycle_rows.push(row_from_raw(raw_row_from_sql(row)?)?);
    }
    Ok(lifecycle_rows)
}

fn raw_row_from_sql(row: &rusqlite::Row<'_>) -> rusqlite::Result<RawSkillLifecycleRow> {
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

fn should_skip_automatic_transition(row: &SkillLifecycleRow) -> bool {
    row.pinned
        || row.state == LifecycleState::Archived
        || matches!(row.created_by, CreatedBy::Foreground | CreatedBy::Bundled)
}

fn latest_activity_at(row: &SkillLifecycleRow) -> Option<DateTime<Utc>> {
    match (row.last_used_at, row.last_patched_at) {
        (Some(used), Some(patched)) => Some(used.max(patched)),
        (Some(used), None) => Some(used),
        (None, Some(patched)) => Some(patched),
        (None, None) => row.created_at,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeDelta;
    use rusqlite::Connection;

    fn migrated_conn() -> Connection {
        let mut conn = Connection::open_in_memory().unwrap();
        right_db::MIGRATIONS.to_latest(&mut conn).unwrap();
        conn
    }

    fn utc(value: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(value)
            .unwrap()
            .with_timezone(&Utc)
    }

    fn insert_lifecycle_row(
        conn: &Connection,
        skill_name: &str,
        state: LifecycleState,
        pinned: bool,
        created_by: CreatedBy,
        use_count: i64,
        patch_count: i64,
        last_used_at: Option<DateTime<Utc>>,
        last_patched_at: Option<DateTime<Utc>>,
    ) {
        conn.execute(
            "INSERT INTO skill_lifecycle (
                skill_name, state, pinned, created_by, use_count, patch_count,
                created_at, last_used_at, last_patched_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            (
                skill_name,
                state.as_db_str(),
                i64::from(pinned),
                created_by.as_db_str(),
                use_count,
                patch_count,
                utc("2026-04-01T00:00:00Z").to_rfc3339(),
                last_used_at.map(|t| t.to_rfc3339()),
                last_patched_at.map(|t| t.to_rfc3339()),
            ),
        )
        .unwrap();
    }

    #[test]
    fn mark_created_inserts_active_row_with_provenance() {
        let conn = migrated_conn();
        let now = utc("2026-05-23T12:00:00Z");

        mark_created(&conn, "rightx-demo", CreatedBy::ProbeWriter, now).unwrap();

        let row = get(&conn, "rightx-demo").unwrap().unwrap();
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

    #[test]
    fn bump_patch_preserves_existing_created_by() {
        let conn = migrated_conn();
        let now = utc("2026-05-23T12:00:00Z");
        insert_lifecycle_row(
            &conn,
            "rightx-demo",
            LifecycleState::Stale,
            false,
            CreatedBy::Curator,
            0,
            2,
            None,
            None,
        );

        bump_patch(&conn, "rightx-demo", CreatedBy::ProbeWriter, now).unwrap();

        let row = get(&conn, "rightx-demo").unwrap().unwrap();
        assert_eq!(row.state, LifecycleState::Active);
        assert_eq!(row.created_by, CreatedBy::Curator);
        assert_eq!(row.patch_count, 3);
        assert_eq!(row.last_patched_at, Some(now));
    }

    #[test]
    fn bump_use_creates_foreground_row_when_missing() {
        let conn = migrated_conn();
        let now = utc("2026-05-23T12:00:00Z");

        bump_use(&conn, "rightx-demo", now).unwrap();

        let row = get(&conn, "rightx-demo").unwrap().unwrap();
        assert_eq!(row.state, LifecycleState::Active);
        assert_eq!(row.created_by, CreatedBy::Foreground);
        assert_eq!(row.use_count, 1);
        assert_eq!(row.patch_count, 0);
        assert_eq!(row.created_at, Some(now));
        assert_eq!(row.last_used_at, Some(now));
    }

    #[test]
    fn set_pinned_toggles_existing_row() {
        let conn = migrated_conn();
        insert_lifecycle_row(
            &conn,
            "rightx-demo",
            LifecycleState::Active,
            false,
            CreatedBy::Curator,
            0,
            0,
            None,
            None,
        );

        assert!(set_pinned(&conn, "rightx-demo", true).unwrap());
        assert!(get(&conn, "rightx-demo").unwrap().unwrap().pinned);
        assert!(set_pinned(&conn, "rightx-demo", false).unwrap());
        assert!(!get(&conn, "rightx-demo").unwrap().unwrap().pinned);

        assert!(!set_pinned(&conn, "missing", true).unwrap());
        assert!(get(&conn, "missing").unwrap().is_none());
    }

    #[test]
    fn automatic_transitions_skip_pinned_foreground_and_bundled_rows() {
        let conn = migrated_conn();
        let now = utc("2026-05-23T12:00:00Z");
        let old = now - TimeDelta::days(45);
        let stale_old = now - TimeDelta::days(20);
        let recent = now - TimeDelta::days(2);
        let config = TransitionConfig {
            stale_after: TimeDelta::days(7),
            archive_after: TimeDelta::days(30),
        };

        insert_lifecycle_row(
            &conn,
            "candidate-probe",
            LifecycleState::Active,
            false,
            CreatedBy::ProbeWriter,
            0,
            0,
            Some(stale_old),
            None,
        );
        insert_lifecycle_row(
            &conn,
            "old-stale-curator",
            LifecycleState::Stale,
            false,
            CreatedBy::Curator,
            0,
            0,
            Some(old),
            None,
        );
        insert_lifecycle_row(
            &conn,
            "pinned-probe",
            LifecycleState::Active,
            true,
            CreatedBy::ProbeWriter,
            0,
            0,
            Some(old),
            None,
        );
        insert_lifecycle_row(
            &conn,
            "foreground-old",
            LifecycleState::Active,
            false,
            CreatedBy::Foreground,
            0,
            0,
            Some(old),
            None,
        );
        insert_lifecycle_row(
            &conn,
            "bundled-old",
            LifecycleState::Active,
            false,
            CreatedBy::Bundled,
            0,
            0,
            Some(old),
            None,
        );
        insert_lifecycle_row(
            &conn,
            "recent-patch-wins",
            LifecycleState::Active,
            false,
            CreatedBy::Curator,
            0,
            0,
            Some(old),
            Some(recent),
        );

        let updated = apply_automatic_transitions(&conn, now, config).unwrap();

        assert_eq!(updated, 2);
        assert_eq!(
            get(&conn, "candidate-probe").unwrap().unwrap().state,
            LifecycleState::Stale
        );
        let archived = get(&conn, "old-stale-curator").unwrap().unwrap();
        assert_eq!(archived.state, LifecycleState::Archived);
        assert_eq!(archived.archived_at, Some(now));
        for skill_name in [
            "pinned-probe",
            "foreground-old",
            "bundled-old",
            "recent-patch-wins",
        ] {
            assert_eq!(
                get(&conn, skill_name).unwrap().unwrap().state,
                LifecycleState::Active,
                "{skill_name} must not transition"
            );
        }
    }
}
