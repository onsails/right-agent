//! Shared Telegram alert dedup helpers. The Aggregator-owned database enforces
//! the 24-hour window; the bot never opens `data.db`.

use std::path::Path;

/// Atomically check and record an alert. Returns true when the caller should
/// send it. Failure is fail-closed to avoid alert storms.
pub(crate) async fn should_fire(db: &Path, alert_type: &str) -> bool {
    let (client, agent) = match crate::db::client_for_agent_dir(db) {
        Ok(pair) => pair,
        Err(error) => {
            tracing::warn!("alerts::should_fire client resolution failed: {error:#}");
            return false;
        }
    };
    let request = right_mcp::internal_db::AlertCheckAndRecordRequest {
        agent,
        request_id: crate::db::request_id(),
        alert_type: alert_type.to_owned(),
    };
    match client.alert_check_and_record(&request).await {
        Ok(response) => response.should_fire,
        Err(error) => {
            tracing::warn!("alerts::should_fire owner operation failed: {error:#}");
            false
        }
    }
}

/// Compatibility no-op: [`should_fire`] now performs the record atomically,
/// eliminating the old check/send/record race.
pub(crate) async fn record_fire(_db: &Path, _alert_type: &str) {}
