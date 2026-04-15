# Hindsight Memory Integration — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Two-mode agent memory — MEMORY.md file (default) and Hindsight Cloud (optional) — replacing the old store_record/query_records MCP tools.

**Architecture:** HindsightClient in rightclaw core wraps the Hindsight HTTP API. Bot performs auto-retain/prefetch. Aggregator exposes 3 MCP tools via HindsightBackend. Prompt assembly injects memory at the end of system prompt. Two skill variants selected at install time by memory provider config.

**Tech Stack:** Rust, reqwest (already in workspace), serde/serde_json, tokio, Arc<RwLock<HashMap>>

**Spec:** `docs/superpowers/specs/2026-04-15-hindsight-memory-design.md`

---

### Task 1: MemoryConfig in AgentConfig

**Files:**
- Modify: `crates/rightclaw/src/agent/types.rs`

- [ ] **Step 1: Write failing tests for MemoryConfig parsing**

Add these tests to the existing `#[cfg(test)] mod tests` block in `crates/rightclaw/src/agent/types.rs`:

```rust
#[test]
fn memory_config_defaults_to_file() {
    let yaml = "{}";
    let config: AgentConfig = serde_saphyr::from_str(yaml).unwrap();
    let mem = config.memory.unwrap_or_default();
    assert_eq!(mem.provider, MemoryProvider::File);
    assert!(mem.api_key.is_none());
    assert!(mem.bank_id.is_none());
}

#[test]
fn memory_config_hindsight_full() {
    let yaml = r#"
memory:
  provider: hindsight
  api_key: "hs_test123"
  bank_id: "my-agent"
  recall_budget: "high"
  recall_max_tokens: 8192
"#;
    let config: AgentConfig = serde_saphyr::from_str(yaml).unwrap();
    let mem = config.memory.unwrap();
    assert_eq!(mem.provider, MemoryProvider::Hindsight);
    assert_eq!(mem.api_key.as_deref(), Some("hs_test123"));
    assert_eq!(mem.bank_id.as_deref(), Some("my-agent"));
    assert_eq!(mem.recall_budget, RecallBudget::High);
    assert_eq!(mem.recall_max_tokens, 8192);
}

#[test]
fn memory_config_file_ignores_hindsight_fields() {
    let yaml = r#"
memory:
  provider: file
"#;
    let config: AgentConfig = serde_saphyr::from_str(yaml).unwrap();
    let mem = config.memory.unwrap();
    assert_eq!(mem.provider, MemoryProvider::File);
    assert!(mem.api_key.is_none());
}

#[test]
fn memory_config_defaults_recall_budget_mid() {
    let yaml = r#"
memory:
  provider: hindsight
  api_key: "hs_test"
"#;
    let config: AgentConfig = serde_saphyr::from_str(yaml).unwrap();
    let mem = config.memory.unwrap();
    assert_eq!(mem.recall_budget, RecallBudget::Mid);
    assert_eq!(mem.recall_max_tokens, 4096);
}

#[test]
fn memory_config_absent_section_is_none() {
    let yaml = "restart: never";
    let config: AgentConfig = serde_saphyr::from_str(yaml).unwrap();
    assert!(config.memory.is_none());
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p rightclaw --lib agent::types::tests::memory_config -- 2>&1 | head -30`
Expected: compilation error — `MemoryConfig`, `MemoryProvider`, `RecallBudget` don't exist.

- [ ] **Step 3: Implement MemoryConfig types**

Add before the `AgentConfig` struct in `crates/rightclaw/src/agent/types.rs`:

```rust
/// Memory provider for an agent.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MemoryProvider {
    /// Simple MEMORY.md file (default).
    #[default]
    File,
    /// Hindsight Cloud.
    Hindsight,
}

/// Recall search thoroughness.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RecallBudget {
    Low,
    #[default]
    Mid,
    High,
}

impl std::fmt::Display for RecallBudget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RecallBudget::Low => write!(f, "low"),
            RecallBudget::Mid => write!(f, "mid"),
            RecallBudget::High => write!(f, "high"),
        }
    }
}

fn default_recall_max_tokens() -> u32 {
    4096
}

/// Memory configuration from agent.yaml `memory:` section.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryConfig {
    #[serde(default)]
    pub provider: MemoryProvider,

    /// Hindsight API key (required when provider=hindsight).
    pub api_key: Option<String>,

    /// Hindsight bank ID (default = agent name).
    pub bank_id: Option<String>,

    /// Recall search thoroughness.
    #[serde(default)]
    pub recall_budget: RecallBudget,

    /// Max tokens returned by recall.
    #[serde(default = "default_recall_max_tokens")]
    pub recall_max_tokens: u32,
}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            provider: MemoryProvider::File,
            api_key: None,
            bank_id: None,
            recall_budget: RecallBudget::Mid,
            recall_max_tokens: 4096,
        }
    }
}
```

Add the `memory` field to `AgentConfig`:

```rust
/// Memory configuration.
#[serde(default)]
pub memory: Option<MemoryConfig>,
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p rightclaw --lib agent::types::tests -- 2>&1 | tail -20`
Expected: ALL tests pass (including pre-existing ones — verify `deny_unknown_fields` still works with optional `memory` section).

- [ ] **Step 5: Commit**

```bash
git add crates/rightclaw/src/agent/types.rs
git commit -m "feat(memory): add MemoryConfig to AgentConfig with file/hindsight provider"
```

---

### Task 2: HindsightClient — types and error

**Files:**
- Create: `crates/rightclaw/src/memory/hindsight.rs`
- Modify: `crates/rightclaw/src/memory/mod.rs`
- Modify: `crates/rightclaw/src/memory/error.rs`

- [ ] **Step 1: Add HindsightError variant to MemoryError**

In `crates/rightclaw/src/memory/error.rs`, add a new variant:

```rust
#[error("hindsight API error (HTTP {status}): {body}")]
Hindsight { status: u16, body: String },

#[error("hindsight request failed: {0}")]
HindsightRequest(String),
```

- [ ] **Step 2: Create hindsight.rs with types and empty client**

Create `crates/rightclaw/src/memory/hindsight.rs`:

```rust
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
}
```

- [ ] **Step 3: Export hindsight module from mod.rs**

In `crates/rightclaw/src/memory/mod.rs`, add:

```rust
pub mod hindsight;
```

- [ ] **Step 4: Verify it compiles**

