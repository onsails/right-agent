//! Idle-compaction debounce: after 2h of inactivity, run CC's native
//! `/compact` on an opus[1m] session that is >=40% full. See
//! docs/superpowers/specs/2026-05-31-idle-compaction-design.md

use std::time::Duration;

use crate::cc::invocation::{ClaudeInvocation, OutputFormat};

/// Idle window before compaction fires. A turn resets this debounce.
const IDLE_AFTER: Duration = Duration::from_secs(2 * 60 * 60);
/// Compact only when the last turn's context footprint reached this many
/// tokens (40% of the opus[1m] 1,000,000-token window).
const MIN_USED_TOKENS: u64 = 400_000;
/// Wall-clock cap on a single `/compact` call. Bounds how long a returning
/// user waits on the session lock if they arrive mid-compaction.
const COMPACT_TIMEOUT: Duration = Duration::from_secs(120);
/// Steers CC's summary toward the active discussion. Static — CC already has
/// the full conversation at compaction time.
const RECENCY_INSTRUCTION: &str = "Prioritize the most recently discussed \
topics and any open or unresolved threads. Preserve concrete details from \
recent exchanges — names, file paths, decisions, values, and the user's \
current goal — over older, settled context.";

/// True for an Opus model running the 1M-context (`[1m]`) window. Matches the
/// suffix rather than a pinned id so an opus version bump keeps working while
/// `sonnet[1m]` and non-1M opus stay excluded.
pub(crate) fn is_opus_1m(model: Option<&str>) -> bool {
    matches!(model, Some(m) if m.starts_with("claude-opus") && m.ends_with("[1m]"))
}

/// The full gate: opus[1m] AND context footprint at/above the threshold.
pub(crate) fn should_compact(model: Option<&str>, used_tokens: u64) -> bool {
    is_opus_1m(model) && used_tokens >= MIN_USED_TOKENS
}

/// Context footprint of the most recent interactive turn for this session:
/// `input + cache_read + cache_creation` tokens. `None` when no turn exists.
/// This is the prompt size going into the last API call — i.e. how full the
/// context is, regardless of how much was cache-served.
pub(crate) async fn latest_interactive_context_tokens(
    conn: &right_db::Connection,
    chat_id: i64,
    thread_id: i64,
) -> Result<Option<u64>, right_db::DbError> {
    use right_db::OptionalExtension as _;
    conn.query_row(
        "SELECT input_tokens + cache_read_tokens + cache_creation_tokens \
         FROM usage_events \
         WHERE chat_id = ?1 AND thread_id = ?2 AND source = 'interactive' \
         ORDER BY ts DESC LIMIT 1",
        right_db::params![chat_id, thread_id],
        |r| r.get::<_, i64>(0),
    )
    .await
    .optional()
    .map(|opt| opt.map(|v| v.max(0) as u64))
}

/// Build the specialized maintenance invocation: `claude -p --resume <id>
/// "/compact <recency instruction>"`, no schema, no MCP, tools disabled.
/// Deliberate exception to the standard session-bearing contract (see
/// ARCHITECTURE.md → Claude Invocation Contract).
pub(crate) fn build_compact_invocation(
    root_session_id: &str,
    debug: std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> ClaudeInvocation {
    ClaudeInvocation {
        mcp_config_path: None,
        json_schema: None,
        output_format: OutputFormat::Json,
        model: None, // inherit the session's model (opus[1m])
        max_budget_usd: None,
        max_turns: None,
        resume_session_id: Some(root_session_id.to_owned()),
        new_session_id: None,
        fork_session: false,
        allowed_tools: vec![],
        disallowed_tools: vec![],
        extra_args: crate::cc::invocation::disable_all_tools_args(),
        prompt: Some(format!("/compact {RECENCY_INSTRUCTION}")),
        debug_flag: Some(debug),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opus_1m_variants_match() {
        assert!(is_opus_1m(Some("claude-opus-4-8[1m]")));
        assert!(is_opus_1m(Some("claude-opus-4-9[1m]"))); // future bump
    }

    #[test]
    fn non_opus_1m_rejected() {
        assert!(!is_opus_1m(Some("claude-sonnet-4-6[1m]")));
        assert!(!is_opus_1m(Some("claude-opus-4-8"))); // not 1m
        assert!(!is_opus_1m(Some("claude-haiku-4-5")));
        assert!(!is_opus_1m(None));
    }

    #[test]
    fn should_compact_boundary() {
        assert!(should_compact(Some("claude-opus-4-8[1m]"), 400_000));
        assert!(!should_compact(Some("claude-opus-4-8[1m]"), 399_999));
        assert!(!should_compact(Some("claude-sonnet-4-6[1m]"), 1_000_000));
        assert!(!should_compact(None, 1_000_000));
    }

    #[test]
    fn compact_invocation_argv_is_maintenance_shaped() {
        let debug = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let args = build_compact_invocation("root-uuid", debug).into_args();
        let joined = args.join(" ");
        // resumes the real session
        let pos = args.iter().position(|a| a == "--resume").unwrap();
        assert_eq!(args[pos + 1], "root-uuid");
        // prompt is the /compact command with the recency instruction
        let dash = args.iter().position(|a| a == "--").unwrap();
        assert!(args[dash + 1].starts_with("/compact "));
        assert!(args[dash + 1].contains("most recently discussed"));
        // maintenance contract: no schema, no MCP
        assert!(!joined.contains("--json-schema"));
        assert!(!joined.contains("--mcp-config"));
    }

    #[tokio::test]
    async fn fullness_reads_latest_interactive_sum() {
        let dir = tempfile::tempdir().unwrap();
        let conn = right_db::open_connection(dir.path(), true).await.unwrap();

        let mk =
            |input: u64, cache_read: u64, cache_create: u64| right_agent::usage::UsageBreakdown {
                session_uuid: "s".into(),
                total_cost_usd: 0.0,
                num_turns: 1,
                input_tokens: input,
                output_tokens: 0,
                cache_creation_tokens: cache_create,
                cache_read_tokens: cache_read,
                web_search_requests: 0,
                web_fetch_requests: 0,
                model_usage_json: "{}".into(),
                api_key_source: "none".into(),
                wall_elapsed_ms: None,
            };

        // Older smaller turn, then a newer larger turn, for the same (chat, thread).
        right_agent::usage::insert::insert_interactive(&conn, &mk(1, 1, 1), 42, 0)
            .await
            .unwrap();
        right_agent::usage::insert::insert_interactive(&conn, &mk(100, 200, 50), 42, 0)
            .await
            .unwrap();
        // A different source must be ignored.
        right_agent::usage::insert::insert_learning_prefilter(&conn, &mk(9_999, 0, 0), 42, 0)
            .await
            .unwrap();

        let used = latest_interactive_context_tokens(&conn, 42, 0)
            .await
            .unwrap();
        assert_eq!(used, Some(350)); // 100 + 200 + 50

        let absent = latest_interactive_context_tokens(&conn, 999, 0)
            .await
            .unwrap();
        assert_eq!(absent, None);
    }
}
