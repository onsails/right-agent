//! Post-turn probe-writer fork — surveys the just-finished foreground turn and
//! either creates a new `rightx-*` skill, updates an existing one, or skips.
//!
//! Spec: docs/superpowers/specs/2026-05-22-skill-learning-writer-curator-design.md

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};

use right_codegen::{PROBE_WRITER_ANCHOR_TEMPLATE, PROBE_WRITER_INSTRUCTIONS};

use crate::telegram::SessionLocks;
use crate::telegram::worker::ProbeAnchor;

const PROBE_WRITER_INIT_TIMEOUT: Duration = Duration::from_secs(30);
const PROBE_WRITER_TIMEOUT: Duration = Duration::from_secs(300);
pub(crate) const PROBE_WRITER_MAX_TURNS: u32 = 16;

/// Hint from the prefilter directing the probe-writer's framing.
#[derive(Debug, Clone)]
pub(crate) enum ProbeWriterHint {
    PatchExisting {
        target_skill: String,
        reason: String,
    },
    CreateNew {
        topic_hint: String,
        reason: String,
    },
}

#[derive(Clone)]
pub(crate) struct ProbeWriterContext {
    pub agent_dir: PathBuf,
    pub agent_db_dir: PathBuf,
    pub agent_name: String,
    /// `None` when the sandbox backend is degraded — the fork simply does not
    /// run; there is no host fallback.
    pub sandbox: Option<crate::sandbox::Sandbox>,
    pub internal_client: Arc<right_mcp::internal_client::InternalClient>,
    pub model: String,
    pub debug_flag: Arc<AtomicBool>,
    pub session_locks: SessionLocks,
    pub chat_id: i64,
    pub thread_id: i64,
    pub incoming_hint: ProbeWriterHint,
}

/// Compose the first user-message body delivered to the fork.
///
/// Layered prompt: canonical class-first instructions + quality (incl. the
/// delegation directive) from `right_codegen::PROBE_WRITER_INSTRUCTIONS`, then
/// the prefilter's per-turn hint, the `hint_outcome` reporting contract, the
/// agent's `rightx-*` skill index, and finally the anchored turn rendered from
/// `right_codegen::PROBE_WRITER_ANCHOR_TEMPLATE`.
pub(crate) fn build_user_prompt(
    anchor: &ProbeAnchor,
    skill_index: &str,
    hint: &ProbeWriterHint,
) -> String {
    let user: String = anchor.user_msg_text.chars().take(8000).collect();
    let assistant: String = anchor.assistant_reply_text.chars().take(12000).collect();

    let hint_block = match hint {
        ProbeWriterHint::PatchExisting {
            target_skill,
            reason,
        } => format!(
            "PREFILTER HINT: patch_existing\n\
TARGET SKILL: {target_skill}\n\
REASON: {reason}\n\n\
Verify this recommendation against the protocol above and the anchored turn \
below, then apply it or override it (patch a different skill, create instead, \
or exit silently) as the protocol directs.",
        ),
        ProbeWriterHint::CreateNew { topic_hint, reason } => format!(
            "PREFILTER HINT: create_new\n\
TOPIC HINT: {topic_hint}\n\
REASON: {reason}\n\n\
Verify this recommendation against the protocol above and the anchored turn \
below, then apply it or override it (patch an existing skill instead, or exit \
silently) as the protocol directs.",
        ),
    };

    let index = if skill_index.is_empty() {
        "(no existing rightx-* skills)"
    } else {
        skill_index
    };

    let anchor_rendered = PROBE_WRITER_ANCHOR_TEMPLATE
        .replace("{user_msg_text}", &user)
        .replace("{assistant_reply_text}", &assistant);

    format!(
        "{PROBE_WRITER_INSTRUCTIONS}\n\n\
{hint_block}\n\n\
When you call mcp__right__skill_learning_finish, ALWAYS include the field\n\
\"hint_outcome\" with one of:\n\
  - \"applied_as_hinted\" — you patched/created exactly as the hint suggested.\n\
  - \"applied_differently\" — you took action but not as hinted (e.g. patched a\n\
    different skill, created instead of patched).\n\
  - \"refused\" — you exited without writing because the hint was unjustified.\n\n\
EXISTING SKILLS:\n{index}\n\n{anchor_rendered}"
    )
}

