use crate::api_types::{
    LearningCapabilities, LearningEpisodeDetail, LearningEventSummary, LearningEvidenceSnippet,
    LearningFunnel, LearningHealth, LearningLifecycle, LearningOverviewResponse, LearningQuality,
    LearningReportDetailResponse, LearningReportSummary, LearningReviewerDetail,
    LearningSelectorDetail,
};
use std::collections::HashSet;

use chrono::{DateTime, Duration, Utc};
use rusqlite::{Connection, OptionalExtension as _, params};

use super::ReadModelError;

pub const LEARNING_REVIEW_DAILY_LIMIT: i64 = 12;
const RECENT_REPORT_LIMIT: i64 = 20;
const RECENT_EVENT_LIMIT: i64 = 10;
const CANDIDATE_NAME_LIMIT: i64 = 20;
const EVIDENCE_SNIPPET_TEXT_MAX_CHARS: usize = 320;
const EVIDENCE_SNIPPET_LIMIT: usize = 24;

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
    let episodes_insufficient_context_24h = episode_count("insufficient_context")?;
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
            episodes_insufficient_context_24h,
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
         WHERE agent_name=?1 AND status='reviewing' AND updated_at < ?2
           AND COALESCE(
                 (SELECT review_running FROM skill_nudge_state WHERE agent_name=?1),
                 0
               ) = 0",
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
           AND status IN ('create_candidate','update_candidate')
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

pub(super) fn report_summary_from_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<LearningReportSummary> {
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

struct ReportDetailRow {
    report: LearningReportSummary,
    learning_episode_id: Option<i64>,
    evidence_refs: Vec<String>,
    review_output_json: serde_json::Value,
}

struct EpisodeDetailRow {
    episode: LearningEpisodeDetail,
    selector: LearningSelectorDetail,
    message_refs: Vec<String>,
    execution_event_refs: Vec<String>,
}

pub fn learning_report_detail(
    conn: &Connection,
    agent: &str,
    report_id: i64,
) -> Result<Option<LearningReportDetailResponse>, ReadModelError> {
    let Some(report_row) = load_report_detail_row(conn, agent, report_id)? else {
        return Ok(None);
    };
    let episode_row = match report_row.learning_episode_id {
        Some(episode_id) => load_episode_detail_row(conn, agent, episode_id)?,
        None => None,
    };
    let allowed_message_refs = episode_row
        .as_ref()
        .map(|row| row.message_refs.as_slice())
        .unwrap_or(&[]);
    let allowed_execution_refs = episode_row
        .as_ref()
        .map(|row| row.execution_event_refs.as_slice())
        .unwrap_or(&[]);
    let evidence_refs = if report_row.evidence_refs.is_empty() {
        episode_row
            .as_ref()
            .map(|row| {
                row.message_refs
                    .iter()
                    .chain(row.execution_event_refs.iter())
                    .take(EVIDENCE_SNIPPET_LIMIT)
                    .cloned()
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    } else {
        report_row
            .evidence_refs
            .iter()
            .take(EVIDENCE_SNIPPET_LIMIT)
            .cloned()
            .collect()
    };
    let evidence = load_evidence_snippets(
        conn,
        agent,
        &evidence_refs,
        allowed_message_refs,
        allowed_execution_refs,
    )?;
    let reviewer = reviewer_detail(&report_row);

    Ok(Some(LearningReportDetailResponse {
        report: report_row.report,
        episode: episode_row.as_ref().map(|row| row.episode.clone()),
        selector: episode_row.map(|row| row.selector),
        evidence,
        reviewer,
    }))
}

fn load_report_detail_row(
    conn: &Connection,
    agent: &str,
    report_id: i64,
) -> Result<Option<ReportDetailRow>, ReadModelError> {
    let row = conn
        .query_row(
            "SELECT id, status, confidence, trigger_kind, candidate_skill_name,
                    candidate_summary, telegram_notified, created_at,
                    learning_episode_id, evidence_refs_json, review_output_json
             FROM skill_review_reports
             WHERE agent_name=?1 AND id=?2",
            params![agent, report_id],
            |row| {
                Ok((
                    report_summary_from_row(row)?,
                    row.get::<_, Option<i64>>(8)?,
                    row.get::<_, String>(9)?,
                    row.get::<_, String>(10)?,
                ))
            },
        )
        .optional()?;
    row.map(
        |(report, learning_episode_id, evidence_refs_json, review_output_json)| {
            Ok(ReportDetailRow {
                report,
                learning_episode_id,
                evidence_refs: parse_string_array(&evidence_refs_json)?,
                review_output_json: serde_json::from_str(&review_output_json)?,
            })
        },
    )
    .transpose()
}

fn load_episode_detail_row(
    conn: &Connection,
    agent: &str,
    episode_id: i64,
) -> Result<Option<EpisodeDetailRow>, ReadModelError> {
    let row = conn
        .query_row(
            "SELECT id, kind, seed_trigger_kind, status, start_ref, end_ref,
                    message_refs_json, execution_event_refs_json, selector_model,
                    selector_output_json, boundary_rationale, confidence,
                    context_incomplete
             FROM learning_episodes
             WHERE agent_name=?1 AND id=?2",
            params![agent, episode_id],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, Option<String>>(8)?,
                    row.get::<_, Option<String>>(9)?,
                    row.get::<_, Option<String>>(10)?,
                    row.get::<_, Option<String>>(11)?,
                    row.get::<_, i64>(12)?,
                ))
            },
        )
        .optional()?;
    row.map(
        |(
            id,
            kind,
            seed_trigger_kind,
            status,
            start_ref,
            end_ref,
            message_refs_json,
            execution_event_refs_json,
            selector_model,
            _selector_output_json,
            boundary_rationale,
            confidence,
            context_incomplete,
        )| {
            let message_refs = parse_string_array(&message_refs_json)?;
            let execution_event_refs = parse_string_array(&execution_event_refs_json)?;
            Ok(EpisodeDetailRow {
                episode: LearningEpisodeDetail {
                    id,
                    kind,
                    seed_trigger_kind,
                    status,
                    start_ref,
                    end_ref,
                    boundary_rationale: boundary_rationale.clone(),
                    confidence: confidence.clone(),
                    context_incomplete: context_incomplete != 0,
                },
                selector: LearningSelectorDetail {
                    model: selector_model,
                    boundary_rationale,
                    selected_message_refs: message_refs.clone(),
                    selected_execution_event_refs: execution_event_refs.clone(),
                },
                message_refs,
                execution_event_refs,
            })
        },
    )
    .transpose()
}