Run: `cargo check -p rightclaw 2>&1 | tail -10`
Expected: compiles with no errors (possibly warnings about unused code, that's fine).

- [ ] **Step 5: Commit**

```bash
git add crates/rightclaw/src/memory/hindsight.rs crates/rightclaw/src/memory/mod.rs crates/rightclaw/src/memory/error.rs
git commit -m "feat(memory): add HindsightClient types and error variants"
```

---

### Task 3: HindsightClient — retain method

**Files:**
- Modify: `crates/rightclaw/src/memory/hindsight.rs`

- [ ] **Step 1: Write failing test for retain**

Add at the bottom of `crates/rightclaw/src/memory/hindsight.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    /// Start a mock HTTP server that captures the request and returns a fixed response.
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

            // Extract method+path from first line
            let first_line = request.lines().next().unwrap_or("").to_string();
            // Extract Authorization header
            let auth = request
                .lines()
                .find(|l| l.to_lowercase().starts_with("authorization:"))
                .unwrap_or("")
                .to_string();
            // Extract body (after \r\n\r\n)
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

    #[tokio::test]
    async fn retain_sends_correct_request() {
        let (handle, url) = mock_hindsight_server(
            r#"{"success": true, "operation_id": "op-123"}"#,
            200,
        ).await;

        let client = HindsightClient::new("hs_testkey", "test-bank", "mid", 4096, Some(&url));
        let result = client.retain("user likes dark mode", Some("user preference")).await;
        assert!(result.is_ok());

        let (first_line, auth, body) = handle.await.unwrap();
        assert!(first_line.contains("POST"));
        assert!(first_line.contains("/v1/default/banks/test-bank/memories"));
        assert!(auth.contains("Bearer hs_testkey"));

        let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(parsed["items"][0]["content"], "user likes dark mode");
        assert_eq!(parsed["items"][0]["context"], "user preference");
        assert_eq!(parsed["async"], true);
    }

    #[tokio::test]
    async fn retain_without_context() {
        let (handle, url) = mock_hindsight_server(
            r#"{"success": true}"#,
            200,
        ).await;

        let client = HindsightClient::new("hs_k", "b", "mid", 4096, Some(&url));
        client.retain("some fact", None).await.unwrap();

        let (_, _, body) = handle.await.unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert!(parsed["items"][0].get("context").is_none() ||
                parsed["items"][0]["context"].is_null());
    }

    #[tokio::test]
    async fn retain_http_error_returns_error() {
        let (handle, url) = mock_hindsight_server(
            r#"{"error": "unauthorized"}"#,
            401,
        ).await;

        let client = HindsightClient::new("bad_key", "b", "mid", 4096, Some(&url));
        let result = client.retain("test", None).await;
        assert!(result.is_err());

        let err = result.unwrap_err();
        match err {
            MemoryError::Hindsight { status, .. } => assert_eq!(status, 401),
            other => panic!("expected Hindsight error, got: {other:?}"),
        }
        let _ = handle.await;
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p rightclaw --lib memory::hindsight::tests::retain -- 2>&1 | head -20`
Expected: compilation error — `retain` method doesn't exist on `HindsightClient`.

- [ ] **Step 3: Implement retain method**

Add to the `impl HindsightClient` block:

```rust
/// Store content in Hindsight memory.
///
/// - `content`: The information to remember
/// - `context`: Optional short label (e.g. "user preference", "conversation")
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
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p rightclaw --lib memory::hindsight::tests::retain -- 2>&1 | tail -15`
Expected: all 3 retain tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/rightclaw/src/memory/hindsight.rs
git commit -m "feat(memory): implement HindsightClient::retain"
```

---

### Task 4: HindsightClient — recall method

**Files:**
- Modify: `crates/rightclaw/src/memory/hindsight.rs`

- [ ] **Step 1: Write failing tests for recall**

Add to the `mod tests` block:

```rust
#[tokio::test]
async fn recall_sends_correct_request() {
    let (handle, url) = mock_hindsight_server(
        r#"{"results": [{"text": "user likes dark mode", "score": 0.95, "type": "world"}]}"#,
        200,
    ).await;

    let client = HindsightClient::new("hs_k", "test-bank", "high", 8192, Some(&url));
    let results = client.recall("user preferences").await.unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].text, "user likes dark mode");
    assert_eq!(results[0].score, Some(0.95));

    let (first_line, auth, body) = handle.await.unwrap();
    assert!(first_line.contains("POST"));
    assert!(first_line.contains("/v1/default/banks/test-bank/memories/recall"));
    assert!(auth.contains("Bearer hs_k"));

    let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(parsed["query"], "user preferences");
    assert_eq!(parsed["budget"], "high");
    assert_eq!(parsed["max_tokens"], 8192);
}

