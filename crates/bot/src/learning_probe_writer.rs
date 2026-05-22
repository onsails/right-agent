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

const PROBE_WRITER_TIMEOUT: Duration = Duration::from_secs(300);
pub(crate) const PROBE_WRITER_MAX_TURNS: u32 = 16;

#[derive(Debug, Clone)]
pub(crate) struct ProbeWriterContext {
    pub agent_dir: PathBuf,
    pub agent_db_dir: PathBuf,
    pub agent_name: String,
    pub ssh_config_path: Option<PathBuf>,
    pub resolved_sandbox: Option<String>,
    pub model: String,
    pub debug_flag: Arc<AtomicBool>,
    pub session_locks: SessionLocks,
    pub chat_id: i64,
    pub thread_id: i64,
}

/// Compose the first user-message body delivered to the fork.
pub(crate) fn build_user_prompt(anchor: &ProbeAnchor, skill_index: &str) -> String {
    let anchor_block = right_codegen::PROBE_WRITER_ANCHOR_TEMPLATE
        .replace("{user_msg_text}", &anchor.user_msg_text)
        .replace("{assistant_reply_text}", &anchor.assistant_reply_text);

    let index = if skill_index.is_empty() {
        "(no existing rightx-* skills)"
    } else {
        skill_index
    };

    format!(
        "{anchor_block}\n\n{instructions}\n\n<skill_index>\n{index}\n</skill_index>",
        instructions = right_codegen::PROBE_WRITER_INSTRUCTIONS,
    )
}

/// Build the `ClaudeInvocation` for the probe-writer fork (pure).
pub(crate) fn build_invocation(
    ctx: &ProbeWriterContext,
    main_session_uuid: &str,
    probe_session_id: &str,
    user_prompt: String,
) -> crate::cc::invocation::ClaudeInvocation {
    use crate::cc::invocation::{ClaudeInvocation, OutputFormat};
    ClaudeInvocation {
        mcp_config_path: Some(crate::cc::invocation::mcp_config_path(
            ctx.ssh_config_path.as_deref(),
            &ctx.agent_dir,
        )),
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
    let user_prompt = build_user_prompt(&anchor, &skill_index);
    let invocation = build_invocation(
        &ctx,
        &anchor.main_session_uuid,
        &probe_session_id,
        user_prompt,
    );
    let args = invocation.into_args();

    // Acquire main-session mutex.
    let lock = ctx
        .session_locks
        .entry(anchor.main_session_uuid.clone())
        .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
        .clone();
    let _guard = lock.lock().await;

    let mut cmd = crate::cc::invocation::build_claude_command(
        &args,
        &ctx.agent_dir,
        ctx.ssh_config_path.as_deref(),
        ctx.resolved_sandbox.as_deref(),
    );
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(
                agent = %ctx.agent_name,
                "probe-writer spawn failed: {e:#}"
            );
            return;
        }
    };

    let stdout = match child.stdout.take() {
        Some(s) => s,
        None => {
            tracing::warn!(agent = %ctx.agent_name, "probe-writer child has no stdout");
            let _ = child.kill().await;
            return;
        }
    };

    // Wait for system/init event, then release mutex by exiting this scope.
    let init_observed = wait_for_system_init(stdout, &probe_session_id).await;
    drop(_guard);

    if !init_observed {
        tracing::warn!(
            agent = %ctx.agent_name,
            "probe-writer never emitted system/init, killing"
        );
        let _ = child.kill().await;
        return;
    }

    // Detach: probe-writer continues running independently. Drain remaining
    // stdout for usage tracking + final log emit.
    let agent_name = ctx.agent_name.clone();
    let agent_db_dir = ctx.agent_db_dir.clone();
    let chat_id = ctx.chat_id;
    let thread_id = ctx.thread_id;
    tokio::spawn(async move {
        let _ = tokio::time::timeout(PROBE_WRITER_TIMEOUT, async {
            match child.wait_with_output().await {
                Ok(output) => {
                    let stdout_str = String::from_utf8_lossy(&output.stdout).into_owned();
                    if let Some(b) = crate::cc::stream::parse_usage_full(&stdout_str)
                        && let Ok(conn) = right_db::open_connection(&agent_db_dir, false)
                        && let Err(e) = right_agent::usage::insert::insert_learning_probe_writer(
                            &conn, &b, chat_id, thread_id,
                        )
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
                Err(e) => {
                    tracing::warn!(agent = %agent_name, "probe-writer wait failed: {e:#}");
                }
            }
        })
        .await;
    });
}

async fn wait_for_system_init<R: AsyncRead + Unpin>(stdout: R, expected_session_id: &str) -> bool {
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

    #[test]
    fn build_user_prompt_includes_anchor_instructions_and_index() {
        let p = build_user_prompt(&anchor("hi", "bye"), "- rightx-foo: bar");
        assert!(p.contains("hi"));
        assert!(p.contains("bye"));
        assert!(p.contains("Survey") || p.contains("survey"));
        assert!(p.contains("rightx-foo: bar"));
    }

    #[test]
    fn build_user_prompt_empty_index_uses_placeholder() {
        let p = build_user_prompt(&anchor("a", "b"), "");
        assert!(p.contains("no existing rightx-* skills"));
    }

    #[test]
    fn probe_writer_max_turns_is_16() {
        assert_eq!(PROBE_WRITER_MAX_TURNS, 16);
    }
}
