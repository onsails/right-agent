use crate::api_types::{
    LearningCapabilities, LearningEventSummary, LearningFunnel, LearningHealth, LearningLifecycle,
    LearningOverviewResponse, LearningQuality, LearningReportSummary,
};
use chrono::{DateTime, Duration, Utc};
use rusqlite::{Connection, OptionalExtension as _, params};

use super::ReadModelError;

pub const LEARNING_REVIEW_DAILY_LIMIT: i64 = 12;
const RECENT_REPORT_LIMIT: i64 = 20;
const RECENT_EVENT_LIMIT: i64 = 10;
const CANDIDATE_NAME_LIMIT: i64 = 20;

pub struct LearningOverviewInput {
    pub agent: String,
    pub generated_at: String,
    pub refresh_interval_secs: u64,
}

fn parse_generated_at(value: &str) -> Result<DateTime<Utc>, ReadModelError> {
    Ok(DateTime::parse_from_rfc3339(value)?.with_timezone(&Utc))
}

fn window_start(generated_at: &str, duration: Duration) -> Result<String, ReadModelError> {
    Ok((parse_generated_at(generated_at)? - duration).to_rfc3339())
}

fn count_since(
    conn: &Connection,
    sql: &str,
    agent: &str,
    since: &str,
) -> Result<i64, ReadModelError> {
    Ok(conn.query_row(sql, params![agent, since], |row| row.get(0))?)
}

fn rate(numerator: i64, denominator: i64) -> Option<f64> {
    if denominator == 0 {
        None
    } else {
        Some(numerator as f64 / denominator as f64)
    }
}

fn learning_capabilities() -> LearningCapabilities {
    LearningCapabilities {
        learning_metrics: true,
        learning_evidence_snippets: true,
        learning_commands: false,
    }
}

pub fn learning_overview(
    conn: &Connection,
    input: LearningOverviewInput,
) -> Result<LearningOverviewResponse, ReadModelError> {
    let since_24h = window_start(&input.generated_at, Duration::hours(24))?;
    let since_7d = window_start(&input.generated_at, Duration::days(7))?;
    let agent_name = input.agent;
    let generated_at = input.generated_at;
    let agent = agent_name.as_str();

    let signals_accepted_24h = count_since(
        conn,
        "SELECT COUNT(*) FROM skill_nudge_signals WHERE agent_name=?1 AND accepted_at >= ?2",
        agent,
        &since_24h,
    )?;

    let episode_count = |status: &str| -> Result<i64, ReadModelError> {
        Ok(conn.query_row(
            "SELECT COUNT(*) FROM learning_episodes
             WHERE agent_name=?1 AND status=?2 AND created_at >= ?3",
            params![agent, status, since_24h],
            |row| row.get(0),
        )?)
    };

    let report_count = |status: &str| -> Result<i64, ReadModelError> {
        Ok(conn.query_row(
            "SELECT COUNT(*) FROM skill_review_reports
             WHERE agent_name=?1 AND status=?2 AND created_at >= ?3",
            params![agent, status, since_24h],
            |row| row.get(0),
        )?)
    };

    let reports_total_24h = count_since(
        conn,
        "SELECT COUNT(*) FROM skill_review_reports WHERE agent_name=?1 AND created_at >= ?2",
        agent,
        &since_24h,
    )?;
    let create_candidates_24h = report_count("create_candidate")?;
    let update_candidates_24h = report_count("update_candidate")?;
    let nothing_to_learn_24h = report_count("nothing_to_learn")?;
    let failed_reviews_24h = report_count("failed")?;
    let non_failed_reports = create_candidates_24h + update_candidates_24h + nothing_to_learn_24h;
    let foreground_created_or_updated_7d = conn.query_row(
        "SELECT COUNT(*) FROM skill_learning_events
         WHERE agent_name=?1
           AND phase='finish'
           AND status IN ('created','updated')
           AND created_at >= ?2",
        params![agent, since_7d],
        |row| row.get(0),
    )?;

    let quality = LearningQuality {
        candidate_rate: rate(
            create_candidates_24h + update_candidates_24h,
            non_failed_reports,
        ),
        nothing_to_learn_rate: rate(nothing_to_learn_24h, non_failed_reports),
        create_count_24h: create_candidates_24h,
        update_count_24h: update_candidates_24h,
        high_confidence_count_24h: confidence_count(conn, agent, &since_24h, "high")?,
        medium_confidence_count_24h: confidence_count(conn, agent, &since_24h, "medium")?,
        low_confidence_count_24h: confidence_count(conn, agent, &since_24h, "low")?,
        failed_count_24h: failed_reviews_24h,
    };

    let health = learning_health(conn, agent, &generated_at)?;
    let lifecycle = learning_lifecycle(conn, agent, &since_7d)?;
    let recent_reports = recent_reports(conn, agent)?;
    let episodes_pending_24h = episode_count("pending")?;
    let episodes_selecting_24h = episode_count("selecting")?;
    let episodes_selected_24h = episode_count("selected")?;
    let episodes_reviewing_24h = episode_count("reviewing")?;
    let episodes_reviewed_24h = episode_count("reviewed")?;
    let episodes_no_episode_24h = episode_count("no_episode")?;
    let episodes_failed_24h = episode_count("failed")?;

    Ok(LearningOverviewResponse {
        agent: agent_name,
        generated_at,
        refresh_interval_secs: input.refresh_interval_secs,
        capabilities: learning_capabilities(),
        funnel: LearningFunnel {
            signals_accepted_24h,
            episodes_pending_24h,
            episodes_selecting_24h,
            episodes_selected_24h,
            episodes_reviewing_24h,
            episodes_reviewed_24h,
            episodes_no_episode_24h,
            episodes_failed_24h,
            reports_total_24h,
            create_candidates_24h,
            update_candidates_24h,
            nothing_to_learn_24h,
            failed_reviews_24h,
            foreground_created_or_updated_7d,
        },
        quality,
        health,
        lifecycle,
        recent_reports,
    })
}

