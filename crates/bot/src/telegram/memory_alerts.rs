//! Watches MemoryStatus + client-flood counters and sends one-shot Telegram alerts
//! with 24h dedup via the `memory_alerts` SQLite table.

use std::path::PathBuf;
use std::sync::Arc;

use right_agent::agent::allowlist::AllowlistHandle;
use right_memory::alert_types::{AUTH_FAILED, CLIENT_FLOOD};
use right_memory::{MemoryStatus, ResilientHindsight};

use super::{BotType, broadcast_to_chats};

pub const CLIENT_FLOOD_POLL: std::time::Duration = std::time::Duration::from_secs(60);

/// Resolve the current recipient chat list from the live allowlist handle.
/// Union of trusted user ids and opened group ids. The read-lock is
/// released before the value returns so the caller can safely `.await`.
fn current_chats(allowlist: &AllowlistHandle) -> Vec<i64> {
    let state = allowlist.0.read().expect("allowlist lock poisoned");
    state
        .users()
        .iter()
        .map(|u| u.id)
        .chain(state.groups().iter().map(|g| g.id))
        .collect()
}

pub(crate) fn spawn_watcher(
    bot: BotType,
    wrapper: Arc<ResilientHindsight>,
    agent_dir: PathBuf,
    allowlist: AllowlistHandle,
) {
    // Startup cleanup: allow memory alerts older than one hour to re-notify.
    if let Ok((client, agent)) = crate::db::client_for_agent_dir(&agent_dir) {
        tokio::spawn(async move {
            for alert_type in [AUTH_FAILED, CLIENT_FLOOD] {
                if let Err(error) = client
                    .alert_clear(&right_mcp::internal_db::AlertClearRequest {
                        agent: agent.clone(),
                        alert_type: alert_type.to_owned(),
                        older_than_secs: Some(3600),
                    })
                    .await
                {
                    tracing::warn!("memory_alerts startup owner cleanup failed: {error:#}");
                }
            }
        });
    }

    // Task A: status watcher.
    {
        let bot = bot.clone();
        let wrapper = wrapper.clone();
        let db = agent_dir.clone();
        let allowlist = allowlist.clone();
        tokio::spawn(async move {
            let mut rx = wrapper.subscribe_status();
            // Initial check — watch channel's changed() only fires on transitions,
            // so we must handle the current value once on startup (e.g. AuthFailed
            // on boot when the Hindsight API key is bad).
            // Copy out the status so the borrow Ref (not Send) isn't held across .await.
            let initial = *rx.borrow();
            handle_status_change(initial, &bot, &allowlist, &db).await;
            loop {
                if rx.changed().await.is_err() {
                    return;
                }
                let status = *rx.borrow();
                handle_status_change(status, &bot, &allowlist, &db).await;
            }
        });
    }

    // Task B: client-flood poller.
    {
        let bot = bot.clone();
        let wrapper = wrapper.clone();
        let db = agent_dir.clone();
        let allowlist = allowlist.clone();
        tokio::spawn(async move {
            let mut t = tokio::time::interval(CLIENT_FLOOD_POLL);
            loop {
                t.tick().await;
                let drops_1h = wrapper.client_drops_1h().await;
                if drops_1h > right_memory::resilient::CLIENT_FLOOD_THRESHOLD
                    && super::alerts::should_fire(&db, CLIENT_FLOOD).await
                {
                    let msg = format!(
                        "\u{26a0} Memory retains persistently rejected (HTTP 4xx) — \
                         possible Hindsight API drift or payload bug. {drops_1h} drops \
                         in the last hour. Check ~/.right/logs/ for details."
                    );
                    // Resolve recipients at broadcast time so /allow / /deny
                    // changes after startup are honored.
                    let chats = current_chats(&allowlist);
                    broadcast_to_chats(&bot, &chats, &msg).await;
                    super::alerts::record_fire(&db, CLIENT_FLOOD).await;
                }
            }
        });
    }
}

async fn handle_status_change(
    status: MemoryStatus,
    bot: &BotType,
    allowlist: &AllowlistHandle,
    db: &std::path::Path,
) {
    if matches!(status, MemoryStatus::AuthFailed { .. }) {
        if super::alerts::should_fire(db, AUTH_FAILED).await {
            let msg = "\u{26a0} Memory provider authentication failed.\n\
                       Rotate the Hindsight API key — set `memory.api_key` in \
                       agent.yaml or the HINDSIGHT_API_KEY env var — and restart \
                       the agent. Memory ops are disabled until then.";
            // Resolve recipients at broadcast time so /allow / /deny
            // changes after startup are honored.
            let chats = current_chats(allowlist);
            broadcast_to_chats(bot, &chats, msg).await;
            super::alerts::record_fire(db, AUTH_FAILED).await;
        }
    } else if matches!(status, MemoryStatus::Healthy) {
        if let Ok((client, agent)) = crate::db::client_for_agent_dir(db)
            && let Err(error) = client
                .alert_clear(&right_mcp::internal_db::AlertClearRequest {
                    agent,
                    alert_type: AUTH_FAILED.to_owned(),
                    older_than_secs: None,
                })
                .await
        {
            tracing::warn!("memory_alerts owner dedup clear failed: {error:#}");
        }
    }
}
