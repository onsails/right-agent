use right_mcp::LEARNED_SKILL_PREFIX;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LearningAction {
    Create,
    Update,
}

impl LearningAction {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Create => "create",
            Self::Update => "update",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LearningPhase {
    Start,
    Finish,
}

impl LearningPhase {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::Finish => "finish",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LearningStatus {
    Created,
    Updated,
    Aborted,
    Failed,
}

impl LearningStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::Updated => "updated",
            Self::Aborted => "aborted",
            Self::Failed => "failed",
        }
    }

    pub fn is_success(self) -> bool {
        matches!(self, Self::Created | Self::Updated)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LearningEvent {
    pub invocation_id: String,
    pub agent_name: String,
    pub action: LearningAction,
    pub skill_name: String,
    pub phase: LearningPhase,
    pub status: Option<LearningStatus>,
    pub reason: Option<String>,
    pub message: Option<String>,
    pub summary: Option<String>,
    pub event_refs: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NudgeSignalKind {
    Learning,
    SkillIssue,
}

impl NudgeSignalKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Learning => "learning",
            Self::SkillIssue => "skill_issue",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct NudgeSignalRecord {
    pub invocation_id: String,
    pub agent_name: String,
    pub root_session_id: Option<String>,
    pub chat_id: Option<i64>,
    pub thread_id: Option<i64>,
    pub signal_kind: NudgeSignalKind,
    pub payload_json: serde_json::Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewTriggerKind {
    LearningSignal,
    SkillIssueSignal,
    EffortThreshold,
}

impl ReviewTriggerKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::LearningSignal => "learning_signal",
            Self::SkillIssueSignal => "skill_issue_signal",
            Self::EffortThreshold => "effort_threshold",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewStatus {
    NothingToLearn,
    CreateCandidate,
    UpdateCandidate,
    Failed,
}

impl ReviewStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NothingToLearn => "nothing_to_learn",
            Self::CreateCandidate => "create_candidate",
            Self::UpdateCandidate => "update_candidate",
            Self::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewConfidence {
    Low,
    Medium,
    High,
}

impl ReviewConfidence {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SkillReviewReport {
    pub agent_name: String,
    pub source_invocation_id: String,
    pub learning_episode_id: Option<i64>,
    pub root_session_id: Option<String>,
    pub chat_id: Option<i64>,
    pub thread_id: Option<i64>,
    pub trigger_kind: ReviewTriggerKind,
    pub status: ReviewStatus,
    pub confidence: ReviewConfidence,
    pub candidate_skill_name: Option<String>,
    pub candidate_summary: Option<String>,
    pub evidence_refs: Vec<String>,
    pub review_output_json: serde_json::Value,
    pub telegram_notified: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ReviewGateInput<'a> {
    pub signal_trigger: Option<ReviewTriggerKind>,
    /// Current UTC time in RFC3339 strict format (e.g. "2026-05-21T03:14:15Z").
    /// Used for the daily-budget date filter and the circuit-window comparison.
    pub now_utc: &'a str,
    pub daily_budget_usd: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewSkipReason {
    AlreadyRunning,
    CircuitOpen,
    DailyBudget,
    BelowThreshold,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewGateDecision {
    Start(ReviewTriggerKind),
    Skip(ReviewSkipReason),
}

pub fn insert_learning_event(
    conn: &rusqlite::Connection,
    event: &LearningEvent,
) -> Result<(), rusqlite::Error> {
    let event_refs_json = serde_json::to_string(&event.event_refs)
        .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "INSERT INTO skill_learning_events \
         (invocation_id, agent_name, action, skill_name, phase, status, reason, message, summary, event_refs_json) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        rusqlite::params![
            event.invocation_id,
            event.agent_name,
            event.action.as_str(),
            event.skill_name,
            event.phase.as_str(),
            event.status.map(LearningStatus::as_str),
            event.reason,
            event.message,
            event.summary,
            event_refs_json,
        ],
    )?;
    tx.execute(
        "INSERT OR IGNORE INTO skill_nudge_state (agent_name) VALUES (?1)",
        [event.agent_name.as_str()],
    )?;
    tx.commit()?;
    Ok(())
}

pub fn successful_finish_exists(
    conn: &rusqlite::Connection,
    invocation_id: &str,
) -> Result<bool, rusqlite::Error> {
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM skill_learning_events \
         WHERE invocation_id=?1 AND phase='finish' AND status IN ('created','updated')",
        [invocation_id],
        |r| r.get(0),
    )?;
    Ok(count > 0)
}

pub fn select_reply_signal(
    conn: &rusqlite::Connection,
    invocation_id: &str,
    learning_signal: Option<serde_json::Value>,
    skill_issue_signal: Option<serde_json::Value>,
) -> Result<Option<(NudgeSignalKind, serde_json::Value)>, rusqlite::Error> {
    if successful_finish_exists(conn, invocation_id)? {
        return Ok(None);
    }

    match (learning_signal, skill_issue_signal) {
        (Some(_), Some(_)) => Ok(None),
        (Some(signal), None) => Ok(validate_nudge_signal(NudgeSignalKind::Learning, signal)),
        (None, Some(signal)) => Ok(validate_nudge_signal(NudgeSignalKind::SkillIssue, signal)),
        (None, None) => Ok(None),
    }
}

const LEARNING_TRIGGERS: &[&str] = &[
    "explicit_user_request",
    "multi_step_workflow",
    "recovered_surprise",
    "user_correction",
    "repeated_tool_pattern",
];

const NUDGE_REASONS: &[&str] = &[
    "conversation_still_evolving",
    "needs_full_context_review",
    "write_or_publish_failed",
    "needs_existing_skill_diff",
];

const SKILL_ISSUES: &[&str] = &[
    "missing_step",
    "stale_command",
    "wrong_api_assumption",
    "overbroad_activation",
    "broken_script",
    "unsafe_instruction",
];

const OBSERVED_EFFECTS: &[&str] = &[
    "retry_after_tool_error",
    "retry_after_user_correction",
    "manual_override",
    "verified_alternative",
];

fn validate_nudge_signal(
    signal_kind: NudgeSignalKind,
    signal: serde_json::Value,
) -> Option<(NudgeSignalKind, serde_json::Value)> {
    let is_explicit_user_request = match signal_kind {
        NudgeSignalKind::Learning => validate_learning_signal(&signal)?,
        NudgeSignalKind::SkillIssue => {
            validate_skill_issue_signal(&signal)?;
            false
        }
    };

    let event_ref_count = valid_event_ref_count(&signal)?;
    let required_refs = if is_explicit_user_request { 1 } else { 2 };
    if event_ref_count < required_refs {
        return None;
    }

    Some((signal_kind, signal))
}

fn valid_event_ref_count(signal: &serde_json::Value) -> Option<usize> {
    let refs = signal.get("event_refs").and_then(|v| v.as_array())?;
    for event_ref in refs {
        let event_ref = event_ref.as_str()?;
        if event_ref.trim().is_empty() {
            return None;
        }
    }
    Some(refs.len())
}

fn validate_learning_signal(signal: &serde_json::Value) -> Option<bool> {
    let kind = signal.get("kind").and_then(|v| v.as_str())?;
    if kind != "create_candidate" {
        return None;
    }

    learned_skill_name(signal, "package_name_hint")?;
    let trigger = enum_str(signal, "trigger", LEARNING_TRIGGERS)?;
    enum_str(signal, "reason_not_written", NUDGE_REASONS)?;
    non_empty_str(signal, "summary")?;

    Some(trigger == "explicit_user_request")
}

fn validate_skill_issue_signal(signal: &serde_json::Value) -> Option<()> {
    let kind = signal.get("kind").and_then(|v| v.as_str())?;
    if kind != "update_candidate" {
        return None;
    }

    learned_skill_name(signal, "skill_name")?;
    enum_str(signal, "issue", SKILL_ISSUES)?;
    enum_str(signal, "reason_not_patched", NUDGE_REASONS)?;
    enum_str(signal, "observed_effect", OBSERVED_EFFECTS)?;
    non_empty_str(signal, "patch_hint")?;

    Some(())
}

fn non_empty_str<'a>(signal: &'a serde_json::Value, field: &str) -> Option<&'a str> {
    let value = signal.get(field).and_then(|v| v.as_str())?;
    if value.trim().is_empty() {
        return None;
    }
    Some(value)
}

fn learned_skill_name<'a>(signal: &'a serde_json::Value, field: &str) -> Option<&'a str> {
    let value = non_empty_str(signal, field)?;
    value.starts_with(LEARNED_SKILL_PREFIX).then_some(value)
}