fn confidence_count(
    conn: &Connection,
    agent: &str,
    since: &str,
    confidence: &str,
) -> Result<i64, ReadModelError> {
    Ok(conn.query_row(
        "SELECT COUNT(*) FROM skill_review_reports
         WHERE agent_name=?1 AND confidence=?2 AND created_at >= ?3",
        params![agent, confidence, since],
        |row| row.get(0),
    )?)
}

fn learning_health(
    conn: &Connection,
    agent: &str,
    generated_at: &str,
) -> Result<LearningHealth, ReadModelError> {
    let row = conn
        .query_row(
            "SELECT review_running, daily_review_count, creation_review_interval,
                    tool_iters_since_review, turns_since_review,
                    skill_issue_hints_since_review, last_review_status, last_review_at
             FROM skill_nudge_state WHERE agent_name=?1",
            params![agent],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, Option<String>>(6)?,
                    row.get::<_, Option<String>>(7)?,
                ))
            },
        )
        .optional()?;
    let (
        review_running,
        daily_review_count,
        creation_review_interval,
        tool_iters_since_review,
        turns_since_review,
        skill_issue_hints_since_review,
        last_review_status,
        last_review_at,
    ) = row.unwrap_or((0, 0, 15, 0, 0, 0, None, None));

    Ok(LearningHealth {
        review_running: review_running != 0,
        daily_review_count,
        daily_limit: LEARNING_REVIEW_DAILY_LIMIT,
        creation_review_interval,
        tool_iters_since_review,
        turns_since_review,
        skill_issue_hints_since_review,
        last_review_status,
        last_review_at,
        possibly_stuck: possibly_stuck(conn, agent, generated_at)?,
    })
}

fn possibly_stuck(
    conn: &Connection,
    agent: &str,
    generated_at: &str,
) -> Result<bool, ReadModelError> {
    let cutoff = (parse_generated_at(generated_at)? - Duration::minutes(10)).to_rfc3339();
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM learning_episodes
         WHERE agent_name=?1 AND status='reviewing' AND updated_at < ?2",
        params![agent, cutoff],
        |row| row.get(0),
    )?;
    Ok(count > 0)
}

fn learning_lifecycle(
    conn: &Connection,
    agent: &str,
    since_7d: &str,
) -> Result<LearningLifecycle, ReadModelError> {
    let status_count = |status: &str| -> Result<i64, ReadModelError> {
        Ok(conn.query_row(
            "SELECT COUNT(*) FROM skill_learning_events
             WHERE agent_name=?1 AND phase='finish' AND status=?2 AND created_at >= ?3",
            params![agent, status, since_7d],
            |row| row.get(0),
        )?)
    };
    let failed_or_aborted_7d = conn.query_row(
        "SELECT COUNT(*) FROM skill_learning_events
         WHERE agent_name=?1
           AND phase='finish'
           AND status IN ('failed','aborted')
           AND created_at >= ?2",
        params![agent, since_7d],
        |row| row.get(0),
    )?;

    Ok(LearningLifecycle {
        created_7d: status_count("created")?,
        updated_7d: status_count("updated")?,
        failed_or_aborted_7d,
        recent_successful_events: recent_successful_events(conn, agent, since_7d)?,
        candidate_skill_names_7d: candidate_skill_names(conn, agent, since_7d)?,
    })
}

