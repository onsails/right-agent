use chrono::{DateTime, Duration, Utc};
use rusqlite::{Connection, OptionalExtension, params};
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
    let tx = conn.unchecked_transaction()?;
    let mut changed = 0;

    for row in list(&tx)? {
        if should_skip_automatic_transition(&row) {
            continue;
        }

        let Some(activity_at) = latest_activity_at(&row) else {
            continue;
        };

        match row.state {
            LifecycleState::Active if activity_at <= archive_cutoff => {
                changed += transition_candidate_to_archived_if_eligible(
                    &tx,
                    &row.skill_name,
                    LifecycleState::Active,
                    archive_cutoff,
                    now_utc,
                )?;
            }
            LifecycleState::Active if activity_at <= stale_cutoff => {
                changed += transition_active_candidate_to_stale_if_eligible(
                    &tx,
                    &row.skill_name,
                    stale_cutoff,
                )?;
            }
            LifecycleState::Stale if activity_at <= archive_cutoff => {
                changed += transition_candidate_to_archived_if_eligible(
                    &tx,
                    &row.skill_name,
                    LifecycleState::Stale,
                    archive_cutoff,
                    now_utc,
                )?;
            }
            _ => {}
        }
    }

    tx.commit()?;
    Ok(changed)
}

fn transition_active_candidate_to_stale_if_eligible(
    conn: &Connection,
    skill_name: &str,
    stale_cutoff: DateTime<Utc>,
) -> Result<usize> {
    let cutoff = stale_cutoff.to_rfc3339();
    let changed = conn.execute(
        "UPDATE skill_lifecycle
         SET state = :to_state
         WHERE skill_name = :skill_name
           AND state = :from_state
           AND pinned = 0
           AND created_by IN (:probe_writer, :curator)
           AND (
             (
               last_used_at IS NOT NULL
               AND last_patched_at IS NOT NULL
               AND CASE
                 WHEN julianday(last_used_at) >= julianday(last_patched_at)
                 THEN julianday(last_used_at)
                 ELSE julianday(last_patched_at)
               END <= julianday(:cutoff)
             )
             OR (
               last_used_at IS NOT NULL
               AND last_patched_at IS NULL
               AND julianday(last_used_at) <= julianday(:cutoff)
             )
             OR (
               last_used_at IS NULL
               AND last_patched_at IS NOT NULL
               AND julianday(last_patched_at) <= julianday(:cutoff)
             )
           )",
        rusqlite::named_params! {
            ":skill_name": skill_name,
            ":to_state": LifecycleState::Stale.as_db_str(),
            ":from_state": LifecycleState::Active.as_db_str(),
            ":probe_writer": CreatedBy::ProbeWriter.as_db_str(),
            ":curator": CreatedBy::Curator.as_db_str(),
            ":cutoff": cutoff,
        },
    )?;
    Ok(changed)
}

