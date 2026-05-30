//! Post-turn probe-writer fork — surveys the just-finished foreground turn and
//! either creates a new `rightx-*` skill, updates an existing one, or skips.
//!
//! Spec: docs/superpowers/specs/2026-05-22-skill-learning-writer-curator-design.md

use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};

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
    pub ssh_config_path: Option<PathBuf>,
    pub resolved_sandbox: Option<String>,
    pub internal_client: Arc<right_mcp::internal_client::InternalClient>,
    pub model: String,
    pub debug_flag: Arc<AtomicBool>,
    pub session_locks: SessionLocks,
    pub chat_id: i64,
    pub thread_id: i64,
    pub incoming_hint: ProbeWriterHint,
}

/// Compose the first user-message body delivered to the fork.
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
Verify the gap described in REASON by reading {target_skill}/SKILL.md \
and the turn transcript below. If you confirm the gap, patch the skill. \
If the hint is mistaken (skill is already correct, or the gap is \
elsewhere), exit silently or create a new skill if a different procedure \
is exposed.",
        ),
        ProbeWriterHint::CreateNew { topic_hint, reason } => format!(
            "PREFILTER HINT: create_new\n\
TOPIC HINT: {topic_hint}\n\
REASON: {reason}\n\n\
Verify that no existing skill covers TOPIC HINT by scanning the index \
below. If a close-enough skill exists, patch it instead. If nothing \
matches, create a new rightx-* skill. If the hint is wrong (the turn \
does not expose a reusable procedure), exit silently.",
        ),
    };

    let index = if skill_index.is_empty() {
        "(no existing rightx-* skills)"
    } else {
        skill_index
    };

    format!(
        "{hint_block}\n\n\
When you call mcp__right__skill_learning_finish, ALWAYS include the field\n\
\"hint_outcome\" with one of:\n\
  - \"applied_as_hinted\" — you patched/created exactly as the hint suggested.\n\
  - \"applied_differently\" — you took action but not as hinted (e.g. patched a\n\
    different skill, created instead of patched).\n\
  - \"refused\" — you exited without writing because the hint was unjustified.\n\n\
EXISTING SKILLS:\n{index}\n\nTURN:\nUSER: {user}\nASSISTANT: {assistant}\n"
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

/// Spawn the probe-writer fork. Holds session mutex during fork init only.
/// Returns when fork is established (system/init received); detached task drains
/// the remainder.
pub(crate) async fn run(ctx: ProbeWriterContext, anchor: ProbeAnchor, skill_index: String) {
    let probe_session_id = uuid::Uuid::new_v4().to_string();
    let user_prompt = build_user_prompt(&anchor, &skill_index, &ctx.incoming_hint);
    let active_invocation = match crate::cc::invocation::register_non_foreground_invocation(
        crate::cc::invocation::NonForegroundInvocationRegistration {
            agent_name: ctx.agent_name.clone(),
            agent_dir: ctx.agent_dir.clone(),
            ssh_config_path: ctx.ssh_config_path.clone(),
            resolved_sandbox: ctx.resolved_sandbox.clone(),
            internal_client: Arc::clone(&ctx.internal_client),
            kind: right_mcp::internal_client::ProgressInvocationKindDto::ProbeWriter,
            chat_id: Some(ctx.chat_id),
            thread_id: Some(ctx.thread_id),
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

    let mut cmd = match crate::cc::invocation::build_claude_command(
        &args,
        &ctx.agent_dir,
        ctx.ssh_config_path.as_deref(),
        ctx.resolved_sandbox.as_deref(),
    )
    .await
    {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(agent = %ctx.agent_name, "skipping probe-writer: {e:#}");
            drop(_guard);
            active_invocation.cleanup().await;
            return;
        }
    };
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    let mut child = match right_process::ProcessGroupChild::spawn(cmd) {
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
            let _ = child.kill().await;
            drop(child);
            drop(_guard);
            active_invocation.cleanup().await;
            return;
        }
    };

    // Wait for system/init event, then release mutex by exiting this scope.
    let init_observed =
        wait_for_system_init(stdout, &probe_session_id, PROBE_WRITER_INIT_TIMEOUT).await;
    drop(_guard);

    if !init_observed {
        tracing::warn!(
            agent = %ctx.agent_name,
            "probe-writer never emitted system/init, killing"
        );
        let _ = child.kill().await;
        drop(child);
        active_invocation.cleanup().await;
        return;
    }

    // Detach: probe-writer continues running independently. Drain remaining
    // stdout for usage tracking + final log emit.
    let agent_name = ctx.agent_name.clone();
    let agent_db_dir = ctx.agent_db_dir.clone();
    let chat_id = ctx.chat_id;
    let thread_id = ctx.thread_id;
    tokio::spawn(async move {
        match crate::cc::invocation::wait_with_output_or_kill(child, PROBE_WRITER_TIMEOUT).await {
            Ok(crate::cc::invocation::ChildOutput::Completed(output)) => {
                let stdout_str = String::from_utf8_lossy(&output.stdout).into_owned();
                if let Some(b) = crate::cc::stream::parse_usage_full(&stdout_str)
                    && let Ok(conn) = right_db::open_connection(&agent_db_dir, false).await
                    && let Err(e) = right_agent::usage::insert::insert_learning_probe_writer(
                        &conn, &b, chat_id, thread_id,
                    )
                    .await
                {
                    tracing::warn!(agent = %agent_name, "probe-writer usage insert failed: {e:#}");
                }
                if !output.status.success() {
                    tracing::warn!(
                        agent = %agent_name,
                        status = ?output.status,
                        "probe-writer exited non-zero"
                    );
                }
            }
            Ok(crate::cc::invocation::ChildOutput::TimedOut) => {
                tracing::warn!(
                    agent = %agent_name,
                    "probe-writer timed out after {}s",
                    PROBE_WRITER_TIMEOUT.as_secs()
                );
            }
            Err(e) => {
                tracing::warn!(agent = %agent_name, "probe-writer wait failed: {e:#}");
            }
        }
        active_invocation.cleanup().await;
    });
}

async fn wait_for_system_init<R: AsyncRead + Unpin>(
    stdout: R,
    expected_session_id: &str,
    timeout: Duration,
) -> bool {
    tokio::time::timeout(
        timeout,
        wait_for_system_init_unbounded(stdout, expected_session_id),
    )
    .await
    .unwrap_or(false)
}

async fn wait_for_system_init_unbounded<R: AsyncRead + Unpin>(
    stdout: R,
    expected_session_id: &str,
) -> bool {
    let reader = BufReader::new(stdout);
    let mut lines = reader.lines();
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
mod tests {
    use super::*;

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
            ssh_config_path: None,
            resolved_sandbox: None,
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

        let observed = wait_for_system_init(reader, "probe-sid", Duration::from_secs(30)).await;

        assert!(
            !observed,
            "silent probe-writer child should not hold the session lock forever"
        );
    }
}
