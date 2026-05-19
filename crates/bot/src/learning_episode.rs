use std::collections::HashSet;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use anyhow::{Context as _, anyhow};
use right_agent::learned_skills::{
    ReviewGateDecision, ReviewGateInput, ReviewSkipReason, ReviewTriggerKind, clear_review_running,
    try_mark_review_started,
};
use right_agent::learning_episodes::{
    EpisodeSeedTriggerKind, ExecutionEventKind, LearningEpisodeKind, LearningEpisodeRow,
    LearningEpisodeStatus, NewLearningEpisodeSeed, SelectedEpisodeUpdate,
};

const LEARNING_EPISODE_REVIEW_DAILY_LIMIT: i64 = 12;
const EPISODE_SELECTOR_TIMEOUT_SECS: u64 = 120;

pub(crate) const EPISODE_SELECTOR_SCHEMA_JSON: &str = r#"{
  "type": "object",
  "properties": {
    "status": { "enum": ["selected", "no_episode", "insufficient_context", "failed"] },
    "kind": { "enum": ["foreground_thread", "async_continuation", "cron_run"] },
    "start_ref": { "type": ["string", "null"] },
    "end_ref": { "type": ["string", "null"] },
    "message_refs": { "type": "array", "items": { "type": "string" } },
    "execution_event_refs": { "type": "array", "items": { "type": "string" } },
    "boundary_rationale": { "type": ["string", "null"] },
    "confidence": { "enum": ["low", "medium", "high"] },
    "context_incomplete": { "type": "boolean" }
  },
  "required": ["status", "kind", "start_ref", "end_ref", "message_refs", "execution_event_refs", "boundary_rationale", "confidence", "context_incomplete"]
}"#;

#[derive(Debug, Clone, Copy)]
pub(crate) struct EpisodeSeedInput<'a> {
    pub(crate) agent_name: &'a str,
    pub(crate) kind: LearningEpisodeKind,
    pub(crate) seed_trigger_kind: EpisodeSeedTriggerKind,
    pub(crate) seed_ref: &'a str,
    pub(crate) target_chat_id: Option<i64>,
    pub(crate) target_thread_id: Option<i64>,
    pub(crate) settle_seconds: u64,
    pub(crate) now: &'a str,
}

#[derive(Debug, Clone)]
pub(crate) struct LearningEpisodeRuntime {
    pub(crate) agent_dir: PathBuf,
    pub(crate) agent_db_dir: PathBuf,
    pub(crate) agent_name: String,
    pub(crate) ssh_config_path: Option<PathBuf>,
    pub(crate) resolved_sandbox: Option<String>,
    pub(crate) debug: Arc<AtomicBool>,
    pub(crate) learning: right_agent::agent::types::LearningConfig,
}

impl LearningEpisodeRuntime {
    pub(crate) fn delayed_drain(self) {
        let delay = self.learning.episode_settle_seconds;
        std::mem::drop(tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(delay)).await;
            if let Err(e) = drain_ready_learning_episodes_once(self).await {
                tracing::warn!("learning episode drain failed: {e:#}");
            }
        }));
    }
}

pub(crate) fn capture_episode_seed(
    conn: &rusqlite::Connection,
    input: EpisodeSeedInput<'_>,
) -> Result<i64, rusqlite::Error> {
    let now = chrono::DateTime::parse_from_rfc3339(input.now)
        .map(|dt| dt.with_timezone(&chrono::Utc))
        .unwrap_or_else(|_| chrono::Utc::now());
    let ready_after = now + chrono::Duration::seconds(input.settle_seconds as i64);
    right_agent::learning_episodes::insert_pending_episode(
        conn,
        &NewLearningEpisodeSeed {
            agent_name: input.agent_name.to_owned(),
            kind: input.kind,
            seed_trigger_kind: input.seed_trigger_kind,
            seed_ref: input.seed_ref.to_owned(),
            target_chat_id: input.target_chat_id,
            target_thread_id: input.target_thread_id,
            ready_after: ready_after.format("%Y-%m-%dT%H:%M:%SZ").to_string(),
        },
    )
}

