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
    #[serde(skip_serializing_if = "Option::is_none")]
    document_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    update_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tags: Option<Vec<String>>,
}

/// Retain request body.
#[derive(Debug, Serialize)]
struct RetainRequest {
    items: Vec<RetainItem>,
    #[serde(rename = "async")]
    is_async: bool,
}

/// Request body shared by recall and reflect endpoints.
#[derive(Debug, Serialize)]
struct QueryRequest {
    query: String,
    budget: String,
    max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    tags: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tags_match: Option<String>,
}

/// Join recall results into a single string separated by double newlines.
pub fn join_recall_texts(results: &[RecallResult]) -> String {
    results
        .iter()
        .map(|r| r.text.as_str())
        .collect::<Vec<_>>()
        .join("\n\n")
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

    #[cfg(test)]
    pub(crate) fn with_http_client(mut self, http: reqwest::Client) -> Self {
        self.http = http;
        self
    }

    /// Store content in the memory bank.
    pub async fn retain(
        &self,
        content: &str,
        context: Option<&str>,
        document_id: Option<&str>,
        update_mode: Option<&str>,
        tags: Option<&[String]>,
    ) -> Result<RetainResponse, MemoryError> {
        let url = format!(
            "{}/v1/default/banks/{}/memories",
            self.base_url, self.bank_id
        );
        let body = RetainRequest {
            items: vec![RetainItem {
                content: content.to_owned(),
                context: context.map(|s| s.to_owned()),
                document_id: document_id.map(|s| s.to_owned()),
                update_mode: update_mode.map(|s| s.to_owned()),
                tags: tags.map(|t| t.to_vec()),
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
            .map_err(MemoryError::from_reqwest)?;

        let status = resp.status().as_u16();
        if !resp.status().is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(MemoryError::Hindsight { status, body });
        }

        resp.json::<RetainResponse>()
            .await
            .map_err(MemoryError::from_reqwest)
    }

    /// Recall memories relevant to a query.
    pub async fn recall(
        &self,
        query: &str,
        tags: Option<&[String]>,
        tags_match: Option<&str>,
    ) -> Result<Vec<RecallResult>, MemoryError> {
        let url = format!(
            "{}/v1/default/banks/{}/memories/recall",
            self.base_url, self.bank_id
        );
        let body = QueryRequest {
            query: query.to_owned(),
            budget: self.budget.clone(),
            max_tokens: self.max_tokens,
            tags: tags.map(|t| t.to_vec()),
            tags_match: tags_match.map(|s| s.to_owned()),
        };

        let resp = self
            .http
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(&body)
            .timeout(RECALL_TIMEOUT)
            .send()
            .await
            .map_err(MemoryError::from_reqwest)?;

        let status = resp.status().as_u16();
        if !resp.status().is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(MemoryError::Hindsight { status, body });
        }

        let response: RecallResponse = resp
            .json()
            .await
            .map_err(MemoryError::from_reqwest)?;
        Ok(response.results)
    }

    /// Reflect on memories — synthesized narrative answer to a query.
    pub async fn reflect(&self, query: &str) -> Result<ReflectResponse, MemoryError> {
        let url = format!(
            "{}/v1/default/banks/{}/reflect",
            self.base_url, self.bank_id
        );
        let body = QueryRequest {
            query: query.to_owned(),
            budget: self.budget.clone(),
            max_tokens: self.max_tokens,
            tags: None,
            tags_match: None,
        };

        let resp = self
            .http
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(&body)
            .timeout(REFLECT_TIMEOUT)
            .send()
            .await
            .map_err(MemoryError::from_reqwest)?;

        let status = resp.status().as_u16();
        if !resp.status().is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(MemoryError::Hindsight { status, body });
        }

        resp.json::<ReflectResponse>()
            .await
            .map_err(MemoryError::from_reqwest)
    }

    /// Get the bank profile, creating the bank if it doesn't exist.
    pub async fn get_or_create_bank(&self) -> Result<BankProfile, MemoryError> {
        let url = format!(
            "{}/v1/default/banks/{}/profile",
            self.base_url, self.bank_id
        );

        let resp = self
            .http
            .get(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .timeout(RECALL_TIMEOUT)
            .send()
            .await
            .map_err(MemoryError::from_reqwest)?;

        let status = resp.status().as_u16();
        if !resp.status().is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(MemoryError::Hindsight { status, body });
        }

        resp.json::<BankProfile>()
            .await
            .map_err(MemoryError::from_reqwest)
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

    // --- retain tests ---

    #[tokio::test]
    async fn retain_sends_correct_request() {
        let (handle, url) = mock_hindsight_server(
            r#"{"success": true, "operation_id": "op-123"}"#,
            200,
        )
        .await;

        let client = test_client(&url);
        let resp = client
            .retain("user likes dark mode", Some("preference setting"), None, None, None)
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
        client.retain("some fact", None, None, None, None).await.unwrap();

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
        let err = client.retain("test", None, None, None, None).await.unwrap_err();

        match err {
            MemoryError::Hindsight { status, .. } => assert_eq!(status, 401),
            other => panic!("expected Hindsight error, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn retain_with_document_id_and_tags() {
        let (handle, url) = mock_hindsight_server(
            r#"{"success": true, "operation_id": "op-456"}"#,
            200,
        )
        .await;

        let client = test_client(&url);
        let tags = vec!["chat:12345".to_string()];
        let resp = client
            .retain(
                "User: hello\nAssistant: hi",
                Some("conversation between RightClaw Agent and the User"),
                Some("session-uuid-abc"),
                Some("append"),
                Some(&tags),
            )
            .await
            .unwrap();

        assert!(resp.success);

        let (_method, _auth, body) = handle.await.unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(parsed["items"][0]["document_id"], "session-uuid-abc");
        assert_eq!(parsed["items"][0]["update_mode"], "append");
        assert_eq!(parsed["items"][0]["tags"][0], "chat:12345");
        assert_eq!(
            parsed["items"][0]["context"],
            "conversation between RightClaw Agent and the User"
        );
    }

    #[tokio::test]
    async fn retain_without_document_id_omits_fields() {
        let (handle, url) = mock_hindsight_server(
            r#"{"success": true}"#,
            200,
        )
        .await;

        let client = test_client(&url);
        client.retain("a fact", None, None, None, None).await.unwrap();

        let (_method, _auth, body) = handle.await.unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert!(parsed["items"][0].get("document_id").is_none());
        assert!(parsed["items"][0].get("update_mode").is_none());
        assert!(parsed["items"][0].get("tags").is_none());
        assert!(parsed["items"][0].get("context").is_none());
    }

    // --- recall tests ---

    #[tokio::test]
    async fn recall_sends_correct_request() {
        let (handle, url) = mock_hindsight_server(
            r#"{"results": [{"text": "user likes dark mode", "score": 0.95, "type": "world"}]}"#,
            200,
        )
        .await;

        let client = test_client(&url);
        let results = client.recall("what does user prefer", None, None).await.unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].text, "user likes dark mode");
        assert_eq!(results[0].score, Some(0.95));
        assert_eq!(results[0].fact_type.as_deref(), Some("world"));

        let (method_line, auth, body) = handle.await.unwrap();
        assert!(method_line.starts_with("POST"));
        assert!(method_line.contains("/v1/default/banks/test-bank/memories/recall"));
        assert!(auth.to_lowercase().contains("bearer hs_testkey"));

        let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(parsed["query"], "what does user prefer");
        assert_eq!(parsed["budget"], "high");
        assert_eq!(parsed["max_tokens"], 8192);
    }

    #[tokio::test]
    async fn recall_empty_results() {
        let (_handle, url) = mock_hindsight_server(
            r#"{"results": []}"#,
            200,
        )
        .await;

        let client = test_client(&url);
        let results = client.recall("nonexistent topic", None, None).await.unwrap();
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn recall_with_tags() {
        let (handle, url) = mock_hindsight_server(
            r#"{"results": [{"text": "user likes dark mode", "score": 0.9}]}"#,
            200,
        )
        .await;

        let client = test_client(&url);
        let tags = vec!["chat:12345".to_string()];
        let results = client
            .recall("preferences", Some(&tags), Some("any"))
            .await
            .unwrap();

        assert_eq!(results.len(), 1);

        let (_method, _auth, body) = handle.await.unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(parsed["tags"][0], "chat:12345");
        assert_eq!(parsed["tags_match"], "any");
    }

    #[tokio::test]
    async fn recall_without_tags_omits_fields() {
        let (handle, url) = mock_hindsight_server(
            r#"{"results": []}"#,
            200,
        )
        .await;

        let client = test_client(&url);
        client.recall("test query", None, None).await.unwrap();

        let (_method, _auth, body) = handle.await.unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert!(parsed.get("tags").is_none());
        assert!(parsed.get("tags_match").is_none());
    }

    // --- reflect tests ---

    #[tokio::test]
    async fn reflect_sends_correct_request() {
        let (handle, url) = mock_hindsight_server(
            r#"{"text": "Based on stored memories, the user prefers dark mode."}"#,
            200,
        )
        .await;

        let client = test_client(&url);
        let resp = client.reflect("what are user preferences").await.unwrap();

        assert_eq!(
            resp.text,
            "Based on stored memories, the user prefers dark mode."
        );

        let (method_line, _auth, body) = handle.await.unwrap();
        assert!(method_line.starts_with("POST"));
        assert!(method_line.contains("/v1/default/banks/test-bank/reflect"));

        let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(parsed["query"], "what are user preferences");
        assert_eq!(parsed["budget"], "high");
        assert_eq!(parsed["max_tokens"], 8192);
    }

    // --- get_or_create_bank tests ---

    #[tokio::test]
    async fn get_or_create_bank_success() {
        let (handle, url) = mock_hindsight_server(
            r#"{"bank_id": "test-bank", "name": "Test Bank"}"#,
            200,
        )
        .await;

        let client = test_client(&url);
        let profile = client.get_or_create_bank().await.unwrap();

        assert_eq!(profile.bank_id, "test-bank");
        assert_eq!(profile.name.as_deref(), Some("Test Bank"));

        let (method_line, auth, _body) = handle.await.unwrap();
        assert!(method_line.starts_with("GET"));
        assert!(method_line.contains("/v1/default/banks/test-bank/profile"));
        assert!(auth.to_lowercase().contains("bearer hs_testkey"));
    }

    #[tokio::test]
    async fn get_or_create_bank_401_error() {
        let (_handle, url) = mock_hindsight_server(
            r#"{"error": "invalid api key"}"#,
            401,
        )
        .await;

        let client = test_client(&url);
        let err = client.get_or_create_bank().await.unwrap_err();

        match err {
            MemoryError::Hindsight { status, .. } => assert_eq!(status, 401),
            other => panic!("expected Hindsight error, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn retain_timeout_maps_to_timeout_variant() {
        // Mock server accepts the TCP connection and holds the stream open so
        // the client waits for a response body that never comes. The code's
        // per-request `.timeout(RETAIN_TIMEOUT)` fires and produces a timeout
        // error classified as HindsightTimeout.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let url = format!("http://127.0.0.1:{port}");
        let _keep = tokio::spawn(async move {
            if let Ok((stream, _)) = listener.accept().await {
                tokio::time::sleep(std::time::Duration::from_secs(60)).await;
                drop(stream);
            }
        });

        // Client-level timeout is ignored once the method sets its own
        // `.timeout(RETAIN_TIMEOUT)` per-request — this test tolerates the
        // real 10s wait as the tradeoff for exercising the real code path.
        let client = HindsightClient::new("hs_x", "b", "high", 1024, Some(&url))
            .with_http_client(
                reqwest::Client::builder()
                    .timeout(std::time::Duration::from_millis(200))
                    .build()
                    .unwrap(),
            );
        let err = client.retain("x", None, None, None, None).await.unwrap_err();
        assert!(
            matches!(err, MemoryError::HindsightTimeout),
            "expected HindsightTimeout, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn retain_connect_failure_maps_to_connect_variant() {
        // Port 1 is unprivileged-closed on typical dev machines.
        let client = HindsightClient::new("hs_x", "b", "high", 1024, Some("http://127.0.0.1:1"));
        let err = client.retain("x", None, None, None, None).await.unwrap_err();
        assert!(
            matches!(err, MemoryError::HindsightConnect(_)),
            "expected HindsightConnect, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn retain_json_body_timeout_maps_to_timeout_variant() {
        // Simulate a mid-body-stream timeout: server accepts the request,
        // writes valid response headers advertising a body larger than what
        // it sends, then stalls past the per-request RETAIN_TIMEOUT (10s).
        // The client's `.json()` call (which reads the full body via
        // `bytes().await`) observes the per-request timeout firing during
        // body read. reqwest classifies this as both `is_timeout() == true`
        // and `is_decode() == true` — because `is_timeout()` walks the source
        // chain looking for the internal `TimedOut` marker. Since our
        // `from_reqwest` checks `is_timeout()` FIRST, this must surface as
        // `HindsightTimeout`, not `HindsightParse`. This guards against
        // routing `.json()` errors through `from_parse` (which would
        // type-erase the timeout kind and misclassify a transient timeout
        // as a malformed-body poison pill).
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let url = format!("http://127.0.0.1:{port}");

        let _keep = tokio::spawn(async move {
            if let Ok((mut stream, _)) = listener.accept().await {
                use tokio::io::{AsyncReadExt, AsyncWriteExt};
                let mut buf = vec![0u8; 8192];
                let _ = stream.read(&mut buf).await;
                let header = "HTTP/1.1 200 OK\r\nContent-Length: 999\r\nContent-Type: application/json\r\n\r\n";
                let _ = stream.write_all(header.as_bytes()).await;
                let _ = stream.write_all(b"{\"id\":\"x\"").await;
                let _ = stream.flush().await;
                // Sleep past RETAIN_TIMEOUT (10s) so the per-request timeout
                // fires during body read instead of EOF on stream drop.
                tokio::time::sleep(std::time::Duration::from_secs(12)).await;
                drop(stream);
            }
        });

        let client = test_client(&url);
        let err = client.retain("x", None, None, None, None).await.unwrap_err();
        assert!(
            matches!(err, MemoryError::HindsightTimeout),
            "expected HindsightTimeout, got: {err:?}"
        );
    }
}
