use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use anyhow::{Context as _, anyhow};
use right_agent::learned_skills::{
    ReviewGateDecision, ReviewGateInput, ReviewSkipReason, ReviewStatus, ReviewTriggerKind,
    clear_review_running, insert_skill_review_report, mark_review_finished_in_tx,
    record_review_failure, try_mark_review_started,
};
use right_agent::learning_episodes::{
    EpisodeSeedTriggerKind, ExecutionEventKind, LearningEpisodeKind, LearningEpisodeRow,
    LearningEpisodeStatus, NewLearningEpisodeSeed, SelectedEpisodeUpdate,
};
use rusqlite::OptionalExtension as _;
use sha2::{Digest as _, Sha256};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
const EPISODE_SELECTOR_TIMEOUT_SECS: u64 = 120;
const EPISODE_REVIEWER_TIMEOUT_SECS: u64 = 180;
const EPISODE_REVIEWER_MAX_BUDGET_USD: f64 = 0.50;
const EPISODE_REVIEWER_MAX_TURNS: u32 = 8;
const EPISODE_REVIEW_LEARNING_EVENTS_LIMIT: i64 = 20;

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

/// Single per-agent debounced drain scheduler.
///
/// A `Notify`-style coalescing trigger backed by an unbounded `mpsc` channel.
/// Each call to [`schedule_drain`](Self::schedule_drain) is a non-blocking
/// `tx.send(())`; the spawned task collapses N rapid notifications into one
/// drain pass after `settle` elapses. This replaces the previous per-seed
/// `tokio::spawn(sleep + drain)` pattern which spawned a fresh timer (and a
/// fresh `rusqlite::Connection`) for every seed captured in a burst.
///
/// Lifetime: spawned once at bot startup, cancelled via `CancellationToken`
/// on shutdown. The `Arc<DrainScheduler>` is threaded through every callsite
/// that captures an episode seed.
///
/// A `noop()` variant carries `tx = None` and short-circuits
/// `schedule_drain`. This is used when `learning.background_review_enabled =
/// false` so the legacy Stage 2 selector/reviewer pipeline never runs;
/// per-seed captures and reviewer requeues still call into this struct, they
/// just do nothing.
#[derive(Debug)]
pub(crate) struct DrainScheduler {
    tx: Option<mpsc::UnboundedSender<()>>,
}

impl DrainScheduler {
    /// Spawn the per-agent drain task. Returns the `Arc<DrainScheduler>` that
    /// callers clone into runtimes plus the `JoinHandle<()>` for the spawned
    /// task. The task exits when `shutdown` is cancelled or all senders are
    /// dropped.
    ///
    /// The handle must be retained and awaited at shutdown — without it, the
    /// drain task detaches and can keep a CC selector/reviewer child alive
    /// past `run_telegram`'s return. Both the outer recv/sleep `select!`s and
    /// the inner pass invocation race against `shutdown.cancelled()`, so a
    /// mid-pass cancel drops the pass future and the in-flight
    /// `ProcessGroupChild::Drop` kills the child's process group.
    pub(crate) fn spawn(
        runtime_factory: LearningEpisodeRuntime,
        settle: Duration,
        shutdown: CancellationToken,
    ) -> (Arc<Self>, tokio::task::JoinHandle<()>) {
        let (tx, rx) = mpsc::unbounded_channel::<()>();
        let scheduler = Arc::new(Self { tx: Some(tx) });
        // The runtime used inside the task carries its own scheduler clone so
        // that the reviewer's requeue path can re-notify on `Skip(AlreadyRunning)`.
        let mut task_runtime = runtime_factory;
        task_runtime.scheduler = Some(Arc::clone(&scheduler));
        let handle = tokio::spawn(run_drain_scheduler_task(task_runtime, rx, settle, shutdown));
        (scheduler, handle)
    }

    /// Inactive scheduler. `schedule_drain` is a cheap no-op; no task is
    /// spawned. Use when `learning.background_review_enabled = false`.
    pub(crate) fn noop() -> Self {
        Self { tx: None }
    }

    /// Non-blocking notify. Multiple calls before the next drain coalesce
    /// into a single drain pass. Never blocks, never spawns. No-op for the
    /// `noop()` variant.
    pub(crate) fn schedule_drain(&self) {
        if let Some(tx) = self.tx.as_ref() {
            // Unbounded send only fails when the receiver has been dropped
            // (task exited at shutdown). Silently discard — there is nothing
            // to do.
            let _ = tx.send(());
        }
    }
}

async fn run_drain_scheduler_task(
    runtime: LearningEpisodeRuntime,
    mut rx: mpsc::UnboundedReceiver<()>,
    settle: Duration,
    shutdown: CancellationToken,
) {
    loop {
        // Wait for the first notification (or shutdown).
        tokio::select! {
            _ = shutdown.cancelled() => return,
            recv = rx.recv() => {
                if recv.is_none() {
                    return;
                }
            }
        }
        // Debounce: sleep `settle` so subsequent rapid notifications collapse.
        tokio::select! {
            _ = shutdown.cancelled() => return,
            _ = tokio::time::sleep(settle) => {}
        }
        // Drain any accumulated notifications so they collapse into this pass.
        while rx.try_recv().is_ok() {}
        // Race the pass against shutdown — on cancel we drop the pass future,
        // which drops any in-flight `ProcessGroupChild` and kills the child's
        // process group via the existing `Drop` impl. The selector/reviewer
        // CC invocations otherwise hold this task for up to
        // `EPISODE_SELECTOR_TIMEOUT_SECS + EPISODE_REVIEWER_TIMEOUT_SECS`.
        tokio::select! {
            _ = shutdown.cancelled() => return,
            result = drain_ready_learning_episodes_once(runtime.clone()) => {
                if let Err(e) = result {
                    tracing::warn!("learning episode drain failed: {e:#}");
                }
            }
        }
    }
}

#[derive(Clone)]
pub(crate) struct LearningEpisodeRuntime {
    pub(crate) agent_dir: PathBuf,
    pub(crate) agent_db_dir: PathBuf,
    pub(crate) agent_name: String,
    pub(crate) inherited_model: Option<String>,
    pub(crate) ssh_config_path: Option<PathBuf>,
    pub(crate) resolved_sandbox: Option<String>,
    pub(crate) debug: Arc<AtomicBool>,
    pub(crate) learning: right_agent::agent::types::LearningConfig,
    /// Per-agent debounced drain trigger. `None` only for tests and for the
    /// initial bootstrap runtime passed into [`DrainScheduler::spawn`] before
    /// the scheduler exists. Production capture callsites must always carry
    /// `Some(...)`.
    pub(crate) scheduler: Option<Arc<DrainScheduler>>,
    /// Bot for sending circuit-open alerts. `None` for seed-only runtimes;
    /// the drain task that fires alerts always carries `Some`.
    pub(crate) bot: Option<Arc<crate::telegram::BotType>>,
}

