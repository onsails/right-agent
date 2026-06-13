use chrono::{DateTime, Duration, Utc};

async fn open_test_conn() -> right_db::Connection {
    let conn = right_db::Connection::open_in_memory().await.unwrap();
    right_db::MIGRATIONS.to_latest(&conn).await.unwrap();
    conn
}

fn dt(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
}

#[allow(clippy::too_many_arguments)]
async fn insert_skill_lifecycle_row(
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
    .await
    .unwrap();
}

#[tokio::test]
async fn curator_lifecycle_pinned_skills_are_skipped_by_automatic_transitions() {
    let conn = open_test_conn().await;
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
    )
    .await;

    let changed = crate::lifecycle::transitions::apply_automatic_transitions(
        &conn,
        now,
        crate::lifecycle::transitions::TransitionConfig {
            stale_after: Duration::days(30),
            archive_after: Duration::days(90),
        },
    )
    .await
    .unwrap();

    assert_eq!(changed, 0);
    let row = right_lifecycle::get(&conn, "rightx-pinned-old")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.state, right_lifecycle::LifecycleState::Active);
}

#[tokio::test]
async fn curator_lifecycle_bundled_pinned_and_archived_rows_are_not_candidates() {
    // foreground (and cron) rows ARE candidates now (decision A: inline-authored
    // skills are curator-auto-managed). Only bundled, pinned, and archived/
    // non-stale rows are excluded.
    let conn = open_test_conn().await;
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
        )
        .await;
    }

    let names: Vec<_> = right_lifecycle::list_curator_candidates(&conn)
        .await
        .unwrap()
        .into_iter()
        .map(|row| row.skill_name)
        .collect();

    assert_eq!(
        names,
        vec!["rightx-curator", "rightx-foreground", "rightx-probe"]
    );
}

#[tokio::test]
async fn curator_lifecycle_probe_writer_and_curator_rows_can_transition() {
    let conn = open_test_conn().await;
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
        )
        .await;
    }

    let changed = crate::lifecycle::transitions::apply_automatic_transitions(
        &conn,
        now,
        crate::lifecycle::transitions::TransitionConfig {
            stale_after: Duration::days(30),
            archive_after: Duration::days(90),
        },
    )
    .await
    .unwrap();

    assert_eq!(changed, 4);
    for skill_name in ["rightx-active-probe", "rightx-active-curator"] {
        assert_eq!(
            right_lifecycle::get(&conn, skill_name)
                .await
                .unwrap()
                .unwrap()
                .state,
            right_lifecycle::LifecycleState::Stale
        );
    }
    for skill_name in ["rightx-stale-probe", "rightx-stale-curator"] {
        assert_eq!(
            right_lifecycle::get(&conn, skill_name)
                .await
                .unwrap()
                .unwrap()
                .state,
            right_lifecycle::LifecycleState::Archived
        );
    }
}

#[tokio::test]
async fn curator_lifecycle_candidate_rendering_includes_db_status_fields() {
    let conn = open_test_conn().await;
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
    )
    .await;
    let candidates = right_lifecycle::list_curator_candidates(&conn)
        .await
        .unwrap();

    let rendered = super::render_candidate_list(&candidates);

    assert!(rendered.contains(
        "- rightx-rendered: state=Stale pinned=false use_count=7 patch_count=3 latest_activity=2026-05-03T00:00:00Z"
    ));
}