pub(crate) fn capture_episode_seed_and_spawn_drain(
    conn: &rusqlite::Connection,
    input: EpisodeSeedInput<'_>,
    runtime: LearningEpisodeRuntime,
) -> Result<i64, rusqlite::Error> {
    let episode_id = capture_episode_seed(conn, input)?;
    runtime.delayed_drain();
    Ok(episode_id)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CorpusMessage {
    pub(crate) id: i64,
    pub(crate) role: String,
    pub(crate) content: String,
    pub(crate) addressed_to_bot: bool,
    pub(crate) routed_to_agent: bool,
    pub(crate) created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CorpusExecutionEvent {
    pub(crate) id: i64,
    pub(crate) event_kind: ExecutionEventKind,
    pub(crate) trust_label: String,
    pub(crate) content_text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SelectorCorpus {
    pub(crate) kind: LearningEpisodeKind,
    pub(crate) messages: Vec<CorpusMessage>,
    pub(crate) execution_events: Vec<CorpusExecutionEvent>,
}

#[derive(Debug, Clone, serde::Serialize)]
struct SelectorPromptCorpus<'a> {
    kind: &'a str,
    seed_ref: &'a str,
    messages: Vec<SelectorPromptMessage<'a>>,
    execution_events: Vec<SelectorPromptExecutionEvent<'a>>,
}

#[derive(Debug, Clone, serde::Serialize)]
struct SelectorPromptMessage<'a> {
    reference: String,
    role: &'a str,
    content: &'a str,
    addressed_to_bot: bool,
    routed_to_agent: bool,
    created_at: &'a str,
}

#[derive(Debug, Clone, serde::Serialize)]
struct SelectorPromptExecutionEvent<'a> {
    reference: String,
    event_kind: &'a str,
    trust_label: &'a str,
    content_text: &'a str,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct EpisodeSelectorOutput {
    pub(crate) status: String,
    pub(crate) kind: LearningEpisodeKind,
    pub(crate) start_ref: Option<String>,
    pub(crate) end_ref: Option<String>,
    pub(crate) message_refs: Vec<String>,
    pub(crate) execution_event_refs: Vec<String>,
    pub(crate) boundary_rationale: Option<String>,
    pub(crate) confidence: String,
    pub(crate) context_incomplete: bool,
    pub(crate) raw: serde_json::Value,
}

impl SelectorCorpus {
    #[cfg(test)]
    pub(crate) fn for_test(
        message_refs: Vec<&str>,
        execution_event_refs: Vec<(&str, ExecutionEventKind)>,
    ) -> Self {
        Self {
            kind: LearningEpisodeKind::ForegroundThread,
            messages: message_refs
                .into_iter()
                .map(|reference| {
                    let id = reference
                        .strip_prefix("msg:")
                        .and_then(|id| id.parse::<i64>().ok())
                        .unwrap_or(0);
                    CorpusMessage {
                        id,
                        role: "user".to_owned(),
                        content: "test message".to_owned(),
                        addressed_to_bot: true,
                        routed_to_agent: true,
                        created_at: "2026-05-19T00:00:00Z".to_owned(),
                    }
                })
                .collect(),
            execution_events: execution_event_refs
                .into_iter()
                .map(|(reference, event_kind)| {
                    let id = reference
                        .strip_prefix("exec:")
                        .and_then(|id| id.parse::<i64>().ok())
                        .unwrap_or(0);
                    CorpusExecutionEvent {
                        id,
                        event_kind,
                        trust_label: "primary".to_owned(),
                        content_text: "test event".to_owned(),
                    }
                })
                .collect(),
        }
    }

    fn prompt_json(&self, episode: &LearningEpisodeRow) -> Result<String, serde_json::Error> {
        let prompt_corpus = SelectorPromptCorpus {
            kind: episode.kind.as_str(),
            seed_ref: &episode.seed_ref,
            messages: self
                .messages
                .iter()
                .map(|message| SelectorPromptMessage {
                    reference: message_ref(message.id),
                    role: &message.role,
                    content: &message.content,
                    addressed_to_bot: message.addressed_to_bot,
                    routed_to_agent: message.routed_to_agent,
                    created_at: &message.created_at,
                })
                .collect(),
            execution_events: self
                .execution_events
                .iter()
                .map(|event| SelectorPromptExecutionEvent {
                    reference: execution_event_ref(event.id),
                    event_kind: event.event_kind.as_str(),
                    trust_label: &event.trust_label,
                    content_text: &event.content_text,
                })
                .collect(),
        };
        serde_json::to_string_pretty(&prompt_corpus)
    }
}

