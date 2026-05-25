use crate::api_types::{
    LearningEpisodeDetailResponse, LearningEpisodeSummary, LearningEpisodesResponse,
    LearningReportSummary, LearningSelectorDetail,
};
use right_db::{Connection, params};

use super::ReadModelError;
use super::learning::{parse_string_array, report_summary_from_row};

const RECENT_EPISODE_LIMIT: i64 = 50;
const REPORTS_PER_EPISODE_LIMIT: i64 = 10;

pub struct LearningEpisodesInput {
    pub agent: String,
    pub generated_at: String,
}

struct EpisodeDetailRow {
    episode: LearningEpisodeSummary,
    selector_model: Option<String>,
    boundary_rationale: Option<String>,
    message_refs: Vec<String>,
    execution_event_refs: Vec<String>,
}

pub fn learning_episodes(
    conn: &Connection,
    input: LearningEpisodesInput,
) -> Result<LearningEpisodesResponse, ReadModelError> {
    let mut stmt = conn.prepare(
        "SELECT id, kind, seed_trigger_kind, seed_ref, status,
                target_chat_id, target_thread_id, start_ref, end_ref,
                confidence, context_incomplete, last_evidence_at, created_at,
                updated_at
         FROM learning_episodes
         WHERE agent_name=?1
         ORDER BY created_at DESC, id DESC
         LIMIT ?2",
    )?;
    let rows = stmt
        .query_map(
            params![input.agent.as_str(), RECENT_EPISODE_LIMIT],
            episode_summary_from_row,
        )?
        .collect::<Result<Vec<_>, _>>()?;
    let mut episodes = Vec::with_capacity(rows.len());
    for mut episode in rows {
        episode.reports = reports_for_episode(conn, &input.agent, episode.id)?;
        episodes.push(episode);
    }

    Ok(LearningEpisodesResponse {
        agent: input.agent,
        generated_at: input.generated_at,
        episodes,
    })
}

pub fn learning_episode_detail(
    conn: &Connection,
    agent: &str,
    episode_id: i64,
) -> Result<Option<LearningEpisodeDetailResponse>, ReadModelError> {
    let Some(mut row) = load_episode_detail(conn, agent, episode_id)? else {
        return Ok(None);
    };
    row.episode.reports = reports_for_episode(conn, agent, episode_id)?;
    let selector = if row.selector_model.is_some()
        || row.boundary_rationale.is_some()
        || !row.message_refs.is_empty()
        || !row.execution_event_refs.is_empty()
    {
        Some(LearningSelectorDetail {
            model: row.selector_model,
            boundary_rationale: row.boundary_rationale,
            selected_message_refs: row.message_refs,
            selected_execution_event_refs: row.execution_event_refs,
        })
    } else {
        None
    };

    Ok(Some(LearningEpisodeDetailResponse {
        episode: row.episode,
        selector,
    }))
}

fn load_episode_detail(
    conn: &Connection,
    agent: &str,
    episode_id: i64,
) -> Result<Option<EpisodeDetailRow>, ReadModelError> {
    let row = match conn.query_row(
        "SELECT id, kind, seed_trigger_kind, seed_ref, status,
                    target_chat_id, target_thread_id, start_ref, end_ref,
                    confidence, context_incomplete, last_evidence_at,
                    created_at, updated_at, selector_model,
                    boundary_rationale, message_refs_json,
                    execution_event_refs_json
             FROM learning_episodes
             WHERE agent_name=?1 AND id=?2",
        params![agent, episode_id],
        |row| {
            Ok((
                episode_summary_from_row(row)?,
                row.get::<_, Option<String>>(14)?,
                row.get::<_, Option<String>>(15)?,
                row.get::<_, String>(16)?,
                row.get::<_, String>(17)?,
            ))
        },
    ) {
        Ok(row) => Some(row),
        Err(right_db::DbError::NotFound) => None,
        Err(error) => return Err(error.into()),
    };

    row.map(
        |(episode, selector_model, boundary_rationale, message_refs_json, execution_refs_json)| {
            Ok(EpisodeDetailRow {
                episode,
                selector_model,
                boundary_rationale,
                message_refs: parse_string_array(&message_refs_json)?,
                execution_event_refs: parse_string_array(&execution_refs_json)?,
            })
        },
    )
    .transpose()
}