/// Build the `ClaudeInvocation` for the probe-writer fork (pure).
pub(crate) fn build_invocation(
    ctx: &ProbeWriterContext,
    main_session_uuid: &str,
    probe_session_id: &str,
    user_prompt: String,
    mcp_config_path: String,
) -> crate::cc::invocation::ClaudeInvocation {
    use crate::cc::invocation::{ClaudeInvocation, OutputFormat};
    ClaudeInvocation {
        mcp_config_path: Some(mcp_config_path),
        json_schema: None,
        output_format: OutputFormat::StreamJson,
        model: Some(ctx.model.clone()),
        max_budget_usd: None,
        max_turns: Some(PROBE_WRITER_MAX_TURNS),
        resume_session_id: Some(main_session_uuid.to_owned()),
        new_session_id: Some(probe_session_id.to_owned()),
        fork_session: true,
        allowed_tools: vec![
            "Write".into(),
            "Read".into(),
            "Bash".into(),
            "mcp__right__skill_learning_start".into(),
            "mcp__right__skill_learning_finish".into(),
        ],
        disallowed_tools: vec![],
        extra_args: vec![],
        prompt: Some(user_prompt),
        debug_flag: Some(Arc::clone(&ctx.debug_flag)),
    }
}

/// Map a `skill_learning_events.status` (finish phase) to a `skill_spend.kind`.
/// Only successful create/update are spend-worthy; aborted/failed are not.
fn finish_status_to_spend_kind(status: &str) -> Option<&'static str> {
    match status {
        "created" => Some("create"),
        "updated" => Some("patch"),
        _ => None,
    }
}