fn reviewer_detail(row: &ReportDetailRow) -> LearningReviewerDetail {
    let user_notice_present = row
        .review_output_json
        .get("user_notice")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .is_some_and(|value| !value.is_empty());
    LearningReviewerDetail {
        status: row.report.status.clone(),
        confidence: row.report.confidence.clone(),
        candidate_skill_name: row.report.candidate_skill_name.clone(),
        candidate_summary: row.report.candidate_summary.clone(),
        evidence_refs: row.evidence_refs.clone(),
        user_notice_present,
    }
}

fn load_evidence_snippets(
    conn: &Connection,
    agent: &str,
    refs: &[String],
    allowed_message_refs: &[String],
    allowed_execution_refs: &[String],
) -> Result<Vec<LearningEvidenceSnippet>, ReadModelError> {
    let allowed_messages: HashSet<&str> = allowed_message_refs.iter().map(String::as_str).collect();
    let allowed_executions: HashSet<&str> =
        allowed_execution_refs.iter().map(String::as_str).collect();
    let mut snippets = Vec::with_capacity(refs.len());
    for ref_id in refs {
        if ref_id.starts_with("msg:") {
            if !allowed_messages.contains(ref_id.as_str()) {
                snippets.push(unavailable_snippet(ref_id.clone(), "message"));
                continue;
            }
            snippets.push(load_message_snippet(conn, ref_id)?);
        } else if ref_id.starts_with("exec:") {
            if !allowed_executions.contains(ref_id.as_str()) {
                snippets.push(unavailable_snippet(ref_id.clone(), "execution_event"));
                continue;
            }
            snippets.push(load_execution_snippet(conn, agent, ref_id)?);
        } else {
            snippets.push(unavailable_snippet(ref_id.clone(), "unknown"));
        }
    }
    Ok(snippets)
}