fn episode_summary_from_row(
    row: &right_db::row::Row<'_>,
) -> Result<LearningEpisodeSummary, right_db::DbError> {
    Ok(LearningEpisodeSummary {
        id: row.get(0)?,
        kind: row.get(1)?,
        seed_trigger_kind: row.get(2)?,
        seed_ref: row.get(3)?,
        status: row.get(4)?,
        target_chat_id: row.get(5)?,
        target_thread_id: row.get(6)?,
        start_ref: row.get(7)?,
        end_ref: row.get(8)?,
        confidence: row.get(9)?,
        context_incomplete: row.get::<_, i64>(10)? != 0,
        last_evidence_at: row.get(11)?,
        created_at: row.get(12)?,
        updated_at: row.get(13)?,
        reports: Vec::new(),
    })
}

fn reports_for_episode(
    conn: &Connection,
    agent: &str,
    episode_id: i64,
) -> Result<Vec<LearningReportSummary>, ReadModelError> {
    let mut stmt = conn.prepare(
        "SELECT id, status, confidence, trigger_kind, candidate_skill_name,
                candidate_summary, telegram_notified, created_at
         FROM skill_review_reports
         WHERE agent_name=?1 AND learning_episode_id=?2
         ORDER BY created_at DESC, id DESC
         LIMIT ?3",
    )?;
    let rows = stmt
        .query_map(
            params![agent, episode_id, REPORTS_PER_EPISODE_LIMIT],
            report_summary_from_row,
        )?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (tempfile::TempDir, Connection) {
        let dir = tempfile::tempdir().expect("tempdir");
        let conn = right_db::open_connection(dir.path(), true).expect("open db");
        (dir, conn)
    }

    fn input() -> LearningEpisodesInput {
        LearningEpisodesInput {
            agent: "right".to_owned(),
            generated_at: "2026-05-20T12:00:00Z".to_owned(),
        }
    }

    #[test]
    fn learning_episodes_list_links_reports() {
        let (_dir, conn) = fixture();
        conn.execute(
            "INSERT INTO learning_episodes (
                id, agent_name, kind, seed_trigger_kind, seed_ref, status,
                target_chat_id, target_thread_id, start_ref, end_ref,
                message_refs_json, execution_event_refs_json, selector_model,
                selector_output_json, boundary_rationale, confidence,
                context_incomplete, ready_after, last_evidence_at, created_at,
                updated_at
             ) VALUES (
                7, 'right', 'foreground_thread', 'learning_signal', 'inv:inv-1',
                'reviewed', 10, 20, 'msg:101', 'exec:202',
                '[\"msg:101\"]', '[\"exec:202\"]', 'claude-sonnet-4-6',
                '{\"status\":\"selected\"}', 'Selected OAuth setup correction.',
                'high', 0, '2026-05-20T10:01:30Z',
                '2026-05-20T10:01:00Z', '2026-05-20T10:00:00Z',
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
                9, 'right', 'inv-1', 7, 'session-1', 10, 20,
                'learning_signal', 'create_candidate', 'high',
                'rightx-oauth-debugging', 'Verify OAuth callback setup.',
                '[\"msg:101\",\"exec:202\"]',
                '{\"status\":\"create_candidate\",\"confidence\":\"high\",\"evidence_refs\":[\"msg:101\",\"exec:202\"],\"user_notice\":\"notice\"}',
                1, '2026-05-20T11:00:00Z'
             )",
            [],
        )
        .unwrap();

        let response = learning_episodes(&conn, input()).unwrap();

        assert_eq!(response.agent, "right");
        assert_eq!(response.generated_at, "2026-05-20T12:00:00Z");
        assert_eq!(response.episodes.len(), 1);
        assert_eq!(response.episodes[0].id, 7);
        assert_eq!(response.episodes[0].status, "reviewed");
        assert_eq!(response.episodes[0].reports.len(), 1);
        assert_eq!(response.episodes[0].reports[0].id, 9);
        assert_eq!(response.episodes[0].reports[0].status, "create_candidate");
    }

    #[test]
    fn learning_episodes_caps_linked_reports() {
        let (_dir, conn) = fixture();
        conn.execute(
            "INSERT INTO learning_episodes (
                id, agent_name, kind, seed_trigger_kind, seed_ref, status,
                message_refs_json, execution_event_refs_json, selector_output_json,
                ready_after, last_evidence_at, created_at, updated_at
             ) VALUES (
                7, 'right', 'foreground_thread', 'learning_signal', 'inv:inv-1',
                'reviewed', '[]', '[]', '{}', '2026-05-20T10:01:30Z',
                '2026-05-20T10:01:00Z', '2026-05-20T10:00:00Z',
                '2026-05-20T10:02:00Z'
             )",
            [],
        )
        .unwrap();
        for id in 1..=12 {
            conn.execute(
                "INSERT INTO skill_review_reports (
                    id, agent_name, source_invocation_id, learning_episode_id,
                    trigger_kind, status, confidence, evidence_refs_json,
                    review_output_json, created_at
                 ) VALUES (
                    ?1, 'right', ?2, 7, 'learning_signal',
                    'nothing_to_learn', 'medium', '[]', '{}', ?3
                 )",
                right_db::params![
                    id,
                    format!("inv-{id}"),
                    format!("2026-05-20T11:{id:02}:00Z")
                ],
            )
            .unwrap();
        }

        let response = learning_episodes(&conn, input()).unwrap();

        assert_eq!(response.episodes[0].reports.len(), 10);
        assert_eq!(response.episodes[0].reports[0].id, 12);
        assert_eq!(response.episodes[0].reports[9].id, 3);
    }

    #[test]
    fn learning_episode_detail_parses_selector_refs() {
        let (_dir, conn) = fixture();
        conn.execute(
            "INSERT INTO learning_episodes (
                id, agent_name, kind, seed_trigger_kind, seed_ref, status,
                message_refs_json, execution_event_refs_json, selector_model,
                selector_output_json, boundary_rationale, confidence,
                context_incomplete, ready_after, last_evidence_at, created_at,
                updated_at
             ) VALUES (
                8, 'right', 'cron_run', 'cron', 'cron:daily/run-1',
                'reviewed', '[\"msg:101\"]', '[\"exec:202\"]',
                'claude-sonnet-4-6', '{\"status\":\"selected\"}',
                'Selected the completed cron run.', 'medium', 0,
                '2026-05-20T09:01:30Z', '2026-05-20T09:01:00Z',
                '2026-05-20T09:00:00Z', '2026-05-20T09:02:00Z'
             )",
            [],
        )
        .unwrap();

        let response = learning_episode_detail(&conn, "right", 8).unwrap().unwrap();

        assert_eq!(response.episode.id, 8);
        assert_eq!(response.episode.kind, "cron_run");
        assert_eq!(
            response.selector.as_ref().unwrap().selected_message_refs,
            vec!["msg:101"]
        );
        assert_eq!(
            response
                .selector
                .as_ref()
                .unwrap()
                .selected_execution_event_refs,
            vec!["exec:202"]
        );
    }

    #[test]
    fn learning_episode_detail_errors_on_malformed_ref_json() {
        let (_dir, conn) = fixture();
        conn.execute(
            "INSERT INTO learning_episodes (
                id, agent_name, kind, seed_trigger_kind, seed_ref, status,
                message_refs_json, execution_event_refs_json, selector_output_json,
                ready_after, last_evidence_at, created_at, updated_at
             ) VALUES (
                9, 'right', 'foreground_thread', 'learning_signal', 'inv:bad',
                'reviewed', '{malformed', '[]', '{}',
                '2026-05-20T10:01:30Z', '2026-05-20T10:01:00Z',
                '2026-05-20T10:00:00Z', '2026-05-20T10:02:00Z'
             )",
            [],
        )
        .unwrap();

        assert!(learning_episode_detail(&conn, "right", 9).is_err());
    }
}