fn transition_candidate_to_archived_if_eligible(
    conn: &Connection,
    skill_name: &str,
    from_state: LifecycleState,
    archive_cutoff: DateTime<Utc>,
    archived_at: DateTime<Utc>,
) -> Result<usize> {
    let cutoff = archive_cutoff.to_rfc3339();
    let archived_at = archived_at.to_rfc3339();
    let changed = conn.execute(
        "UPDATE skill_lifecycle
         SET state = :to_state, archived_at = :archived_at
         WHERE skill_name = :skill_name
           AND state = :from_state
           AND pinned = 0
           AND created_by IN (:probe_writer, :curator)
           AND (
             (
               last_used_at IS NOT NULL
               AND last_patched_at IS NOT NULL
               AND CASE
                 WHEN julianday(last_used_at) >= julianday(last_patched_at)
                 THEN julianday(last_used_at)
                 ELSE julianday(last_patched_at)
               END <= julianday(:cutoff)
             )
             OR (
               last_used_at IS NOT NULL
               AND last_patched_at IS NULL
               AND julianday(last_used_at) <= julianday(:cutoff)
             )
             OR (
               last_used_at IS NULL
               AND last_patched_at IS NOT NULL
               AND julianday(last_patched_at) <= julianday(:cutoff)
             )
           )",
        rusqlite::named_params! {
            ":skill_name": skill_name,
            ":to_state": LifecycleState::Archived.as_db_str(),
            ":archived_at": archived_at,
            ":from_state": from_state.as_db_str(),
            ":probe_writer": CreatedBy::ProbeWriter.as_db_str(),
            ":curator": CreatedBy::Curator.as_db_str(),
            ":cutoff": cutoff,
        },
    )?;
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
        (None, None) => None,
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

    #[test]
    fn lifecycle_enums_serialize_as_snake_case() {
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

        fn insert(self, conn: &Connection) {
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
            .unwrap();
        }
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
        LifecycleRowFixture::new("rightx-demo")
            .state(LifecycleState::Stale)
            .created_by(CreatedBy::Curator)
            .patch_count(2)
            .insert(&conn);

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
        LifecycleRowFixture::new("rightx-demo")
            .created_by(CreatedBy::Curator)
            .insert(&conn);

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

        LifecycleRowFixture::new("candidate-probe")
            .created_by(CreatedBy::ProbeWriter)
            .last_used_at(stale_old)
            .insert(&conn);
        LifecycleRowFixture::new("old-stale-curator")
            .state(LifecycleState::Stale)
            .created_by(CreatedBy::Curator)
            .last_used_at(old)
            .insert(&conn);
        LifecycleRowFixture::new("pinned-probe")
            .pinned(true)
            .created_by(CreatedBy::ProbeWriter)
            .last_used_at(old)
            .insert(&conn);
        LifecycleRowFixture::new("foreground-old")
            .created_by(CreatedBy::Foreground)
            .last_used_at(old)
            .insert(&conn);
        LifecycleRowFixture::new("bundled-old")
            .created_by(CreatedBy::Bundled)
            .last_used_at(old)
            .insert(&conn);
        LifecycleRowFixture::new("recent-patch-wins")
            .created_by(CreatedBy::Curator)
            .last_used_at(old)
            .last_patched_at(recent)
            .insert(&conn);

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

    #[test]
    fn automatic_transitions_leave_rows_without_activity_unchanged() {
        let conn = migrated_conn();
        let now = utc("2026-05-23T12:00:00Z");
        let config = TransitionConfig {
            stale_after: TimeDelta::days(7),
            archive_after: TimeDelta::days(30),
        };
        LifecycleRowFixture::new("created-only-probe")
            .created_by(CreatedBy::ProbeWriter)
            .insert(&conn);

        let updated = apply_automatic_transitions(&conn, now, config).unwrap();

        assert_eq!(updated, 0);
        assert_eq!(
            get(&conn, "created-only-probe").unwrap().unwrap().state,
            LifecycleState::Active
        );
    }

    #[test]
    fn active_rows_older_than_archive_after_archive_directly() {
        let conn = migrated_conn();
        let now = utc("2026-05-23T12:00:00Z");
        let old = now - TimeDelta::days(45);
        let config = TransitionConfig {
            stale_after: TimeDelta::days(7),
            archive_after: TimeDelta::days(30),
        };
        LifecycleRowFixture::new("ancient-active-curator")
            .created_by(CreatedBy::Curator)
            .last_used_at(old)
            .insert(&conn);

        let updated = apply_automatic_transitions(&conn, now, config).unwrap();

        assert_eq!(updated, 1);
        let row = get(&conn, "ancient-active-curator").unwrap().unwrap();
        assert_eq!(row.state, LifecycleState::Archived);
        assert_eq!(row.archived_at, Some(now));
    }

    #[test]
    fn stale_transition_update_rechecks_current_pinned_and_activity() {
        let conn = migrated_conn();
        let now = utc("2026-05-23T12:00:00Z");
        let old = now - TimeDelta::days(20);
        let stale_cutoff = now - TimeDelta::days(7);

        LifecycleRowFixture::new("pinned-after-selection")
            .created_by(CreatedBy::ProbeWriter)
            .last_used_at(old)
            .insert(&conn);
        LifecycleRowFixture::new("used-after-selection")
            .created_by(CreatedBy::Curator)
            .last_used_at(old)
            .insert(&conn);
        let selected = list(&conn).unwrap();
        assert_eq!(selected.len(), 2);

        set_pinned(&conn, "pinned-after-selection", true).unwrap();
        bump_use(&conn, "used-after-selection", now).unwrap();

        assert_eq!(
            transition_active_candidate_to_stale_if_eligible(
                &conn,
                "pinned-after-selection",
                stale_cutoff
            )
            .unwrap(),
            0
        );
        assert_eq!(
            transition_active_candidate_to_stale_if_eligible(
                &conn,
                "used-after-selection",
                stale_cutoff
            )
            .unwrap(),
            0
        );
        assert_eq!(
            get(&conn, "pinned-after-selection").unwrap().unwrap().state,
            LifecycleState::Active
        );
        assert_eq!(
            get(&conn, "used-after-selection").unwrap().unwrap().state,
            LifecycleState::Active
        );
    }

    #[test]
    fn stale_transition_predicate_compares_rfc3339_instants() {
        let conn = migrated_conn();
        let cutoff = utc("2026-05-23T12:00:00Z");
        conn.execute(
            "INSERT INTO skill_lifecycle (
                skill_name, state, pinned, created_by, created_at, last_used_at
             ) VALUES (
                'offset-equivalent', 'active', 0, 'curator',
                '2026-04-01T00:00:00Z', '2026-05-23T13:00:00+01:00'
             )",
            [],
        )
        .unwrap();

        let updated =
            transition_active_candidate_to_stale_if_eligible(&conn, "offset-equivalent", cutoff)
                .unwrap();

        assert_eq!(updated, 1);
        assert_eq!(
            get(&conn, "offset-equivalent").unwrap().unwrap().state,
            LifecycleState::Stale
        );
    }
}