fn load_message_snippet(
    conn: &Connection,
    ref_id: &str,
) -> Result<LearningEvidenceSnippet, ReadModelError> {
    let Some(id) = parse_ref_id(ref_id, "msg:") else {
        return Ok(unavailable_snippet(ref_id.to_owned(), "message"));
    };
    let row = conn
        .query_row(
            "SELECT role, content, created_at, addressed_to_bot, routed_to_agent
             FROM conversation_messages
             WHERE id=?1 AND role IN ('user','assistant')",
            params![id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)?,
                ))
            },
        )
        .optional()?;
    let Some((role, content, created_at, addressed_to_bot, routed_to_agent)) = row else {
        return Ok(unavailable_snippet(ref_id.to_owned(), "message"));
    };
    if addressed_to_bot == 0 && routed_to_agent == 0 {
        return Ok(unavailable_snippet(ref_id.to_owned(), "message"));
    }
    Ok(LearningEvidenceSnippet {
        ref_id: ref_id.to_owned(),
        source: "message".to_owned(),
        available: true,
        trust_label: Some("primary".to_owned()),
        role: Some(role),
        event_kind: None,
        tool_name: None,
        created_at: Some(created_at),
        text: Some(bounded_text(content)),
    })
}

fn load_execution_snippet(
    conn: &Connection,
    agent: &str,
    ref_id: &str,
) -> Result<LearningEvidenceSnippet, ReadModelError> {
    let Some(id) = parse_ref_id(ref_id, "exec:") else {
        return Ok(unavailable_snippet(ref_id.to_owned(), "execution_event"));
    };
    let row = conn
        .query_row(
            "SELECT event_kind, tool_name, content_text, trust_label, created_at
             FROM execution_events
             WHERE agent_name=?1 AND id=?2",
            params![agent, id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                ))
            },
        )
        .optional()?;
    let Some((event_kind, tool_name, content_text, trust_label, created_at)) = row else {
        return Ok(unavailable_snippet(ref_id.to_owned(), "execution_event"));
    };
    if event_kind == "thinking" || trust_label == "low_trust" {
        return Ok(unavailable_snippet(ref_id.to_owned(), "execution_event"));
    }
    Ok(LearningEvidenceSnippet {
        ref_id: ref_id.to_owned(),
        source: "execution_event".to_owned(),
        available: true,
        trust_label: Some(trust_label),
        role: None,
        event_kind: Some(event_kind),
        tool_name,
        created_at: Some(created_at),
        text: Some(bounded_text(content_text)),
    })
}

pub(super) fn parse_string_array(raw: &str) -> Result<Vec<String>, ReadModelError> {
    Ok(serde_json::from_str(raw)?)
}

fn bounded_text(value: String) -> String {
    let mut chars = value.chars();
    let mut out = chars
        .by_ref()
        .take(EVIDENCE_SNIPPET_TEXT_MAX_CHARS)
        .collect::<String>();
    if chars.next().is_some() {
        out.push_str("... [truncated]");
    }
    out
}

fn parse_ref_id(reference: &str, prefix: &str) -> Option<i64> {
    reference.strip_prefix(prefix)?.parse::<i64>().ok()
}