impl EpisodeSelectorOutput {
    #[cfg(test)]
    pub(crate) fn for_test_selected(
        message_refs: Vec<&str>,
        execution_event_refs: Vec<&str>,
    ) -> Self {
        Self {
            status: "selected".to_owned(),
            kind: LearningEpisodeKind::ForegroundThread,
            start_ref: message_refs
                .first()
                .or_else(|| execution_event_refs.first())
                .map(|value| (*value).to_owned()),
            end_ref: message_refs
                .last()
                .or_else(|| execution_event_refs.last())
                .map(|value| (*value).to_owned()),
            message_refs: message_refs.into_iter().map(str::to_owned).collect(),
            execution_event_refs: execution_event_refs
                .into_iter()
                .map(str::to_owned)
                .collect(),
            boundary_rationale: Some("test".to_owned()),
            confidence: "high".to_owned(),
            context_incomplete: false,
            raw: serde_json::json!({"status":"selected"}),
        }
    }

    fn parse(raw: serde_json::Value) -> Result<Self, String> {
        let status = required_string(&raw, "status")?;
        let kind = parse_episode_kind(&required_string(&raw, "kind")?)?;
        let start_ref = optional_string(&raw, "start_ref");
        let end_ref = optional_string(&raw, "end_ref");
        let message_refs = string_array(&raw, "message_refs")?;
        let execution_event_refs = string_array(&raw, "execution_event_refs")?;
        let boundary_rationale = optional_string(&raw, "boundary_rationale");
        let confidence = required_string(&raw, "confidence")?;
        if !matches!(confidence.as_str(), "low" | "medium" | "high") {
            return Err("selector output confidence is invalid".to_owned());
        }
        let context_incomplete = raw
            .get("context_incomplete")
            .and_then(serde_json::Value::as_bool)
            .ok_or_else(|| "selector output context_incomplete must be a boolean".to_owned())?;

        Ok(Self {
            status,
            kind,
            start_ref,
            end_ref,
            message_refs,
            execution_event_refs,
            boundary_rationale,
            confidence,
            context_incomplete,
            raw,
        })
    }
}

pub(crate) fn validate_selector_output(
    corpus: &SelectorCorpus,
    output: &EpisodeSelectorOutput,
) -> Result<(), String> {
    if output.kind != corpus.kind {
        return Err("selector output kind does not match episode kind".to_owned());
    }

    let message_refs: HashSet<String> = corpus
        .messages
        .iter()
        .map(|message| message_ref(message.id))
        .collect();
    let execution_refs: HashSet<String> = corpus
        .execution_events
        .iter()
        .map(|event| execution_event_ref(event.id))
        .collect();

    for reference in &output.message_refs {
        if !message_refs.contains(reference) {
            return Err(format!(
                "selector chose message ref outside corpus: {reference}"
            ));
        }
    }
    for reference in &output.execution_event_refs {
        if !execution_refs.contains(reference) {
            return Err(format!(
                "selector chose execution event ref outside corpus: {reference}"
            ));
        }
    }
    for reference in output.start_ref.iter().chain(output.end_ref.iter()) {
        if !(message_refs.contains(reference) || execution_refs.contains(reference)) {
            return Err(format!("selector boundary ref outside corpus: {reference}"));
        }
    }

    if output.status == "selected" {
        let has_observable_execution = output.execution_event_refs.iter().any(|reference| {
            corpus.execution_events.iter().any(|event| {
                execution_event_ref(event.id) == *reference
                    && event.event_kind != ExecutionEventKind::Thinking
            })
        });
        if output.message_refs.is_empty() && !has_observable_execution {
            return Err("selector chose no observable episode evidence".to_owned());
        }
    }

    Ok(())
}