/// Spawn the probe-writer fork. Holds session mutex during fork init only.
/// Returns when fork is established (system/init received); detached task drains
/// the remainder.
pub(crate) async fn run(ctx: ProbeWriterContext, anchor: ProbeAnchor, skill_index: String) {
    let sandbox = match crate::cc::invocation::guard_no_sandboxed_host_exec(
        &ctx.agent_name,
        ctx.sandbox.as_ref(),
    ) {
        Ok(sandbox) => Arc::clone(sandbox),
        Err(e) => {
            tracing::warn!(agent = %ctx.agent_name, "skipping probe-writer: {e:#}");
            return;
        }
    };
    let probe_session_id = uuid::Uuid::new_v4().to_string();
    let user_prompt = build_user_prompt(&anchor, &skill_index, &ctx.incoming_hint);
    let active_invocation = match crate::cc::invocation::register_non_foreground_invocation(
        crate::cc::invocation::NonForegroundInvocationRegistration {
            agent_name: ctx.agent_name.clone(),
            agent_dir: ctx.agent_dir.clone(),
            sandbox: Arc::clone(&sandbox),
            internal_client: Arc::clone(&ctx.internal_client),
            kind: right_mcp::internal_client::ProgressInvocationKindDto::ProbeWriter,
            chat_id: Some(ctx.chat_id),
            thread_id: Some(ctx.thread_id),
            progress_state: None,
        },
    )
    .await
    {
        Ok(active) => active,
        Err(e) => {
            tracing::warn!(
                agent = %ctx.agent_name,
                "probe-writer invocation registration failed: {e:#}"
            );
            return;
        }
    };
    let invocation = build_invocation(
        &ctx,
        &anchor.main_session_uuid,
        &probe_session_id,
        user_prompt,
        active_invocation.mcp_config_path().to_owned(),
    );
    let args = invocation.into_args();

    // Acquire main-session mutex.
    let lock = ctx
        .session_locks
        .entry(anchor.main_session_uuid.clone())
        .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
        .clone();
    let _guard = lock.lock().await;

    let command =
        match crate::cc::invocation::build_claude_command(&args, &ctx.agent_dir, &sandbox).await {
            Ok(command) => command
                .stdout(crate::cc::sandbox_process::Capture::Pipe)
                .stderr(crate::cc::sandbox_process::Capture::Pipe),
            Err(e) => {
                tracing::warn!(agent = %ctx.agent_name, "probe-writer command build failed: {e:#}");
                drop(_guard);
                active_invocation.cleanup().await;
                return;
            }
        };

    let mut child = match command.spawn().await {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(
                agent = %ctx.agent_name,
                "probe-writer spawn failed: {e:#}"
            );
            drop(_guard);
            active_invocation.cleanup().await;
            return;
        }
    };

    let stdout = match child.stdout() {
        Some(s) => s,
        None => {
            tracing::warn!(agent = %ctx.agent_name, "probe-writer child has no stdout");
            child.kill().await;
            drop(child);
            drop(_guard);
            active_invocation.cleanup().await;
            return;
        }
    };
    let mut lines = BufReader::new(stdout).lines();

    // Phase 1: read until system/init (bounded), holding the session mutex.
    // ONE reader drains the whole stream — `read_until_init` borrows `lines`
    // and leaves it positioned just past the init event for Phase 2.
    let init_observed = tokio::time::timeout(
        PROBE_WRITER_INIT_TIMEOUT,
        read_until_init(&mut lines, &probe_session_id),
    )
    .await
    .unwrap_or(false);
    drop(_guard);

    if !init_observed {
        tracing::warn!(
            agent = %ctx.agent_name,
            "probe-writer never emitted system/init, killing"
        );
        child.kill().await;
        drop(child);
        active_invocation.cleanup().await;
        return;
    }

    // Phase 2: detached — drain the REST of the same reader to EOF (prevents
    // pipe-fill hang) while awaiting the process, capturing the final result.
    let agent_name = ctx.agent_name.clone();
    let internal_client = Arc::clone(&ctx.internal_client);
    let chat_id = ctx.chat_id;
    let thread_id = ctx.thread_id;
    let invocation_id = active_invocation.invocation_id().to_owned();
    let origin_cron_job = anchor.origin_cron_job.clone();
    tokio::spawn(async move {
        let mut tail = String::new();
        let drain = async {
            while let Ok(Some(line)) = lines.next_line().await {
                if line.len() < 1_000_000 {
                    tail.push_str(&line);
                    tail.push('\n');
                }
            }
        };
        // Drain + wait for exit, bounded by PROBE_WRITER_TIMEOUT.
        let completed = tokio::time::timeout(PROBE_WRITER_TIMEOUT, async {
            tokio::join!(drain, child.wait())
        })
        .await;
        match completed {
            Ok((_, Ok(code))) => {
                if code != 0 {
                    tracing::warn!(agent = %agent_name, code, "probe-writer exited non-zero");
                }
            }
            Ok((_, Err(e))) => {
                tracing::warn!(agent = %agent_name, "probe-writer wait failed: {e:#}");
            }
            Err(_) => {
                tracing::warn!(
                    agent = %agent_name,
                    "probe-writer timed out after {}s",
                    PROBE_WRITER_TIMEOUT.as_secs()
                );
                child.kill().await;
            }
        }

        if let Some(result_line) = crate::cc::stream::last_result_line(&tail)
            && let Some(breakdown) = crate::cc::stream::parse_usage_full(&result_line)
            && let Err(error) = internal_client
                .usage_insert_event(&right_mcp::internal_db::UsageInsertEventRequest {
                    agent: agent_name.clone(),
                    request_id: crate::db::request_id(),
                    source: right_mcp::internal_db::UsageSourceDto::LearningProbeWriter {
                        chat_id,
                        thread_id,
                    },
                    event: crate::db::usage_dto(&breakdown),
                })
                .await
        {
            tracing::warn!(agent = %agent_name, "probe-writer owner usage insert failed: {error:#}");
        }

        if let Some(job_name) = origin_cron_job.as_deref()
            && let Err(error) = internal_client
                .learning_link_cron_authored(
                    &right_mcp::internal_db::LearningLinkCronAuthoredRequest {
                        agent: agent_name.clone(),
                        job_name: job_name.to_owned(),
                        invocation_id: invocation_id.clone(),
                    },
                )
                .await
        {
            tracing::warn!(agent = %agent_name, job = %job_name, "cron auto-link owner operation failed: {error:#}");
        }

        active_invocation.cleanup().await;
    });
}