fn unavailable_snippet(ref_id: String, source: &str) -> LearningEvidenceSnippet {
    LearningEvidenceSnippet {
        ref_id,
        source: source.to_owned(),
        available: false,
        trust_label: None,
        role: None,
        event_kind: None,
        tool_name: None,
        created_at: None,
        text: None,
    }
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
             ) VALUES ('right', 0, 15, 1)",
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

        assert!(!response.health.review_running);
        assert!(response.health.possibly_stuck);
    }

    #[test]
    fn learning_overview_does_not_flag_stuck_while_reviewer_running() {
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
        assert!(!response.health.possibly_stuck);
    }

    #[test]
    fn learning_overview_candidate_names_include_only_candidate_reports() {
        let (_dir, conn) = fixture();
        conn.execute(
            "INSERT INTO skill_review_reports (
                agent_name, source_invocation_id, trigger_kind, status,
                confidence, candidate_skill_name, evidence_refs_json,
                review_output_json, created_at
             ) VALUES (
                'right', 'inv-1', 'learning_signal', 'create_candidate',
                'high', 'rightx-valid-candidate', '[]', '{}',
                '2026-05-20T11:00:00Z'
             )",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO skill_review_reports (
                agent_name, source_invocation_id, trigger_kind, status,
                confidence, candidate_skill_name, evidence_refs_json,
                review_output_json, created_at
             ) VALUES (
                'right', 'inv-2', 'effort_threshold', 'nothing_to_learn',
                'medium', 'rightx-not-a-candidate', '[]', '{}',
                '2026-05-20T11:30:00Z'
             )",
            [],
        )
        .unwrap();

        let response = learning_overview(&conn, input()).unwrap();

        assert_eq!(
            response.lifecycle.candidate_skill_names_7d,
            vec!["rightx-valid-candidate"]
        );
    }

    #[test]
    fn learning_overview_counts_insufficient_context_episodes() {
        let (_dir, conn) = fixture();
        conn.execute(
            "INSERT INTO learning_episodes (
                agent_name, kind, seed_trigger_kind, seed_ref, status,
                message_refs_json, execution_event_refs_json, ready_after,
                created_at, updated_at
             ) VALUES (
                'right', 'foreground_thread', 'learning_signal', 'inv:context',
                'insufficient_context', '[]', '[]', '2026-05-20T09:00:00Z',
                '2026-05-20T09:00:00Z', '2026-05-20T09:05:00Z'
             )",
            [],
        )
        .unwrap();

        let response = learning_overview(&conn, input()).unwrap();

        assert_eq!(response.funnel.episodes_insufficient_context_24h, 1);
    }

    #[test]
    fn learning_report_detail_returns_message_and_execution_snippets() {
        let (_dir, conn) = fixture();
        conn.execute(
            "INSERT INTO conversation_messages (
                id, platform, chat_id, thread_id, message_id, role, content,
                root_session_id, turn_id, routed_to_agent, created_at
             ) VALUES (
                101, 'telegram', 10, 20, 77, 'user',
                'Verify the OAuth callback URL before retrying auth.',
                'session-1', 3, 1, '2026-05-20T10:00:00Z'
             )",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO execution_events (
                id, agent_name, root_session_id, invocation_id, turn_id, seq,
                event_kind, tool_name, content_json, content_text, trust_label,
                created_at
             ) VALUES (
                202, 'right', 'session-1', 'inv-1', 3, 9,
                'tool_result', 'shell', '{}', 'callback verified', 'primary',
                '2026-05-20T10:01:00Z'
             )",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO learning_episodes (
                id, agent_name, kind, seed_trigger_kind, seed_ref, status,
                target_chat_id, target_thread_id, start_ref, end_ref,
                message_refs_json, execution_event_refs_json, selector_model,
                selector_output_json, boundary_rationale, confidence,
                context_incomplete, ready_after, created_at, updated_at
             ) VALUES (
                4, 'right', 'foreground_thread', 'learning_signal', 'inv:inv-1',
                'reviewed', 10, 20, 'msg:101', 'exec:202',
                '[\"msg:101\"]', '[\"exec:202\"]', 'claude-sonnet-4-6',
                '{\"status\":\"selected\"}', 'Selected OAuth setup correction.',
                'high', 0, '2026-05-20T10:01:30Z',
                '2026-05-20T10:00:00Z', '2026-05-20T10:02:00Z'
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
                9, 'right', 'inv-1', 4, 'session-1', 10, 20,
                'learning_signal', 'create_candidate', 'high',
                'rightx-oauth-debugging', 'Verify OAuth callback setup.',
                '[\"msg:101\",\"exec:202\"]',
                '{\"status\":\"create_candidate\",\"confidence\":\"high\",\"candidate_skill_name\":\"rightx-oauth-debugging\",\"candidate_summary\":\"Verify OAuth callback setup.\",\"evidence_refs\":[\"msg:101\",\"exec:202\"],\"user_notice\":\"notice\"}',
                1, '2026-05-20T11:00:00Z'
             )",
            [],
        )
        .unwrap();

        let detail = learning_report_detail(&conn, "right", 9).unwrap().unwrap();

        assert_eq!(detail.report.id, 9);
        assert_eq!(detail.episode.as_ref().unwrap().id, 4);
        assert_eq!(
            detail.selector.as_ref().unwrap().selected_message_refs,
            vec!["msg:101"]
        );
        assert_eq!(detail.evidence.len(), 2);
        assert_eq!(detail.evidence[0].source, "message");
        assert_eq!(
            detail.evidence[0].text.as_deref(),
            Some("Verify the OAuth callback URL before retrying auth.")
        );
        assert_eq!(detail.evidence[1].source, "execution_event");
        assert_eq!(
            detail.evidence[1].event_kind.as_deref(),
            Some("tool_result")
        );
        assert!(detail.reviewer.user_notice_present);
    }

    #[test]
    fn learning_report_detail_marks_missing_refs_unavailable_and_hides_thinking() {
        let (_dir, conn) = fixture();
        conn.execute(
            "INSERT INTO execution_events (
                id, agent_name, root_session_id, invocation_id, turn_id, seq,
                event_kind, content_json, content_text, trust_label, created_at
             ) VALUES (
                303, 'right', 'session-1', 'inv-2', 5, 1,
                'thinking', '{}', 'private reasoning', 'secondary',
                '2026-05-20T10:01:00Z'
             )",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO learning_episodes (
                id, agent_name, kind, seed_trigger_kind, seed_ref, status,
                message_refs_json, execution_event_refs_json, selector_output_json,
                ready_after, created_at, updated_at
             ) VALUES (
                5, 'right', 'foreground_thread', 'effort_threshold', 'inv:inv-2',
                'reviewed', '[\"msg:404\"]', '[\"exec:303\"]', '{}',
                '2026-05-20T10:01:30Z', '2026-05-20T10:00:00Z',
                '2026-05-20T10:02:00Z'
             )",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO skill_review_reports (
                id, agent_name, source_invocation_id, learning_episode_id,
                trigger_kind, status, confidence, evidence_refs_json,
                review_output_json, telegram_notified, created_at
             ) VALUES (
                10, 'right', 'inv-2', 5, 'effort_threshold',
                'nothing_to_learn', 'medium', '[\"msg:404\",\"exec:303\"]',
                '{\"status\":\"nothing_to_learn\",\"confidence\":\"medium\",\"evidence_refs\":[\"msg:404\",\"exec:303\"],\"user_notice\":null}',
                0, '2026-05-20T11:00:00Z'
             )",
            [],
        )
        .unwrap();

        let detail = learning_report_detail(&conn, "right", 10).unwrap().unwrap();

        assert_eq!(detail.evidence.len(), 2);
        assert!(!detail.evidence[0].available);
        assert_eq!(detail.evidence[0].ref_id, "msg:404");
        assert!(!detail.evidence[1].available);
        assert_eq!(detail.evidence[1].ref_id, "exec:303");
        assert_eq!(detail.evidence[1].text, None);
    }

    #[test]
    fn learning_report_detail_hides_low_trust_messages() {
        let (_dir, conn) = fixture();
        conn.execute(
            "INSERT INTO conversation_messages (
                id, platform, chat_id, thread_id, message_id, role, content,
                addressed_to_bot, routed_to_agent, created_at
             ) VALUES (
                102, 'telegram', 10, 20, 78, 'user',
                'Ambient chat that was not routed to the agent.',
                0, 0, '2026-05-20T10:00:00Z'
             )",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO learning_episodes (
                id, agent_name, kind, seed_trigger_kind, seed_ref, status,
                message_refs_json, execution_event_refs_json, selector_output_json,
                ready_after, created_at, updated_at
             ) VALUES (
                6, 'right', 'foreground_thread', 'effort_threshold', 'inv:inv-4',
                'reviewed', '[\"msg:102\"]', '[]', '{}',
                '2026-05-20T10:01:30Z', '2026-05-20T10:00:00Z',
                '2026-05-20T10:02:00Z'
             )",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO skill_review_reports (
                id, agent_name, source_invocation_id, learning_episode_id,
                trigger_kind, status, confidence, evidence_refs_json,
                review_output_json, telegram_notified, created_at
             ) VALUES (
                12, 'right', 'inv-4', 6, 'effort_threshold',
                'nothing_to_learn', 'low', '[\"msg:102\"]',
                '{\"status\":\"nothing_to_learn\",\"confidence\":\"low\",\"evidence_refs\":[\"msg:102\"],\"user_notice\":null}',
                0, '2026-05-20T11:00:00Z'
             )",
            [],
        )
        .unwrap();

        let detail = learning_report_detail(&conn, "right", 12).unwrap().unwrap();

        assert_eq!(detail.evidence.len(), 1);
        assert_eq!(detail.evidence[0].ref_id, "msg:102");
        assert_eq!(detail.evidence[0].source, "message");
        assert!(!detail.evidence[0].available);
        assert_eq!(detail.evidence[0].text, None);
    }

    #[test]
    fn learning_report_detail_errors_on_malformed_report_json() {
        let (_dir, conn) = fixture();
        conn.execute(
            "INSERT INTO skill_review_reports (
                id, agent_name, source_invocation_id, trigger_kind, status,
                confidence, evidence_refs_json, review_output_json, created_at
             ) VALUES (
                11, 'right', 'inv-3', 'effort_threshold', 'failed',
                'low', '[]', '{malformed', '2026-05-20T11:00:00Z'
             )",
            [],
        )
        .unwrap();

        assert!(learning_report_detail(&conn, "right", 11).is_err());
    }
}