fn enum_str<'a>(signal: &'a serde_json::Value, field: &str, allowed: &[&str]) -> Option<&'a str> {
    let value = non_empty_str(signal, field)?;
    if !allowed.contains(&value) {
        return None;
    }
    Some(value)
}

pub fn ensure_nudge_state(
    conn: &rusqlite::Connection,
    agent_name: &str,
) -> Result<(), rusqlite::Error> {
    conn.execute(
        "INSERT OR IGNORE INTO skill_nudge_state (agent_name) VALUES (?1)",
        [agent_name],
    )?;
    Ok(())
}

pub fn increment_turn_nudge_counters(
    conn: &rusqlite::Connection,
    agent_name: &str,
    tool_iters: i64,
) -> Result<(), rusqlite::Error> {
    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "INSERT OR IGNORE INTO skill_nudge_state (agent_name) VALUES (?1)",
        [agent_name],
    )?;
    tx.execute(
        "UPDATE skill_nudge_state \
         SET turns_since_review = turns_since_review + 1, \
             tool_iters_since_review = tool_iters_since_review + ?2 \
         WHERE agent_name = ?1",
        rusqlite::params![agent_name, tool_iters.max(0)],
    )?;
    tx.commit()?;
    Ok(())
}

pub fn record_nudge_signal(
    conn: &rusqlite::Connection,
    record: &NudgeSignalRecord,
) -> Result<(), rusqlite::Error> {
    let payload = serde_json::to_string(&record.payload_json)
        .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "INSERT OR IGNORE INTO skill_nudge_state (agent_name) VALUES (?1)",
        [record.agent_name.as_str()],
    )?;
    tx.execute(
        "INSERT INTO skill_nudge_signals \
         (invocation_id, agent_name, root_session_id, chat_id, thread_id, signal_kind, payload_json) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        rusqlite::params![
            record.invocation_id,
            record.agent_name,
            record.root_session_id,
            record.chat_id,
            record.thread_id,
            record.signal_kind.as_str(),
            payload,
        ],
    )?;
    if matches!(record.signal_kind, NudgeSignalKind::SkillIssue) {
        tx.execute(
            "UPDATE skill_nudge_state \
             SET skill_issue_hints_since_review = skill_issue_hints_since_review + 1 \
             WHERE agent_name = ?1",
            [record.agent_name.as_str()],
        )?;
    }
    tx.commit()?;
    Ok(())
}

pub fn insert_skill_review_report(
    conn: &rusqlite::Connection,
    report: &SkillReviewReport,
) -> Result<(), rusqlite::Error> {
    let evidence_refs_json = serde_json::to_string(&report.evidence_refs)
        .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
    let review_output_json = serde_json::to_string(&report.review_output_json)
        .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
    conn.execute(
        "INSERT INTO skill_review_reports \
         (agent_name, source_invocation_id, learning_episode_id, root_session_id, chat_id, thread_id, trigger_kind, status, confidence, candidate_skill_name, candidate_summary, evidence_refs_json, review_output_json, telegram_notified) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
        rusqlite::params![
            report.agent_name.as_str(),
            report.source_invocation_id.as_str(),
            report.learning_episode_id,
            report.root_session_id.as_deref(),
            report.chat_id,
            report.thread_id,
            report.trigger_kind.as_str(),
            report.status.as_str(),
            report.confidence.as_str(),
            report.candidate_skill_name.as_deref(),
            report.candidate_summary.as_deref(),
            evidence_refs_json,
            review_output_json,
            if report.telegram_notified { 1_i64 } else { 0_i64 },
        ],
    )?;
    Ok(())
}

pub fn review_gate_decision(
    conn: &rusqlite::Connection,
    agent_name: &str,
    input: ReviewGateInput<'_>,
) -> Result<ReviewGateDecision, rusqlite::Error> {
    let tx = conn.unchecked_transaction()?;
    ensure_nudge_state(&tx, agent_name)?;
    let decision = review_gate_decision_in_tx(&tx, agent_name, input)?;
    tx.commit()?;
    Ok(decision)
}

fn review_gate_decision_in_tx(
    tx: &rusqlite::Transaction<'_>,
    agent_name: &str,
    input: ReviewGateInput<'_>,
) -> Result<ReviewGateDecision, rusqlite::Error> {
    // Parse now_utc once into a typed DateTime. A malformed string would
    // otherwise silently produce a junk `today_start` (via `split_once('T')`
    // falling back to the whole string) and the daily-budget SUM would
    // return 0.0, bypassing the guard. Match the existing error pattern in
    // record_review_failure.
    let parsed_now = chrono::DateTime::parse_from_rfc3339(input.now_utc)
        .map_err(|e| {
            rusqlite::Error::ToSqlConversionFailure(Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("invalid now_utc {:?}: {e}", input.now_utc),
            )))
        })?
        .with_timezone(&chrono::Utc);

    let (review_running, circuit_open_until, tool_iters, interval): (
        i64,
        Option<String>,
        i64,
        i64,
    ) = tx.query_row(
        "SELECT review_running, review_circuit_open_until, \
                    tool_iters_since_review, creation_review_interval \
             FROM skill_nudge_state WHERE agent_name = ?1",
        [agent_name],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
    )?;

    if review_running != 0 {
        return Ok(ReviewGateDecision::Skip(ReviewSkipReason::AlreadyRunning));
    }
    if let Some(until) = circuit_open_until {
        // RFC3339-Z strings are lexicographically orderable, so a string
        // compare matches the typed DateTime comparison by construction
        // (both sides are produced as "%Y-%m-%dT%H:%M:%SZ").
        if until.as_str() > input.now_utc {
            return Ok(ReviewGateDecision::Skip(ReviewSkipReason::CircuitOpen));
        }
        // Window expired. Clear both fields so the next attempt has a fresh
        // failure budget. Otherwise consecutive_review_failures stays elevated
        // and the next failure immediately reopens the circuit.
        tx.execute(
            "UPDATE skill_nudge_state SET \
                review_circuit_open_until = NULL, \
                consecutive_review_failures = 0 \
             WHERE agent_name = ?1",
            [agent_name],
        )?;
    }

    let prospective = if let Some(trigger) = input.signal_trigger {
        trigger
    } else if interval > 0 && tool_iters >= interval {
        ReviewTriggerKind::EffortThreshold
    } else {
        return Ok(ReviewGateDecision::Skip(ReviewSkipReason::BelowThreshold));
    };

    // Only query usage_events when a trigger would otherwise fire. The index
    // scan is cheap but pointless for the common no-trigger branch.
    let today_start = parsed_now
        .date_naive()
        .and_hms_opt(0, 0, 0)
        .expect("00:00:00 is a valid time")
        .format("%Y-%m-%dT%H:%M:%SZ")
        .to_string();
    let placeholders = (0..crate::usage::LEARNING_SOURCES.len())
        .map(|i| format!("?{}", i + 2))
        .collect::<Vec<_>>()
        .join(",");
    let query = format!(
        "SELECT COALESCE(SUM(total_cost_usd), 0.0) FROM usage_events \
         WHERE ts >= ?1 AND source IN ({placeholders})"
    );
    let mut stmt = tx.prepare(&query)?;
    let mut params: Vec<&dyn rusqlite::ToSql> = vec![&today_start];
    for source in crate::usage::LEARNING_SOURCES {
        params.push(source);
    }
    let spent: f64 = stmt.query_row(params.as_slice(), |r| r.get(0))?;
    if spent >= input.daily_budget_usd {
        return Ok(ReviewGateDecision::Skip(ReviewSkipReason::DailyBudget));
    }

    Ok(ReviewGateDecision::Start(prospective))
}