fn recent_successful_events(
    conn: &Connection,
    agent: &str,
    since_7d: &str,
) -> Result<Vec<LearningEventSummary>, ReadModelError> {
    let mut stmt = conn.prepare(
        "SELECT skill_name, action, status, message, summary, created_at
         FROM skill_learning_events
         WHERE agent_name=?1
           AND phase='finish'
           AND status IN ('created','updated')
           AND created_at >= ?2
         ORDER BY created_at DESC, id DESC
         LIMIT ?3",
    )?;
    let rows = stmt
        .query_map(params![agent, since_7d, RECENT_EVENT_LIMIT], |row| {
            Ok(LearningEventSummary {
                skill_name: row.get(0)?,
                action: row.get(1)?,
                status: row.get(2)?,
                message: row.get(3)?,
                summary: row.get(4)?,
                created_at: row.get(5)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

fn candidate_skill_names(
    conn: &Connection,
    agent: &str,
    since_7d: &str,
) -> Result<Vec<String>, ReadModelError> {
    let mut stmt = conn.prepare(
        "SELECT candidate_skill_name
         FROM skill_review_reports
         WHERE agent_name=?1
           AND candidate_skill_name IS NOT NULL
           AND created_at >= ?2
         GROUP BY candidate_skill_name
         ORDER BY MAX(created_at) DESC
         LIMIT ?3",
    )?;
    let rows = stmt
        .query_map(params![agent, since_7d, CANDIDATE_NAME_LIMIT], |row| {
            row.get(0)
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

fn recent_reports(
    conn: &Connection,
    agent: &str,
) -> Result<Vec<LearningReportSummary>, ReadModelError> {
    let mut stmt = conn.prepare(
        "SELECT id, status, confidence, trigger_kind, candidate_skill_name,
                candidate_summary, telegram_notified, created_at
         FROM skill_review_reports
         WHERE agent_name=?1
         ORDER BY created_at DESC, id DESC
         LIMIT ?2",
    )?;
    let rows = stmt
        .query_map(params![agent, RECENT_REPORT_LIMIT], report_summary_from_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

fn report_summary_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<LearningReportSummary> {
    Ok(LearningReportSummary {
        id: row.get(0)?,
        status: row.get(1)?,
        confidence: row.get(2)?,
        trigger_kind: row.get(3)?,
        candidate_skill_name: row.get(4)?,
        candidate_summary: row.get(5)?,
        telegram_notified: row.get::<_, i64>(6)? != 0,
        created_at: row.get(7)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (tempfile::TempDir, Connection) {
        let dir = tempfile::tempdir().expect("tempdir");
        let conn = right_db::open_connection(dir.path(), true).expect("open db");
        (dir, conn)
    }

    fn input() -> LearningOverviewInput {
        LearningOverviewInput {
            agent: "right".to_owned(),
            generated_at: "2026-05-20T12:00:00Z".to_owned(),
            refresh_interval_secs: 5,
        }
    }

    #[test]
    fn learning_overview_builds_funnel_quality_health_and_lifecycle() {
        let (_dir, conn) = fixture();
        conn.execute(
            "INSERT INTO skill_nudge_state (
                agent_name, tool_iters_since_review, turns_since_review,
                skill_issue_hints_since_review, last_review_at, review_running,
                creation_review_interval, daily_review_count, daily_review_date,
                last_review_status
             ) VALUES (
                'right', 6, 2, 1, '2026-05-20T11:00:00Z', 0,
                15, 4, '2026-05-20', 'nothing_to_learn'
             )",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO skill_nudge_signals (
                invocation_id, agent_name, root_session_id, chat_id, thread_id,
                signal_kind, payload_json, accepted_at
             ) VALUES (
                'inv-1', 'right', 'session-1', 10, 20,
                'learning', '{}', '2026-05-20T10:00:00Z'
             )",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO learning_episodes (
                id, agent_name, kind, seed_trigger_kind, seed_ref, status,
                target_chat_id, target_thread_id, message_refs_json,
                execution_event_refs_json, selector_output_json, ready_after,
                created_at, updated_at
             ) VALUES (
                1, 'right', 'foreground_thread', 'learning_signal', 'inv:inv-1',
                'reviewed', 10, 20, '[]', '[]', '{}',
                '2026-05-20T10:01:30Z', '2026-05-20T10:00:00Z',
                '2026-05-20T10:02:00Z'
             )",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO skill_review_reports (
                id, agent_name, source_invocation_id, learning_episode_id,
                root_session_id, chat_id, thread_id, trigger_kind, status,
                confidence, candidate_skill_name, candidate_summary,
                evidence_refs_json, review_output_json, telegram_notified,
                created_at
             ) VALUES (
                7, 'right', 'inv-1', 1, 'session-1', 10, 20,
                'learning_signal', 'create_candidate', 'high',
                'rightx-oauth-debugging', 'Verify OAuth callback setup.',
                '[\"msg:1\"]',
                '{\"status\":\"create_candidate\",\"confidence\":\"high\",\"evidence_refs\":[\"msg:1\"],\"user_notice\":\"notice\"}',
                1, '2026-05-20T11:00:00Z'
             )",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO skill_learning_events (
                invocation_id, agent_name, action, skill_name, phase, status,
                message, summary, event_refs_json, created_at
             ) VALUES (
                'inv-2', 'right', 'create', 'rightx-oauth-debugging', 'finish',
                'created', 'Learned OAuth callback verification.',
                'Reusable OAuth setup workflow.', '[]',
                '2026-05-20T11:10:00Z'
             )",
            [],
        )
        .unwrap();

        let response = learning_overview(&conn, input()).unwrap();

        assert_eq!(response.funnel.signals_accepted_24h, 1);
        assert_eq!(response.funnel.episodes_reviewed_24h, 1);
        assert_eq!(response.funnel.create_candidates_24h, 1);
        assert_eq!(response.funnel.foreground_created_or_updated_7d, 1);
        assert_eq!(response.quality.candidate_rate, Some(1.0));
        assert_eq!(response.quality.nothing_to_learn_rate, Some(0.0));
        assert!(!response.health.review_running);
        assert_eq!(response.health.daily_review_count, 4);
        assert_eq!(response.lifecycle.created_7d, 1);
        assert_eq!(
            response.lifecycle.candidate_skill_names_7d,
            vec!["rightx-oauth-debugging"]
        );
        assert_eq!(response.recent_reports[0].id, 7);
    }

    #[test]
    fn learning_overview_rates_are_null_without_non_failed_reports() {
        let (_dir, conn) = fixture();
        conn.execute(
            "INSERT INTO skill_review_reports (
                agent_name, source_invocation_id, trigger_kind, status,
                confidence, evidence_refs_json, review_output_json, created_at
             ) VALUES (
                'right', 'inv-1', 'effort_threshold', 'failed',
                'low', '[]', '{}', '2026-05-20T11:00:00Z'
             )",
            [],
        )
        .unwrap();

        let response = learning_overview(&conn, input()).unwrap();

        assert_eq!(response.quality.candidate_rate, None);
        assert_eq!(response.quality.nothing_to_learn_rate, None);
        assert_eq!(response.quality.failed_count_24h, 1);
    }

    #[test]
    fn learning_overview_detects_old_reviewing_episode_as_possibly_stuck() {
        let (_dir, conn) = fixture();
        conn.execute(
            "INSERT INTO skill_nudge_state (
                agent_name, review_running, creation_review_interval,
                daily_review_count
             ) VALUES ('right', 1, 15, 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO learning_episodes (
                agent_name, kind, seed_trigger_kind, seed_ref, status,
                message_refs_json, execution_event_refs_json, ready_after,
                created_at, updated_at
             ) VALUES (
                'right', 'foreground_thread', 'learning_signal', 'inv:stuck',
                'reviewing', '[]', '[]', '2026-05-20T09:00:00Z',
                '2026-05-20T09:00:00Z', '2026-05-20T09:05:00Z'
             )",
            [],
        )
        .unwrap();

        let response = learning_overview(&conn, input()).unwrap();

        assert!(response.health.review_running);
        assert!(response.health.possibly_stuck);
    }
}