pub(crate) async fn drain_ready_learning_episodes_once(
    runtime: LearningEpisodeRuntime,
) -> anyhow::Result<()> {
    let now = chrono::Utc::now();
    let now_str = now.format("%Y-%m-%dT%H:%M:%SZ").to_string();
    let today = now.format("%Y-%m-%d").to_string();
    let (episode, corpus) = {
        let conn = right_db::open_connection(&runtime.agent_db_dir, false)
            .with_context(|| format!("open {} data.db", runtime.agent_name))?;
        let Some(episode) = right_agent::learning_episodes::claim_ready_episode(
            &conn,
            &runtime.agent_name,
            &now_str,
        )?
        else {
            return Ok(());
        };

        let gate = try_mark_review_started(
            &conn,
            &runtime.agent_name,
            ReviewGateInput {
                signal_trigger: review_trigger_for_episode(episode.seed_trigger_kind),
                today: &today,
                daily_limit: LEARNING_EPISODE_REVIEW_DAILY_LIMIT,
            },
        )?;

        match gate {
            ReviewGateDecision::Start(_) => {}
            ReviewGateDecision::Skip(
                ReviewSkipReason::AlreadyRunning | ReviewSkipReason::DailyLimit,
            ) => {
                requeue_episode(
                    &conn,
                    episode.id,
                    now,
                    runtime.learning.episode_settle_seconds,
                )?;
                return Ok(());
            }
            ReviewGateDecision::Skip(ReviewSkipReason::BelowThreshold) => {
                right_agent::learning_episodes::mark_episode_terminal(
                    &conn,
                    episode.id,
                    LearningEpisodeStatus::NoEpisode,
                    &serde_json::json!({"status":"no_episode","reason":"review gate below threshold"}),
                )?;
                return Ok(());
            }
        }

        let corpus = load_selector_corpus(&conn, &episode)?;
        (episode, corpus)
    };

    let output = run_episode_selector(&runtime, &episode, &corpus).await;
    let conn = right_db::open_connection(&runtime.agent_db_dir, false)
        .with_context(|| format!("reopen {} data.db", runtime.agent_name))?;
    match output {
        Ok(output) => {
            if let Err(e) = record_selector_output(&conn, &runtime, &episode, &corpus, output) {
                let reason = format!("{e:#}");
                right_agent::learning_episodes::mark_episode_failed(&conn, episode.id, &reason)?;
                clear_review_running(&conn, &runtime.agent_name)?;
                return Err(anyhow!(reason));
            }
        }
        Err(e) => {
            let reason = format!("{e:#}");
            right_agent::learning_episodes::mark_episode_failed(&conn, episode.id, &reason)?;
            clear_review_running(&conn, &runtime.agent_name)?;
            return Err(anyhow!(reason));
        }
    }

    Ok(())
}

fn record_selector_output(
    conn: &rusqlite::Connection,
    runtime: &LearningEpisodeRuntime,
    episode: &LearningEpisodeRow,
    corpus: &SelectorCorpus,
    output: EpisodeSelectorOutput,
) -> anyhow::Result<()> {
    match output.status.as_str() {
        "selected" => {
            validate_selector_output(corpus, &output).map_err(anyhow::Error::msg)?;
            let selection = SelectedEpisodeUpdate {
                start_ref: output.start_ref.clone(),
                end_ref: output.end_ref.clone(),
                message_refs: output.message_refs.clone(),
                execution_event_refs: output.execution_event_refs.clone(),
                selector_model: runtime.learning.episode_selector_model.clone(),
                selector_output_json: output.raw.clone(),
                boundary_rationale: output.boundary_rationale.clone(),
                confidence: Some(output.confidence.clone()),
                context_incomplete: output.context_incomplete,
                episode_hash: None,
                last_evidence_at: selected_last_evidence_at(corpus, &output),
            };
            right_agent::learning_episodes::mark_episode_selected(conn, episode.id, &selection)?;
            call_episode_reviewer_bridge_placeholder(conn, &runtime.agent_name, episode.id)?;
        }
        "no_episode" => {
            right_agent::learning_episodes::mark_episode_terminal(
                &conn,
                episode.id,
                LearningEpisodeStatus::NoEpisode,
                &output.raw,
            )?;
            clear_review_running(conn, &runtime.agent_name)?;
        }
        "insufficient_context" => {
            right_agent::learning_episodes::mark_episode_terminal(
                conn,
                episode.id,
                LearningEpisodeStatus::InsufficientContext,
                &output.raw,
            )?;
            clear_review_running(conn, &runtime.agent_name)?;
        }
        "failed" => {
            right_agent::learning_episodes::mark_episode_failed(
                conn,
                episode.id,
                "selector returned failed status",
            )?;
            clear_review_running(conn, &runtime.agent_name)?;
        }
        status => return Err(anyhow!("selector returned invalid status {status:?}")),
    }
    Ok(())
}