pub fn try_mark_review_started(
    conn: &rusqlite::Connection,
    agent_name: &str,
    input: ReviewGateInput<'_>,
) -> Result<ReviewGateDecision, rusqlite::Error> {
    let tx = conn.unchecked_transaction()?;
    ensure_nudge_state(&tx, agent_name)?;

    let decision = review_gate_decision_in_tx(&tx, agent_name, input)?;
    let ReviewGateDecision::Start(trigger) = decision else {
        tx.commit()?;
        return Ok(decision);
    };

    // Only flip `review_running`. The daily-budget guard is enforced by the
    // SUM query above — no counter to increment.
    let updated = tx.execute(
        "UPDATE skill_nudge_state \
         SET review_running = 1 \
         WHERE agent_name = ?1 AND review_running = 0",
        [agent_name],
    )?;
    if updated == 1 {
        tx.commit()?;
        return Ok(ReviewGateDecision::Start(trigger));
    }

    // Lost the race — another caller marked it running. Re-read.
    let decision = review_gate_decision_in_tx(&tx, agent_name, input)?;
    tx.commit()?;
    Ok(decision)
}

pub fn mark_review_finished(
    conn: &rusqlite::Connection,
    agent_name: &str,
    trigger: ReviewTriggerKind,
    status: ReviewStatus,
    reset_activity_counters: bool,
) -> Result<(), rusqlite::Error> {
    let tx = conn.unchecked_transaction()?;
    mark_review_finished_in_tx(&tx, agent_name, trigger, status, reset_activity_counters)?;
    tx.commit()?;
    Ok(())
}

/// Same as `mark_review_finished` but runs inside an existing transaction.
/// Used when the caller is already coordinating multiple writes inside
/// `conn.unchecked_transaction()` and cannot tolerate a nested BEGIN.
///
/// # Transaction contract
///
/// This function MUST be called from inside an outer transaction owned by
/// the caller. It does NOT start its own transaction — that is the whole
/// point of the `_in_tx` suffix: the caller owns commit/rollback so this
/// function's writes can be atomic with the caller's surrounding writes.
///
/// Callers MUST NOT call `tx.commit()` between writes inside this
/// function; commit (or implicit rollback on drop) is the caller's
/// responsibility once all coordinated writes have run. A nested
/// `BEGIN`/`COMMIT` here would break atomicity for the outer transaction.
///
/// # Implicit invariant on inner helpers
///
/// Every helper this function calls (currently `ensure_nudge_state` and
/// the direct `tx.execute` UPDATE) accepts `&rusqlite::Connection` but
/// receives `&rusqlite::Transaction<'_>` via `Transaction: Deref<Target =
/// Connection>`. That is intentional — the connection-typed helpers reuse
/// the active transaction without opening a savepoint. Any new helper
/// introduced here that internally calls `conn.unchecked_transaction()`,
/// `conn.transaction()`, or opens a SAVEPOINT would silently break the
/// nested-tx contract and corrupt the caller's atomicity guarantees. If
/// such a helper is needed, add an `_in_tx` variant that takes
/// `&Transaction<'_>` directly and call that instead.
///
/// There is no runtime way to assert "we are inside a transaction" from a
/// `&Connection`, so this contract is enforced by documentation and code
/// review only.
pub fn mark_review_finished_in_tx(
    tx: &rusqlite::Transaction<'_>,
    agent_name: &str,
    trigger: ReviewTriggerKind,
    status: ReviewStatus,
    reset_activity_counters: bool,
) -> Result<(), rusqlite::Error> {
    ensure_nudge_state(tx, agent_name)?;
    let reset_activity_counters =
        reset_activity_counters && !matches!(status, ReviewStatus::Failed);
    let reset_issue_hints = !matches!(status, ReviewStatus::Failed)
        && (matches!(trigger, ReviewTriggerKind::SkillIssueSignal)
            || matches!(status, ReviewStatus::NothingToLearn));
    tx.execute(
        "UPDATE skill_nudge_state \
         SET review_running = 0, \
             tool_iters_since_review = CASE WHEN ?3 THEN 0 ELSE tool_iters_since_review END, \
             turns_since_review = CASE WHEN ?3 THEN 0 ELSE turns_since_review END, \
             skill_issue_hints_since_review = CASE WHEN ?4 THEN 0 ELSE skill_issue_hints_since_review END, \
             last_review_at = strftime('%Y-%m-%dT%H:%M:%SZ','now'), \
             last_review_status = ?2, \
             consecutive_review_failures = 0, \
             review_circuit_open_until = NULL \
         WHERE agent_name = ?1",
        rusqlite::params![
            agent_name,
            status.as_str(),
            if reset_activity_counters { 1_i64 } else { 0_i64 },
            if reset_issue_hints { 1_i64 } else { 0_i64 },
        ],
    )?;
    Ok(())
}

/// Clear stale `review_running = 1` rows.
///
/// The bot is the only writer for this flag. If the previous boot exited
/// non-gracefully (panic, SIGKILL, OOM) between `try_mark_review_started`
/// and `mark_review_finished`, the flag is left at 1 and the review gate
/// returns `Skip(AlreadyRunning)` forever. This one-shot startup reaper
/// resets every such row to 0 — without touching `last_review_at`,
/// `last_review_status`, or any counters — so the next eligible signal
/// can start a new review.
///
/// Returns the count of rows reset (for logging).
///
/// Single-statement write — no transaction needed.
pub fn reset_stale_review_running(conn: &rusqlite::Connection) -> Result<usize, rusqlite::Error> {
    conn.execute(
        "UPDATE skill_nudge_state SET review_running = 0 WHERE review_running = 1",
        [],
    )
}

/// Record a learning review failure: increment `consecutive_review_failures`,
/// open the circuit if the threshold is reached. Atomic.
///
/// Returns `(new_failure_count, opened_circuit_now)`:
/// - `new_failure_count` is the updated counter value.
/// - `opened_circuit_now` is `true` iff this call transitioned the circuit
///   from closed to open. Useful for callers that need to emit a one-shot
///   Telegram alert without re-alerting on every subsequent failure while
///   the circuit stays open.
///
/// `now_utc` must be RFC3339 strict (e.g. "2026-05-21T03:14:15Z").
pub fn record_review_failure(
    conn: &rusqlite::Connection,
    agent_name: &str,
    now_utc: &str,
    threshold: u32,
    cooldown_minutes: u32,
) -> Result<(i64, bool), rusqlite::Error> {
    // Parse now_utc once up front so the open-window comparison and the
    // cooldown computation share a single validation point. Otherwise the
    // open-window branch silently accepts a malformed string while only
    // the should_open branch would have caught it.
    let parsed_now = chrono::DateTime::parse_from_rfc3339(now_utc)
        .map_err(|e| {
            rusqlite::Error::ToSqlConversionFailure(Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("invalid now_utc {now_utc:?}: {e}"),
            )))
        })?
        .with_timezone(&chrono::Utc);

    let tx = conn.unchecked_transaction()?;
    ensure_nudge_state(&tx, agent_name)?;

    let (prev_count, prev_open_until): (i64, Option<String>) = tx.query_row(
        "SELECT consecutive_review_failures, review_circuit_open_until \
         FROM skill_nudge_state WHERE agent_name = ?1",
        [agent_name],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;

    let new_count = prev_count + 1;
    // RFC3339-Z strings are lexicographically orderable; both sides are
    // produced as "%Y-%m-%dT%H:%M:%SZ" by construction.
    let circuit_already_open = prev_open_until
        .as_deref()
        .map(|s| s > now_utc)
        .unwrap_or(false);
    let should_open = new_count >= i64::from(threshold) && !circuit_already_open;
    let opened_now = should_open;

    let new_open_until: Option<String> = if should_open {
        // Compute now_utc + cooldown_minutes in Rust to avoid SQLite datetime
        // round-trips that drop the 'Z' suffix.
        let until = parsed_now + chrono::Duration::minutes(i64::from(cooldown_minutes));
        Some(until.format("%Y-%m-%dT%H:%M:%SZ").to_string())
    } else {
        prev_open_until
    };

    tx.execute(
        "UPDATE skill_nudge_state SET \
            review_running = 0, \
            consecutive_review_failures = ?2, \
            review_circuit_open_until = ?3 \
         WHERE agent_name = ?1",
        rusqlite::params![agent_name, new_count, new_open_until],
    )?;
    tx.commit()?;
    Ok((new_count, opened_now))
}

