//! Telegram alert when the learning review circuit breaker opens.
//!
//! Fired reactively from the failure path in `learning_episode.rs` and the
//! worker skill review path when `record_review_failure` reports
//! `opened_circuit = true`. Dedup is 24 hours per agent (alert_type key
//! `"learning_circuit_open"`) via the shared `alerts` module.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use teloxide::Bot;
use teloxide::prelude::*;

use super::alerts;

const ALERT_TYPE: &str = "learning_circuit_open";

/// Fire-and-forget the circuit-open Telegram alert. Used by every callsite that
/// receives `opened_circuit = true` from `record_review_failure`. Errors are
/// logged inside the spawned task; the caller never awaits.
pub(crate) fn spawn_circuit_open_alert(
    bot: Arc<Bot>,
    agent_db_dir: PathBuf,
    agent_name: String,
    agent_dir: PathBuf,
    reason: String,
    failure_threshold: u32,
    cooldown_minutes: u32,
) {
    tokio::spawn(async move {
        if let Err(e) = maybe_alert_circuit_open(
            bot,
            &agent_db_dir,
            &agent_name,
            &agent_dir,
            &reason,
            cooldown_minutes,
            failure_threshold,
        )
        .await
        {
            tracing::warn!("maybe_alert_circuit_open failed: {e:#}");
        }
    });
}

/// Send a circuit-open alert to the first chat in the agent's allowlist if
/// the 24-hour dedup window allows. No-op if the dedup window blocks the
/// alert or if no recipient can be resolved.
pub(crate) async fn maybe_alert_circuit_open(
    bot: Arc<Bot>,
    db: &Path,
    agent_name: &str,
    agent_dir: &Path,
    last_failure_reason: &str,
    cooldown_minutes: u32,
    failure_threshold: u32,
) -> Result<()> {
    if !alerts::should_fire(db, ALERT_TYPE) {
        return Ok(());
    }
    let Some(chat_id) = first_allowlist_chat(agent_dir)? else {
        tracing::warn!(
            agent = %agent_name,
            "learning circuit open but allowlist has no chat; skipping alert"
        );
        return Ok(());
    };

    let truncated = truncate_with_ellipsis(last_failure_reason, 200);
    let body = format!(
        "❌ <b>Learning review circuit opened</b>\n\n\
         Selector failed {failure_threshold}× in a row. New reviews paused for {cooldown_minutes} minutes.\n\n\
         Last error: <code>{}</code>\n\n\
         ➡️ Check <code>~/.right/logs/{agent_name}.log</code> for details.",
        teloxide::utils::html::escape(&truncated),
    );

    bot.send_message(teloxide::types::ChatId(chat_id), body)
        .parse_mode(teloxide::types::ParseMode::Html)
        .await
        .context("send learning_circuit_open alert")?;
    alerts::record_fire(db, ALERT_TYPE);
    Ok(())
}

/// Truncate `s` to at most `max_chars` characters (Unicode scalar values),
/// appending `…` when truncation occurred. Character-safe — never panics on
/// multi-byte UTF-8 sequences (Cyrillic, CJK, emoji).
fn truncate_with_ellipsis(s: &str, max_chars: usize) -> String {
    let collected: String = s.chars().take(max_chars).collect();
    if collected.len() < s.len() {
        format!("{collected}…")
    } else {
        collected
    }
}

/// Return the first chat id in the agent's allowlist (users first, then
/// groups). Returns `None` if the file does not exist or the lists are empty.
fn first_allowlist_chat(agent_dir: &Path) -> Result<Option<i64>> {
    use right_agent::agent::allowlist;
    let file =
        allowlist::read_file(agent_dir).map_err(|e| anyhow::anyhow!("read allowlist: {e}"))?;
    let Some(file) = file else {
        return Ok(None);
    };
    let id = file
        .users
        .first()
        .map(|u| u.id)
        .or_else(|| file.groups.first().map(|g| g.id));
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::tempdir;

    #[test]
    fn first_allowlist_chat_returns_user_before_group() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("allowlist.yaml");
        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(
            f,
            "version: 1\nusers:\n  - id: 100\n    added_at: 2026-01-01T00:00:00Z\n\
             groups:\n  - id: -200\n    opened_at: 2026-01-01T00:00:00Z\n"
        )
        .unwrap();
        let id = first_allowlist_chat(dir.path()).unwrap();
        assert_eq!(id, Some(100));
    }

    #[test]
    fn first_allowlist_chat_group_when_no_users() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("allowlist.yaml");
        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(
            f,
            "version: 1\nusers: []\ngroups:\n  - id: -200\n    opened_at: 2026-01-01T00:00:00Z\n"
        )
        .unwrap();
        let id = first_allowlist_chat(dir.path()).unwrap();
        assert_eq!(id, Some(-200));
    }

    #[test]
    fn first_allowlist_chat_none_when_both_empty() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("allowlist.yaml");
        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(f, "version: 1\nusers: []\ngroups: []\n").unwrap();
        let id = first_allowlist_chat(dir.path()).unwrap();
        assert_eq!(id, None);
    }

    #[test]
    fn first_allowlist_chat_none_when_file_missing() {
        let dir = tempdir().unwrap();
        let id = first_allowlist_chat(dir.path()).unwrap();
        assert_eq!(id, None);
    }

    #[test]
    fn truncate_with_ellipsis_does_not_panic_on_cyrillic() {
        // 300 Cyrillic chars (2 bytes each in UTF-8). Byte 200 lies inside a
        // codepoint, so byte-slicing at 200 would panic. The char-safe path
        // must return a 200-char prefix plus the ellipsis.
        let input: String = "я".repeat(300);
        let out = truncate_with_ellipsis(&input, 200);
        assert_eq!(out.chars().count(), 201);
        assert!(out.ends_with('…'));
        assert_eq!(out.chars().take(200).collect::<String>(), "я".repeat(200));
    }

    #[test]
    fn truncate_with_ellipsis_no_ellipsis_when_within_limit() {
        let input = "hello";
        let out = truncate_with_ellipsis(input, 200);
        assert_eq!(out, "hello");
    }

    #[test]
    fn truncate_with_ellipsis_handles_emoji_boundary() {
        // Each 😀 is 4 bytes; 60 of them = 240 bytes, 60 chars.
        let input: String = "😀".repeat(60);
        let out = truncate_with_ellipsis(&input, 50);
        assert_eq!(out.chars().count(), 51);
        assert!(out.ends_with('…'));
    }
}
