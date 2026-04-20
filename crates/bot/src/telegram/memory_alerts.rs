//! Watches MemoryStatus + client-flood counters and sends one-shot Telegram alerts
//! with 24h dedup via the `memory_alerts` SQLite table.

use std::path::PathBuf;
use std::sync::Arc;

use chrono::Utc;
use teloxide::prelude::*;
use teloxide::types::ChatId;

use rightclaw::memory::{MemoryStatus, ResilientHindsight};

use super::BotType;

pub const CLIENT_FLOOD_POLL: std::time::Duration = std::time::Duration::from_secs(60);

pub fn spawn_watcher(
    bot: BotType,
    wrapper: Arc<ResilientHindsight>,
    agent_dir: PathBuf,
    allowlist_chats: Vec<i64>,
) {
    // Startup cleanup: delete alerts older than 1h so crash-loops re-notify.
    match rightclaw::memory::open_connection(&agent_dir, false) {
        Ok(conn) => {
            if let Err(e) = conn.execute(
                "DELETE FROM memory_alerts WHERE datetime(first_sent_at) < datetime('now', '-1 hour')",
                [],
            ) {
                tracing::warn!("memory_alerts startup cleanup failed: {e:#}");
            }
        }
        Err(e) => tracing::warn!("memory_alerts startup open_connection failed: {e:#}"),
    }

    // Task A: status watcher.
    {
        let bot = bot.clone();
        let wrapper = wrapper.clone();
        let db = agent_dir.clone();
        let chats = allowlist_chats.clone();
        tokio::spawn(async move {
            let mut rx = wrapper.subscribe_status();
            // Initial check — watch channel's changed() only fires on transitions,
            // so we must handle the current value once on startup (e.g. AuthFailed
            // on boot when the Hindsight API key is bad).
            // Copy out the status so the borrow Ref (not Send) isn't held across .await.
            let initial = *rx.borrow();
            handle_status_change(initial, &bot, &chats, &db).await;
            loop {
                if rx.changed().await.is_err() {
                    return;
                }
                let status = *rx.borrow();
                handle_status_change(status, &bot, &chats, &db).await;
            }
        });
    }

    // Task B: client-flood poller.
    {
        let bot = bot.clone();
        let wrapper = wrapper.clone();
        let db = agent_dir.clone();
        let chats = allowlist_chats.clone();
        tokio::spawn(async move {
            let mut t = tokio::time::interval(CLIENT_FLOOD_POLL);
            loop {
                t.tick().await;
                let drops_1h = wrapper.client_drops_1h().await;
                if drops_1h > rightclaw::memory::resilient::CLIENT_FLOOD_THRESHOLD
                    && should_fire(&db, "client_flood")
                {
                    let msg = format!(
                        "\u{26a0} Memory retains persistently rejected (HTTP 4xx) — \
                         possible Hindsight API drift or payload bug. {drops_1h} drops \
                         in the last hour. Check ~/.rightclaw/logs/ for details."
                    );
                    send_to_chats(&bot, &chats, &msg).await;
                    record_fire(&db, "client_flood");
                }
            }
        });
    }
}

async fn handle_status_change(
    status: MemoryStatus,
    bot: &BotType,
    chats: &[i64],
    db: &std::path::Path,
) {
    if matches!(status, MemoryStatus::AuthFailed { .. }) {
        if should_fire(db, "auth_failed") {
            let msg = "\u{26a0} Memory provider authentication failed.\n\
                       Rotate the Hindsight API key — set `memory.api_key` in \
                       agent.yaml or the HINDSIGHT_API_KEY env var — and restart \
                       the agent. Memory ops are disabled until then.";
            send_to_chats(bot, chats, msg).await;
            record_fire(db, "auth_failed");
        }
    } else if matches!(status, MemoryStatus::Healthy) {
        // Clear dedup on recovery.
        match rightclaw::memory::open_connection(db, false) {
            Ok(conn) => {
                if let Err(e) = conn.execute(
                    "DELETE FROM memory_alerts WHERE alert_type = 'auth_failed'",
                    [],
                ) {
                    tracing::warn!("memory_alerts dedup clear failed: {e:#}");
                }
            }
            Err(e) => tracing::warn!("memory_alerts dedup clear open failed: {e:#}"),
        }
    }
}

fn should_fire(db: &std::path::Path, alert_type: &str) -> bool {
    let conn = match rightclaw::memory::open_connection(db, false) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("should_fire open failed: {e:#}");
            return false;
        }
    };
    let existing: Option<String> = match conn.query_row(
        "SELECT first_sent_at FROM memory_alerts WHERE alert_type = ?1",
        [alert_type],
        |r| r.get(0),
    ) {
        Ok(v) => Some(v),
        Err(rusqlite::Error::QueryReturnedNoRows) => None,
        Err(e) => {
            tracing::warn!("should_fire query failed: {e:#}");
            return false;
        }
    };
    let Some(sent) = existing else { return true };
    let parsed = match chrono::DateTime::parse_from_rfc3339(&sent) {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!("should_fire parse failed: {e:#}");
            return true;
        }
    };
    Utc::now().signed_duration_since(parsed.with_timezone(&Utc)) > chrono::Duration::hours(24)
}

fn record_fire(db: &std::path::Path, alert_type: &str) {
    match rightclaw::memory::open_connection(db, false) {
        Ok(conn) => {
            let now = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
            if let Err(e) = conn.execute(
                "INSERT INTO memory_alerts(alert_type, first_sent_at) VALUES (?1, ?2) \
                 ON CONFLICT(alert_type) DO UPDATE SET first_sent_at = excluded.first_sent_at",
                [alert_type, &now],
            ) {
                tracing::warn!("record_fire failed: {e:#}");
            }
        }
        Err(e) => tracing::warn!("record_fire open failed: {e:#}"),
    }
}

async fn send_to_chats(bot: &BotType, chats: &[i64], text: &str) {
    for &chat_id in chats {
        if let Err(e) = bot.send_message(ChatId(chat_id), text).await {
            tracing::warn!(chat_id, "memory alert send failed: {e:#}");
        }
    }
}