impl std::fmt::Debug for LearningEpisodeRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LearningEpisodeRuntime")
            .field("agent_dir", &self.agent_dir)
            .field("agent_db_dir", &self.agent_db_dir)
            .field("agent_name", &self.agent_name)
            .field("inherited_model", &self.inherited_model)
            .field("ssh_config_path", &self.ssh_config_path)
            .field("resolved_sandbox", &self.resolved_sandbox)
            .field("learning", &self.learning)
            .field("scheduler", &self.scheduler.is_some())
            .field("bot", &"<Bot>")
            .finish()
    }
}

impl LearningEpisodeRuntime {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        agent_dir: PathBuf,
        agent_db_dir: PathBuf,
        agent_name: String,
        inherited_model: Option<String>,
        ssh_config_path: Option<PathBuf>,
        resolved_sandbox: Option<String>,
        debug: Arc<AtomicBool>,
        learning: right_agent::agent::types::LearningConfig,
        scheduler: Option<Arc<DrainScheduler>>,
        bot: Option<Arc<crate::telegram::BotType>>,
    ) -> Self {
        Self {
            agent_dir,
            agent_db_dir,
            agent_name,
            inherited_model,
            ssh_config_path,
            resolved_sandbox,
            debug,
            learning,
            scheduler,
            bot,
        }
    }

    /// Build an `EpisodeSeedInput` from this runtime + per-callsite fields,
    /// insert the seed row, and notify the per-agent debounced drain task.
    /// Centralises the `chrono::Utc::now()` formatting and `EpisodeSeedInput`
    /// construction shared by every completion-seed callsite.
    pub(crate) fn capture_completion_seed(
        &self,
        conn: &rusqlite::Connection,
        kind: LearningEpisodeKind,
        seed_trigger_kind: EpisodeSeedTriggerKind,
        seed_ref: &str,
        target_chat_id: Option<i64>,
        target_thread_id: Option<i64>,
    ) -> Result<i64, rusqlite::Error> {
        let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
        let input = EpisodeSeedInput {
            agent_name: &self.agent_name,
            kind,
            seed_trigger_kind,
            seed_ref,
            target_chat_id,
            target_thread_id,
            settle_seconds: self.learning.episode_settle_seconds,
            now: &now,
        };
        let episode_id = capture_episode_seed(conn, input)?;
        if let Some(scheduler) = &self.scheduler {
            scheduler.schedule_drain();
        }
        Ok(episode_id)
    }

    /// Trigger a debounced drain pass on the per-agent scheduler. No-op when
    /// no scheduler is attached (tests, bootstrap runtime).
    pub(crate) fn schedule_drain(&self) {
        if let Some(scheduler) = &self.scheduler {
            scheduler.schedule_drain();
        }
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CorpusMessage {
    pub(crate) id: i64,
    pub(crate) role: String,
    pub(crate) content: String,
    pub(crate) addressed_to_bot: bool,
    pub(crate) routed_to_agent: bool,
    pub(crate) root_session_id: Option<String>,
    pub(crate) turn_id: Option<i64>,
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
    trust_label: &'a str,
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
                        root_session_id: None,
                        turn_id: None,
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
                    trust_label: message_trust_label(message),
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
        if output.start_ref.is_none() || output.end_ref.is_none() {
            return Err("selected output requires start_ref and end_ref".to_owned());
        }
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
    drain_ready_learning_episodes_once_with_selector(runtime, run_episode_selector_boxed).await
}

type EpisodeSelectorFuture =
    Pin<Box<dyn Future<Output = anyhow::Result<EpisodeSelectorOutput>> + Send>>;
type EpisodeSelector =
    fn(LearningEpisodeRuntime, LearningEpisodeRow, SelectorCorpus) -> EpisodeSelectorFuture;
type EpisodeReviewerFuture = Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send>>;
type EpisodeReviewer = fn(LearningEpisodeRuntime, i64) -> EpisodeReviewerFuture;
type EpisodeReviewInvocationFuture =
    Pin<Box<dyn Future<Output = anyhow::Result<crate::learning_review::ReviewOutput>> + Send>>;
type EpisodeReviewInvocation = fn(
    LearningEpisodeRuntime,
    crate::learning_review::ReviewBundle,
) -> EpisodeReviewInvocationFuture;

fn run_episode_selector_boxed(
    runtime: LearningEpisodeRuntime,
    episode: LearningEpisodeRow,
    corpus: SelectorCorpus,
) -> EpisodeSelectorFuture {
    Box::pin(async move { run_episode_selector(&runtime, &episode, &corpus).await })
}

fn run_episode_reviewer_boxed(
    runtime: LearningEpisodeRuntime,
    episode_id: i64,
) -> EpisodeReviewerFuture {
    Box::pin(async move { run_episode_reviewer(runtime, episode_id).await })
}

fn run_episode_review_invocation_boxed(
    runtime: LearningEpisodeRuntime,
    bundle: crate::learning_review::ReviewBundle,
) -> EpisodeReviewInvocationFuture {
    Box::pin(async move { run_episode_review_invocation(&runtime, &bundle).await })
}

pub(crate) async fn drain_ready_learning_episodes_once_with_selector(
    runtime: LearningEpisodeRuntime,
    selector: EpisodeSelector,
) -> anyhow::Result<()> {
    drain_ready_learning_episodes_once_with_selector_and_reviewer(
        runtime,
        selector,
        run_episode_reviewer_boxed,
    )
    .await
}

async fn drain_ready_learning_episodes_once_with_selector_and_reviewer(
    runtime: LearningEpisodeRuntime,
    selector: EpisodeSelector,
    reviewer: EpisodeReviewer,
) -> anyhow::Result<()> {
    let now = chrono::Utc::now();
    let now_str = now.format("%Y-%m-%dT%H:%M:%SZ").to_string();
    let conn = right_db::open_connection(&runtime.agent_db_dir, false)
        .with_context(|| format!("open {} data.db", runtime.agent_name))?;
    let Some(episode) =
        right_agent::learning_episodes::claim_ready_episode(&conn, &runtime.agent_name, &now_str)?
    else {
        return Ok(());
    };

    let gate = match try_mark_review_started(
        &conn,
        &runtime.agent_name,
        ReviewGateInput {
            signal_trigger: review_trigger_for_episode(episode.seed_trigger_kind),
            now_utc: &now_str,
            daily_budget_usd: runtime.learning.max_daily_budget_usd,
        },
    ) {
        Ok(gate) => gate,
        Err(e) => {
            let reason = format!("learning episode review gate failed: {e:#}");
            // Gate acquisition failed — we never entered the gate, so do not
            // record a failure against the circuit breaker.
            mark_claimed_episode_failed(&conn, &runtime, episode.id, &reason, &now_str, false)?;
            return Err(anyhow!(reason));
        }
    };

    match gate {
        ReviewGateDecision::Start(_) => {}
        ReviewGateDecision::Skip(
            ReviewSkipReason::AlreadyRunning
            | ReviewSkipReason::DailyBudget
            | ReviewSkipReason::CircuitOpen,
        ) => {
            requeue_episode_or_fail(
                &conn,
                &runtime,
                episode.id,
                now,
                runtime.learning.episode_settle_seconds,
                &now_str,
            )?;
            runtime.schedule_drain();
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

    let corpus = match load_selector_corpus(&conn, &episode) {
        Ok(corpus) => corpus,
        Err(e) => {
            let reason = format!("learning episode corpus load failed: {e:#}");
            let outcome =
                mark_claimed_episode_failed(&conn, &runtime, episode.id, &reason, &now_str, true)?;
            if let Some((_, true)) = outcome {
                spawn_circuit_open_alert(&runtime, &reason);
            }
            return Err(anyhow!(reason));
        }
    };

    let output = selector(runtime.clone(), episode.clone(), corpus.clone()).await;
    match output {
        Ok(output) => {
            let selected = match record_selector_output(&conn, &runtime, &episode, &corpus, output)
            {
                Ok(selected) => selected,
                Err(e) => {
                    let reason = format!("{e:#}");
                    let outcome = mark_claimed_episode_failed(
                        &conn, &runtime, episode.id, &reason, &now_str, true,
                    )?;
                    if let Some((_, true)) = outcome {
                        spawn_circuit_open_alert(&runtime, &reason);
                    }
                    return Err(anyhow!(reason));
                }
            };
            drop(conn);
            if selected && let Err(e) = reviewer(runtime.clone(), episode.id).await {
                let reason = format!("{e:#}");
                return Err(anyhow!(reason));
            }
        }
        Err(e) => {
            let reason = format!("{e:#}");
            let outcome =
                mark_claimed_episode_failed(&conn, &runtime, episode.id, &reason, &now_str, true)?;
            if let Some((_, true)) = outcome {
                spawn_circuit_open_alert(&runtime, &reason);
            }
            return Err(anyhow!(reason));
        }
    }

    Ok(())
}

/// Mark a claimed episode as failed and optionally drive the circuit-breaker.
///
/// When `record_failure = false` the episode is marked failed but the gate is
/// left untouched (used when we never entered the gate, e.g. gate-acquisition
/// itself errored out, or the requeue fallback failed after a Skip decision).
///
/// When `record_failure = true` the episode write is committed first, then
/// `record_review_failure` is called in its own transaction.  Atomicity between
/// the two writes is intentionally lost — the rows live in different tables and
/// a crash between them leaves the episode failed but the circuit counter
/// untouched, which is self-healing: the next failure will still increment the
/// counter.  Returns `Some((new_count, opened_now))` when `record_failure =
/// true`, `None` otherwise.
fn mark_claimed_episode_failed(
    conn: &rusqlite::Connection,
    runtime: &LearningEpisodeRuntime,
    episode_id: i64,
    reason: &str,
    now_utc: &str,
    record_failure: bool,
) -> anyhow::Result<Option<(i64, bool)>> {
    // First write: mark the episode row failed.
    {
        let tx = conn.unchecked_transaction()?;
        right_agent::learning_episodes::mark_episode_failed(&tx, episode_id, reason)
            .with_context(|| format!("mark learning episode {episode_id} failed"))?;
        tx.commit()?;
    }
    if !record_failure {
        return Ok(None);
    }
    // Second write: drive the circuit-breaker gate (its own transaction).
    let (count, opened) = record_review_failure(
        conn,
        &runtime.agent_name,
        now_utc,
        runtime.learning.circuit_failure_threshold,
        runtime.learning.circuit_cooldown_minutes,
    )
    .with_context(|| format!("record review failure for {}", runtime.agent_name))?;
    Ok(Some((count, opened)))
}

/// Spawn a fire-and-forget task to send the circuit-open Telegram alert.
/// Called whenever `record_review_failure` returns `opened_circuit = true`.
fn spawn_circuit_open_alert(runtime: &LearningEpisodeRuntime, reason: &str) {
    let Some(bot) = runtime.bot.as_ref().map(Arc::clone) else {
        return;
    };
    crate::telegram::learning_alerts::spawn_circuit_open_alert(
        bot,
        runtime.agent_db_dir.clone(),
        runtime.agent_name.clone(),
        runtime.agent_dir.clone(),
        reason.to_owned(),
        runtime.learning.circuit_failure_threshold,
        runtime.learning.circuit_cooldown_minutes,
    );
}

fn record_selector_output(
    conn: &rusqlite::Connection,
    runtime: &LearningEpisodeRuntime,
    episode: &LearningEpisodeRow,
    corpus: &SelectorCorpus,
    output: EpisodeSelectorOutput,
) -> anyhow::Result<bool> {
    validate_selector_output(corpus, &output).map_err(anyhow::Error::msg)?;

    match output.status.as_str() {
        "selected" => {
            let episode_hash = compute_episode_hash(episode, &output);
            if let Some(status) =
                find_duplicate_episode_status(conn, &runtime.agent_name, episode.id, &episode_hash)?
            {
                let output_json = serde_json::json!({
                    "status": "no_episode",
                    "reason": "duplicate_episode",
                    "duplicate_status": status,
                    "episode_hash": episode_hash,
                });
                let tx = conn.unchecked_transaction()?;
                right_agent::learning_episodes::mark_episode_terminal(
                    &tx,
                    episode.id,
                    LearningEpisodeStatus::NoEpisode,
                    &output_json,
                )?;
                clear_review_running(&tx, &runtime.agent_name)?;
                tx.commit()?;
                return Ok(false);
            }
            let selection = SelectedEpisodeUpdate {
                start_ref: output.start_ref.clone(),
                end_ref: output.end_ref.clone(),
                message_refs: output.message_refs.clone(),
                execution_event_refs: output.execution_event_refs.clone(),
                selector_model: effective_selector_model(runtime),
                selector_output_json: output.raw.clone(),
                boundary_rationale: output.boundary_rationale.clone(),
                confidence: Some(output.confidence.clone()),
                context_incomplete: output.context_incomplete,
                episode_hash: Some(episode_hash),
                last_evidence_at: selected_last_evidence_at(corpus, &output),
            };
            right_agent::learning_episodes::mark_episode_selected(conn, episode.id, &selection)?;
            Ok(true)
        }
        "no_episode" => {
            let tx = conn.unchecked_transaction()?;
            right_agent::learning_episodes::mark_episode_terminal(
                &tx,
                episode.id,
                LearningEpisodeStatus::NoEpisode,
                &output.raw,
            )?;
            clear_review_running(&tx, &runtime.agent_name)?;
            tx.commit()?;
            Ok(false)
        }
        "insufficient_context" => {
            let tx = conn.unchecked_transaction()?;
            right_agent::learning_episodes::mark_episode_terminal(
                &tx,
                episode.id,
                LearningEpisodeStatus::InsufficientContext,
                &output.raw,
            )?;
            clear_review_running(&tx, &runtime.agent_name)?;
            tx.commit()?;
            Ok(false)
        }
        "failed" => {
            let tx = conn.unchecked_transaction()?;
            right_agent::learning_episodes::mark_episode_failed(
                &tx,
                episode.id,
                "selector returned failed status",
            )?;
            clear_review_running(&tx, &runtime.agent_name)?;
            tx.commit()?;
            Ok(false)
        }
        status => return Err(anyhow!("selector returned invalid status {status:?}")),
    }
}

fn effective_selector_model(runtime: &LearningEpisodeRuntime) -> Option<String> {
    runtime
        .learning
        .episode_selector_model
        .clone()
        .or_else(|| runtime.inherited_model.clone())
}

fn compute_episode_hash(episode: &LearningEpisodeRow, output: &EpisodeSelectorOutput) -> String {
    let mut hasher = Sha256::new();
    hash_part(&mut hasher, &episode.agent_name);
    hash_part(&mut hasher, episode.kind.as_str());
    hash_part(&mut hasher, episode.seed_trigger_kind.as_str());
    for reference in &output.message_refs {
        hash_part(&mut hasher, reference);
    }
    hash_part(&mut hasher, "");
    for reference in &output.execution_event_refs {
        hash_part(&mut hasher, reference);
    }
    hex_lower(&hasher.finalize())
}

fn hash_part(hasher: &mut Sha256, value: &str) {
    hasher.update(value.len().to_le_bytes());
    hasher.update(value.as_bytes());
}

fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

fn find_duplicate_episode_status(
    conn: &rusqlite::Connection,
    agent_name: &str,
    current_episode_id: i64,
    episode_hash: &str,
) -> Result<Option<String>, rusqlite::Error> {
    conn.query_row(
        "SELECT status FROM learning_episodes
         WHERE agent_name=?1
           AND episode_hash=?2
           AND id<>?3
           AND status IN ('pending','selecting','selected','reviewing','reviewed')
         ORDER BY CASE status WHEN 'reviewed' THEN 0 ELSE 1 END, id ASC
         LIMIT 1",
        rusqlite::params![agent_name, episode_hash, current_episode_id],
        |row| row.get(0),
    )
    .optional()
}

pub(crate) async fn run_episode_reviewer(
    runtime: LearningEpisodeRuntime,
    episode_id: i64,
) -> anyhow::Result<()> {
    run_episode_reviewer_with_invocation(runtime, episode_id, run_episode_review_invocation_boxed)
        .await
}

async fn run_episode_reviewer_with_invocation(
    runtime: LearningEpisodeRuntime,
    episode_id: i64,
    invoke_review: EpisodeReviewInvocation,
) -> anyhow::Result<()> {
    let result = run_episode_reviewer_inner(runtime.clone(), episode_id, invoke_review).await;
    if let Err(e) = &result {
        let reason = format!("{e:#}");
        match mark_episode_review_failed_and_finish(&runtime, episode_id, &reason) {
            Ok(outcome) => {
                if let Some((_, true)) = outcome {
                    spawn_circuit_open_alert(&runtime, &reason);
                }
            }
            Err(cleanup) => {
                tracing::warn!(
                    agent = %runtime.agent_name,
                    episode_id,
                    "learning episode review failure cleanup failed: {cleanup:#}"
                );
            }
        }
    }
    result
}

async fn run_episode_reviewer_inner(
    runtime: LearningEpisodeRuntime,
    episode_id: i64,
    invoke_review: EpisodeReviewInvocation,
) -> anyhow::Result<()> {
    let conn = right_db::open_connection(&runtime.agent_db_dir, false)
        .with_context(|| format!("open {} data.db", runtime.agent_name))?;
    let episode = load_selected_episode_for_review(&conn, episode_id)?;
    let trigger_kind = review_trigger_for_episode(episode.seed_trigger_kind)
        .ok_or_else(|| anyhow!("learning episode {episode_id} has no review trigger"))?;
    mark_episode_reviewing(&conn, episode_id)?;
    let selected = load_selected_review_evidence(&conn, &episode)?;
    let learning_events =
        load_episode_review_learning_events(&conn, &episode.source_invocation_id)?;
    drop(conn);

    let learned_skills = collect_episode_review_skill_index(&runtime).await?;
    let bundle = crate::learning_review::ReviewBundle {
        agent_name: episode.agent_name.clone(),
        source_invocation_id: episode.source_invocation_id.clone(),
        learning_episode_id: Some(episode.id),
        root_session_id: selected.root_session_id.clone(),
        trigger_kind: trigger_kind.as_str().to_owned(),
        accepted_signal_json: None,
        tool_iters_since_review: 0,
        turns_since_review: 0,
        skill_issue_hints_since_review: 0,
        episode_messages: selected.messages,
        episode_execution_events: selected.execution_events,
        learning_events,
        learned_skills,
    };
    let output = invoke_review(runtime.clone(), bundle).await?;
    output
        .validate_candidate_evidence(&selected.evidence_index)
        .map_err(anyhow::Error::msg)?;

    let conn = right_db::open_connection(&runtime.agent_db_dir, false)
        .with_context(|| format!("reopen {} data.db", runtime.agent_name))?;
    let report = output.to_report(crate::learning_review::ReviewReportContext {
        agent_name: episode.agent_name.clone(),
        source_invocation_id: episode.source_invocation_id,
        learning_episode_id: Some(episode.id),
        root_session_id: selected.root_session_id,
        chat_id: episode.target_chat_id,
        thread_id: episode.target_thread_id,
        trigger_kind,
        telegram_notified: false,
    });
    let status = report.status;
    if status == ReviewStatus::Failed {
        // Reviewer ran successfully but produced a Failed verdict — still a
        // failure-to-learn, so drive the circuit breaker.  Commit the episode /
        // report writes first, then record_review_failure in its own tx.
        // Atomicity loss is acceptable (independent tables, self-healing).
        {
            let tx = conn.unchecked_transaction()?;
            insert_skill_review_report(&tx, &report)
                .with_context(|| format!("insert learning episode {episode_id} review report"))?;
            right_agent::learning_episodes::mark_episode_failed(
                &tx,
                episode_id,
                "reviewer returned failed status",
            )
            .with_context(|| format!("mark learning episode {episode_id} failed"))?;
            tx.commit()?;
        }
        let now_utc = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
        let (_, opened) = record_review_failure(
            &conn,
            &episode.agent_name,
            &now_utc,
            runtime.learning.circuit_failure_threshold,
            runtime.learning.circuit_cooldown_minutes,
        )
        .with_context(|| {
            format!(
                "record review failure for {} (reviewer returned Failed, episode {episode_id})",
                episode.agent_name
            )
        })?;
        if opened {
            spawn_circuit_open_alert(&runtime, "reviewer returned status=failed");
        }
        return Ok(());
    }
    let tx = conn.unchecked_transaction()?;
    insert_skill_review_report(&tx, &report)
        .with_context(|| format!("insert learning episode {episode_id} review report"))?;
    mark_episode_reviewed(&tx, episode_id)
        .with_context(|| format!("mark learning episode {episode_id} reviewed"))?;
    mark_review_finished_in_tx(
        &tx,
        &episode.agent_name,
        trigger_kind,
        status,
        status != ReviewStatus::Failed,
    )
    .with_context(|| format!("finish learning episode {episode_id} review gate"))?;
    tx.commit()?;
    Ok(())
}

async fn run_episode_review_invocation(
    runtime: &LearningEpisodeRuntime,
    bundle: &crate::learning_review::ReviewBundle,
) -> anyhow::Result<crate::learning_review::ReviewOutput> {
    let prompt = crate::learning_review::build_review_prompt(bundle);
    let invocation = crate::cc::invocation::ClaudeInvocation {
        mcp_config_path: None,
        json_schema: Some(crate::learning_review::REVIEW_SCHEMA_JSON.to_owned()),
        output_format: crate::cc::invocation::OutputFormat::Json,
        model: runtime.inherited_model.clone(),
        max_budget_usd: Some(EPISODE_REVIEWER_MAX_BUDGET_USD),
        max_turns: Some(EPISODE_REVIEWER_MAX_TURNS),
        resume_session_id: None,
        new_session_id: None,
        fork_session: false,
        allowed_tools: Vec::new(),
        disallowed_tools: Vec::new(),
        extra_args: crate::cc::invocation::disable_all_tools_args(),
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
        .context("spawn learning episode reviewer claude")?;
    let output = tokio::time::timeout(
        Duration::from_secs(EPISODE_REVIEWER_TIMEOUT_SECS),
        child.wait_with_output(),
    )
    .await
    .context("learning episode reviewer claude timed out")?
    .context("wait for learning episode reviewer claude")?;
    if !output.status.success() {
        return Err(anyhow!(
            "learning episode reviewer claude exited {:?}: {}",
            output.status.code(),
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    let stdout = String::from_utf8(output.stdout).context("reviewer stdout was not UTF-8")?;
    let result =
        crate::learning_review::parse_review_process_stdout(&stdout).map_err(anyhow::Error::msg)?;

    // Record usage — best-effort, never fail the reviewer over this.
    if let Some(breakdown) = crate::cc::stream::parse_usage_full(&stdout)
        && let Some(episode_id) = bundle.learning_episode_id
    {
        match right_db::open_connection(&runtime.agent_db_dir, false) {
            Ok(conn) => {
                if let Err(e) = right_agent::usage::insert::insert_learning_reviewer(
                    &conn, &breakdown, episode_id,
                ) {
                    tracing::warn!(
                        agent = %runtime.agent_name,
                        episode_id,
                        "failed to record reviewer usage event: {e:#}"
                    );
                }
            }
            Err(e) => tracing::warn!(
                agent = %runtime.agent_name,
                episode_id,
                "failed to open db for reviewer usage event: {e:#}"
            ),
        }
    }

    Ok(result)
}

#[derive(Debug, Clone)]
struct SelectedEpisodeForReview {
    id: i64,
    agent_name: String,
    seed_trigger_kind: EpisodeSeedTriggerKind,
    source_invocation_id: String,
    target_chat_id: Option<i64>,
    target_thread_id: Option<i64>,
    message_refs: Vec<String>,
    execution_event_refs: Vec<String>,
}

#[derive(Debug, Clone)]
struct SelectedReviewEvidence {
    messages: Vec<crate::learning_review::ReviewMessage>,
    execution_events: Vec<crate::learning_review::ReviewExecutionEvent>,
    evidence_index: crate::learning_review::EpisodeEvidenceIndex,
    root_session_id: Option<String>,
}

fn load_selected_episode_for_review(
    conn: &rusqlite::Connection,
    episode_id: i64,
) -> anyhow::Result<SelectedEpisodeForReview> {
    let row = conn
        .query_row(
            "SELECT id, agent_name, seed_trigger_kind, seed_ref, target_chat_id, target_thread_id, \
                    message_refs_json, execution_event_refs_json \
             FROM learning_episodes WHERE id=?1 AND status='selected'",
            [episode_id],
            |row| {
                let seed_trigger_kind: String = row.get(2)?;
                let seed_ref: String = row.get(3)?;
                let message_refs_json: String = row.get(6)?;
                let execution_event_refs_json: String = row.get(7)?;
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    seed_trigger_kind,
                    seed_ref,
                    row.get::<_, Option<i64>>(4)?,
                    row.get::<_, Option<i64>>(5)?,
                    message_refs_json,
                    execution_event_refs_json,
                ))
            },
        )
        .optional()?
        .ok_or_else(|| anyhow!("learning episode {episode_id} is not selected"))?;
    Ok(SelectedEpisodeForReview {
        id: row.0,
        agent_name: row.1,
        seed_trigger_kind: parse_seed_trigger_kind(&row.2)?,
        source_invocation_id: source_invocation_id_for_episode(row.0, &row.3),
        target_chat_id: row.4,
        target_thread_id: row.5,
        message_refs: parse_review_refs_json(&row.6)?,
        execution_event_refs: parse_review_refs_json(&row.7)?,
    })
}

fn load_selected_review_evidence(
    conn: &rusqlite::Connection,
    episode: &SelectedEpisodeForReview,
) -> anyhow::Result<SelectedReviewEvidence> {
    let mut evidence_index = crate::learning_review::EpisodeEvidenceIndex::default();

    // Batch-load messages by id, preserving episode.message_refs order.
    let message_ids: Vec<i64> = episode
        .message_refs
        .iter()
        .map(|ref_id| parse_prefixed_ref_id(ref_id, "msg:"))
        .collect::<anyhow::Result<_>>()?;
    let message_rows = load_review_messages_batched(conn, &message_ids)?;
    let mut messages = Vec::with_capacity(episode.message_refs.len());
    for (ref_id, id) in episode.message_refs.iter().zip(message_ids.iter()) {
        let row = message_rows
            .get(id)
            .ok_or_else(|| anyhow!("selected message ref not found: {ref_id}"))?;
        let trust_label = if row.addressed_to_bot != 0 || row.routed_to_agent != 0 {
            right_agent::learning_episodes::TrustLabel::Primary
        } else {
            right_agent::learning_episodes::TrustLabel::LowTrust
        };
        let evidence_kind = match trust_label {
            right_agent::learning_episodes::TrustLabel::LowTrust => {
                crate::learning_review::EvidenceKind::LowTrustMessage
            }
            right_agent::learning_episodes::TrustLabel::Primary
            | right_agent::learning_episodes::TrustLabel::Secondary => {
                crate::learning_review::EvidenceKind::Message
            }
        };
        evidence_index.insert(ref_id.clone(), evidence_kind);
        messages.push(crate::learning_review::ReviewMessage {
            ref_id: ref_id.clone(),
            role: row.role.clone(),
            trust_label,
            content: row.content.clone(),
        });
    }

    // Batch-load execution events by id, preserving episode.execution_event_refs order.
    let event_ids: Vec<i64> = episode
        .execution_event_refs
        .iter()
        .map(|ref_id| parse_prefixed_ref_id(ref_id, "exec:"))
        .collect::<anyhow::Result<_>>()?;
    let event_rows = load_review_execution_events_batched(conn, &event_ids)?;
    let mut root_session_id = None;
    let mut execution_events = Vec::with_capacity(episode.execution_event_refs.len());
    for (ref_id, id) in episode.execution_event_refs.iter().zip(event_ids.iter()) {
        let row = event_rows
            .get(id)
            .ok_or_else(|| anyhow!("selected execution event ref not found: {ref_id}"))?;
        let event_kind =
            right_agent::learning_episodes::ExecutionEventKind::from_db(&row.event_kind)
                .map_err(|e| anyhow!("execution event {ref_id}: {e}"))?;
        let trust_label = right_agent::learning_episodes::TrustLabel::from_db(&row.trust_label)
            .map_err(|e| anyhow!("execution event {ref_id}: {e}"))?;
        if root_session_id.is_none() {
            root_session_id.clone_from(&row.root_session_id);
        }
        let evidence_kind = match event_kind {
            right_agent::learning_episodes::ExecutionEventKind::Thinking => {
                crate::learning_review::EvidenceKind::Thinking
            }
            right_agent::learning_episodes::ExecutionEventKind::AssistantText
            | right_agent::learning_episodes::ExecutionEventKind::ToolCall
            | right_agent::learning_episodes::ExecutionEventKind::ToolResult
            | right_agent::learning_episodes::ExecutionEventKind::ToolError
            | right_agent::learning_episodes::ExecutionEventKind::InvocationResult
            | right_agent::learning_episodes::ExecutionEventKind::Other
            | right_agent::learning_episodes::ExecutionEventKind::StreamEvent => {
                crate::learning_review::EvidenceKind::ObservableExecution
            }
        };
        evidence_index.insert(ref_id.clone(), evidence_kind);
        execution_events.push(crate::learning_review::ReviewExecutionEvent {
            ref_id: ref_id.clone(),
            event_kind,
            trust_label,
            content: row.content.clone(),
        });
    }

    Ok(SelectedReviewEvidence {
        messages,
        execution_events,
        evidence_index,
        root_session_id,
    })
}

struct BatchedReviewMessageRow {
    role: String,
    content: String,
    addressed_to_bot: i64,
    routed_to_agent: i64,
}

struct BatchedReviewExecutionEventRow {
    event_kind: String,
    trust_label: String,
    content: String,
    root_session_id: Option<String>,
}

fn load_review_messages_batched(
    conn: &rusqlite::Connection,
    ids: &[i64],
) -> anyhow::Result<HashMap<i64, BatchedReviewMessageRow>> {
    let mut rows: HashMap<i64, BatchedReviewMessageRow> = HashMap::with_capacity(ids.len());
    if ids.is_empty() {
        return Ok(rows);
    }
    let placeholders = std::iter::repeat_n("?", ids.len())
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!(
        "SELECT id, role, content, addressed_to_bot, routed_to_agent \
         FROM conversation_messages WHERE id IN ({placeholders})"
    );
    let mut stmt = conn.prepare(&sql)?;
    let mut mapped = stmt.query_map(rusqlite::params_from_iter(ids.iter()), |row| {
        let id: i64 = row.get(0)?;
        let role: String = row.get(1)?;
        let content: String = row.get(2)?;
        let addressed_to_bot: i64 = row.get(3)?;
        let routed_to_agent: i64 = row.get(4)?;
        Ok((
            id,
            BatchedReviewMessageRow {
                role,
                content,
                addressed_to_bot,
                routed_to_agent,
            },
        ))
    })?;
    while let Some(item) = mapped.next() {
        let (id, row) = item?;
        rows.insert(id, row);
    }
    Ok(rows)
}

fn load_review_execution_events_batched(
    conn: &rusqlite::Connection,
    ids: &[i64],
) -> anyhow::Result<HashMap<i64, BatchedReviewExecutionEventRow>> {
    let mut rows: HashMap<i64, BatchedReviewExecutionEventRow> = HashMap::with_capacity(ids.len());
    if ids.is_empty() {
        return Ok(rows);
    }
    let placeholders = std::iter::repeat_n("?", ids.len())
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!(
        "SELECT id, event_kind, trust_label, content_text, root_session_id \
         FROM execution_events WHERE id IN ({placeholders})"
    );
    let mut stmt = conn.prepare(&sql)?;
    let mut mapped = stmt.query_map(rusqlite::params_from_iter(ids.iter()), |row| {
        let id: i64 = row.get(0)?;
        let event_kind: String = row.get(1)?;
        let trust_label: String = row.get(2)?;
        let content: String = row.get(3)?;
        let root_session_id: Option<String> = row.get(4)?;
        Ok((
            id,
            BatchedReviewExecutionEventRow {
                event_kind,
                trust_label,
                content,
                root_session_id,
            },
        ))
    })?;
    while let Some(item) = mapped.next() {
        let (id, row) = item?;
        rows.insert(id, row);
    }
    Ok(rows)
}

fn load_episode_review_learning_events(
    conn: &rusqlite::Connection,
    source_invocation_id: &str,
) -> anyhow::Result<Vec<String>> {
    if source_invocation_id.starts_with("episode:") {
        return Ok(Vec::new());
    }
    let mut stmt = conn.prepare(
        "SELECT action, skill_name, phase, COALESCE(status, ''), COALESCE(summary, '') \
         FROM skill_learning_events WHERE invocation_id = ?1 ORDER BY id LIMIT ?2",
    )?;
    let rows = stmt.query_map(
        rusqlite::params![source_invocation_id, EPISODE_REVIEW_LEARNING_EVENTS_LIMIT],
        |row| {
            let action: String = row.get(0)?;
            let skill_name: String = row.get(1)?;
            let phase: String = row.get(2)?;
            let status: String = row.get(3)?;
            let summary: String = row.get(4)?;
            Ok(format!(
                "{phase} {action} {skill_name} status={status} summary={summary}"
            ))
        },
    )?;
    let mut events = Vec::new();
    for row in rows {
        events.push(row?);
    }
    Ok(events)
}

async fn collect_episode_review_skill_index(
    runtime: &LearningEpisodeRuntime,
) -> anyhow::Result<Vec<crate::learning_review::LearnedSkillSummary>> {
    if runtime.ssh_config_path.is_some() {
        crate::telegram::worker::collect_sandbox_review_skill_index(
            runtime.resolved_sandbox.as_deref(),
        )
        .await
    } else {
        crate::learning_review::collect_host_rightx_skill_index(&runtime.agent_dir)
            .map_err(anyhow::Error::from)
    }
}

fn mark_episode_reviewing(conn: &rusqlite::Connection, episode_id: i64) -> anyhow::Result<()> {
    match right_agent::learning_episodes::mark_episode_reviewing(conn, episode_id) {
        Ok(()) => Ok(()),
        Err(rusqlite::Error::QueryReturnedNoRows) => Err(anyhow!(
            "learning episode {episode_id} could not enter reviewing"
        )),
        Err(e) => Err(anyhow::Error::from(e)),
    }
}

fn mark_episode_reviewed(conn: &rusqlite::Connection, episode_id: i64) -> anyhow::Result<()> {
    match right_agent::learning_episodes::mark_episode_reviewed(conn, episode_id) {
        Ok(()) => Ok(()),
        Err(rusqlite::Error::QueryReturnedNoRows) => Err(anyhow!(
            "learning episode {episode_id} could not be marked reviewed"
        )),
        Err(e) => Err(anyhow::Error::from(e)),
    }
}

fn mark_episode_review_failed_and_finish(
    runtime: &LearningEpisodeRuntime,
    episode_id: i64,
    reason: &str,
) -> anyhow::Result<Option<(i64, bool)>> {
    let now_utc = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
    let conn = right_db::open_connection(&runtime.agent_db_dir, false)
        .with_context(|| format!("open {} data.db for review failure", runtime.agent_name))?;
    // First write: mark the episode row failed (its own transaction).
    {
        let tx = conn.unchecked_transaction()?;
        right_agent::learning_episodes::mark_episode_failed(&tx, episode_id, reason)
            .with_context(|| format!("mark learning episode {episode_id} failed"))?;
        tx.commit()?;
    }
    // Second write: drive the circuit-breaker gate (its own transaction).
    // Atomicity loss between the two writes is acceptable — they touch
    // independent tables and the circuit self-heals on the next failure.
    let (count, opened) = record_review_failure(
        &conn,
        &runtime.agent_name,
        &now_utc,
        runtime.learning.circuit_failure_threshold,
        runtime.learning.circuit_cooldown_minutes,
    )
    .with_context(|| {
        format!(
            "record review failure for {} (episode {episode_id} reviewer crash)",
            runtime.agent_name
        )
    })?;
    Ok(Some((count, opened)))
}

fn parse_review_refs_json(raw: &str) -> anyhow::Result<Vec<String>> {
    serde_json::from_str(raw).with_context(|| "parse selected learning episode refs")
}

fn parse_prefixed_ref_id(ref_id: &str, prefix: &str) -> anyhow::Result<i64> {
    ref_id
        .strip_prefix(prefix)
        .and_then(|value| value.parse::<i64>().ok())
        .ok_or_else(|| anyhow!("selected ref has invalid prefix or id: {ref_id}"))
}

fn source_invocation_id_for_episode(episode_id: i64, seed_ref: &str) -> String {
    seed_ref
        .strip_prefix("inv:")
        .map(str::to_owned)
        .unwrap_or_else(|| format!("episode:{episode_id}"))
}

fn parse_seed_trigger_kind(value: &str) -> anyhow::Result<EpisodeSeedTriggerKind> {
    match value {
        "learning_signal" => Ok(EpisodeSeedTriggerKind::LearningSignal),
        "skill_issue_signal" => Ok(EpisodeSeedTriggerKind::SkillIssueSignal),
        "effort_threshold" => Ok(EpisodeSeedTriggerKind::EffortThreshold),
        "cron" => Ok(EpisodeSeedTriggerKind::Cron),
        "async_result" => Ok(EpisodeSeedTriggerKind::AsyncResult),
        _ => Err(anyhow!("invalid episode seed trigger kind: {value:?}")),
    }
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
        &messages,
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
        "SELECT id, role, content, addressed_to_bot, routed_to_agent, root_session_id, turn_id, created_at
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
            root_session_id: row.get(5)?,
            turn_id: row.get(6)?,
            created_at: row.get(7)?,
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
    messages: &[CorpusMessage],
) -> Result<Vec<CorpusExecutionEvent>, rusqlite::Error> {
    let turn_pairs: Vec<(&str, i64)> = messages
        .iter()
        .filter_map(|message| Some((message.root_session_id.as_deref()?, message.turn_id?)))
        .collect();
    let turn_clause = if turn_pairs.is_empty() {
        String::new()
    } else {
        let placeholders = std::iter::repeat_n("(?, ?)", turn_pairs.len())
            .collect::<Vec<_>>()
            .join(", ");
        format!(" OR (root_session_id, turn_id) IN ({placeholders})")
    };
    let sql = format!(
        "SELECT id, event_kind, trust_label, content_text
         FROM execution_events
         WHERE agent_name=? AND (root_session_id=? OR invocation_id=? OR async_run_id=? OR cron_run_id=?{turn_clause})
         ORDER BY seq ASC, id ASC
         LIMIT 120"
    );
    let mut params: Vec<rusqlite::types::Value> = vec![
        rusqlite::types::Value::Text(agent_name.to_owned()),
        root_session_id
            .map(|value| rusqlite::types::Value::Text(value.to_owned()))
            .unwrap_or(rusqlite::types::Value::Null),
        invocation_id
            .map(|value| rusqlite::types::Value::Text(value.to_owned()))
            .unwrap_or(rusqlite::types::Value::Null),
        async_run_id
            .map(|value| rusqlite::types::Value::Text(value.to_owned()))
            .unwrap_or(rusqlite::types::Value::Null),
        cron_run_id
            .map(|value| rusqlite::types::Value::Text(value.to_owned()))
            .unwrap_or(rusqlite::types::Value::Null),
    ];
    for (root_session_id, turn_id) in &turn_pairs {
        params.push(rusqlite::types::Value::Text((*root_session_id).to_owned()));
        params.push(rusqlite::types::Value::Integer(*turn_id));
    }
    let mut stmt = conn.prepare(&sql)?;
    stmt.query_map(rusqlite::params_from_iter(params), |row| {
        Ok(CorpusExecutionEvent {
            id: row.get(0)?,
            event_kind: parse_execution_event_kind(row.get::<_, String>(1)?.as_str()),
            trust_label: row.get(2)?,
            content_text: row.get(3)?,
        })
    })?
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
        model: effective_selector_model(runtime),
        max_budget_usd: None, // Per-call budget removed; replaced by max_daily_budget_usd gate.
        max_turns: Some(3),
        resume_session_id: None,
        new_session_id: None,
        fork_session: false,
        allowed_tools: Vec::new(),
        disallowed_tools: Vec::new(),
        extra_args: crate::cc::invocation::disable_all_tools_args(),
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
    let result = parse_selector_process_stdout(&stdout).map_err(anyhow::Error::msg)?;

    // Record usage — best-effort, never fail the selector over this.
    if let Some(breakdown) = crate::cc::stream::parse_usage_full(&stdout) {
        let conn = right_db::open_connection(&runtime.agent_db_dir, false);
        match conn {
            Ok(conn) => {
                if let Err(e) = right_agent::usage::insert::insert_learning_selector(
                    &conn, &breakdown, episode.id,
                ) {
                    tracing::warn!(
                        agent = %runtime.agent_name,
                        episode_id = episode.id,
                        "failed to record selector usage event: {e:#}"
                    );
                }
            }
            Err(e) => {
                tracing::warn!(
                    agent = %runtime.agent_name,
                    episode_id = episode.id,
                    "failed to open db for selector usage event: {e:#}"
                );
            }
        }
    }

    Ok(result)
}

fn build_selector_prompt(corpus_json: &str) -> String {
    let mut prompt = String::new();
    prompt.push_str("# Learning Episode Selector\n\n");
    prompt.push_str(
        "Select the smallest useful learning episode from the corpus. \
         Use only refs present in the corpus. Thinking-only evidence is not observable. \
         Message trust_label=low_trust is nearby unaddressed context, not primary evidence. \
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
    let raw = crate::learning_review::unwrap_structured_output_payload(stdout, "selector")?;
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
    right_agent::learning_episodes::requeue_episode(conn, episode_id, &ready_after)
}

fn requeue_episode_or_fail(
    conn: &rusqlite::Connection,
    runtime: &LearningEpisodeRuntime,
    episode_id: i64,
    now: chrono::DateTime<chrono::Utc>,
    settle_seconds: u64,
    now_utc: &str,
) -> anyhow::Result<()> {
    match requeue_episode(conn, episode_id, now, settle_seconds) {
        Ok(()) => Ok(()),
        Err(rusqlite::Error::QueryReturnedNoRows) => {
            // Row moved out of 'selecting' under us. This helper is used after
            // Skip(AlreadyRunning/DailyBudget/CircuitOpen), so we do not own the review gate
            // and must leave it untouched.
            tracing::debug!(
                episode_id,
                "learning episode requeue: no row in 'selecting' state"
            );
            Ok(())
        }
        Err(e) => {
            let reason = format!("learning episode requeue failed: {e:#}");
            // Requeue failure after a Skip decision — we do not own the gate and
            // this is not a review-pipeline failure, so do not record a circuit failure.
            mark_claimed_episode_failed(conn, runtime, episode_id, &reason, now_utc, false)?;
            Err(anyhow!(reason))
        }
    }
}

pub(crate) fn recover_stale_inflight_episodes(
    conn: &rusqlite::Connection,
    agent_name: &str,
    now: &str,
) -> Result<usize, rusqlite::Error> {
    right_agent::learning_episodes::recover_stale_inflight_episodes(conn, agent_name, now)
}

pub(crate) fn has_ready_pending_episodes(
    conn: &rusqlite::Connection,
    agent_name: &str,
    now: &str,
) -> Result<bool, rusqlite::Error> {
    conn.query_row(
        "SELECT EXISTS(
            SELECT 1 FROM learning_episodes
            WHERE agent_name=?1 AND status='pending' AND ready_after <= ?2
            LIMIT 1
        )",
        rusqlite::params![agent_name, now],
        |row| row.get::<_, i64>(0),
    )
    .map(|value| value != 0)
}

fn review_trigger_for_episode(
    seed_trigger_kind: EpisodeSeedTriggerKind,
) -> Option<ReviewTriggerKind> {
    match seed_trigger_kind {
        EpisodeSeedTriggerKind::LearningSignal => Some(ReviewTriggerKind::LearningSignal),
        EpisodeSeedTriggerKind::SkillIssueSignal => Some(ReviewTriggerKind::SkillIssueSignal),
        EpisodeSeedTriggerKind::EffortThreshold => Some(ReviewTriggerKind::EffortThreshold),
        EpisodeSeedTriggerKind::Cron | EpisodeSeedTriggerKind::AsyncResult => {
            Some(ReviewTriggerKind::EffortThreshold)
        }
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

fn message_trust_label(message: &CorpusMessage) -> &'static str {
    if message.addressed_to_bot || message.routed_to_agent {
        "primary"
    } else {
        "low_trust"
    }
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
#[path = "learning_episode_tests.rs"]
mod tests;