#[tokio::test]
async fn recall_empty_results() {
    let (_handle, url) = mock_hindsight_server(
        r#"{"results": []}"#,
        200,
    ).await;

    let client = HindsightClient::new("hs_k", "b", "mid", 4096, Some(&url));
    let results = client.recall("nonexistent topic").await.unwrap();
    assert!(results.is_empty());
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p rightclaw --lib memory::hindsight::tests::recall -- 2>&1 | head -10`
Expected: compilation error — `recall` method doesn't exist.

- [ ] **Step 3: Implement recall method**

Add to the `impl HindsightClient` block:

```rust
/// Search Hindsight memory.
///
/// Returns ranked results using semantic, keyword, graph, and temporal search.
pub async fn recall(&self, query: &str) -> Result<Vec<RecallResult>, MemoryError> {
    let url = format!(
        "{}/v1/default/banks/{}/memories/recall",
        self.base_url, self.bank_id
    );
    let body = RecallRequest {
        query: query.to_owned(),
        budget: self.budget.clone(),
        max_tokens: self.max_tokens,
    };

    let resp = self
        .http
        .post(&url)
        .header("Authorization", format!("Bearer {}", self.api_key))
        .json(&body)
        .timeout(RECALL_TIMEOUT)
        .send()
        .await
        .map_err(|e| MemoryError::HindsightRequest(format!("{e:#}")))?;

    let status = resp.status().as_u16();
    if !resp.status().is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(MemoryError::Hindsight { status, body });
    }

    let response: RecallResponse = resp
        .json()
        .await
        .map_err(|e| MemoryError::HindsightRequest(format!("parse recall response: {e:#}")))?;
    Ok(response.results)
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p rightclaw --lib memory::hindsight::tests::recall -- 2>&1 | tail -10`
Expected: both recall tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/rightclaw/src/memory/hindsight.rs
git commit -m "feat(memory): implement HindsightClient::recall"
```

---

### Task 5: HindsightClient — reflect and get_or_create_bank methods

**Files:**
- Modify: `crates/rightclaw/src/memory/hindsight.rs`

- [ ] **Step 1: Write failing tests for reflect and get_or_create_bank**

Add to the `mod tests` block:

```rust
#[tokio::test]
async fn reflect_sends_correct_request() {
    let (handle, url) = mock_hindsight_server(
        r#"{"text": "Based on memories, the user prefers minimal UI with dark themes."}"#,
        200,
    ).await;

    let client = HindsightClient::new("hs_k", "test-bank", "mid", 4096, Some(&url));
    let result = client.reflect("What are the user's UI preferences?").await.unwrap();
    assert!(result.text.contains("dark themes"));

    let (first_line, _, body) = handle.await.unwrap();
    assert!(first_line.contains("POST"));
    assert!(first_line.contains("/v1/default/banks/test-bank/reflect"));

    let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(parsed["query"], "What are the user's UI preferences?");
    assert_eq!(parsed["budget"], "mid");
}

#[tokio::test]
async fn get_or_create_bank_success() {
    let (_handle, url) = mock_hindsight_server(
        r#"{"bank_id": "my-agent", "name": "My Agent"}"#,
        200,
    ).await;

    let client = HindsightClient::new("hs_k", "my-agent", "mid", 4096, Some(&url));
    let profile = client.get_or_create_bank().await.unwrap();
    assert_eq!(profile.bank_id, "my-agent");
}

#[tokio::test]
async fn get_or_create_bank_401_error() {
    let (_handle, url) = mock_hindsight_server(
        r#"{"error": "invalid api key"}"#,
        401,
    ).await;

    let client = HindsightClient::new("bad_key", "b", "mid", 4096, Some(&url));
    let result = client.get_or_create_bank().await;
    assert!(result.is_err());
    match result.unwrap_err() {
        MemoryError::Hindsight { status, .. } => assert_eq!(status, 401),
        other => panic!("expected Hindsight error, got: {other:?}"),
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p rightclaw --lib memory::hindsight::tests::reflect -- 2>&1 | head -10`
Expected: compilation error — `reflect` and `get_or_create_bank` methods don't exist.

- [ ] **Step 3: Implement reflect and get_or_create_bank**

Add to the `impl HindsightClient` block:

```rust
/// Synthesize a reasoned answer from long-term memories.
///
/// Unlike recall, this uses an LLM to reason across all stored memories.
pub async fn reflect(&self, query: &str) -> Result<ReflectResponse, MemoryError> {
    let url = format!(
        "{}/v1/default/banks/{}/reflect",
        self.base_url, self.bank_id
    );
    let body = ReflectRequest {
        query: query.to_owned(),
        budget: self.budget.clone(),
        max_tokens: self.max_tokens,
    };

    let resp = self
        .http
        .post(&url)
        .header("Authorization", format!("Bearer {}", self.api_key))
        .json(&body)
        .timeout(REFLECT_TIMEOUT)
        .send()
        .await
        .map_err(|e| MemoryError::HindsightRequest(format!("{e:#}")))?;

    let status = resp.status().as_u16();
    if !resp.status().is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(MemoryError::Hindsight { status, body });
    }

    resp.json::<ReflectResponse>()
        .await
        .map_err(|e| MemoryError::HindsightRequest(format!("parse reflect response: {e:#}")))
}

/// Get bank profile (auto-creates the bank if it doesn't exist).
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
        .map_err(|e| MemoryError::HindsightRequest(format!("{e:#}")))?;

    let status = resp.status().as_u16();
    if !resp.status().is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(MemoryError::Hindsight { status, body });
    }

    resp.json::<BankProfile>()
        .await
        .map_err(|e| MemoryError::HindsightRequest(format!("parse bank profile: {e:#}")))
}
```

- [ ] **Step 4: Run all hindsight tests to verify they pass**

Run: `cargo test -p rightclaw --lib memory::hindsight -- 2>&1 | tail -15`
Expected: all hindsight tests pass (retain, recall, reflect, bank).

- [ ] **Step 5: Commit**

```bash
git add crates/rightclaw/src/memory/hindsight.rs
git commit -m "feat(memory): implement HindsightClient::reflect and get_or_create_bank"
```

---

### Task 6: Prefetch cache

**Files:**
- Create: `crates/rightclaw/src/memory/prefetch.rs`
- Modify: `crates/rightclaw/src/memory/mod.rs`

- [ ] **Step 1: Create prefetch.rs with tests first**

Create `crates/rightclaw/src/memory/prefetch.rs`:

```rust
//! In-memory prefetch cache for Hindsight auto-recall results.
//!
//! Keyed by arbitrary string (worker uses `"{chat_id}:{thread_id}"`,
//! cron uses job_name). No TTL — entries are overwritten after each turn.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::RwLock;

/// Cached recall result.
#[derive(Debug, Clone)]
pub struct PrefetchEntry {
    /// Formatted recall results ready for prompt injection.
    pub content: String,
}

/// Thread-safe prefetch cache.
#[derive(Clone)]
pub struct PrefetchCache {
    inner: Arc<RwLock<HashMap<String, PrefetchEntry>>>,
}

impl PrefetchCache {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Store a prefetch result.
    pub async fn put(&self, key: &str, content: String) {
        self.inner
            .write()
            .await
            .insert(key.to_owned(), PrefetchEntry { content });
    }

    /// Get a cached prefetch result, if any.
    pub async fn get(&self, key: &str) -> Option<String> {
        self.inner
            .read()
            .await
            .get(key)
            .map(|e| e.content.clone())
    }

    /// Invalidate all entries (e.g. after cron retain).
    pub async fn clear(&self) {
        self.inner.write().await.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn put_and_get() {
        let cache = PrefetchCache::new();
        cache.put("42:0", "recalled memory".into()).await;
        assert_eq!(cache.get("42:0").await.as_deref(), Some("recalled memory"));
    }

    #[tokio::test]
    async fn get_missing_returns_none() {
        let cache = PrefetchCache::new();
        assert!(cache.get("nonexistent").await.is_none());
    }

    #[tokio::test]
    async fn clear_invalidates_all() {
        let cache = PrefetchCache::new();
        cache.put("a", "1".into()).await;
        cache.put("b", "2".into()).await;
        cache.clear().await;
        assert!(cache.get("a").await.is_none());
        assert!(cache.get("b").await.is_none());
    }

    #[tokio::test]
    async fn overwrite_entry() {
        let cache = PrefetchCache::new();
        cache.put("k", "old".into()).await;
        cache.put("k", "new".into()).await;
        assert_eq!(cache.get("k").await.as_deref(), Some("new"));
    }

    #[tokio::test]
    async fn concurrent_access() {
        let cache = PrefetchCache::new();
        let c1 = cache.clone();
        let c2 = cache.clone();

        let w = tokio::spawn(async move { c1.put("k", "val".into()).await });
        let r = tokio::spawn(async move {
            // May or may not see the write — just shouldn't deadlock.
            let _ = c2.get("k").await;
        });

        w.await.unwrap();
        r.await.unwrap();
        // After both complete, value is present.
        assert_eq!(cache.get("k").await.as_deref(), Some("val"));
    }
}
```

- [ ] **Step 2: Export from mod.rs**

Add to `crates/rightclaw/src/memory/mod.rs`:

```rust
pub mod prefetch;
```

- [ ] **Step 3: Run tests to verify they pass**

Run: `cargo test -p rightclaw --lib memory::prefetch -- 2>&1 | tail -15`
Expected: all 5 tests pass.

- [ ] **Step 4: Commit**

```bash
git add crates/rightclaw/src/memory/prefetch.rs crates/rightclaw/src/memory/mod.rs
git commit -m "feat(memory): add PrefetchCache for auto-recall results"
```

---

### Task 7: Prompt assembly — memory injection

**Files:**
- Modify: `crates/bot/src/telegram/prompt.rs`

- [ ] **Step 1: Write failing tests for memory section in prompt**

Add to the existing `mod tests` in `crates/bot/src/telegram/prompt.rs`:

```rust
#[test]
fn script_includes_memory_section_for_file_mode() {
    let script = build_prompt_assembly_script(
        "Base",
        false,
        "/sandbox",
        "/tmp/rightclaw-system-prompt.md",
        "/sandbox",
        &["claude".into()],
        None,
        Some(&MemoryMode::File),
    );
    assert!(script.contains("MEMORY.md"), "must reference MEMORY.md for file mode");
    assert!(script.contains("head -200"), "must truncate to 200 lines");
    assert!(script.contains("if [ -s"), "must check file exists and is non-empty");
}

#[test]
fn script_includes_composite_memory_for_hindsight_mode() {
    let hs_mode = MemoryMode::Hindsight {
        composite_memory_path: "/sandbox/.claude/composite-memory.md".to_owned(),
    };
    let script = build_prompt_assembly_script(
        "Base",
        false,
        "/sandbox",
        "/tmp/rightclaw-system-prompt.md",
        "/sandbox",
        &["claude".into()],
        None,
        Some(&hs_mode),
    );
    assert!(script.contains("composite-memory.md"), "must reference composite-memory for hindsight");
    assert!(script.contains("if [ -s"), "must check file exists");
}

#[test]
fn script_no_memory_section_when_none() {
    let script = build_prompt_assembly_script(
        "Base",
        false,
        "/sandbox",
        "/tmp/rightclaw-system-prompt.md",
        "/sandbox",
        &["claude".into()],
        None,
        None::<&MemoryMode>,
    );
    assert!(!script.contains("MEMORY.md"), "must not reference MEMORY.md when no memory mode");
    assert!(!script.contains("composite-memory"), "must not reference composite-memory when no memory mode");
}

#[test]
fn script_memory_section_is_last() {
    let script = build_prompt_assembly_script(
        "Base",
        false,
        "/sandbox",
        "/tmp/rightclaw-system-prompt.md",
        "/sandbox",
        &["claude".into()],
        Some("# MCP Instructions\n\n## composio\n"),
        Some(&MemoryMode::File),
    );
    let mcp_pos = script.rfind("MCP").unwrap();
    let memory_pos = script.rfind("MEMORY.md").unwrap();
    assert!(memory_pos > mcp_pos, "memory section must come after MCP instructions");
}

#[test]
fn script_bootstrap_no_memory() {
    let script = build_prompt_assembly_script(
        "Base",
        true,
        "/sandbox",
        "/tmp/rightclaw-system-prompt.md",
        "/sandbox",
        &["claude".into()],
        None,
        Some(&MemoryMode::File),
    );
    assert!(!script.contains("MEMORY.md"), "bootstrap mode must not include memory");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p rightclaw-bot --lib telegram::prompt::tests -- 2>&1 | head -20`
Expected: compilation error — `MemoryMode` doesn't exist, `build_prompt_assembly_script` has wrong signature.

- [ ] **Step 3: Add MemoryMode enum and update build_prompt_assembly_script**

At the top of `crates/bot/src/telegram/prompt.rs`, add:

```rust
/// Memory injection mode for prompt assembly.
pub(crate) enum MemoryMode {
    /// Inject MEMORY.md from agent directory.
    File,
    /// Inject composite memory file written by bot (Hindsight recall results).
    Hindsight { composite_memory_path: String },
}
```

Update `build_prompt_assembly_script` signature to add `memory_mode: Option<&MemoryMode>` as the last parameter:

```rust
pub(crate) fn build_prompt_assembly_script(
    base_prompt: &str,
    bootstrap_mode: bool,
    root_path: &str,
    prompt_file: &str,
    workdir: &str,
    claude_args: &[String],
    mcp_instructions: Option<&str>,
    memory_mode: Option<&MemoryMode>,
) -> String {
```

At the end of the function, after the `mcp_section` but before the final `format!`, add:

```rust
let memory_section = if bootstrap_mode {
    String::new()
} else {
    match memory_mode {
        Some(&MemoryMode::File) => format!(
            r#"
if [ -s {root_path}/MEMORY.md ]; then
  printf '\n## Long-Term Memory\n\n'
  head -200 {root_path}/MEMORY.md
fi"#
        ),
        Some(MemoryMode::Hindsight { composite_memory_path }) => format!(
            r#"
if [ -s {composite_memory_path} ]; then
  cat {composite_memory_path}
fi"#
        ),
        None => String::new(),
    }
};
```

Update the final `format!` to include `{memory_section}` after `{mcp_section}`:

```rust
format!(
    "{{ printf '{escaped_base}'\n{file_sections}\n{mcp_section}\n{memory_section}\n}} > {prompt_file}\ncd {workdir} && {claude_cmd} --system-prompt-file {prompt_file}"
)
```

- [ ] **Step 4: Fix all existing callers**

Update all callers of `build_prompt_assembly_script` to pass `None` for the new `memory_mode` parameter — this preserves existing behavior while we add memory support in later tasks:

In `crates/bot/src/telegram/worker.rs` (two call sites — sandbox and no-sandbox): add `None,` as the last argument.

In `crates/bot/src/cron.rs` (two call sites): add `None,` as the last argument.

In `crates/bot/src/cron_delivery.rs` (two call sites): add `None,` as the last argument.

Also update existing tests in `crates/bot/src/telegram/prompt.rs` — the `test_script` helper needs the new parameter:

```rust
fn test_script(base: &str, bootstrap: bool, args: &[String], mcp: Option<&str>) -> String {
    build_prompt_assembly_script(base, bootstrap, "/sandbox", "/tmp/rightclaw-system-prompt.md", "/sandbox", args, mcp, None::<&MemoryMode>)
}
```

- [ ] **Step 5: Run all tests to verify they pass**

Run: `cargo test -p rightclaw-bot -- 2>&1 | tail -20`
Expected: all tests pass, including existing prompt tests and new memory tests.

- [ ] **Step 6: Commit**

```bash
git add crates/bot/src/telegram/prompt.rs crates/bot/src/telegram/worker.rs crates/bot/src/cron.rs crates/bot/src/cron_delivery.rs
git commit -m "feat(memory): add MemoryMode to prompt assembly with file/hindsight injection"
```

---

### Task 8: Skills — rightmemory-file and rightmemory-hindsight

**Files:**
- Create: `skills/rightmemory-file/SKILL.md`
- Create: `skills/rightmemory-hindsight/SKILL.md`
- Modify: `crates/rightclaw/src/codegen/skills.rs`

- [ ] **Step 1: Create rightmemory-file skill**

Create `skills/rightmemory-file/SKILL.md`:

```markdown
---
name: rightmemory
description: Manage your long-term memory via MEMORY.md
---

# Memory Management

Your long-term memory is stored in `MEMORY.md` in your home directory.
This file is automatically injected into your system prompt at the start
of each interaction (truncated to 200 lines).

## How to use

Use Claude Code's built-in `Edit` and `Write` tools to manage MEMORY.md:

- **Add an entry:** Append a new line or section to MEMORY.md
- **Update an entry:** Edit an existing line to correct or refine it
- **Remove stale entries:** Delete lines that are no longer relevant

## What to save

- User preferences ("prefers dark mode", "uses vim keybindings")
- Correct API formats after fixing validation errors
- Project decisions that affect future work
- Lessons learned / mistakes to avoid
- Important facts about the user's environment or workflow

## What NOT to save

- Regular conversation content (it's already in session context)
- Information that's in code, configs, or documentation
- Temporary debugging notes or one-off commands

## Keep it concise

MEMORY.md is truncated to **200 lines** in your prompt. If it grows
too large, periodically review and:
- Remove entries that are no longer relevant
- Consolidate related entries into single lines
- Remove duplicates
```

- [ ] **Step 2: Create rightmemory-hindsight skill**

Create `skills/rightmemory-hindsight/SKILL.md`:

```markdown
---
name: rightmemory
description: Manage your long-term memory powered by Hindsight
---

# Memory Management

Your memory is powered by Hindsight. It works in two ways:

**Automatic:** Your conversations are retained and relevant context
is recalled before each interaction. You don't need to do anything
for this to work.

**Explicit tools** — use when automatic isn't enough:

- `mcp__right__memory_retain(content, context)` — save a fact permanently
  - Use for: user preferences, correct API formats, decisions,
    lessons learned, project conventions
  - `context` is a short label: "user preference", "api format",
    "project decision", "mistake to avoid"
  - Example: after fixing a wrong API call, retain the correct format

- `mcp__right__memory_recall(query)` — search your memory
  - Use before: answering questions about past work, making decisions
    that might have prior context
  - Returns ranked results from semantic + keyword + graph search

- `mcp__right__memory_reflect(query)` — deep analysis across memories
  - Use for: synthesizing patterns, comparing past decisions,
    understanding evolution of a project
  - More expensive than recall — use when you need reasoning, not lookup

## When to use explicit retain

- You discovered a user preference ("prefers tabs over spaces")
- You fixed a tool call after a validation error (save correct format)
- A decision was made that affects future work
- You learned something non-obvious about the codebase or APIs

## When NOT to retain explicitly

- Regular conversation — auto-retain handles this
- Information already in files (code, configs, docs)
- Temporary/ephemeral context (debugging steps, one-off commands)
```

- [ ] **Step 3: Update codegen/skills.rs to install rightmemory**

In `crates/rightclaw/src/codegen/skills.rs`, add:

```rust
const SKILL_RIGHTMEMORY_FILE: Dir = include_dir!("$CARGO_MANIFEST_DIR/../../skills/rightmemory-file");
const SKILL_RIGHTMEMORY_HINDSIGHT: Dir = include_dir!("$CARGO_MANIFEST_DIR/../../skills/rightmemory-hindsight");
```

Update `install_builtin_skills` to accept `memory_provider`:

```rust
pub fn install_builtin_skills(agent_path: &Path, memory_provider: &str) -> miette::Result<()> {
```

Add rightmemory to the skills list, selecting the variant based on `memory_provider`:

```rust
let rightmemory_dir: &Dir = if memory_provider == "hindsight" {
    &SKILL_RIGHTMEMORY_HINDSIGHT
} else {
    &SKILL_RIGHTMEMORY_FILE
};

let skills: &[(&str, &Dir)] = &[
    ("rightskills", &SKILL_RIGHTSKILLS),
    ("rightcron", &SKILL_RIGHTCRON),
    ("rightmcp", &SKILL_RIGHTMCP),
    ("rightmemory", rightmemory_dir),
];
```

- [ ] **Step 4: Fix all callers of install_builtin_skills**

Search for all call sites of `install_builtin_skills` and add `"file"` as the default provider parameter. Key locations:

- `crates/rightclaw/src/init.rs` — `install_builtin_skills(agent_path, "file")?;`
- `crates/rightclaw/src/codegen/` — any other callers

Run: `cargo build --workspace 2>&1 | grep 'install_builtin_skills'` to find all call sites.

- [ ] **Step 5: Write test for skill variant selection**

Add to `crates/rightclaw/src/codegen/skills.rs` tests:

```rust
#[test]
fn installs_rightmemory_file_variant() {
    let dir = tempdir().unwrap();
    install_builtin_skills(dir.path(), "file").unwrap();
    let content = std::fs::read_to_string(
        dir.path().join(".claude/skills/rightmemory/SKILL.md")
    ).unwrap();
    assert!(content.contains("MEMORY.md"), "file variant must reference MEMORY.md");
    assert!(!content.contains("memory_retain"), "file variant must NOT reference MCP tools");
}

#[test]
fn installs_rightmemory_hindsight_variant() {
    let dir = tempdir().unwrap();
    install_builtin_skills(dir.path(), "hindsight").unwrap();
    let content = std::fs::read_to_string(
        dir.path().join(".claude/skills/rightmemory/SKILL.md")
    ).unwrap();
    assert!(content.contains("memory_retain"), "hindsight variant must reference MCP tools");
    assert!(!content.contains("Edit and Write tools to manage MEMORY.md"), "hindsight variant must NOT reference Edit/Write for MEMORY.md");
}
```

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo test -p rightclaw --lib codegen::skills -- 2>&1 | tail -20`
Expected: all skills tests pass.

- [ ] **Step 7: Commit**

```bash
git add skills/rightmemory-file/ skills/rightmemory-hindsight/ crates/rightclaw/src/codegen/skills.rs crates/rightclaw/src/init.rs
git commit -m "feat(memory): add rightmemory skill with file and hindsight variants"
```

---

### Task 9: Update OPERATING_INSTRUCTIONS.md

**Files:**
- Modify: `templates/right/prompt/OPERATING_INSTRUCTIONS.md`

- [ ] **Step 1: Replace Memory section**

In `templates/right/prompt/OPERATING_INSTRUCTIONS.md`, replace the entire `## Memory` section (lines ~17-31 starting with `## Memory` and ending before `## MCP Management`) with:

```markdown
## Memory

Your memory skill (`/rightmemory`) defines how memory works in your setup.
Consult it to understand your memory capabilities.

Key behaviors regardless of memory mode:
- When you learn something important (user preferences, API formats,
  mistakes to avoid), save it to memory immediately
- When answering questions about prior work or context, check memory first
- When you fix an error after trial-and-error, save the correct approach
```

This removes all references to `store_record`, `query_records`, `search_records`, `delete_record`.

- [ ] **Step 2: Verify it compiles**

Run: `cargo build -p rightclaw 2>&1 | tail -5`
Expected: compiles (OPERATING_INSTRUCTIONS is embedded via `include_str!`).

- [ ] **Step 3: Commit**

```bash
git add templates/right/prompt/OPERATING_INSTRUCTIONS.md
git commit -m "docs: replace Memory section in OPERATING_INSTRUCTIONS with skill pointer"
```

---

### Task 10: Remove old memory tools from RightBackend

**Files:**
- Modify: `crates/rightclaw-cli/src/right_backend.rs`
- Modify: `crates/rightclaw-cli/src/memory_server.rs`

- [ ] **Step 1: Remove old memory tool definitions from tools_list()**

In `crates/rightclaw-cli/src/right_backend.rs`, remove the 4 memory tool entries from the `tools_list()` method:

```rust
// Remove these 4 Tool::new entries:
// - "store_record"
// - "query_records"
// - "search_records"
// - "delete_record"
```

- [ ] **Step 2: Remove old memory tool dispatch from tools_call()**

In the `tools_call()` match arms, remove:

```rust
// Remove these 4 arms:
// "store_record" => self.call_store_record(agent_name, &args),
// "query_records" => self.call_query_records(agent_name, &args),
// "search_records" => self.call_search_records(agent_name, &args),
// "delete_record" => self.call_delete_record(agent_name, &args),
```

- [ ] **Step 3: Remove old memory tool handler methods**

Remove the entire implementations of:
- `call_store_record()`
- `call_query_records()`
- `call_search_records()`
- `call_delete_record()`

- [ ] **Step 4: Remove unused imports**

Remove the imports from `memory_server.rs` that are no longer used by `right_backend.rs`:
- `StoreRecordParams`
- `QueryRecordsParams`
- `SearchRecordsParams`
- `DeleteRecordParams`
- `entry_to_json`

Check which imports in the `use crate::memory_server::{...}` block are still needed (cron params, `cron_run_to_json`, `McpListParams` will still be used).

- [ ] **Step 5: Update with_instructions() in memory_server.rs**

In `crates/rightclaw-cli/src/memory_server.rs`, update the `with_instructions()` string to remove the `## Memory` section with the 4 old tools. Keep the `## Cron`, `## MCP Management`, and `## Bootstrap` sections.

- [ ] **Step 6: Verify it compiles**

Run: `cargo build --workspace 2>&1 | tail -10`
Expected: compiles. There may be warnings about unused functions in `memory/store.rs` — that's expected and will be cleaned up in the next task.

- [ ] **Step 7: Commit**

```bash
git add crates/rightclaw-cli/src/right_backend.rs crates/rightclaw-cli/src/memory_server.rs
git commit -m "refactor: remove old store_record/query_records/search_records/delete_record MCP tools"
```

---

### Task 11: Remove old memory store functions

**Files:**
- Modify: `crates/rightclaw/src/memory/store.rs`
- Modify: `crates/rightclaw/src/memory/mod.rs`

- [ ] **Step 1: Remove old functions from store.rs**

Remove these functions from `crates/rightclaw/src/memory/store.rs`:
- `store_memory()`
- `recall_memories()`
- `search_memories()`
- `search_memories_paged()` (if it exists)
- `forget_memory()`
- `hard_delete_memory()` (if it exists)
- `list_memories()` (if it exists)
- `MemoryEntry` struct

Keep any auth-token related functions (`save_auth_token`, `get_auth_token`, `delete_auth_token`) — these are unrelated to agent memory.

- [ ] **Step 2: Update mod.rs exports**

In `crates/rightclaw/src/memory/mod.rs`, remove the re-exports for the deleted functions:

```rust
// Remove these lines:
pub use store::{
    forget_memory, hard_delete_memory, list_memories, recall_memories, search_memories,
    search_memories_paged, store_memory, MemoryEntry,
};
```

Keep `pub mod store;` (still has auth_token functions).

- [ ] **Step 3: Fix any remaining references**

Run: `cargo build --workspace 2>&1` and fix any compilation errors from removed symbols. Likely locations:
- `crates/rightclaw-cli/src/memory_server.rs` — may import the removed functions
- Test files referencing old store functions

- [ ] **Step 4: Verify it compiles**

Run: `cargo build --workspace 2>&1 | tail -10`
Expected: compiles with no errors.

- [ ] **Step 5: Commit**

```bash
git add crates/rightclaw/src/memory/store.rs crates/rightclaw/src/memory/mod.rs
git commit -m "refactor: remove old memory store functions (store/recall/search/forget)"
```

---

### Task 12: HindsightBackend in aggregator

**Files:**
- Modify: `crates/rightclaw-cli/src/aggregator.rs`

- [ ] **Step 1: Add HindsightBackend to BackendRegistry**

In `crates/rightclaw-cli/src/aggregator.rs`, add a field to `BackendRegistry`:

```rust
pub(crate) struct BackendRegistry {
    pub right: RightBackend,
    pub proxies: Arc<tokio::sync::RwLock<HashMap<String, Arc<ProxyBackend>>>>,
    pub agent_dir: PathBuf,
    /// Hindsight memory backend (present only when agent has memory.provider=hindsight).
    pub hindsight: Option<Arc<HindsightBackend>>,
}
```

- [ ] **Step 2: Create HindsightBackend struct**

Add to the aggregator file:

```rust
/// MCP backend for Hindsight memory tools.
///
/// Exposes memory_retain, memory_recall, memory_reflect to agents.
/// Injects bank_id and budget from agent config — agents never see these params.
pub(crate) struct HindsightBackend {
    client: rightclaw::memory::hindsight::HindsightClient,
}

impl HindsightBackend {
    pub fn new(client: rightclaw::memory::hindsight::HindsightClient) -> Self {
        Self { client }
    }

    pub fn tools_list() -> Vec<Tool> {
        vec![
            Tool::new(
                "memory_retain",
                "Store information to long-term memory. Hindsight automatically extracts structured facts, resolves entities, and indexes for retrieval.",
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "content": {"type": "string", "description": "The information to store."},
                        "context": {"type": "string", "description": "Short label (e.g. 'user preference', 'api format', 'mistake to avoid')."}
                    },
                    "required": ["content"]
                }),
            ),
            Tool::new(
                "memory_recall",
                "Search long-term memory. Returns memories ranked by relevance using semantic search, keyword matching, entity graph traversal, and reranking.",
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "query": {"type": "string", "description": "What to search for."}
                    },
                    "required": ["query"]
                }),
            ),
            Tool::new(
                "memory_reflect",
                "Synthesize a reasoned answer from long-term memories. Unlike recall, this reasons across all stored memories to produce a coherent response.",
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "query": {"type": "string", "description": "The question to reflect on."}
                    },
                    "required": ["query"]
                }),
            ),
        ]
    }

    pub async fn tools_call(
        &self,
        tool_name: &str,
        args: serde_json::Value,
    ) -> Result<CallToolResult, anyhow::Error> {
        match tool_name {
            "memory_retain" => {
                let content = args["content"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("missing required param: content"))?;
                let context = args["context"].as_str();
                let result = self.client.retain(content, context).await
                    .map_err(|e| anyhow::anyhow!("{e:#}"))?;
                let json = serde_json::json!({
                    "status": "accepted",
                    "operation_id": result.operation_id,
                });
                Ok(CallToolResult::success(vec![Content::text(
                    serde_json::to_string_pretty(&json)?,
                )]))
            }
            "memory_recall" => {
                let query = args["query"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("missing required param: query"))?;
                let results = self.client.recall(query).await
                    .map_err(|e| anyhow::anyhow!("{e:#}"))?;
                let json = serde_json::json!({ "results": results });
                Ok(CallToolResult::success(vec![Content::text(
                    serde_json::to_string_pretty(&json)?,
                )]))
            }
            "memory_reflect" => {
                let query = args["query"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("missing required param: query"))?;
                let result = self.client.reflect(query).await
                    .map_err(|e| anyhow::anyhow!("{e:#}"))?;
                let json = serde_json::json!({ "text": result.text });
                Ok(CallToolResult::success(vec![Content::text(
                    serde_json::to_string_pretty(&json)?,
                )]))
            }
            other => bail!("unknown hindsight tool: {other}"),
        }
    }
}
```

- [ ] **Step 3: Update tool dispatch in ToolDispatcher**

In `ToolDispatcher::dispatch()`, update the routing to handle hindsight tools before the proxy fallback:

```rust
match split_prefix(tool_name) {
    None => {
        // Unprefixed → check if it's a hindsight tool first, then RightBackend
        if let Some(ref hs) = registry.hindsight {
            if matches!(tool_name, "memory_retain" | "memory_recall" | "memory_reflect") {
                return hs.tools_call(tool_name, args).await;
            }
        }
        registry
            .right
            .tools_call(agent_name, &registry.agent_dir, tool_name, args)
            .await
    }
    // ... rest unchanged
}
```

- [ ] **Step 4: Update tools_list in ToolDispatcher**

In `ToolDispatcher::tools_list()`, add hindsight tools if backend is present:

```rust
pub(crate) fn tools_list(&self, agent_name: &str) -> Vec<Tool> {
    let Some(registry) = self.agents.get(agent_name) else {
        return Vec::new();
    };

    let mut tools = registry.right.tools_list();

    // Add hindsight memory tools if configured
    if registry.hindsight.is_some() {
        tools.extend(HindsightBackend::tools_list());
    }

    // Add rightmeta__mcp_list
    tools.push(BackendRegistry::mcp_list_tool_def());

    // ... rest unchanged (proxy tools)
```

- [ ] **Step 5: Fix all BackendRegistry construction sites**

Adding the `hindsight` field to `BackendRegistry` breaks all existing struct literals. Add `hindsight: None` to each:

In `crates/rightclaw-cli/src/main.rs` (~line 560):
```rust
let registry = aggregator::BackendRegistry {
    right,
    proxies: std::sync::Arc::new(tokio::sync::RwLock::new(proxies)),
    agent_dir: agent_dir.clone(),
    hindsight: None, // wired up in Task 18
};
```

In `crates/rightclaw-cli/src/internal_api.rs` (~line 593):
```rust
let registry = BackendRegistry {
    right,
    proxies: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
    agent_dir,
    hindsight: None,
};
```

In `crates/rightclaw-cli/src/aggregator.rs` test helper `make_test_registry()` (~line 431):
```rust
BackendRegistry {
    right,
    proxies: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
    agent_dir,
    hindsight: None,
}
```

- [ ] **Step 6: Verify it compiles**

Run: `cargo build --workspace 2>&1 | tail -10`
Expected: compiles with no errors.

- [ ] **Step 7: Commit**

```bash
git add crates/rightclaw-cli/src/aggregator.rs crates/rightclaw-cli/src/main.rs crates/rightclaw-cli/src/internal_api.rs
git commit -m "feat(memory): add HindsightBackend to MCP aggregator with 3 memory tools"
```

---

### Task 13: Update with_instructions() in aggregator

**Files:**
- Modify: `crates/rightclaw-cli/src/aggregator.rs`

- [ ] **Step 1: Update aggregator with_instructions()**

In `crates/rightclaw-cli/src/aggregator.rs`, update the `get_info()` method's `with_instructions()`:

```rust
.with_instructions(
    "RightClaw MCP Aggregator — routes tool calls to built-in RightClaw tools \
     and connected external MCP servers via prefix-based dispatch.\n\n\
     Memory tools (when Hindsight is configured):\n\
     - memory_retain: Store facts to long-term memory\n\
     - memory_recall: Search memory by relevance\n\
     - memory_reflect: Synthesize reasoned answers from memory",
)
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo build -p rightclaw-cli 2>&1 | tail -5`
Expected: compiles.

- [ ] **Step 3: Commit**

```bash
git add crates/rightclaw-cli/src/aggregator.rs
git commit -m "docs: update aggregator with_instructions with memory tool references"
```

---

### Task 14: Bot startup — HindsightClient initialization

**Files:**
- Modify: `crates/bot/src/lib.rs`

This task wires up HindsightClient and PrefetchCache creation during bot startup. The client and cache are passed to worker, cron, and delivery tasks.

- [ ] **Step 1: Add HindsightClient and PrefetchCache to bot startup**

In `crates/bot/src/lib.rs` `run_async()`, after parsing `config` (around line 104), add:

```rust
// Memory: initialize HindsightClient and prefetch cache if configured.
let memory_provider = config
    .memory
    .as_ref()
    .map(|m| &m.provider)
    .cloned()
    .unwrap_or_default();

let (hindsight_client, prefetch_cache) = match &memory_provider {
    rightclaw::agent::types::MemoryProvider::Hindsight => {
        let mem_config = config.memory.as_ref().unwrap();
        let api_key = mem_config.api_key.as_deref().ok_or_else(|| {
            miette::miette!(
                help = "Add `memory.api_key` to agent.yaml or switch to `memory.provider: file`",
                "Hindsight memory provider requires an API key"
            )
        })?;
        let bank_id = mem_config
            .bank_id
            .as_deref()
            .unwrap_or(&args.agent);
        let budget = mem_config.recall_budget.to_string();

        let client = rightclaw::memory::hindsight::HindsightClient::new(
            api_key,
            bank_id,
            &budget,
            mem_config.recall_max_tokens,
            None,
        );

        // Verify connectivity and create bank if needed.
        match client.get_or_create_bank().await {
            Ok(profile) => {
                tracing::info!(
                    agent = %args.agent,
                    bank_id = %profile.bank_id,
                    "Hindsight memory bank ready"
                );
            }
            Err(e) => {
                return Err(miette::miette!(
                    "Hindsight bank init failed: {e:#}. Check api_key in agent.yaml."
                ));
            }
        }

        let cache = rightclaw::memory::prefetch::PrefetchCache::new();
        (Some(Arc::new(client)), Some(cache))
    }
    rightclaw::agent::types::MemoryProvider::File => (None, None),
};

tracing::info!(
    agent = %args.agent,
    memory_provider = ?memory_provider,
    "memory system initialized"
);
```

- [ ] **Step 2: Pass memory provider to install_builtin_skills**

Find where `install_builtin_skills` is called during codegen (in `rightclaw::codegen::run_single_agent_codegen` or equivalent) and pass the memory provider string. This may require updating `run_single_agent_codegen` to accept a memory_provider parameter, or reading it from the agent config at that call site.

Alternatively — since codegen runs before config parse, and `install_builtin_skills` is called from bot startup too — the simplest approach may be to call `install_builtin_skills` again after config parse with the correct provider:

```rust
// Re-install skills with correct memory variant
let provider_str = match &memory_provider {
    rightclaw::agent::types::MemoryProvider::Hindsight => "hindsight",
    rightclaw::agent::types::MemoryProvider::File => "file",
};
rightclaw::codegen::skills::install_builtin_skills(&agent_dir, provider_str)?;
```

- [ ] **Step 3: Thread client and cache to worker/cron/delivery spawns**

This step requires updating the signatures of `run_telegram()`, `run_cron_task()`, and `run_delivery_loop()` to accept the optional hindsight client and prefetch cache. Since those changes are large and will be implemented in Tasks 15-17, for now just ensure the variables are created and available in scope.

- [ ] **Step 4: Verify it compiles**

Run: `cargo build --workspace 2>&1 | tail -10`
Expected: compiles with possible warnings about unused variables.

- [ ] **Step 5: Commit**

```bash
git add crates/bot/src/lib.rs
git commit -m "feat(memory): initialize HindsightClient and PrefetchCache at bot startup"
```

---

### Task 15: Worker — auto-retain and prefetch injection

**Files:**
- Modify: `crates/bot/src/telegram/worker.rs`

This is the largest integration task. The worker needs to:
1. After each turn: spawn auto-retain (fire-and-forget) and prefetch recall (cached)
2. Before each claude -p: check cache or blocking recall, write composite memory file

- [ ] **Step 1: Add hindsight client and cache to WorkerContext**

Find the `WorkerContext` struct in `worker.rs` and add:

```rust
/// Hindsight client for auto-retain/recall (None when memory.provider=file).
pub hindsight: Option<Arc<rightclaw::memory::hindsight::HindsightClient>>,
/// Prefetch cache for auto-recall results (None when memory.provider=file).
pub prefetch_cache: Option<rightclaw::memory::prefetch::PrefetchCache>,
```

- [ ] **Step 2: Prepare composite memory before claude -p invocation**

In the `invoke_cc()` function, before the prompt assembly (around line 762), add memory handling:

```rust
// Prepare composite memory for prompt injection.
let memory_mode = if ctx.hindsight.is_some() {
    // Hindsight mode: check prefetch cache or do blocking recall.
    let cache_key = format!("{}:{}", chat_id, eff_thread_id);
    let cached = if let Some(ref cache) = ctx.prefetch_cache {
        cache.get(&cache_key).await
    } else {
        None
    };

    let recall_content = if let Some(content) = cached {
        Some(content)
    } else if let Some(ref hs) = ctx.hindsight {
        // Cache miss → blocking recall.
        tracing::info!(?chat_id, "prefetch cache miss, doing blocking recall");
        match tokio::time::timeout(
            std::time::Duration::from_secs(5),
            hs.recall(input),
        ).await {
            Ok(Ok(results)) if !results.is_empty() => {
                let formatted: Vec<String> = results
                    .iter()
                    .map(|r| r.text.clone())
                    .collect();
                let content = formatted.join("\n\n");
                // Cache for next time
                if let Some(ref cache) = ctx.prefetch_cache {
                    cache.put(&cache_key, content.clone()).await;
                }
                Some(content)
            }
            Ok(Ok(_)) => None, // empty results
            Ok(Err(e)) => {
                tracing::warn!(?chat_id, "blocking recall failed: {e:#}");
                None
            }
            Err(_) => {
                tracing::warn!(?chat_id, "blocking recall timed out");
                None
            }
        }
    } else {
        None
    };

    // Determine paths based on sandbox mode.
    let (sandbox_composite_path, host_composite_path) = if ctx.ssh_config_path.is_some() {
        (
            "/sandbox/.claude/composite-memory.md".to_owned(),
            ctx.agent_dir.join(".claude").join("composite-memory.md"),
        )
    } else {
        let p = ctx.agent_dir.join(".claude").join("composite-memory.md");
        (p.to_string_lossy().into_owned(), p)
    };

    match recall_content {
        Some(content) => {
            let fenced = format!(
                "<memory-context>\n\
                 [System: recalled memory context, NOT new user input. Treat as background.]\n\n\
                 {content}\n\
                 </memory-context>"
            );
            // Write composite memory file on host.
            if let Err(e) = std::fs::write(&host_composite_path, &fenced) {
                tracing::warn!("failed to write composite-memory.md: {e:#}");
            }
            // Upload to sandbox if sandboxed.
            if let Some(ref sandbox_name) = ctx.resolved_sandbox {
                if let Err(e) = rightclaw::openshell::upload_file(
                    sandbox_name,
                    &host_composite_path,
                    "/sandbox/.claude/",
                ).await {
                    tracing::warn!("failed to upload composite-memory.md to sandbox: {e:#}");
                }
            }
            Some(super::prompt::MemoryMode::Hindsight {
                composite_memory_path: sandbox_composite_path,
            })
        }
        None => {
            // No recall content — delete stale composite file so prompt assembly skips it.
            let _ = std::fs::remove_file(&host_composite_path);
            Some(super::prompt::MemoryMode::Hindsight {
                composite_memory_path: sandbox_composite_path,
            })
        }
    }
} else {
    // File mode: inject MEMORY.md if it exists.
    Some(super::prompt::MemoryMode::File)
};
```

Then update both `build_prompt_assembly_script` calls (sandbox and no-sandbox) to pass `memory_mode.as_ref()` instead of `None`.

Note: The `MemoryMode` type needs to be updated to accept `Option<&MemoryMode>` or the calls need to be adjusted. Update the function signature as needed.

- [ ] **Step 3: Add auto-retain and prefetch after turn completes**

After the claude -p response is parsed and before returning from `invoke_cc()`, add:

```rust
// Auto-retain and prefetch (fire-and-forget).
if let Some(ref hs) = ctx.hindsight {
    let hs_retain = Arc::clone(hs);
    let retain_input = input.to_owned();
    let retain_response = /* extracted response text */ reply_text.clone();
    tokio::spawn(async move {
        let content = format!("User: {retain_input}\nAssistant: {retain_response}");
        if let Err(e) = hs_retain.retain(&content, Some("conversation")).await {
            tracing::warn!("auto-retain failed: {e:#}");
        }
    });

    // Prefetch for next turn.
    let hs_recall = Arc::clone(hs);
    let recall_query = input.to_owned();
    let cache_key = format!("{}:{}", chat_id, eff_thread_id);
    let cache = ctx.prefetch_cache.clone();
    tokio::spawn(async move {
        match hs_recall.recall(&recall_query).await {
            Ok(results) if !results.is_empty() => {
                let formatted: Vec<String> = results.iter().map(|r| r.text.clone()).collect();
                let content = formatted.join("\n\n");
                if let Some(ref c) = cache {
                    c.put(&cache_key, content).await;
                }
            }
            Ok(_) => {} // empty results — don't cache
            Err(e) => tracing::warn!("prefetch recall failed: {e:#}"),
        }
    });
}
```

- [ ] **Step 4: Verify it compiles**

Run: `cargo build --workspace 2>&1 | tail -10`
Expected: compiles (possibly with warnings).

- [ ] **Step 5: Commit**

```bash
git add crates/bot/src/telegram/worker.rs
git commit -m "feat(memory): add auto-retain and prefetch to worker"
```

---

### Task 16: Cron — auto-retain, prefetch, and cache invalidation

**Files:**
- Modify: `crates/bot/src/cron.rs`

- [ ] **Step 1: Add hindsight client and cache parameters to run_cron_job**

The `run_cron_job` function (the one that does `build_prompt_assembly_script` and spawns claude) needs optional Hindsight client and cache parameters. Add:

```rust
hindsight: Option<Arc<rightclaw::memory::hindsight::HindsightClient>>,
prefetch_cache: Option<rightclaw::memory::prefetch::PrefetchCache>,
```

- [ ] **Step 2: Add prefetch before cron claude -p**

Before the `build_prompt_assembly_script` call in `run_cron_job`, add memory handling similar to the worker:

```rust
let memory_mode = if let Some(ref hs) = hindsight {
    // Check prefetch cache for this cron job.
    let cache_key = format!("cron:{job_name}");
    let cached = if let Some(ref cache) = prefetch_cache {
        cache.get(&cache_key).await
    } else {
        None
    };

    let recall_content = if let Some(content) = cached {
        Some(content)
    } else {
        // First run or cache miss — blocking recall using cron prompt.
        tracing::info!(job = %job_name, "cron prefetch cache miss, doing blocking recall");
        match tokio::time::timeout(
            std::time::Duration::from_secs(5),
            hs.recall(&spec.prompt),
        ).await {
            Ok(Ok(results)) if !results.is_empty() => {
                let formatted: Vec<String> = results.iter().map(|r| r.text.clone()).collect();
                Some(formatted.join("\n\n"))
            }
            _ => None,
        }
    };

    // Write composite memory file (same logic as worker)
    // ... write fenced content to composite-memory.md ...

    Some(crate::telegram::prompt::MemoryMode::Hindsight {
        composite_memory_path: /* path */,
    })
} else {
    Some(crate::telegram::prompt::MemoryMode::File)
};
```

Update the `build_prompt_assembly_script` calls to pass `memory_mode.as_ref()`.

- [ ] **Step 3: Add auto-retain after cron completes**

After the cron job output is parsed (around line 445), add:

```rust
if let Some(ref hs) = hindsight {
    let hs_retain = Arc::clone(hs);
    let retain_summary = cron_output.summary.clone();
    let retain_context = format!("cron:{job_name}");
    let cache_to_clear = prefetch_cache.clone();

    tokio::spawn(async move {
        // Retain the cron result.
        if let Err(e) = hs_retain.retain(&retain_summary, Some(&retain_context)).await {
            tracing::warn!("cron auto-retain failed: {e:#}");
        }
        // Invalidate worker cache — cron may have retained new facts.
        if let Some(ref c) = cache_to_clear {
            c.clear().await;
        }
    });

    // Prefetch for next cron run.
    let hs_recall = Arc::clone(hs);
    let recall_prompt = spec.prompt.clone();
    let job_cache_key = format!("cron:{job_name}");
    let job_cache = prefetch_cache.clone();
    tokio::spawn(async move {
        match hs_recall.recall(&recall_prompt).await {
            Ok(results) if !results.is_empty() => {
                let formatted: Vec<String> = results.iter().map(|r| r.text.clone()).collect();
                if let Some(ref c) = job_cache {
                    c.put(&job_cache_key, formatted.join("\n\n")).await;
                }
            }
            _ => {}
        }
    });
}
```

- [ ] **Step 4: Verify it compiles**

Run: `cargo build --workspace 2>&1 | tail -10`
Expected: compiles.

- [ ] **Step 5: Commit**

```bash
git add crates/bot/src/cron.rs
git commit -m "feat(memory): add auto-retain, prefetch, and cache invalidation to cron"
```

---

### Task 17: Cron delivery — blocking recall

**Files:**
- Modify: `crates/bot/src/cron_delivery.rs`

- [ ] **Step 1: Add hindsight client parameter to delivery**

Add optional `hindsight: Option<Arc<rightclaw::memory::hindsight::HindsightClient>>` to the `run_delivery_loop` function and its `deliver_result` helper.

- [ ] **Step 2: Add blocking recall before delivery claude -p**

Before the `build_prompt_assembly_script` call in the delivery function, add:

```rust
let memory_mode = if let Some(ref hs) = hindsight {
    // Blocking recall using notification content.
    let recall_content = match tokio::time::timeout(
        std::time::Duration::from_secs(5),
        hs.recall(&notify_content),
    ).await {
        Ok(Ok(results)) if !results.is_empty() => {
            let formatted: Vec<String> = results.iter().map(|r| r.text.clone()).collect();
            Some(formatted.join("\n\n"))
        }
        _ => None,
    };

    // Write composite memory and return mode (same pattern as worker/cron)
    // ...

    Some(crate::telegram::prompt::MemoryMode::Hindsight {
        composite_memory_path: /* path */,
    })
} else {
    Some(crate::telegram::prompt::MemoryMode::File)
};
```

Update `build_prompt_assembly_script` calls to pass `memory_mode.as_ref()`.

- [ ] **Step 3: Thread hindsight client from lib.rs**

In `crates/bot/src/lib.rs`, pass the `hindsight_client` to `run_delivery_loop`:

```rust
let delivery_hindsight = hindsight_client.clone();
```

And add it to the `run_delivery_loop` call.

- [ ] **Step 4: Verify it compiles**

Run: `cargo build --workspace 2>&1 | tail -10`
Expected: compiles.

- [ ] **Step 5: Commit**

```bash
git add crates/bot/src/cron_delivery.rs crates/bot/src/lib.rs
git commit -m "feat(memory): add blocking recall to cron delivery"
```

---

### Task 18: Wire up HindsightBackend in aggregator startup

**Files:**
- Modify: `crates/rightclaw-cli/src/aggregator.rs` (or wherever BackendRegistry is constructed)

- [ ] **Step 1: Find BackendRegistry construction**

Search for where `BackendRegistry` instances are created — likely in `main.rs` or the aggregator setup code. This is where we need to conditionally create `HindsightBackend` based on agent config.

Run: `rg "BackendRegistry" crates/rightclaw-cli/src/` to find all usages.

- [ ] **Step 2: Create HindsightBackend from agent config**

Where `BackendRegistry` is constructed for each agent, add:

```rust
let hindsight = if let Some(mem_config) = agent_config.memory.as_ref() {
    if mem_config.provider == rightclaw::agent::types::MemoryProvider::Hindsight {
        let api_key = mem_config.api_key.as_deref().unwrap_or("");
        let bank_id = mem_config.bank_id.as_deref().unwrap_or(&agent_name);
        let budget = mem_config.recall_budget.to_string();
        let client = rightclaw::memory::hindsight::HindsightClient::new(
            api_key, bank_id, &budget, mem_config.recall_max_tokens, None,
        );
        Some(Arc::new(HindsightBackend::new(client)))
    } else {
        None
    }
} else {
    None
};

BackendRegistry {
    right,
    proxies,
    agent_dir,
    hindsight,
}
```

- [ ] **Step 3: Verify it compiles**

Run: `cargo build --workspace 2>&1 | tail -10`
Expected: compiles.

- [ ] **Step 4: Commit**

```bash
git add crates/rightclaw-cli/src/aggregator.rs
git commit -m "feat(memory): wire up HindsightBackend in aggregator from agent config"
```

---

### Task 19: Agent init wizard — memory step

**Files:**
- Modify: `crates/rightclaw/src/init.rs`

- [ ] **Step 1: Add memory fields to InitOverrides**

In `crates/rightclaw/src/init.rs`, add to `InitOverrides`:

```rust
pub memory_provider: MemoryProvider,
pub memory_api_key: Option<String>,
pub memory_bank_id: Option<String>,
```

Update `default_overrides` in `init_agent()`:

```rust
memory_provider: MemoryProvider::File,
memory_api_key: None,
memory_bank_id: None,
```

- [ ] **Step 2: Write memory config to agent.yaml**

In `init_agent()`, when writing agent.yaml, include the memory section if provider is hindsight:

```rust
// After existing agent.yaml modifications, append memory config:
if ov.memory_provider != MemoryProvider::File {
    // Append memory section to agent.yaml content
    let memory_yaml = match &ov.memory_provider {
        MemoryProvider::Hindsight => {
            let mut s = String::from("\nmemory:\n  provider: hindsight\n");
            if let Some(ref key) = ov.memory_api_key {
                s.push_str(&format!("  api_key: \"{key}\"\n"));
            }
            if let Some(ref bank) = ov.memory_bank_id {
                s.push_str(&format!("  bank_id: \"{bank}\"\n"));
            }
            s
        }
        MemoryProvider::File => String::new(),
    };
    // Append to agent.yaml
}
```

- [ ] **Step 3: Verify it compiles**

Run: `cargo build -p rightclaw 2>&1 | tail -5`
Expected: compiles.

- [ ] **Step 4: Commit**

```bash
git add crates/rightclaw/src/init.rs
git commit -m "feat(memory): add memory provider step to agent init"
```

---

### Task 20: Update ARCHITECTURE.md and PROMPT_SYSTEM.md

**Files:**
- Modify: `ARCHITECTURE.md`
- Modify: `PROMPT_SYSTEM.md`

- [ ] **Step 1: Update Memory Schema section in ARCHITECTURE.md**

In the `### Memory Schema (SQLite)` section, add a note about the new Hindsight integration:

```markdown
### Memory

Two modes, configured per-agent via `memory.provider` in agent.yaml:

**File mode (default):** Agent manages `MEMORY.md` via CC Edit/Write.
Bot injects file contents into system prompt (truncated to 200 lines).
No MCP memory tools.

**Hindsight mode (optional):** Hindsight Cloud API (`api.hindsight.vectorize.io`),
one bank per agent. Bot performs auto-retain after each turn and auto-recall
(prefetch) before each claude -p. Three MCP tools exposed via aggregator:
`memory_retain`, `memory_recall`, `memory_reflect`. Prefetch cache is in-memory
(lost on restart → blocking recall on first interaction).

Old tools (`store_record`, `query_records`, `search_records`, `delete_record`)
are removed. Old SQLite tables (`memories`, `memories_fts`, `memory_events`)
are retained in schema but unused.
```

- [ ] **Step 2: Update PROMPT_SYSTEM.md**

Update tool references to replace old memory tools with the new Hindsight tools (when configured). Update the prompt assembly order to show memory at the end.

- [ ] **Step 3: Commit**

```bash
git add ARCHITECTURE.md PROMPT_SYSTEM.md
git commit -m "docs: update ARCHITECTURE.md and PROMPT_SYSTEM.md for Hindsight memory"
```

---

### Task 21: Full build and test verification

**Files:** None (verification only)

- [ ] **Step 1: Full workspace build**

Run: `cargo build --workspace 2>&1 | tail -10`
Expected: compiles with no errors.

- [ ] **Step 2: Run all tests**

Run: `cargo test --workspace 2>&1 | tail -30`
Expected: all tests pass.

- [ ] **Step 3: Run clippy**

Run: `cargo clippy --workspace 2>&1 | tail -20`
Expected: no errors (warnings OK but should be reviewed).

- [ ] **Step 4: Verify old tools are fully removed**

Run: `rg "store_record|query_records|search_records|delete_record" crates/ --type rust -l`
Expected: no matches in production code (test files referencing old behavior are OK to clean up).

- [ ] **Step 5: Verify new memory tools are wired**

Run: `rg "memory_retain|memory_recall|memory_reflect" crates/ --type rust -l`
Expected: matches in `aggregator.rs` (tool definitions + dispatch), `hindsight.rs` (client methods).

- [ ] **Step 6: Verify skill files exist**

Run: `ls skills/rightmemory-file/SKILL.md skills/rightmemory-hindsight/SKILL.md`
Expected: both files exist.