fn call_episode_reviewer_bridge_placeholder(
    conn: &rusqlite::Connection,
    agent_name: &str,
    episode_id: i64,
) -> Result<(), rusqlite::Error> {
    tracing::debug!(
        agent = %agent_name,
        episode_id,
        "learning episode reviewer bridge placeholder reached; Task 6 replaces this call point"
    );
    clear_review_running(conn, agent_name)
}

fn load_selector_corpus(
    conn: &rusqlite::Connection,
    episode: &LearningEpisodeRow,
) -> Result<SelectorCorpus, rusqlite::Error> {
    let messages = match (episode.target_chat_id, episode.target_thread_id) {
        (Some(chat_id), Some(thread_id)) => load_corpus_messages(conn, chat_id, thread_id)?,
        (Some(chat_id), None) => load_corpus_messages(conn, chat_id, 0)?,
        _ => Vec::new(),
    };
    let (root_session_id, invocation_id, async_run_id, cron_run_id) = corpus_scope_refs(episode);
    let execution_events = load_corpus_execution_events(
        conn,
        &episode.agent_name,
        root_session_id.as_deref(),
        invocation_id.as_deref(),
        async_run_id.as_deref(),
        cron_run_id.as_deref(),
    )?;
    Ok(SelectorCorpus {
        kind: episode.kind,
        messages,
        execution_events,
    })
}

fn load_corpus_messages(
    conn: &rusqlite::Connection,
    chat_id: i64,
    thread_id: i64,
) -> Result<Vec<CorpusMessage>, rusqlite::Error> {
    let mut stmt = conn.prepare(
        "SELECT id, role, content, addressed_to_bot, routed_to_agent, created_at
         FROM conversation_messages
         WHERE platform='telegram' AND chat_id=?1 AND thread_id=?2
         ORDER BY created_at DESC
         LIMIT 40",
    )?;
    stmt.query_map(rusqlite::params![chat_id, thread_id], |row| {
        let addressed_to_bot: i64 = row.get(3)?;
        let routed_to_agent: i64 = row.get(4)?;
        Ok(CorpusMessage {
            id: row.get(0)?,
            role: row.get(1)?,
            content: row.get(2)?,
            addressed_to_bot: addressed_to_bot != 0,
            routed_to_agent: routed_to_agent != 0,
            created_at: row.get(5)?,
        })
    })?
    .collect()
}

fn load_corpus_execution_events(
    conn: &rusqlite::Connection,
    agent_name: &str,
    root_session_id: Option<&str>,
    invocation_id: Option<&str>,
    async_run_id: Option<&str>,
    cron_run_id: Option<&str>,
) -> Result<Vec<CorpusExecutionEvent>, rusqlite::Error> {
    let mut stmt = conn.prepare(
        "SELECT id, event_kind, trust_label, content_text
         FROM execution_events
         WHERE agent_name=?1 AND (root_session_id=?2 OR invocation_id=?3 OR async_run_id=?4 OR cron_run_id=?5)
         ORDER BY seq
         LIMIT 120",
    )?;
    stmt.query_map(
        rusqlite::params![
            agent_name,
            root_session_id,
            invocation_id,
            async_run_id,
            cron_run_id
        ],
        |row| {
            Ok(CorpusExecutionEvent {
                id: row.get(0)?,
                event_kind: parse_execution_event_kind(row.get::<_, String>(1)?.as_str()),
                trust_label: row.get(2)?,
                content_text: row.get(3)?,
            })
        },
    )?
    .collect()
}