/// Clear `review_running` for a single agent without touching any other
/// field.
///
/// Used by the bot shutdown path when a background review is aborted
/// mid-flight: no review actually finished, so `last_review_at`,
/// `last_review_status`, and counters must remain as the in-flight review
/// left them.
///
/// Single-statement write — no transaction needed.
pub fn clear_review_running(
    conn: &rusqlite::Connection,
    agent_name: &str,
) -> Result<(), rusqlite::Error> {
    conn.execute(
        "UPDATE skill_nudge_state SET review_running = 0 WHERE agent_name = ?1",
        [agent_name],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn() -> rusqlite::Connection {
        let mut conn = rusqlite::Connection::open_in_memory().unwrap();
        right_db::MIGRATIONS.to_latest(&mut conn).unwrap();
        conn
    }

    fn review_gate_input(signal_trigger: Option<ReviewTriggerKind>) -> ReviewGateInput<'static> {
        ReviewGateInput {
            signal_trigger,
            now_utc: "2026-05-18T12:00:00Z",
            daily_budget_usd: 5.00,
        }
    }

    fn insert_usage(conn: &rusqlite::Connection, ts: &str, source: &str, cost: f64) {
        conn.execute(
            "INSERT INTO usage_events (
                ts, source, chat_id, thread_id, job_name, session_uuid,
                total_cost_usd, num_turns, input_tokens, output_tokens,
                cache_creation_tokens, cache_read_tokens, web_search_requests,
                web_fetch_requests, model_usage_json, api_key_source
             ) VALUES (?1, ?2, NULL, NULL, NULL, 's', ?3, 1, 0, 0, 0, 0, 0, 0, '{}', 'none')",
            rusqlite::params![ts, source, cost],
        )
        .unwrap();
    }

    fn ensure_agent_nudge_state(conn: &rusqlite::Connection, agent: &str) {
        let tx = conn.unchecked_transaction().unwrap();
        ensure_nudge_state(&tx, agent).unwrap();
        tx.commit().unwrap();
    }

    #[test]
    fn review_report_persistence_round_trips_candidate() {
        let conn = conn();
        let report = SkillReviewReport {
            agent_name: "right".to_owned(),
            source_invocation_id: "inv-1".to_owned(),
            learning_episode_id: None,
            root_session_id: Some("session-1".to_owned()),
            chat_id: Some(100),
            thread_id: Some(200),
            trigger_kind: ReviewTriggerKind::LearningSignal,
            status: ReviewStatus::CreateCandidate,
            confidence: ReviewConfidence::High,
            candidate_skill_name: Some("rightx-oauth-debugging".to_owned()),
            candidate_summary: Some("OAuth MCP setup needs callback URL verification.".to_owned()),
            evidence_refs: vec!["event-1".to_owned(), "event-2".to_owned()],
            review_output_json: serde_json::json!({
                "status": "create_candidate",
                "confidence": "high",
                "candidate_skill_name": "rightx-oauth-debugging"
            }),
            telegram_notified: true,
        };

        insert_skill_review_report(&conn, &report).unwrap();

        let row: (String, String, String, String, String, i64) = conn
            .query_row(
                "SELECT trigger_kind, status, confidence, candidate_skill_name, evidence_refs_json, telegram_notified \
                 FROM skill_review_reports WHERE source_invocation_id='inv-1'",
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
        assert_eq!(row.0, "learning_signal");
        assert_eq!(row.1, "create_candidate");
        assert_eq!(row.2, "high");
        assert_eq!(row.3, "rightx-oauth-debugging");
        assert_eq!(row.4, r#"["event-1","event-2"]"#);
        assert_eq!(row.5, 1);
    }

    #[test]
    fn review_gate_accepts_signal_and_effort_threshold() {
        let conn = conn();
        ensure_nudge_state(&conn, "right").unwrap();

        let signal_decision = review_gate_decision(
            &conn,
            "right",
            ReviewGateInput {
                signal_trigger: Some(ReviewTriggerKind::LearningSignal),
                now_utc: "2026-05-18T12:00:00Z",
                daily_budget_usd: 5.00,
            },
        )
        .unwrap();
        assert_eq!(
            signal_decision,
            ReviewGateDecision::Start(ReviewTriggerKind::LearningSignal)
        );

        let issue_decision = review_gate_decision(
            &conn,
            "right",
            ReviewGateInput {
                signal_trigger: Some(ReviewTriggerKind::SkillIssueSignal),
                now_utc: "2026-05-18T12:00:00Z",
                daily_budget_usd: 5.00,
            },
        )
        .unwrap();
        assert_eq!(
            issue_decision,
            ReviewGateDecision::Start(ReviewTriggerKind::SkillIssueSignal)
        );

        conn.execute(
            "UPDATE skill_nudge_state SET tool_iters_since_review = 15 WHERE agent_name='right'",
            [],
        )
        .unwrap();
        let effort_decision = review_gate_decision(
            &conn,
            "right",
            ReviewGateInput {
                signal_trigger: None,
                now_utc: "2026-05-18T12:00:00Z",
                daily_budget_usd: 5.00,
            },
        )
        .unwrap();
        assert_eq!(
            effort_decision,
            ReviewGateDecision::Start(ReviewTriggerKind::EffortThreshold)
        );
    }

    #[test]
    fn review_gate_blocks_running() {
        let conn = conn();
        ensure_nudge_state(&conn, "right").unwrap();

        conn.execute(
            "UPDATE skill_nudge_state SET review_running = 1 WHERE agent_name='right'",
            [],
        )
        .unwrap();
        let running = review_gate_decision(
            &conn,
            "right",
            ReviewGateInput {
                signal_trigger: Some(ReviewTriggerKind::LearningSignal),
                now_utc: "2026-05-18T12:00:00Z",
                daily_budget_usd: 5.00,
            },
        )
        .unwrap();
        assert_eq!(
            running,
            ReviewGateDecision::Skip(ReviewSkipReason::AlreadyRunning)
        );

        // Clearing review_running allows a start again.
        conn.execute(
            "UPDATE skill_nudge_state SET review_running = 0 WHERE agent_name='right'",
            [],
        )
        .unwrap();
        let started = review_gate_decision(
            &conn,
            "right",
            ReviewGateInput {
                signal_trigger: Some(ReviewTriggerKind::LearningSignal),
                now_utc: "2026-05-18T12:00:00Z",
                daily_budget_usd: 5.00,
            },
        )
        .unwrap();
        assert_eq!(
            started,
            ReviewGateDecision::Start(ReviewTriggerKind::LearningSignal)
        );
    }

    #[test]
    fn review_start_and_finish_update_nudge_state() {
        let conn = conn();
        ensure_nudge_state(&conn, "right").unwrap();
        conn.execute(
            "UPDATE skill_nudge_state SET tool_iters_since_review = 19, turns_since_review = 3 WHERE agent_name='right'",
            [],
        )
        .unwrap();

        let started = try_mark_review_started(
            &conn,
            "right",
            ReviewGateInput {
                signal_trigger: None,
                now_utc: "2026-05-18T12:00:00Z",
                daily_budget_usd: 5.00,
            },
        )
        .unwrap();
        assert_eq!(
            started,
            ReviewGateDecision::Start(ReviewTriggerKind::EffortThreshold)
        );
        let running: i64 = conn
            .query_row(
                "SELECT review_running FROM skill_nudge_state WHERE agent_name='right'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(running, 1);

        mark_review_finished(
            &conn,
            "right",
            ReviewTriggerKind::EffortThreshold,
            ReviewStatus::NothingToLearn,
            true,
        )
        .unwrap();
        let row: (i64, i64, i64, String) = conn
            .query_row(
                "SELECT review_running, tool_iters_since_review, turns_since_review, last_review_status \
                 FROM skill_nudge_state WHERE agent_name='right'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!(row, (0, 0, 0, "nothing_to_learn".to_owned()));
    }

    #[test]
    fn reset_stale_review_running_clears_flag_without_touching_other_fields() {
        let conn = conn();
        ensure_nudge_state(&conn, "stale").unwrap();
        // Seed a row that looks like a stranded review: running=1, with
        // pre-existing counters and last_review_at/status that
        // the reaper MUST leave alone.
        conn.execute(
            "UPDATE skill_nudge_state \
             SET review_running = 1, \
                 tool_iters_since_review = 7, \
                 turns_since_review = 2, \
                 skill_issue_hints_since_review = 1, \
                 daily_review_count = 4, \
                 daily_review_date = '2026-05-18', \
                 last_review_at = '2026-05-18T11:30:00Z', \
                 last_review_status = 'nothing_to_learn' \
             WHERE agent_name='stale'",
            [],
        )
        .unwrap();

        // Also seed a non-stale row to confirm the WHERE clause is selective.
        ensure_nudge_state(&conn, "idle").unwrap();
        conn.execute(
            "UPDATE skill_nudge_state \
             SET review_running = 0, last_review_at = '2026-05-18T10:00:00Z' \
             WHERE agent_name='idle'",
            [],
        )
        .unwrap();

        let reset = reset_stale_review_running(&conn).unwrap();
        assert_eq!(reset, 1);

        let stale_row: (i64, i64, i64, i64, i64, String, String, String) = conn
            .query_row(
                "SELECT review_running, tool_iters_since_review, turns_since_review, \
                        skill_issue_hints_since_review, daily_review_count, daily_review_date, \
                        last_review_at, last_review_status \
                 FROM skill_nudge_state WHERE agent_name='stale'",
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
                        r.get(7)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(
            stale_row,
            (
                0,
                7,
                2,
                1,
                4,
                "2026-05-18".to_owned(),
                "2026-05-18T11:30:00Z".to_owned(),
                "nothing_to_learn".to_owned(),
            )
        );

        // Idle row untouched.
        let idle_running: i64 = conn
            .query_row(
                "SELECT review_running FROM skill_nudge_state WHERE agent_name='idle'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(idle_running, 0);

        // Re-running the reaper on a clean DB is a no-op.
        let reset_again = reset_stale_review_running(&conn).unwrap();
        assert_eq!(reset_again, 0);
    }

    #[test]
    fn clear_review_running_clears_flag_without_touching_other_fields() {
        let conn = conn();
        ensure_nudge_state(&conn, "agent-x").unwrap();
        // Seed a mid-review row: running=1, with a prior review timestamp,
        // counters, and last_review_status that the shutdown helper MUST
        // leave alone.
        conn.execute(
            "UPDATE skill_nudge_state \
             SET review_running = 1, \
                 tool_iters_since_review = 11, \
                 turns_since_review = 4, \
                 skill_issue_hints_since_review = 2, \
                 daily_review_count = 3, \
                 daily_review_date = '2026-01-01', \
                 last_review_at = '2026-01-01T00:00:00Z', \
                 last_review_status = 'created' \
             WHERE agent_name='agent-x'",
            [],
        )
        .unwrap();

        clear_review_running(&conn, "agent-x").unwrap();

        let row: (i64, i64, i64, i64, i64, String, String, String) = conn
            .query_row(
                "SELECT review_running, tool_iters_since_review, turns_since_review, \
                        skill_issue_hints_since_review, daily_review_count, daily_review_date, \
                        last_review_at, last_review_status \
                 FROM skill_nudge_state WHERE agent_name='agent-x'",
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
                        r.get(7)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(
            row,
            (
                0,
                11,
                4,
                2,
                3,
                "2026-01-01".to_owned(),
                "2026-01-01T00:00:00Z".to_owned(),
                "created".to_owned(),
            )
        );
    }

    #[test]
    fn review_start_rechecks_running_after_stale_gate() {
        let conn = conn();
        ensure_nudge_state(&conn, "right").unwrap();
        let stale_decision = review_gate_decision(
            &conn,
            "right",
            review_gate_input(Some(ReviewTriggerKind::LearningSignal)),
        )
        .unwrap();
        assert_eq!(
            stale_decision,
            ReviewGateDecision::Start(ReviewTriggerKind::LearningSignal)
        );

        conn.execute(
            "UPDATE skill_nudge_state SET review_running = 1 WHERE agent_name='right'",
            [],
        )
        .unwrap();
        let running = try_mark_review_started(
            &conn,
            "right",
            review_gate_input(Some(ReviewTriggerKind::LearningSignal)),
        )
        .unwrap();
        assert_eq!(
            running,
            ReviewGateDecision::Skip(ReviewSkipReason::AlreadyRunning)
        );

        // After clearing running, a budget-within-limit signal starts.
        conn.execute(
            "UPDATE skill_nudge_state SET review_running = 0 WHERE agent_name='right'",
            [],
        )
        .unwrap();
        let started = try_mark_review_started(
            &conn,
            "right",
            review_gate_input(Some(ReviewTriggerKind::LearningSignal)),
        )
        .unwrap();
        assert_eq!(
            started,
            ReviewGateDecision::Start(ReviewTriggerKind::LearningSignal)
        );
    }

    #[test]
    fn review_start_ignores_recent_last_review_at_after_stale_gate() {
        let conn = conn();
        ensure_nudge_state(&conn, "right").unwrap();
        let input = ReviewGateInput {
            signal_trigger: Some(ReviewTriggerKind::LearningSignal),
            now_utc: "2026-05-18T12:00:00Z",
            daily_budget_usd: 5.00,
        };

        let stale_decision = review_gate_decision(&conn, "right", input).unwrap();
        assert_eq!(
            stale_decision,
            ReviewGateDecision::Start(ReviewTriggerKind::LearningSignal)
        );

        conn.execute(
            "UPDATE skill_nudge_state SET last_review_at = '2026-05-18T12:05:00Z' WHERE agent_name='right'",
            [],
        )
        .unwrap();

        let started = try_mark_review_started(&conn, "right", input).unwrap();
        assert_eq!(
            started,
            ReviewGateDecision::Start(ReviewTriggerKind::LearningSignal)
        );

        let running: i64 = conn
            .query_row(
                "SELECT review_running FROM skill_nudge_state WHERE agent_name='right'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(running, 1);
    }

    #[test]
    fn review_gate_disables_non_positive_effort_threshold() {
        let conn = conn();
        ensure_nudge_state(&conn, "right").unwrap();

        for interval in [0, -1] {
            conn.execute(
                "UPDATE skill_nudge_state SET tool_iters_since_review = 999, creation_review_interval = ?1 \
                 WHERE agent_name='right'",
                [interval],
            )
            .unwrap();

            let effort = review_gate_decision(&conn, "right", review_gate_input(None)).unwrap();
            assert_eq!(
                effort,
                ReviewGateDecision::Skip(ReviewSkipReason::BelowThreshold)
            );

            let signal = review_gate_decision(
                &conn,
                "right",
                review_gate_input(Some(ReviewTriggerKind::SkillIssueSignal)),
            )
            .unwrap();
            assert_eq!(
                signal,
                ReviewGateDecision::Start(ReviewTriggerKind::SkillIssueSignal)
            );
        }
    }

    #[test]
    fn review_public_helpers_create_missing_nudge_rows() {
        let conn = conn();

        let gate = review_gate_decision(&conn, "gate-missing", review_gate_input(None)).unwrap();
        assert_eq!(
            gate,
            ReviewGateDecision::Skip(ReviewSkipReason::BelowThreshold)
        );

        let start = try_mark_review_started(
            &conn,
            "start-missing",
            review_gate_input(Some(ReviewTriggerKind::SkillIssueSignal)),
        )
        .unwrap();
        assert_eq!(
            start,
            ReviewGateDecision::Start(ReviewTriggerKind::SkillIssueSignal)
        );

        mark_review_finished(
            &conn,
            "finish-missing",
            ReviewTriggerKind::EffortThreshold,
            ReviewStatus::Failed,
            false,
        )
        .unwrap();

        for agent_name in ["gate-missing", "start-missing", "finish-missing"] {
            let count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM skill_nudge_state WHERE agent_name = ?1",
                    [agent_name],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(count, 1, "{agent_name} row should exist");
        }

        let started: i64 = conn
            .query_row(
                "SELECT review_running FROM skill_nudge_state WHERE agent_name='start-missing'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(started, 1);

        let finished: (i64, String, Option<String>) = conn
            .query_row(
                "SELECT review_running, last_review_status, last_review_at \
                 FROM skill_nudge_state WHERE agent_name='finish-missing'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(finished.0, 0);
        assert_eq!(finished.1, "failed");
        assert!(finished.2.is_some());
    }

    #[test]
    fn review_failed_finish_preserves_counters_and_sets_last_review_at() {
        let conn = conn();
        ensure_nudge_state(&conn, "right").unwrap();
        conn.execute(
            "UPDATE skill_nudge_state \
             SET review_running = 1, tool_iters_since_review = 11, turns_since_review = 4, skill_issue_hints_since_review = 3 \
             WHERE agent_name='right'",
            [],
        )
        .unwrap();

        mark_review_finished(
            &conn,
            "right",
            ReviewTriggerKind::LearningSignal,
            ReviewStatus::Failed,
            true,
        )
        .unwrap();

        let row: (i64, i64, i64, i64, String, Option<String>) = conn
            .query_row(
                "SELECT review_running, tool_iters_since_review, turns_since_review, skill_issue_hints_since_review, last_review_status, last_review_at \
                 FROM skill_nudge_state WHERE agent_name='right'",
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
        assert_eq!(row.0, 0);
        assert_eq!(row.1, 11);
        assert_eq!(row.2, 4);
        assert_eq!(row.3, 3);
        assert_eq!(row.4, "failed");
        assert!(row.5.is_some());
    }

    #[test]
    fn review_finish_resets_activity_counters_only_when_requested() {
        let conn = conn();
        for agent_name in ["preserve", "reset"] {
            ensure_nudge_state(&conn, agent_name).unwrap();
            conn.execute(
                "UPDATE skill_nudge_state \
                 SET review_running = 1, tool_iters_since_review = 9, turns_since_review = 2, skill_issue_hints_since_review = 5 \
                 WHERE agent_name = ?1",
                [agent_name],
            )
            .unwrap();
        }

        mark_review_finished(
            &conn,
            "preserve",
            ReviewTriggerKind::LearningSignal,
            ReviewStatus::CreateCandidate,
            false,
        )
        .unwrap();
        mark_review_finished(
            &conn,
            "reset",
            ReviewTriggerKind::LearningSignal,
            ReviewStatus::CreateCandidate,
            true,
        )
        .unwrap();

        let preserve: (i64, i64, i64, i64) = conn
            .query_row(
                "SELECT review_running, tool_iters_since_review, turns_since_review, skill_issue_hints_since_review \
                 FROM skill_nudge_state WHERE agent_name='preserve'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!(preserve, (0, 9, 2, 5));

        let reset: (i64, i64, i64, i64) = conn
            .query_row(
                "SELECT review_running, tool_iters_since_review, turns_since_review, skill_issue_hints_since_review \
                 FROM skill_nudge_state WHERE agent_name='reset'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!(reset, (0, 0, 0, 5));
    }

    #[test]
    fn review_finish_resets_issue_hints_only_for_issue_or_nothing_to_learn() {
        let conn = conn();
        for agent_name in ["learning", "issue", "nothing", "effort-update"] {
            ensure_nudge_state(&conn, agent_name).unwrap();
            conn.execute(
                "UPDATE skill_nudge_state \
                 SET review_running = 1, tool_iters_since_review = 9, turns_since_review = 2, skill_issue_hints_since_review = 5 \
                 WHERE agent_name = ?1",
                [agent_name],
            )
            .unwrap();
        }

        mark_review_finished(
            &conn,
            "learning",
            ReviewTriggerKind::LearningSignal,
            ReviewStatus::CreateCandidate,
            true,
        )
        .unwrap();
        mark_review_finished(
            &conn,
            "issue",
            ReviewTriggerKind::SkillIssueSignal,
            ReviewStatus::UpdateCandidate,
            true,
        )
        .unwrap();
        mark_review_finished(
            &conn,
            "nothing",
            ReviewTriggerKind::EffortThreshold,
            ReviewStatus::NothingToLearn,
            true,
        )
        .unwrap();
        mark_review_finished(
            &conn,
            "effort-update",
            ReviewTriggerKind::EffortThreshold,
            ReviewStatus::UpdateCandidate,
            true,
        )
        .unwrap();

        let learning: (i64, i64, i64, i64, String) = conn
            .query_row(
                "SELECT review_running, tool_iters_since_review, turns_since_review, skill_issue_hints_since_review, last_review_status \
                 FROM skill_nudge_state WHERE agent_name='learning'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .unwrap();
        assert_eq!(learning, (0, 0, 0, 5, "create_candidate".to_owned()));

        let issue_hints: i64 = conn
            .query_row(
                "SELECT skill_issue_hints_since_review FROM skill_nudge_state WHERE agent_name='issue'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(issue_hints, 0);

        let nothing_hints: i64 = conn
            .query_row(
                "SELECT skill_issue_hints_since_review FROM skill_nudge_state WHERE agent_name='nothing'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(nothing_hints, 0);

        let effort_update_hints: i64 = conn
            .query_row(
                "SELECT skill_issue_hints_since_review FROM skill_nudge_state WHERE agent_name='effort-update'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(effort_update_hints, 5);
    }

    #[test]
    fn successful_finish_exists_only_for_created_or_updated() {
        let conn = conn();
        insert_learning_event(
            &conn,
            &LearningEvent {
                invocation_id: "inv-1".to_owned(),
                agent_name: "right".to_owned(),
                action: LearningAction::Create,
                skill_name: "rightx-demo".to_owned(),
                phase: LearningPhase::Finish,
                status: Some(LearningStatus::Failed),
                reason: None,
                message: None,
                summary: Some("write failed".to_owned()),
                event_refs: vec![],
            },
        )
        .unwrap();
        assert!(!successful_finish_exists(&conn, "inv-1").unwrap());

        insert_learning_event(
            &conn,
            &LearningEvent {
                invocation_id: "inv-1".to_owned(),
                agent_name: "right".to_owned(),
                action: LearningAction::Create,
                skill_name: "rightx-demo".to_owned(),
                phase: LearningPhase::Finish,
                status: Some(LearningStatus::Created),
                reason: None,
                message: Some("Learned skill: rightx-demo".to_owned()),
                summary: Some("captured workflow".to_owned()),
                event_refs: vec!["e1".to_owned(), "e2".to_owned()],
            },
        )
        .unwrap();
        assert!(successful_finish_exists(&conn, "inv-1").unwrap());
    }

    #[test]
    fn record_nudge_signal_persists_payload_and_updates_counter() {
        let conn = conn();
        record_nudge_signal(
            &conn,
            &NudgeSignalRecord {
                invocation_id: "inv-2".to_owned(),
                agent_name: "right".to_owned(),
                root_session_id: Some("root-1".to_owned()),
                chat_id: Some(10),
                thread_id: Some(20),
                signal_kind: NudgeSignalKind::SkillIssue,
                payload_json: serde_json::json!({"kind":"update_candidate"}),
            },
        )
        .unwrap();

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM skill_nudge_signals", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);

        let payload_json: String = conn
            .query_row("SELECT payload_json FROM skill_nudge_signals", [], |r| {
                r.get(0)
            })
            .unwrap();
        let payload: serde_json::Value = serde_json::from_str(&payload_json).unwrap();
        assert_eq!(
            payload,
            serde_json::json!({"kind":"update_candidate"}),
            "payload_json should persist the accepted signal payload"
        );

        let hints: i64 = conn
            .query_row(
                "SELECT skill_issue_hints_since_review FROM skill_nudge_state WHERE agent_name='right'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(hints, 1);
    }

    fn learning_signal(trigger: &str, event_refs: Vec<&str>, summary: &str) -> serde_json::Value {
        serde_json::json!({
            "kind": "create_candidate",
            "package_name_hint": "rightx-demo",
            "trigger": trigger,
            "reason_not_written": "needs_full_context_review",
            "event_refs": event_refs,
            "summary": summary,
        })
    }

    fn skill_issue_signal(event_refs: Vec<&str>, patch_hint: &str) -> serde_json::Value {
        serde_json::json!({
            "kind": "update_candidate",
            "skill_name": "rightx-demo",
            "issue": "stale_command",
            "reason_not_patched": "needs_full_context_review",
            "observed_effect": "retry_after_tool_error",
            "event_refs": event_refs,
            "patch_hint": patch_hint,
        })
    }

    #[test]
    fn nudge_signal_is_dropped_when_successful_finish_exists() {
        let conn = conn();
        insert_learning_event(
            &conn,
            &LearningEvent {
                invocation_id: "inv-success".to_owned(),
                agent_name: "right".to_owned(),
                action: LearningAction::Create,
                skill_name: "rightx-demo".to_owned(),
                phase: LearningPhase::Finish,
                status: Some(LearningStatus::Created),
                reason: None,
                message: Some("Learned skill: rightx-demo".to_owned()),
                summary: Some("captured workflow".to_owned()),
                event_refs: vec!["event-1".to_owned()],
            },
        )
        .unwrap();

        let selected = select_reply_signal(
            &conn,
            "inv-success",
            Some(learning_signal(
                "explicit_user_request",
                vec!["event-1"],
                "Capture this workflow.",
            )),
            None,
        )
        .unwrap();

        assert!(selected.is_none());
    }

    #[test]
    fn nudge_signal_is_dropped_when_both_signals_present() {
        let conn = conn();
        let selected = select_reply_signal(
            &conn,
            "inv-both",
            Some(learning_signal(
                "explicit_user_request",
                vec!["event-1"],
                "Capture this workflow.",
            )),
            Some(skill_issue_signal(
                vec!["event-2"],
                "Patch the stale command.",
            )),
        )
        .unwrap();

        assert!(selected.is_none());
    }

    #[test]
    fn nudge_signal_requires_two_event_refs_unless_explicit_user_request() {
        let conn = conn();
        let dropped = select_reply_signal(
            &conn,
            "inv-short",
            Some(learning_signal(
                "multi_step_workflow",
                vec!["event-1"],
                "Capture this workflow.",
            )),
            None,
        )
        .unwrap();
        assert!(dropped.is_none());

        let accepted = select_reply_signal(
            &conn,
            "inv-explicit",
            Some(learning_signal(
                "explicit_user_request",
                vec!["event-1"],
                "Capture this workflow.",
            )),
            None,
        )
        .unwrap();
        assert_eq!(
            accepted.map(|(kind, _)| kind),
            Some(NudgeSignalKind::Learning)
        );

        let empty_summary = select_reply_signal(
            &conn,
            "inv-empty-summary",
            Some(learning_signal(
                "explicit_user_request",
                vec!["event-1"],
                "",
            )),
            None,
        )
        .unwrap();
        assert!(empty_summary.is_none());

        let empty_patch_hint = select_reply_signal(
            &conn,
            "inv-empty-patch",
            None,
            Some(skill_issue_signal(vec!["event-1", "event-2"], "")),
        )
        .unwrap();
        assert!(empty_patch_hint.is_none());
    }

    #[test]
    fn nudge_signal_rejects_empty_or_whitespace_event_refs() {
        let conn = conn();
        let explicit_with_blank_ref = select_reply_signal(
            &conn,
            "inv-blank-explicit",
            Some(learning_signal(
                "explicit_user_request",
                vec![" \t\n"],
                "Capture this workflow.",
            )),
            None,
        )
        .unwrap();
        assert!(explicit_with_blank_ref.is_none());

        let non_explicit_with_one_nonblank_ref = select_reply_signal(
            &conn,
            "inv-blank-non-explicit",
            Some(learning_signal(
                "multi_step_workflow",
                vec!["event-1", " "],
                "Capture this workflow.",
            )),
            None,
        )
        .unwrap();
        assert!(non_explicit_with_one_nonblank_ref.is_none());
    }

    #[test]
    fn nudge_signal_rejects_blank_ref_even_when_enough_nonblank_refs_exist() {
        let conn = conn();
        let selected = select_reply_signal(
            &conn,
            "inv-mixed-blank",
            Some(learning_signal(
                "multi_step_workflow",
                vec!["event-1", " ", "event-2"],
                "Capture this workflow.",
            )),
            None,
        )
        .unwrap();

        assert!(selected.is_none());
    }

    #[test]
    fn nudge_signal_rejects_non_string_event_ref() {
        let conn = conn();
        let selected = select_reply_signal(
            &conn,
            "inv-non-string-ref",
            Some(serde_json::json!({
                "kind": "create_candidate",
                "package_name_hint": "rightx-demo",
                "trigger": "multi_step_workflow",
                "reason_not_written": "needs_full_context_review",
                "event_refs": ["event-1", 42, "event-2"],
                "summary": "Capture this workflow.",
            })),
            None,
        )
        .unwrap();

        assert!(selected.is_none());
    }

    #[test]
    fn nudge_signal_accepts_valid_two_event_refs() {
        let conn = conn();
        let selected = select_reply_signal(
            &conn,
            "inv-two-refs",
            Some(learning_signal(
                "multi_step_workflow",
                vec!["event-1", "event-2"],
                "Capture this workflow.",
            )),
            None,
        )
        .unwrap();

        assert_eq!(
            selected.map(|(kind, _)| kind),
            Some(NudgeSignalKind::Learning)
        );
    }

    #[test]
    fn nudge_signal_requires_learned_skill_prefix() {
        let conn = conn();
        let create_without_prefix = select_reply_signal(
            &conn,
            "inv-create-prefix",
            Some(serde_json::json!({
                "kind": "create_candidate",
                "package_name_hint": "custom-demo",
                "trigger": "explicit_user_request",
                "reason_not_written": "needs_full_context_review",
                "event_refs": ["event-1"],
                "summary": "Capture this workflow.",
            })),
            None,
        )
        .unwrap();
        assert!(create_without_prefix.is_none());

        let update_without_prefix = select_reply_signal(
            &conn,
            "inv-update-prefix",
            None,
            Some(serde_json::json!({
                "kind": "update_candidate",
                "skill_name": "custom-demo",
                "issue": "stale_command",
                "reason_not_patched": "needs_full_context_review",
                "observed_effect": "retry_after_tool_error",
                "event_refs": ["event-1", "event-2"],
                "patch_hint": "Patch the stale command.",
            })),
        )
        .unwrap();
        assert!(update_without_prefix.is_none());
    }

    #[test]
    fn nudge_signal_rejects_invalid_enum_values() {
        let conn = conn();
        let invalid_trigger = select_reply_signal(
            &conn,
            "inv-invalid-trigger",
            Some(learning_signal(
                "agent_observed_repetition",
                vec!["event-1", "event-2"],
                "Capture this workflow.",
            )),
            None,
        )
        .unwrap();
        assert!(invalid_trigger.is_none());

        let invalid_learning_reason = select_reply_signal(
            &conn,
            "inv-invalid-learning-reason",
            Some(serde_json::json!({
                "kind": "create_candidate",
                "package_name_hint": "rightx-demo",
                "trigger": "explicit_user_request",
                "reason_not_written": "needs review",
                "event_refs": ["event-1"],
                "summary": "Capture this workflow.",
            })),
            None,
        )
        .unwrap();
        assert!(invalid_learning_reason.is_none());

        let invalid_issue = select_reply_signal(
            &conn,
            "inv-invalid-issue",
            None,
            Some(serde_json::json!({
                "kind": "update_candidate",
                "skill_name": "rightx-demo",
                "issue": "stale command",
                "reason_not_patched": "needs_full_context_review",
                "observed_effect": "retry_after_tool_error",
                "event_refs": ["event-1", "event-2"],
                "patch_hint": "Patch the stale command.",
            })),
        )
        .unwrap();
        assert!(invalid_issue.is_none());

        let invalid_observed_effect = select_reply_signal(
            &conn,
            "inv-invalid-observed-effect",
            None,
            Some(serde_json::json!({
                "kind": "update_candidate",
                "skill_name": "rightx-demo",
                "issue": "stale_command",
                "reason_not_patched": "needs_full_context_review",
                "observed_effect": "user had to retry",
                "event_refs": ["event-1", "event-2"],
                "patch_hint": "Patch the stale command.",
            })),
        )
        .unwrap();
        assert!(invalid_observed_effect.is_none());
    }

    #[test]
    fn gate_skips_when_daily_budget_exceeded() {
        let conn = conn();
        ensure_agent_nudge_state(&conn, "him");
        insert_usage(&conn, "2026-05-21T01:00:00Z", "learning_selector", 2.50);
        insert_usage(&conn, "2026-05-21T02:00:00Z", "learning_reviewer", 3.00);

        let input = ReviewGateInput {
            signal_trigger: Some(ReviewTriggerKind::EffortThreshold),
            now_utc: "2026-05-21T03:00:00Z",
            daily_budget_usd: 5.00,
        };
        let decision = try_mark_review_started(&conn, "him", input).unwrap();
        assert_eq!(
            decision,
            ReviewGateDecision::Skip(ReviewSkipReason::DailyBudget)
        );
    }

    #[test]
    fn gate_ignores_non_learning_sources_and_yesterdays_spend() {
        let conn = conn();
        ensure_agent_nudge_state(&conn, "him");
        insert_usage(&conn, "2026-05-20T23:59:00Z", "learning_selector", 10.00);
        insert_usage(&conn, "2026-05-21T01:00:00Z", "interactive", 10.00);
        insert_usage(&conn, "2026-05-21T02:00:00Z", "learning_selector", 1.00);

        let input = ReviewGateInput {
            signal_trigger: Some(ReviewTriggerKind::EffortThreshold),
            now_utc: "2026-05-21T03:00:00Z",
            daily_budget_usd: 5.00,
        };
        let decision = try_mark_review_started(&conn, "him", input).unwrap();
        assert!(matches!(decision, ReviewGateDecision::Start(_)));
    }

    #[test]
    fn gate_skips_when_circuit_open() {
        let conn = conn();
        ensure_agent_nudge_state(&conn, "him");
        conn.execute(
            "UPDATE skill_nudge_state SET review_circuit_open_until = ?1 WHERE agent_name = 'him'",
            ["2026-05-21T04:00:00Z"],
        )
        .unwrap();

        let input = ReviewGateInput {
            signal_trigger: Some(ReviewTriggerKind::EffortThreshold),
            now_utc: "2026-05-21T03:00:00Z",
            daily_budget_usd: 5.00,
        };
        let decision = try_mark_review_started(&conn, "him", input).unwrap();
        assert_eq!(
            decision,
            ReviewGateDecision::Skip(ReviewSkipReason::CircuitOpen)
        );
    }

    #[test]
    fn gate_clears_expired_circuit_and_resets_failure_count() {
        let conn = conn();
        ensure_agent_nudge_state(&conn, "him");
        conn.execute(
            "UPDATE skill_nudge_state SET \
                review_circuit_open_until = '2026-05-21T02:30:00Z', \
                consecutive_review_failures = 5 \
             WHERE agent_name = 'him'",
            [],
        )
        .unwrap();

        let input = ReviewGateInput {
            signal_trigger: Some(ReviewTriggerKind::EffortThreshold),
            now_utc: "2026-05-21T03:00:00Z",
            daily_budget_usd: 5.00,
        };
        let decision = try_mark_review_started(&conn, "him", input).unwrap();
        assert!(matches!(decision, ReviewGateDecision::Start(_)));

        let (open_until, count): (Option<String>, i64) = conn
            .query_row(
                "SELECT review_circuit_open_until, consecutive_review_failures \
                 FROM skill_nudge_state WHERE agent_name = 'him'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(open_until, None);
        assert_eq!(count, 0);
    }

    #[test]
    fn mark_review_finished_resets_circuit_and_failures() {
        let conn = conn();
        ensure_agent_nudge_state(&conn, "him");
        conn.execute(
            "UPDATE skill_nudge_state SET \
                review_running = 1, \
                consecutive_review_failures = 4, \
                review_circuit_open_until = '2026-05-21T05:00:00Z' \
             WHERE agent_name = 'him'",
            [],
        )
        .unwrap();

        mark_review_finished(
            &conn,
            "him",
            ReviewTriggerKind::EffortThreshold,
            ReviewStatus::NothingToLearn,
            false,
        )
        .unwrap();

        let (running, failures, open_until): (i64, i64, Option<String>) = conn
            .query_row(
                "SELECT review_running, consecutive_review_failures, review_circuit_open_until \
                 FROM skill_nudge_state WHERE agent_name = 'him'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(running, 0);
        assert_eq!(failures, 0);
        assert_eq!(open_until, None);
    }

    #[test]
    fn record_review_failure_increments_and_returns_opened_false_below_threshold() {
        let conn = conn();
        ensure_agent_nudge_state(&conn, "him");
        conn.execute(
            "UPDATE skill_nudge_state SET review_running = 1, consecutive_review_failures = 2 WHERE agent_name = 'him'",
            [],
        )
        .unwrap();

        let (count, opened) =
            record_review_failure(&conn, "him", "2026-05-21T03:00:00Z", 5, 60).unwrap();
        assert_eq!(count, 3);
        assert!(!opened);

        let (running, open_until): (i64, Option<String>) = conn
            .query_row(
                "SELECT review_running, review_circuit_open_until FROM skill_nudge_state WHERE agent_name = 'him'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(running, 0);
        assert_eq!(open_until, None);
    }

    #[test]
    fn record_review_failure_opens_circuit_exactly_at_threshold() {
        let conn = conn();
        ensure_agent_nudge_state(&conn, "him");
        conn.execute(
            "UPDATE skill_nudge_state SET review_running = 1, consecutive_review_failures = 4 WHERE agent_name = 'him'",
            [],
        )
        .unwrap();

        let (count, opened) =
            record_review_failure(&conn, "him", "2026-05-21T03:00:00Z", 5, 60).unwrap();
        assert_eq!(count, 5);
        assert!(opened);

        let open_until: Option<String> = conn
            .query_row(
                "SELECT review_circuit_open_until FROM skill_nudge_state WHERE agent_name = 'him'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(open_until.as_deref(), Some("2026-05-21T04:00:00Z"));
    }

    #[test]
    fn record_review_failure_does_not_reopen_already_open_circuit() {
        let conn = conn();
        ensure_agent_nudge_state(&conn, "him");
        conn.execute(
            "UPDATE skill_nudge_state SET \
                review_running = 1, \
                consecutive_review_failures = 7, \
                review_circuit_open_until = '2026-05-21T05:00:00Z' \
             WHERE agent_name = 'him'",
            [],
        )
        .unwrap();

        let (count, opened) =
            record_review_failure(&conn, "him", "2026-05-21T03:00:00Z", 5, 60).unwrap();
        assert_eq!(count, 8);
        assert!(!opened);

        let open_until: Option<String> = conn
            .query_row(
                "SELECT review_circuit_open_until FROM skill_nudge_state WHERE agent_name = 'him'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(open_until.as_deref(), Some("2026-05-21T05:00:00Z"));
    }

    #[test]
    fn record_review_failure_rejects_invalid_now_utc() {
        let conn = conn();
        ensure_agent_nudge_state(&conn, "him");
        let result = record_review_failure(&conn, "him", "not-a-timestamp", 5, 60);
        assert!(
            matches!(result, Err(_)),
            "expected Err for malformed now_utc, got {result:?}"
        );
    }

    #[test]
    fn gate_decision_rejects_invalid_now_utc() {
        let conn = conn();
        ensure_nudge_state(&conn, "right").unwrap();
        let result = review_gate_decision(
            &conn,
            "right",
            ReviewGateInput {
                signal_trigger: Some(ReviewTriggerKind::LearningSignal),
                now_utc: "not-a-timestamp",
                daily_budget_usd: 5.00,
            },
        );
        assert!(
            matches!(result, Err(_)),
            "expected Err for malformed now_utc, got {result:?}"
        );
    }
}