#[tokio::test]
async fn curator_maintain_spend_split_evenly_across_archived_skills() {
    let conn = open_test_conn().await;
    let now = dt("2026-05-24T00:00:00Z");

    // Two stale rows old enough to archive (last_used 143 days before now,
    // archive_after=90 days) and an active row that will only become stale.
    insert_skill_lifecycle_row(
        &conn,
        "rightx-archive-a",
        right_lifecycle::LifecycleState::Stale,
        false,
        right_lifecycle::CreatedBy::ProbeWriter,
        0,
        0,
        Some("2026-01-01T00:00:00Z"), // 143 days before now
        None,
    )
    .await;
    insert_skill_lifecycle_row(
        &conn,
        "rightx-archive-b",
        right_lifecycle::LifecycleState::Stale,
        false,
        right_lifecycle::CreatedBy::Curator,
        0,
        0,
        Some("2026-01-01T00:00:00Z"), // 143 days before now
        None,
    )
    .await;
    insert_skill_lifecycle_row(
        &conn,
        "rightx-only-stale",
        right_lifecycle::LifecycleState::Active,
        false,
        right_lifecycle::CreatedBy::Curator,
        0,
        0,
        Some("2026-05-01T00:00:00Z"), // 23 days before now — stales, not archives
        None,
    )
    .await;

    let _changed = crate::lifecycle::transitions::apply_automatic_transitions(
        &conn,
        now,
        crate::lifecycle::transitions::TransitionConfig {
            stale_after: Duration::days(14),
            archive_after: Duration::days(90),
        },
    )
    .await
    .unwrap();

    // Sanity: confirm exactly the two skills archived at `now`.
    let archived: Vec<String> = conn
        .query_all(
            "SELECT skill_name FROM skill_lifecycle WHERE archived_at = ?1 ORDER BY skill_name",
            right_db::params![now.to_rfc3339()],
            |r| r.get::<_, String>(0),
        )
        .await
        .unwrap();
    assert_eq!(archived, vec!["rightx-archive-a", "rightx-archive-b"]);

    // Known pass cost C and cache, split evenly across N=2 archived skills.
    let total_cost_usd = 0.042_f64;
    let cache_creation_tokens = 11_u64; // 11/2 = 5 after integer division
    let cache_read_tokens = 21_u64; // 21/2 = 10 after integer division
    let b = right_agent::usage::UsageBreakdown {
        session_uuid: "test-session".to_string(),
        total_cost_usd,
        num_turns: 1,
        input_tokens: 100,
        output_tokens: 50,
        cache_creation_tokens,
        cache_read_tokens,
        web_search_requests: 0,
        web_fetch_requests: 0,
        model_usage_json: "{}".to_string(),
        api_key_source: "none".to_string(),
        wall_elapsed_ms: None,
    };

    super::record_curator_maintain_spend(&conn, &archived, &b, Some("inv-curator")).await;

    let n = archived.len() as f64;
    let expected_cost = total_cost_usd / n; // C/N
    let expected_cache_read = (cache_read_tokens / archived.len() as u64) as i64; // cache/N
    let expected_cache_creation = (cache_creation_tokens / archived.len() as u64) as i64;

    // Exactly N maintain rows, one per archived skill, each carrying C/N.
    let rows: Vec<(String, f64, i64, i64, Option<String>)> = conn
        .query_all(
            "SELECT skill_name, cost_usd, cache_read, cache_creation, invocation_id \
             FROM skill_spend WHERE kind='maintain' ORDER BY skill_name",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .await
        .unwrap();
    assert_eq!(rows.len(), 2, "expected exactly N=2 maintain rows");
    for (i, name) in ["rightx-archive-a", "rightx-archive-b"].iter().enumerate() {
        let (skill, cost, cr, cc, inv) = &rows[i];
        assert_eq!(skill, name);
        assert!(
            (cost - expected_cost).abs() < 1e-9,
            "cost_usd mismatch for {skill}: {cost} != {expected_cost}"
        );
        assert_eq!(*cr, expected_cache_read);
        assert_eq!(*cc, expected_cache_creation);
        assert_eq!(inv.as_deref(), Some("inv-curator"));
    }

    // Summing the maintain rows recovers the exact pass cost.
    let summed: f64 = conn
        .query_row(
            "SELECT COALESCE(SUM(cost_usd),0) FROM skill_spend WHERE kind='maintain'",
            [],
            |r| r.get(0),
        )
        .await
        .unwrap();
    assert!(
        (summed - total_cost_usd).abs() < 1e-9,
        "summed maintain cost {summed} != pass cost {total_cost_usd}"
    );

    // The 'only-stale' skill must not have a maintain row.
    let count2: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM skill_spend WHERE kind='maintain' AND skill_name='rightx-only-stale'",
            [],
            |r| r.get(0),
        )
        .await
        .unwrap();
    assert_eq!(count2, 0);
}