async fn run_episode_selector(
    runtime: &LearningEpisodeRuntime,
    episode: &LearningEpisodeRow,
    corpus: &SelectorCorpus,
) -> anyhow::Result<EpisodeSelectorOutput> {
    let corpus_json = corpus.prompt_json(episode)?;
    let prompt = build_selector_prompt(&corpus_json);
    let invocation = crate::cc::invocation::ClaudeInvocation {
        mcp_config_path: None,
        json_schema: Some(EPISODE_SELECTOR_SCHEMA_JSON.to_owned()),
        output_format: crate::cc::invocation::OutputFormat::Json,
        model: runtime.learning.episode_selector_model.clone(),
        max_budget_usd: Some(runtime.learning.episode_selector_max_budget_usd),
        max_turns: Some(3),
        resume_session_id: None,
        new_session_id: None,
        fork_session: false,
        allowed_tools: Vec::new(),
        disallowed_tools: crate::cc::invocation::disallow_background_review_mutation_tools(
            crate::cc::invocation::baseline_disallowed_tools(),
        ),
        extra_args: Vec::new(),
        prompt: Some(prompt),
        debug_flag: Some(Arc::clone(&runtime.debug)),
    };
    let args = invocation.into_args();
    let mut cmd = crate::cc::invocation::build_claude_command(
        &args,
        &runtime.agent_dir,
        runtime.ssh_config_path.as_deref(),
        runtime.resolved_sandbox.as_deref(),
    );
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    let mut child = right_process::ProcessGroupChild::spawn(cmd)
        .context("spawn learning episode selector claude")?;
    let output = tokio::time::timeout(
        Duration::from_secs(EPISODE_SELECTOR_TIMEOUT_SECS),
        child.wait_with_output(),
    )
    .await
    .context("learning episode selector claude timed out")?
    .context("wait for learning episode selector claude")?;
    if !output.status.success() {
        return Err(anyhow!(
            "learning episode selector claude exited {:?}: {}",
            output.status.code(),
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    let stdout = String::from_utf8(output.stdout).context("selector stdout was not UTF-8")?;
    parse_selector_process_stdout(&stdout).map_err(anyhow::Error::msg)
}

fn build_selector_prompt(corpus_json: &str) -> String {
    let mut prompt = String::new();
    prompt.push_str("# Learning Episode Selector\n\n");
    prompt.push_str(
        "Select the smallest useful learning episode from the corpus. \
         Use only refs present in the corpus. Thinking-only evidence is not observable. \
         Return no_episode when there is no actionable learning evidence, and \
         insufficient_context when the boundary cannot be selected from available evidence.\n\n",
    );
    prompt.push_str("corpus:\n");
    prompt.push_str(&right_prompt_safety::wrap_external(
        "learning-episode-selector/corpus",
        corpus_json,
    ));
    prompt
}

fn parse_selector_process_stdout(stdout: &str) -> Result<EpisodeSelectorOutput, String> {
    let root: serde_json::Value =
        serde_json::from_str(stdout).map_err(|e| format!("parse selector stdout JSON: {e}"))?;
    let selected = root
        .get("structured_output")
        .filter(|value| !value.is_null())
        .or_else(|| root.get("result").filter(|value| !value.is_null()))
        .unwrap_or(&root);
    let raw = match selected.as_str() {
        Some(json) => serde_json::from_str(json)
            .map_err(|e| format!("parse selector stdout wrapper JSON string: {e}"))?,
        None => selected.clone(),
    };
    EpisodeSelectorOutput::parse(raw)
}

fn requeue_episode(
    conn: &rusqlite::Connection,
    episode_id: i64,
    now: chrono::DateTime<chrono::Utc>,
    settle_seconds: u64,
) -> Result<(), rusqlite::Error> {
    let ready_after = (now + chrono::Duration::seconds(settle_seconds as i64))
        .format("%Y-%m-%dT%H:%M:%SZ")
        .to_string();
    let updated = conn.execute(
        "UPDATE learning_episodes
         SET status='pending', ready_after=?2, updated_at=strftime('%Y-%m-%dT%H:%M:%SZ','now')
         WHERE id=?1 AND status='selecting'",
        rusqlite::params![episode_id, ready_after],
    )?;
    if updated == 1 {
        Ok(())
    } else {
        Err(rusqlite::Error::QueryReturnedNoRows)
    }
}

fn review_trigger_for_episode(
    seed_trigger_kind: EpisodeSeedTriggerKind,
) -> Option<ReviewTriggerKind> {
    match seed_trigger_kind {
        EpisodeSeedTriggerKind::LearningSignal => Some(ReviewTriggerKind::LearningSignal),
        EpisodeSeedTriggerKind::SkillIssueSignal => Some(ReviewTriggerKind::SkillIssueSignal),
        EpisodeSeedTriggerKind::EffortThreshold => Some(ReviewTriggerKind::EffortThreshold),
        EpisodeSeedTriggerKind::Cron | EpisodeSeedTriggerKind::AsyncResult => None,
    }
}

fn corpus_scope_refs(
    episode: &LearningEpisodeRow,
) -> (
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
) {
    if let Some(invocation_id) = episode.seed_ref.strip_prefix("inv:") {
        return (None, Some(invocation_id.to_owned()), None, None);
    }
    if let Some(run_id) = episode.seed_ref.strip_prefix("cron:") {
        let run_id = run_id.to_owned();
        return (
            Some(run_id.clone()),
            None,
            Some(run_id.clone()),
            Some(run_id),
        );
    }
    if let Some(run_id) = episode.seed_ref.strip_prefix("async:") {
        let run_id = run_id.to_owned();
        return (Some(run_id.clone()), None, Some(run_id), None);
    }
    (None, None, None, None)
}

fn selected_last_evidence_at(
    corpus: &SelectorCorpus,
    output: &EpisodeSelectorOutput,
) -> Option<String> {
    output
        .message_refs
        .iter()
        .filter_map(|reference| {
            let id = reference.strip_prefix("msg:")?.parse::<i64>().ok()?;
            corpus
                .messages
                .iter()
                .find(|message| message.id == id)
                .map(|message| message.created_at.clone())
        })
        .max()
}

fn parse_episode_kind(value: &str) -> Result<LearningEpisodeKind, String> {
    match value {
        "foreground_thread" => Ok(LearningEpisodeKind::ForegroundThread),
        "async_continuation" => Ok(LearningEpisodeKind::AsyncContinuation),
        "cron_run" => Ok(LearningEpisodeKind::CronRun),
        _ => Err(format!("selector output kind is invalid: {value:?}")),
    }
}

fn parse_execution_event_kind(value: &str) -> ExecutionEventKind {
    match value {
        "assistant_text" => ExecutionEventKind::AssistantText,
        "thinking" => ExecutionEventKind::Thinking,
        "tool_call" => ExecutionEventKind::ToolCall,
        "tool_result" => ExecutionEventKind::ToolResult,
        "tool_error" => ExecutionEventKind::ToolError,
        "invocation_result" => ExecutionEventKind::InvocationResult,
        _ => ExecutionEventKind::Other,
    }
}

fn required_string(raw: &serde_json::Value, field: &str) -> Result<String, String> {
    raw.get(field)
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| format!("selector output {field} must be a non-empty string"))
}

fn optional_string(raw: &serde_json::Value, field: &str) -> Option<String> {
    raw.get(field)
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn string_array(raw: &serde_json::Value, field: &str) -> Result<Vec<String>, String> {
    raw.get(field)
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| format!("selector output {field} must be an array"))?
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
                .ok_or_else(|| format!("selector output {field} contains a non-string ref"))
        })
        .collect()
}

