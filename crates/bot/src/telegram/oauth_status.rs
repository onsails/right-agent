use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::Serialize;
use tokio::sync::Mutex;

#[derive(Clone, Default)]
pub(crate) struct OAuthFlowStatusStore {
    inner: Arc<Mutex<HashMap<String, OAuthFlowStatusEntry>>>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct OAuthFlowStatusResponse {
    pub flow_id: String,
    pub server_name: Option<String>,
    pub status: OAuthFlowStatus,
    pub message: Option<String>,
    pub updated_at: String,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum OAuthFlowStatus {
    Pending,
    Succeeded,
    Failed,
    Expired,
    Unknown,
}

#[derive(Debug, Clone)]
struct OAuthFlowStatusEntry {
    server_name: String,
    status: OAuthFlowStatus,
    message: Option<String>,
    started_at: Instant,
    updated_at: chrono::DateTime<chrono::Utc>,
}

impl OAuthFlowStatusStore {
    pub(crate) async fn insert_pending(&self, flow_id: String, server_name: String) {
        self.inner.lock().await.insert(
            flow_id,
            OAuthFlowStatusEntry {
                server_name,
                status: OAuthFlowStatus::Pending,
                message: None,
                started_at: Instant::now(),
                updated_at: chrono::Utc::now(),
            },
        );
    }

    pub(crate) async fn mark_succeeded(&self, flow_id: &str) {
        self.update(flow_id, OAuthFlowStatus::Succeeded, None).await;
    }

    pub(crate) async fn mark_failed(&self, flow_id: &str, message: impl Into<String>) {
        self.update(flow_id, OAuthFlowStatus::Failed, Some(message.into()))
            .await;
    }

    pub(crate) async fn status(&self, flow_id: &str) -> OAuthFlowStatusResponse {
        match self.inner.lock().await.get(flow_id) {
            Some(entry) => OAuthFlowStatusResponse {
                flow_id: flow_id.to_string(),
                server_name: Some(entry.server_name.clone()),
                status: entry.status,
                message: entry.message.clone(),
                updated_at: entry.updated_at.to_rfc3339(),
            },
            None => OAuthFlowStatusResponse {
                flow_id: flow_id.to_string(),
                server_name: None,
                status: OAuthFlowStatus::Unknown,
                message: Some("OAuth flow is no longer active.".to_string()),
                updated_at: chrono::Utc::now().to_rfc3339(),
            },
        }
    }

    pub(crate) async fn expire_pending_older_than(&self, max_age: Duration) -> usize {
        let mut inner = self.inner.lock().await;
        let now = Instant::now();
        let updated_at = chrono::Utc::now();
        let mut expired = 0;

        for entry in inner.values_mut() {
            if entry.status == OAuthFlowStatus::Pending
                && now.duration_since(entry.started_at) > max_age
            {
                entry.status = OAuthFlowStatus::Expired;
                entry.message = Some("OAuth flow expired before completion.".to_string());
                entry.updated_at = updated_at;
                expired += 1;
            }
        }

        expired
    }

    async fn update(&self, flow_id: &str, status: OAuthFlowStatus, message: Option<String>) {
        if let Some(entry) = self.inner.lock().await.get_mut(flow_id) {
            entry.status = status;
            entry.message = message;
            entry.updated_at = chrono::Utc::now();
        }
    }

    #[cfg(test)]
    async fn force_started_at_for_test(&self, flow_id: &str, started_at: Instant) {
        if let Some(entry) = self.inner.lock().await.get_mut(flow_id) {
            entry.started_at = started_at;
        }
    }
}

pub(crate) fn compact_dashboard_error(detail: &str) -> String {
    const SERVER_ERROR_PREFIX: &str = "Server error (502): ";

    if let Some(body) = detail.strip_prefix(SERVER_ERROR_PREFIX)
        && let Ok(value) = serde_json::from_str::<serde_json::Value>(body)
        && let Some(error) = value.get("error").and_then(|error| error.as_str())
    {
        if contains_secret_like_word(error) {
            return "OAuth error details were redacted.".to_string();
        }
        return format!("{SERVER_ERROR_PREFIX}{error}");
    }

    if contains_secret_like_word(detail) {
        return "OAuth error details were redacted.".to_string();
    }

    detail.chars().take(240).collect()
}

fn contains_secret_like_word(detail: &str) -> bool {
    const SECRET_WORDS: &[&str] = &[
        "access_token",
        "api_key",
        "authorization",
        "client_secret",
        "code_verifier",
        "password",
        "secret",
        "token",
    ];

    let detail = detail.to_ascii_lowercase();
    SECRET_WORDS.iter().any(|word| detail.contains(word))
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::*;

    #[tokio::test]
    async fn pending_status_round_trips_by_flow_id() {
        let store = OAuthFlowStatusStore::default();
        store
            .insert_pending("flow-1".to_string(), "composio".to_string())
            .await;

        let status = store.status("flow-1").await;

        assert_eq!(status.flow_id, "flow-1");
        assert_eq!(status.server_name.as_deref(), Some("composio"));
        assert_eq!(status.status, OAuthFlowStatus::Pending);
        assert_eq!(status.message, None);
    }

    #[tokio::test]
    async fn terminal_status_updates_existing_flow() {
        let store = OAuthFlowStatusStore::default();
        store
            .insert_pending("flow-1".to_string(), "composio".to_string())
            .await;

        store.mark_failed("flow-1", "MCP readiness failed").await;
        assert_eq!(store.status("flow-1").await.status, OAuthFlowStatus::Failed);

        store.mark_succeeded("flow-1").await;
        let status = store.status("flow-1").await;
        assert_eq!(status.status, OAuthFlowStatus::Succeeded);
        assert_eq!(status.message, None);
    }

    #[tokio::test]
    async fn mark_failed_preserves_already_sanitized_dashboard_message() {
        let store = OAuthFlowStatusStore::default();
        store
            .insert_pending("flow-1".to_string(), "composio".to_string())
            .await;
        let message = "Token exchange completed, but MCP readiness failed: Server error (502): mcp_reconnect_failed";

        store.mark_failed("flow-1", message).await;

        let status = store.status("flow-1").await;
        assert_eq!(status.status, OAuthFlowStatus::Failed);
        assert_eq!(status.message.as_deref(), Some(message));
    }

    #[tokio::test]
    async fn unknown_flow_returns_terminal_unknown_status() {
        let status = OAuthFlowStatusStore::default().status("missing").await;

        assert_eq!(status.flow_id, "missing");
        assert_eq!(status.server_name, None);
        assert_eq!(status.status, OAuthFlowStatus::Unknown);
        assert_eq!(
            status.message.as_deref(),
            Some("OAuth flow is no longer active.")
        );
    }

    #[tokio::test]
    async fn cleanup_marks_old_pending_flows_expired() {
        let store = OAuthFlowStatusStore::default();
        store
            .insert_pending("flow-1".to_string(), "composio".to_string())
            .await;
        store
            .force_started_at_for_test("flow-1", Instant::now() - Duration::from_secs(700))
            .await;

        assert_eq!(
            store
                .expire_pending_older_than(Duration::from_secs(600))
                .await,
            1
        );
        let status = store.status("flow-1").await;
        assert_eq!(status.status, OAuthFlowStatus::Expired);
        assert_eq!(
            status.message.as_deref(),
            Some("OAuth flow expired before completion.")
        );
    }

    #[test]
    fn compact_internal_client_error_removes_secret_bearing_body() {
        let message = compact_dashboard_error(
            "Server error (502): {\"error\":\"mcp_reconnect_failed\",\"detail\":\"Unavailable resource\",\"access_token\":\"secret\"}",
        );

        assert_eq!(message, "Server error (502): mcp_reconnect_failed");
        assert!(!message.contains("secret"));
        assert!(!message.contains("access_token"));
        assert!(!message.contains("Unavailable resource"));
    }

    #[test]
    fn compact_dashboard_error_redacts_secret_like_json_error() {
        let message =
            compact_dashboard_error("Server error (502): {\"error\":\"access_token_secret\"}");

        assert_eq!(message, "OAuth error details were redacted.");
        assert!(!message.contains("access_token_secret"));
    }

    #[test]
    fn compact_dashboard_error_redacts_code_verifier() {
        let message = compact_dashboard_error("OAuth failed: code_verifier=abc123");

        assert_eq!(message, "OAuth error details were redacted.");
        assert!(!message.contains("code_verifier"));
        assert!(!message.contains("abc123"));
    }
}