/// Read lines until a `system/init` for `expected_session_id` is seen. Returns
/// `true` on init, `false` on EOF. Does NOT consume the reader (borrows it) so
/// the same reader can be drained afterwards.
async fn read_until_init<R: AsyncRead + Unpin>(
    lines: &mut tokio::io::Lines<BufReader<R>>,
    expected_session_id: &str,
) -> bool {
    while let Ok(Some(line)) = lines.next_line().await {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        if v.get("type").and_then(|t| t.as_str()) == Some("system")
            && v.get("subtype").and_then(|s| s.as_str()) == Some("init")
            && v.get("session_id").and_then(|s| s.as_str()) == Some(expected_session_id)
        {
            return true;
        }
    }
    false
}

#[cfg(test)]
pub(crate) async fn link_cron_authored(
    conn: &right_db::Connection,
    job: &str,
    invocation_id: &str,
) -> Result<usize, right_db::DbError> {
    let mut authored: Vec<String> =
        right_agent::learned_skills::successful_finishes_for_invocation(conn, invocation_id)
            .await?
            .into_iter()
            .map(|(name, _status)| name)
            .collect();
    authored.sort();
    authored.dedup();
    let n = authored.len();
    right_agent::cron_skill_link::link_auto(conn, job, &authored).await?;
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finish_status_to_spend_kind_maps_created_and_updated() {
        assert_eq!(finish_status_to_spend_kind("created"), Some("create"));
        assert_eq!(finish_status_to_spend_kind("updated"), Some("patch"));
        assert_eq!(finish_status_to_spend_kind("aborted"), None);
        assert_eq!(finish_status_to_spend_kind("failed"), None);
    }

    #[test]
    fn last_result_line_picks_final_result_event() {
        let stream = "\
{\"type\":\"system\",\"subtype\":\"init\"}\n\
{\"type\":\"assistant\"}\n\
{\"type\":\"result\",\"num_turns\":3,\"total_cost_usd\":0.2,\"session_id\":\"s\"}\n";
        let line = crate::cc::stream::last_result_line(stream).unwrap();
        assert!(line.contains("\"type\":\"result\""));
        assert!(line.contains("\"total_cost_usd\":0.2"));
    }

    fn anchor(user: &str, asst: &str) -> ProbeAnchor {
        ProbeAnchor {
            user_msg_text: user.to_owned(),
            assistant_reply_text: asst.to_owned(),
            main_session_uuid: "main-sid".to_owned(),
            captured_at: chrono::Utc::now(),
            chat_id: 1,
            thread_id: 0,
            num_turns: 1,
            total_cost_usd: 0.0,
            wall_elapsed_ms: 0,
            used_skill_receipts: Vec::new(),
            learning_invocation_id: None,
            origin_cron_job: None,
        }
    }

    fn default_hint() -> ProbeWriterHint {
        ProbeWriterHint::CreateNew {
            topic_hint: "topic".into(),
            reason: "reason".into(),
        }
    }

    fn context(agent_dir: PathBuf) -> ProbeWriterContext {
        ProbeWriterContext {
            agent_dir,
            agent_db_dir: PathBuf::from("/tmp/db"),
            agent_name: "agent-1".into(),
            sandbox: None,
            internal_client: std::sync::Arc::new(right_mcp::internal_client::InternalClient::new(
                "/tmp/fake.sock",
            )),
            model: "claude-sonnet-4-5".into(),
            debug_flag: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            session_locks: SessionLocks::default(),
            chat_id: 42,
            thread_id: 7,
            incoming_hint: default_hint(),
        }
    }

    #[tokio::test]
    async fn build_user_prompt_includes_anchor_instructions_and_index() {
        let p = build_user_prompt(&anchor("hi", "bye"), "- rightx-foo: bar", &default_hint());
        assert!(p.contains("hi"));
        assert!(p.contains("bye"));
        assert!(p.contains("rightx-foo: bar"));
        assert!(p.contains("hint_outcome"));
        // Composed from the canonical codegen constants (drift fixed).
        assert!(
            p.contains("Survey"),
            "must include PROBE_WRITER_INSTRUCTIONS body"
        );
        assert!(
            p.contains("disposable-intermediate"),
            "must include delegation directive"
        );
        assert!(
            p.contains("probe_writer_anchor"),
            "must include the anchor template markers"
        );
    }

    #[tokio::test]
    async fn build_user_prompt_empty_index_uses_placeholder() {
        let p = build_user_prompt(&anchor("a", "b"), "", &default_hint());
        assert!(p.contains("no existing rightx-* skills"));
    }

    #[tokio::test]
    async fn probe_writer_max_turns_is_16() {
        assert_eq!(PROBE_WRITER_MAX_TURNS, 16);
    }

    #[tokio::test]
    async fn background_invocation_probe_writer_uses_invocation_scoped_mcp_config() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = context(dir.path().to_path_buf());

        let mcp_path = dir
            .path()
            .join(".claude")
            .join("mcp-inv-1.json")
            .to_string_lossy()
            .into_owned();
        let invocation = build_invocation(&ctx, "main-sid", "probe-sid", "prompt".into(), mcp_path);

        let path = invocation
            .mcp_config_path
            .expect("probe-writer should pass an MCP config path");
        assert!(
            path.contains("/.claude/mcp-"),
            "probe-writer path should include a generated invocation id: {path}"
        );
        assert!(
            !path.ends_with("/mcp.json"),
            "probe-writer must not use the static agent mcp.json: {path}"
        );
    }

    #[tokio::test]
    async fn build_user_prompt_includes_patch_block_for_patch_hint() {
        let p = build_user_prompt(
            &anchor("u", "a"),
            "- rightx-foo: ...",
            &ProbeWriterHint::PatchExisting {
                target_skill: "rightx-foo".into(),
                reason: "missed step".into(),
            },
        );
        assert!(p.contains("PREFILTER HINT: patch_existing"));
        assert!(p.contains("TARGET SKILL: rightx-foo"));
        assert!(p.contains("missed step"));
        assert!(p.contains("hint_outcome"));
    }

    #[tokio::test]
    async fn build_user_prompt_includes_create_block_for_create_hint() {
        let p = build_user_prompt(
            &anchor("u", "a"),
            "- rightx-foo: ...",
            &ProbeWriterHint::CreateNew {
                topic_hint: "git rebase recovery".into(),
                reason: "new procedure".into(),
            },
        );
        assert!(p.contains("PREFILTER HINT: create_new"));
        assert!(p.contains("TOPIC HINT: git rebase recovery"));
        assert!(p.contains("new procedure"));
        assert!(p.contains("hint_outcome"));
    }

    #[tokio::test(start_paused = true)]
    async fn background_invocation_probe_writer_init_wait_times_out_when_child_is_silent() {
        let (_writer, reader) = tokio::io::duplex(64);
        let mut lines = BufReader::new(reader).lines();

        let observed = tokio::time::timeout(
            PROBE_WRITER_INIT_TIMEOUT,
            read_until_init(&mut lines, "probe-sid"),
        )
        .await
        .unwrap_or(false);

        assert!(
            !observed,
            "silent probe-writer child should not hold the session lock forever"
        );
    }

    #[tokio::test]
    async fn link_cron_authored_links_created_and_updated_skills() {
        let (_t, c) = right_db::test_support::migrated_connection().await;
        c.execute(
            "INSERT INTO cron_specs (job_name, schedule, prompt, max_budget_usd, created_at, updated_at) \
             VALUES ('j','17 9 * * *','x',2.0,'t','t')",
            (),
        ).await.unwrap();
        for (skill, status) in [
            ("rightx-a", "created"),
            ("rightx-b", "updated"),
            ("rightx-c", "aborted"),
        ] {
            c.execute(
                "INSERT INTO skill_learning_events (invocation_id, agent_name, action, skill_name, phase, status, created_at) \
                 VALUES ('inv','a','create',?1,'finish',?2,'t')",
                right_db::params![skill, status],
            ).await.unwrap();
        }
        let n = link_cron_authored(&c, "j", "inv").await.unwrap();
        assert_eq!(n, 2, "only created+updated link, not aborted");
        let mut linked = right_agent::cron_skill_link::list_for_job(&c, "j")
            .await
            .unwrap();
        linked.sort();
        assert_eq!(linked, vec!["rightx-a".to_string(), "rightx-b".to_string()]);

        // No-op when an invocation authored nothing.
        let n0 = link_cron_authored(&c, "j", "other-inv").await.unwrap();
        assert_eq!(n0, 0);
    }

    #[tokio::test]
    async fn link_cron_authored_partitions_by_invocation_no_double_link() {
        // The inline seam (cron_invocation_id) and async seam (probe-writer id)
        // query DISJOINT invocation_ids; each links its own skill exactly once.
        let (_t, c) = right_db::test_support::migrated_connection().await;
        c.execute(
            "INSERT INTO cron_specs (job_name, schedule, prompt, max_budget_usd, created_at, updated_at) \
             VALUES ('j','17 9 * * *','x',2.0,'t','t')",
            (),
        ).await.unwrap();
        c.execute(
            "INSERT INTO skill_learning_events (invocation_id, agent_name, action, skill_name, phase, status, created_at) \
             VALUES ('inv-cron','a','create','rightx-inline','finish','created','t')",
            (),
        ).await.unwrap();
        c.execute(
            "INSERT INTO skill_learning_events (invocation_id, agent_name, action, skill_name, phase, status, created_at) \
             VALUES ('inv-probe','a','create','rightx-async','finish','created','t')",
            (),
        ).await.unwrap();
        assert_eq!(link_cron_authored(&c, "j", "inv-cron").await.unwrap(), 1);
        assert_eq!(link_cron_authored(&c, "j", "inv-probe").await.unwrap(), 1);
        let mut linked = right_agent::cron_skill_link::list_for_job(&c, "j")
            .await
            .unwrap();
        linked.sort();
        assert_eq!(
            linked,
            vec!["rightx-async".to_string(), "rightx-inline".to_string()]
        );
    }

    #[tokio::test]
    async fn link_cron_authored_dedups_repeated_skill_in_one_invocation() {
        // One invocation can finish the SAME skill twice (created then updated).
        // The count must be the distinct skill count, with a single link row.
        let (_t, c) = right_db::test_support::migrated_connection().await;
        c.execute(
            "INSERT INTO cron_specs (job_name, schedule, prompt, max_budget_usd, created_at, updated_at) \
             VALUES ('j','17 9 * * *','x',2.0,'t','t')",
            (),
        ).await.unwrap();
        for status in ["created", "updated"] {
            c.execute(
                "INSERT INTO skill_learning_events (invocation_id, agent_name, action, skill_name, phase, status, created_at) \
                 VALUES ('inv','a','create','rightx-dup','finish',?1,'t')",
                right_db::params![status],
            ).await.unwrap();
        }
        let n = link_cron_authored(&c, "j", "inv").await.unwrap();
        assert_eq!(n, 1, "two finishes for one skill collapse to one");
        let linked = right_agent::cron_skill_link::list_for_job(&c, "j")
            .await
            .unwrap();
        assert_eq!(linked, vec!["rightx-dup".to_string()]);
    }
}