fn message_ref(id: i64) -> String {
    format!("msg:{id}")
}

fn execution_event_ref(id: i64) -> String {
    format!("exec:{id}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use right_agent::learning_episodes::{
        EpisodeSeedTriggerKind, ExecutionEventKind, LearningEpisodeKind,
    };

    fn conn() -> rusqlite::Connection {
        let mut conn = rusqlite::Connection::open_in_memory().unwrap();
        right_db::MIGRATIONS.to_latest(&mut conn).unwrap();
        conn
    }

    #[test]
    fn accepted_signal_creates_pending_seed_without_cooldown() {
        let conn = conn();
        capture_episode_seed(
            &conn,
            EpisodeSeedInput {
                agent_name: "right",
                kind: LearningEpisodeKind::ForegroundThread,
                seed_trigger_kind: EpisodeSeedTriggerKind::LearningSignal,
                seed_ref: "inv:inv-1",
                target_chat_id: Some(10),
                target_thread_id: Some(20),
                settle_seconds: 90,
                now: "2026-05-19T10:00:00Z",
            },
        )
        .unwrap();
        let row: (String, String) = conn
            .query_row(
                "SELECT status, ready_after FROM learning_episodes WHERE seed_ref='inv:inv-1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            row,
            ("pending".to_owned(), "2026-05-19T10:01:30Z".to_owned())
        );
    }

    #[test]
    fn selector_rejects_refs_outside_corpus() {
        let corpus = SelectorCorpus::for_test(vec!["msg:1"], vec![]);
        let output = EpisodeSelectorOutput::for_test_selected(vec!["msg:2"], vec![]);
        assert!(validate_selector_output(&corpus, &output).is_err());
    }

    #[test]
    fn selector_rejects_thinking_only_episode() {
        let corpus =
            SelectorCorpus::for_test(vec![], vec![("exec:10", ExecutionEventKind::Thinking)]);
        let output = EpisodeSelectorOutput::for_test_selected(vec![], vec!["exec:10"]);
        assert!(validate_selector_output(&corpus, &output).is_err());
    }
}
