//! Hindsight Cloud API client for agent memory.
//!
//! Wraps the Hindsight HTTP API (retain, recall, reflect, bank management).
//! Used by both the bot (auto-retain/recall) and the MCP aggregator
//! (HindsightBackend for explicit agent tool calls).

use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::MemoryError;

const DEFAULT_BASE_URL: &str = "https://api.hindsight.vectorize.io";
const RETAIN_TIMEOUT: Duration = Duration::from_secs(10);
const RECALL_TIMEOUT: Duration = Duration::from_secs(5);
const REFLECT_TIMEOUT: Duration = Duration::from_secs(15);

/// A single recall result from Hindsight.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RecallResult {
    pub text: String,
    #[serde(default)]
    pub score: Option<f64>,
    #[serde(rename = "type", default)]
    pub fact_type: Option<String>,
}

/// Response from the recall endpoint.
#[derive(Debug, Deserialize)]
pub struct RecallResponse {
    pub results: Vec<RecallResult>,
}

/// Response from the retain endpoint.
#[derive(Debug, Deserialize)]
pub struct RetainResponse {
    #[serde(default)]
    pub success: bool,
    #[serde(default)]
    pub operation_id: Option<String>,
}

/// Response from the reflect endpoint.
#[derive(Debug, Deserialize)]
pub struct ReflectResponse {
    pub text: String,
}

/// Bank profile (returned by GET /profile, auto-creates if absent).
#[derive(Debug, Deserialize)]
pub struct BankProfile {
    pub bank_id: String,
    #[serde(default)]
    pub name: Option<String>,
}

/// Retain request item.
#[derive(Debug, Serialize)]
struct RetainItem {
    content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    context: Option<String>,
}

/// Retain request body.
#[derive(Debug, Serialize)]
struct RetainRequest {
    items: Vec<RetainItem>,
    #[serde(rename = "async")]
    is_async: bool,
}

/// Recall request body.
#[derive(Debug, Serialize)]
struct RecallRequest {
    query: String,
    budget: String,
    max_tokens: u32,
}

/// Reflect request body.
#[derive(Debug, Serialize)]
struct ReflectRequest {
    query: String,
    budget: String,
    max_tokens: u32,
}

/// Client for the Hindsight Cloud HTTP API.
#[derive(Clone)]
pub struct HindsightClient {
    http: reqwest::Client,
    base_url: String,
    api_key: String,
    bank_id: String,
    budget: String,
    max_tokens: u32,
}

impl HindsightClient {
    /// Create a new client.
    ///
    /// - `api_key`: Hindsight API key (e.g. `hs_...`)
    /// - `bank_id`: Memory bank identifier (typically agent name)
    /// - `budget`: Recall budget level ("low", "mid", "high")
    /// - `max_tokens`: Max tokens for recall results
    /// - `base_url`: Override for testing; `None` uses Hindsight Cloud
    pub fn new(
        api_key: &str,
        bank_id: &str,
        budget: &str,
        max_tokens: u32,
        base_url: Option<&str>,
    ) -> Self {
        Self {
            http: reqwest::Client::new(),
            base_url: base_url
                .unwrap_or(DEFAULT_BASE_URL)
                .trim_end_matches('/')
                .to_owned(),
            api_key: api_key.to_owned(),
            bank_id: bank_id.to_owned(),
            budget: budget.to_owned(),
            max_tokens,
        }
    }

    /// Store content in the memory bank.
    pub async fn retain(
        &self,
        content: &str,
        context: Option<&str>,
    ) -> Result<RetainResponse, MemoryError> {
        let url = format!(
            "{}/v1/default/banks/{}/memories",
            self.base_url, self.bank_id
        );
        let body = RetainRequest {
            items: vec![RetainItem {
                content: content.to_owned(),
                context: context.map(|s| s.to_owned()),
            }],
            is_async: true,
        };

        let resp = self
            .http
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(&body)
            .timeout(RETAIN_TIMEOUT)
            .send()
            .await
            .map_err(|e| MemoryError::HindsightRequest(format!("{e:#}")))?;

        let status = resp.status().as_u16();
        if !resp.status().is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(MemoryError::Hindsight { status, body });
        }

        resp.json::<RetainResponse>()
            .await
            .map_err(|e| MemoryError::HindsightRequest(format!("parse retain response: {e:#}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn mock_hindsight_server(
        response_body: &str,
        response_status: u16,
    ) -> (tokio::task::JoinHandle<(String, String, String)>, String) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let url = format!("http://127.0.0.1:{port}");
        let body = response_body.to_owned();

        let handle = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            use tokio::io::{AsyncReadExt, AsyncWriteExt};

            let mut buf = vec![0u8; 8192];
            let n = stream.read(&mut buf).await.unwrap();
            let request = String::from_utf8_lossy(&buf[..n]).to_string();

            let first_line = request.lines().next().unwrap_or("").to_string();
            let auth = request
                .lines()
                .find(|l| l.to_lowercase().starts_with("authorization:"))
                .unwrap_or("")
                .to_string();
            let req_body = request
                .split("\r\n\r\n")
                .nth(1)
                .unwrap_or("")
                .to_string();

            let response = format!(
                "HTTP/1.1 {response_status} OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(response.as_bytes()).await.unwrap();
            (first_line, auth, req_body)
        });

        (handle, url)
    }

    fn test_client(base_url: &str) -> HindsightClient {
        HindsightClient::new("hs_testkey", "test-bank", "high", 8192, Some(base_url))
    }

    #[tokio::test]
    async fn retain_sends_correct_request() {
        let (handle, url) = mock_hindsight_server(
            r#"{"success": true, "operation_id": "op-123"}"#,
            200,
        )
        .await;

        let client = test_client(&url);
        let resp = client
            .retain("user likes dark mode", Some("preference setting"))
            .await
            .unwrap();

        assert!(resp.success);
        assert_eq!(resp.operation_id.as_deref(), Some("op-123"));

        let (method_line, auth, body) = handle.await.unwrap();
        assert!(method_line.starts_with("POST"));
        assert!(method_line.contains("/v1/default/banks/test-bank/memories"));
        assert!(auth.to_lowercase().contains("bearer hs_testkey"));

        let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(parsed["items"][0]["content"], "user likes dark mode");
        assert_eq!(parsed["items"][0]["context"], "preference setting");
        assert_eq!(parsed["async"], true);
    }

    #[tokio::test]
    async fn retain_without_context() {
        let (handle, url) = mock_hindsight_server(
            r#"{"success": true}"#,
            200,
        )
        .await;

        let client = test_client(&url);
        client.retain("some fact", None).await.unwrap();

        let (_method, _auth, body) = handle.await.unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert!(parsed["items"][0].get("context").is_none());
    }

    #[tokio::test]
    async fn retain_http_error_returns_error() {
        let (_handle, url) = mock_hindsight_server(
            r#"{"error": "unauthorized"}"#,
            401,
        )
        .await;

        let client = test_client(&url);
        let err = client.retain("test", None).await.unwrap_err();

        match err {
            MemoryError::Hindsight { status, .. } => assert_eq!(status, 401),
            other => panic!("expected Hindsight error, got: {other:?}"),
        }
    }
}
