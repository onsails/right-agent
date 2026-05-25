use chrono::{DateTime, Duration, Utc};

fn open_test_conn() -> right_db::Connection {
    let conn = right_db::Connection::open_in_memory().unwrap();
    right_db::MIGRATIONS.to_latest(&conn).unwrap();
    conn
}

fn dt(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
}

#[allow(clippy::too_many_arguments)]
fn insert_skill_lifecycle_row(
    conn: &right_db::Connection,
    skill_name: &str,
    state: right_lifecycle::LifecycleState,
    pinned: bool,
    created_by: right_lifecycle::CreatedBy,
    use_count: i64,
    patch_count: i64,
    last_used_at: Option<&str>,
    last_patched_at: Option<&str>,
) {
    conn.execute(
        "INSERT INTO skill_lifecycle (
            skill_name, state, pinned, created_by, use_count, patch_count,
            created_at, last_used_at, last_patched_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, '2026-01-01T00:00:00Z', ?7, ?8)",
        right_db::params![
            skill_name,
            state.as_db_str(),
            if pinned { 1 } else { 0 },
            created_by.as_db_str(),
            use_count,
            patch_count,
            last_used_at,
            last_patched_at,
        ],
    )
    .unwrap();
}

#[test]
fn curator_lifecycle_pinned_skills_are_skipped_by_automatic_transitions() {
    let conn = open_test_conn();
    let now = dt("2026-05-24T00:00:00Z");
    insert_skill_lifecycle_row(
        &conn,
        "rightx-pinned-old",
        right_lifecycle::LifecycleState::Active,
        true,
        right_lifecycle::CreatedBy::ProbeWriter,
        0,
        0,
        Some("2026-01-01T00:00:00Z"),
        None,
    );

    let changed = crate::lifecycle::transitions::apply_automatic_transitions(
        &conn,
        now,
        crate::lifecycle::transitions::TransitionConfig {
            stale_after: Duration::days(30),
            archive_after: Duration::days(90),
        },
    )
    .unwrap();

    assert_eq!(changed, 0);
    let row = right_lifecycle::get(&conn, "rightx-pinned-old")
        .unwrap()
        .unwrap();
    assert_eq!(row.state, right_lifecycle::LifecycleState::Active);
}

#[test]
fn curator_lifecycle_foreground_bundled_pinned_and_archived_rows_are_not_candidates() {
    let conn = open_test_conn();
    for (skill_name, state, pinned, created_by) in [
        (
            "rightx-probe",
            right_lifecycle::LifecycleState::Stale,
            false,
            right_lifecycle::CreatedBy::ProbeWriter,
        ),
        (
            "rightx-curator",
            right_lifecycle::LifecycleState::Stale,
            false,
            right_lifecycle::CreatedBy::Curator,
        ),
        (
            "rightx-foreground",
            right_lifecycle::LifecycleState::Stale,
            false,
            right_lifecycle::CreatedBy::Foreground,
        ),
        (
            "rightx-bundled",
            right_lifecycle::LifecycleState::Stale,
            false,
            right_lifecycle::CreatedBy::Bundled,
        ),
        (
            "rightx-pinned",
            right_lifecycle::LifecycleState::Stale,
            true,
            right_lifecycle::CreatedBy::ProbeWriter,
        ),
        (
            "rightx-archived",
            right_lifecycle::LifecycleState::Archived,
            false,
            right_lifecycle::CreatedBy::Curator,
        ),
    ] {
        insert_skill_lifecycle_row(
            &conn,
            skill_name,
            state,
            pinned,
            created_by,
            0,
            0,
            Some("2026-05-01T00:00:00Z"),
            None,
        );
    }

    let names: Vec<_> = right_lifecycle::list_curator_candidates(&conn)
        .unwrap()
        .into_iter()
        .map(|row| row.skill_name)
        .collect();

    assert_eq!(names, vec!["rightx-curator", "rightx-probe"]);
}

#[test]
fn curator_lifecycle_probe_writer_and_curator_rows_can_transition() {
    let conn = open_test_conn();
    let now = dt("2026-05-24T00:00:00Z");
    for (skill_name, state, created_by, last_used_at) in [
        (
            "rightx-active-probe",
            right_lifecycle::LifecycleState::Active,
            right_lifecycle::CreatedBy::ProbeWriter,
            "2026-04-01T00:00:00Z",
        ),
        (
            "rightx-active-curator",
            right_lifecycle::LifecycleState::Active,
            right_lifecycle::CreatedBy::Curator,
            "2026-04-01T00:00:00Z",
        ),
        (
            "rightx-stale-probe",
            right_lifecycle::LifecycleState::Stale,
            right_lifecycle::CreatedBy::ProbeWriter,
            "2026-01-01T00:00:00Z",
        ),
        (
            "rightx-stale-curator",
            right_lifecycle::LifecycleState::Stale,
            right_lifecycle::CreatedBy::Curator,
            "2026-01-01T00:00:00Z",
        ),
    ] {
        insert_skill_lifecycle_row(
            &conn,
            skill_name,
            state,
            false,
            created_by,
            0,
            0,
            Some(last_used_at),
            None,
        );
    }

    let changed = crate::lifecycle::transitions::apply_automatic_transitions(
        &conn,
        now,
        crate::lifecycle::transitions::TransitionConfig {
            stale_after: Duration::days(30),
            archive_after: Duration::days(90),
        },
    )
    .unwrap();

    assert_eq!(changed, 4);
    for skill_name in ["rightx-active-probe", "rightx-active-curator"] {
        assert_eq!(
            right_lifecycle::get(&conn, skill_name)
                .unwrap()
                .unwrap()
                .state,
            right_lifecycle::LifecycleState::Stale
        );
    }
    for skill_name in ["rightx-stale-probe", "rightx-stale-curator"] {
        assert_eq!(
            right_lifecycle::get(&conn, skill_name)
                .unwrap()
                .unwrap()
                .state,
            right_lifecycle::LifecycleState::Archived
        );
    }
}

#[test]
fn curator_lifecycle_candidate_rendering_includes_db_status_fields() {
    let conn = open_test_conn();
    insert_skill_lifecycle_row(
        &conn,
        "rightx-rendered",
        right_lifecycle::LifecycleState::Stale,
        false,
        right_lifecycle::CreatedBy::ProbeWriter,
        7,
        3,
        Some("2026-05-01T00:00:00Z"),
        Some("2026-05-03T00:00:00Z"),
    );
    let candidates = right_lifecycle::list_curator_candidates(&conn).unwrap();

    let rendered = super::render_candidate_list(&candidates);

    assert!(rendered.contains(
        "- rightx-rendered: state=Stale pinned=false use_count=7 patch_count=3 latest_activity=2026-05-03T00:00:00Z"
    ));
}
