# MCP Auth Choice Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Change `/mcp add` so auth heuristics recommend `OAuth`, `Header`, or `URL as-is`, while the user's button choice is authoritative.

**Architecture:** Add a focused Telegram auth-choice module for pure state, rendering, parsing, and recommendation rules. Keep network calls and Telegram side effects in `handler.rs`, and keep registration through the internal `/mcp-add` API. Split MCP URL validation so public classification remains strict while explicit user registration allows loopback HTTP/HTTPS but still rejects broad private/link-local ranges.

**Tech Stack:** Rust 2024, teloxide inline keyboards/callback queries, tokio mutex state slots, reqwest URL parsing, right-mcp credentials/internal API, cargo tests through `devenv shell --`.

---

## Scope And File Structure

This is one coherent change to the MCP add/auth flow. Do not reorganize unrelated Telegram handlers.

Files:

- Create: `crates/bot/src/telegram/mcp_auth_choice.rs`
  - Owns pure auth-choice enums, pending-choice state, recommendation rules, callback-data parsing, keyboard rendering, and token-input parsing.
- Modify: `crates/bot/src/telegram/mod.rs`
  - Expose the new `mcp_auth_choice` module inside `telegram`.
- Modify: `crates/bot/src/telegram/dispatch.rs`
  - Construct and inject the pending choice slot.
  - Add callback routing for `mcpauth:`.
  - Update dispatcher smoke test dependencies.
- Modify: `crates/bot/src/telegram/handler.rs`
  - Add choice-slot dependency to `/mcp add`.
  - Replace immediate registration in `handle_mcp_add` with "park pending choice + send keyboard".
  - Add `handle_mcp_auth_choice_callback`.
  - Reuse the existing token request flow after the `Header` button.
- Modify: `crates/right-mcp/src/credentials.rs`
  - Split public classification from user-managed URL validation.
  - Allow loopback HTTP/HTTPS for explicit registration.
  - Keep public `is_public_url()` false for loopback/private URLs.
- Modify: `crates/right/src/internal_api.rs`
  - Update tests around URL validation; production handler can continue calling `validate_server_url`.
- Modify: `ARCHITECTURE.md`
  - Update MCP auth type detection wording.
- Modify: `docs/architecture/mcp.md`
  - Document auth-choice flow and validation boundary.
- Modify: `docs/superpowers/specs/2026-05-18-mcp-auth-choice-design.md`
  - Already tightened for loopback-only local support; include in the final docs commit if not already committed.

Verification cadence:

- Start with targeted baseline tests for the touched packages.
- For each behavior slice, write narrow failing tests, verify red, implement, verify targeted green.
- Run `devenv shell -- cargo test -p right-mcp credentials`.
- Run `devenv shell -- cargo test -p right-bot mcp_auth_choice`.
- Run `devenv shell -- cargo test -p right-bot dispatcher_builds_without_panic`.
- Run `devenv shell -- cargo test -p right mcp_add_validates_url_private_ip`.
- Run `devenv shell -- cargo test -p right mcp_add_allows_loopback_oauth_registration`.
- Final mandatory check: `devenv shell -- cargo test --workspace`.

## Task 1: Add Pure MCP Auth Choice Types

**Files:**
- Create: `crates/bot/src/telegram/mcp_auth_choice.rs`
- Modify: `crates/bot/src/telegram/mod.rs`

- [ ] **Step 1: Write failing tests for choice parsing, keyboard labels, recommendation priority, and token parsing**

Create `crates/bot/src/telegram/mcp_auth_choice.rs` with this initial test module and enough imports for the compiler to identify missing production items:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use teloxide::types::InlineKeyboardButtonKind;

    fn button_rows(kb: teloxide::types::InlineKeyboardMarkup) -> Vec<Vec<(String, String)>> {
        kb.inline_keyboard
            .into_iter()
            .map(|row| {
                row.into_iter()
                    .map(|b| {
                        let data = match b.kind {
                            InlineKeyboardButtonKind::CallbackData(d) => d,
                            other => panic!("expected callback button, got {other:?}"),
                        };
                        (b.text, data)
                    })
                    .collect()
            })
            .collect()
    }

    #[test]
    fn parse_auth_choice_callback_data_accepts_valid_choices() {
        assert_eq!(
            parse_auth_choice_callback_data("mcpauth:42:oauth"),
            Some((42, McpAuthChoice::OAuth))
        );
        assert_eq!(
            parse_auth_choice_callback_data("mcpauth:42:header"),
            Some((42, McpAuthChoice::Header))
        );
        assert_eq!(
            parse_auth_choice_callback_data("mcpauth:42:url"),
            Some((42, McpAuthChoice::UrlAsIs))
        );
    }

    #[test]
    fn parse_auth_choice_callback_data_rejects_invalid_data() {
        for bad in [
            "",
            "model:42:oauth",
            "mcpauth",
            "mcpauth:abc:oauth",
            "mcpauth:42:unknown",
            "mcpauth:42",
        ] {
            assert_eq!(parse_auth_choice_callback_data(bad), None, "bad={bad}");
        }
    }

    #[test]
    fn render_auth_choice_keyboard_marks_recommended_button() {
        let rows = button_rows(render_auth_choice_keyboard(7, McpAuthChoice::Header));
        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0],
            vec![
                ("OAuth".to_string(), "mcpauth:7:oauth".to_string()),
                ("✓ Header".to_string(), "mcpauth:7:header".to_string()),
                ("URL as-is".to_string(), "mcpauth:7:url".to_string()),
            ]
        );
    }

    #[test]
    fn recommend_auth_choice_prefers_query_string_over_oauth() {
        let rec = recommend_auth_choice(AuthChoiceSignals {
            has_query: true,
            oauth_discovered: true,
            is_loopback: false,
            is_public: true,
            detection: Some(AuthDetectionResult {
                auth_type: "header".to_string(),
                header_name: Some("X-Api-Key".to_string()),
            }),
        });

        assert_eq!(rec.choice, McpAuthChoice::UrlAsIs);
        assert_eq!(rec.header_auth_type, "bearer");
        assert_eq!(rec.header_name, None);
    }

    #[test]
    fn recommend_auth_choice_uses_oauth_when_discovered_without_query() {
        let rec = recommend_auth_choice(AuthChoiceSignals {
            has_query: false,
            oauth_discovered: true,
            is_loopback: false,
            is_public: true,
            detection: None,
        });

        assert_eq!(rec.choice, McpAuthChoice::OAuth);
    }

    #[test]
    fn recommend_auth_choice_uses_detected_custom_header() {
        let rec = recommend_auth_choice(AuthChoiceSignals {
            has_query: false,
            oauth_discovered: false,
            is_loopback: false,
            is_public: true,
            detection: Some(AuthDetectionResult {
                auth_type: "header".to_string(),
                header_name: Some("X-Api-Key".to_string()),
            }),
        });

        assert_eq!(rec.choice, McpAuthChoice::Header);
        assert_eq!(rec.header_auth_type, "header");
        assert_eq!(rec.header_name.as_deref(), Some("X-Api-Key"));
    }

    #[test]
    fn recommend_auth_choice_uses_header_for_public_detection_failure() {
        let rec = recommend_auth_choice(AuthChoiceSignals {
            has_query: false,
            oauth_discovered: false,
            is_loopback: false,
            is_public: true,
            detection: None,
        });

        assert_eq!(rec.choice, McpAuthChoice::Header);
        assert_eq!(rec.header_auth_type, "bearer");
        assert_eq!(rec.header_name, None);
    }

    #[test]
    fn recommend_auth_choice_uses_url_as_is_for_loopback() {
        let rec = recommend_auth_choice(AuthChoiceSignals {
            has_query: false,
            oauth_discovered: false,
            is_loopback: true,
            is_public: false,
            detection: None,
        });

        assert_eq!(rec.choice, McpAuthChoice::UrlAsIs);
    }

    #[test]
    fn parse_token_input_supports_custom_header_override() {
        let parsed = parse_token_input(
            "X-Api-Key: secret".to_string(),
            "bearer".to_string(),
            None,
        );

        assert_eq!(parsed.token, "secret");
        assert_eq!(parsed.auth_type, "header");
        assert_eq!(parsed.auth_header.as_deref(), Some("X-Api-Key"));
    }

    #[test]
    fn parse_token_input_keeps_raw_token_when_prefix_is_not_header() {
        let parsed = parse_token_input(
            "Bearer token with spaces".to_string(),
            "bearer".to_string(),
            None,
        );

        assert_eq!(parsed.token, "Bearer token with spaces");
        assert_eq!(parsed.auth_type, "bearer");
        assert_eq!(parsed.auth_header, None);
    }
}
```

Add the module declaration to `crates/bot/src/telegram/mod.rs`:

```rust
pub(crate) mod mcp_auth_choice;
```

- [ ] **Step 2: Run the new test target and verify it fails**

Run:

```bash
devenv shell -- cargo test -p right-bot mcp_auth_choice
```

Expected: FAIL with missing types/functions such as `McpAuthChoice`, `render_auth_choice_keyboard`, and `recommend_auth_choice`.

- [ ] **Step 3: Implement pure choice helpers**

Replace the top of `crates/bot/src/telegram/mcp_auth_choice.rs` above the test module with:

```rust
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use teloxide::types::{InlineKeyboardButton, InlineKeyboardMarkup};
use tokio::sync::Mutex;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum McpAuthChoice {
    OAuth,
    Header,
    UrlAsIs,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub(crate) struct AuthDetectionResult {
    pub auth_type: String,
    #[serde(default)]
    pub header_name: Option<String>,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct AuthChoiceSignals<'a> {
    pub has_query: bool,
    pub oauth_discovered: bool,
    pub is_loopback: bool,
    pub is_public: bool,
    pub detection: Option<&'a AuthDetectionResult>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct McpAuthRecommendation {
    pub choice: McpAuthChoice,
    pub header_auth_type: String,
    pub header_name: Option<String>,
}

#[derive(Debug)]
pub(crate) struct PendingMcpAuthChoiceRequest {
    pub id: u64,
    pub chat_id: i64,
    pub thread_id: i64,
    pub agent_name: String,
    pub server_name: String,
    pub original_url: String,
    pub bare_url: String,
    pub has_query: bool,
    pub recommendation: McpAuthRecommendation,
    pub expires_at: Instant,
}

impl PendingMcpAuthChoiceRequest {
    pub(crate) fn is_expired(&self, now: Instant) -> bool {
        now >= self.expires_at
    }
}

#[derive(Clone)]
pub(crate) struct PendingMcpAuthChoiceSlot(
    pub Arc<Mutex<Option<PendingMcpAuthChoiceRequest>>>,
);

static NEXT_MCP_AUTH_CHOICE_ID: AtomicU64 = AtomicU64::new(1);
pub(crate) const MCP_AUTH_CHOICE_TTL: Duration = Duration::from_secs(120);

pub(crate) fn next_mcp_auth_choice_id() -> u64 {
    NEXT_MCP_AUTH_CHOICE_ID.fetch_add(1, Ordering::Relaxed)
}

pub(crate) fn parse_auth_choice_callback_data(data: &str) -> Option<(u64, McpAuthChoice)> {
    let parts: Vec<&str> = data.splitn(3, ':').collect();
    if parts.len() != 3 || parts[0] != "mcpauth" {
        return None;
    }
    let id = parts[1].parse::<u64>().ok()?;
    let choice = match parts[2] {
        "oauth" => McpAuthChoice::OAuth,
        "header" => McpAuthChoice::Header,
        "url" => McpAuthChoice::UrlAsIs,
        _ => return None,
    };
    Some((id, choice))
}

fn callback_suffix(choice: McpAuthChoice) -> &'static str {
    match choice {
        McpAuthChoice::OAuth => "oauth",
        McpAuthChoice::Header => "header",
        McpAuthChoice::UrlAsIs => "url",
    }
}

fn choice_label(choice: McpAuthChoice) -> &'static str {
    match choice {
        McpAuthChoice::OAuth => "OAuth",
        McpAuthChoice::Header => "Header",
        McpAuthChoice::UrlAsIs => "URL as-is",
    }
}

pub(crate) fn render_auth_choice_keyboard(
    request_id: u64,
    recommended: McpAuthChoice,
) -> InlineKeyboardMarkup {
    let button = |choice: McpAuthChoice| {
        let label = if choice == recommended {
            format!("✓ {}", choice_label(choice))
        } else {
            choice_label(choice).to_string()
        };
        InlineKeyboardButton::callback(
            label,
            format!("mcpauth:{request_id}:{}", callback_suffix(choice)),
        )
    };

    InlineKeyboardMarkup::new(vec![vec![
        button(McpAuthChoice::OAuth),
        button(McpAuthChoice::Header),
        button(McpAuthChoice::UrlAsIs),
    ]])
}

pub(crate) fn recommend_auth_choice(signals: AuthChoiceSignals<'_>) -> McpAuthRecommendation {
    if signals.has_query {
        return McpAuthRecommendation {
            choice: McpAuthChoice::UrlAsIs,
            header_auth_type: "bearer".to_string(),
            header_name: None,
        };
    }

    if signals.oauth_discovered {
        return McpAuthRecommendation {
            choice: McpAuthChoice::OAuth,
            header_auth_type: "bearer".to_string(),
            header_name: None,
        };
    }

    if signals.is_loopback || !signals.is_public {
        return McpAuthRecommendation {
            choice: McpAuthChoice::UrlAsIs,
            header_auth_type: "bearer".to_string(),
            header_name: None,
        };
    }

    let mut rec = McpAuthRecommendation {
        choice: McpAuthChoice::Header,
        header_auth_type: "bearer".to_string(),
        header_name: None,
    };

    if let Some(detection) = signals.detection {
        if detection.auth_type == "header" {
            if let Some(header) = detection.header_name.as_ref().filter(|h| !h.trim().is_empty()) {
                rec.header_auth_type = "header".to_string();
                rec.header_name = Some(header.trim().to_string());
            }
        }
    }

    rec
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ParsedTokenInput {
    pub token: String,
    pub auth_type: String,
    pub auth_header: Option<String>,
}

pub(crate) fn parse_token_input(
    raw_input: String,
    initial_auth_type: String,
    initial_auth_header: Option<String>,
) -> ParsedTokenInput {
    if let Some((header, value)) = raw_input.split_once(": ") {
        let header = header.trim();
        let value = value.trim();
        if !header.is_empty()
            && !header.contains(' ')
            && header
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return ParsedTokenInput {
                token: value.to_string(),
                auth_type: "header".to_string(),
                auth_header: Some(header.to_string()),
            };
        }
    }

    ParsedTokenInput {
        token: raw_input,
        auth_type: initial_auth_type,
        auth_header: initial_auth_header,
    }
}
```

- [ ] **Step 4: Run the pure choice tests and verify they pass**

Run:

```bash
devenv shell -- cargo test -p right-bot mcp_auth_choice
```

Expected: PASS for all `mcp_auth_choice` tests.

- [ ] **Step 5: Commit pure choice helpers**

Run:

```bash
devenv shell -- git add crates/bot/src/telegram/mcp_auth_choice.rs crates/bot/src/telegram/mod.rs
devenv shell -- git commit -m "feat(mcp): add auth choice helpers"
```

## Task 2: Split MCP URL Validation For Public Detection And Loopback Registration

**Files:**
- Modify: `crates/right-mcp/src/credentials.rs`
- Modify: `crates/right/src/internal_api.rs`

- [ ] **Step 1: Update credentials tests for loopback registration and strict public detection**

In `crates/right-mcp/src/credentials.rs`, replace `validate_server_url_private_ips_rejected` and `is_public_url_test` with:

```rust
#[test]
fn validate_server_url_rejects_private_networks_but_allows_loopback() {
    // RFC1918
    assert!(validate_server_url("https://192.168.1.1/mcp").is_err());
    assert!(validate_server_url("https://10.0.0.1/mcp").is_err());
    assert!(validate_server_url("https://172.16.0.1/mcp").is_err());
    // Link-local
    assert!(validate_server_url("https://169.254.1.1/mcp").is_err());

    // Explicit user-managed loopback MCP servers are allowed.
    validate_server_url("http://localhost:3333/mcp").unwrap();
    validate_server_url("http://127.0.0.1:3333/mcp").unwrap();
    validate_server_url("http://[::1]:3333/mcp").unwrap();
    validate_server_url("https://localhost/mcp").unwrap();
}

#[test]
fn is_public_url_remains_false_for_loopback_and_private_urls() {
    assert!(is_public_url("https://mcp.notion.com/mcp"));
    assert!(!is_public_url("http://localhost:3333/mcp"));
    assert!(!is_public_url("https://localhost/mcp"));
    assert!(!is_public_url("https://192.168.1.1/mcp"));
    assert!(!is_public_url("http://mcp.notion.com/mcp"));
}

#[test]
fn is_loopback_url_detects_localhost_and_loopback_ips() {
    assert!(is_loopback_url("http://localhost:3333/mcp"));
    assert!(is_loopback_url("https://127.0.0.1/mcp"));
    assert!(is_loopback_url("http://[::1]:3333/mcp"));
    assert!(!is_loopback_url("https://mcp.notion.com/mcp"));
    assert!(!is_loopback_url("https://192.168.1.1/mcp"));
}
```

In `crates/right/src/internal_api.rs`, add this test after `mcp_add_validates_url_private_ip`:

```rust
#[tokio::test]
async fn mcp_add_allows_loopback_oauth_registration() {
    let tmp = tempfile::tempdir().unwrap();
    let app = make_test_router(tmp.path());

    let (status, body) = send_json(
        app,
        "/mcp-add",
        serde_json::json!({
            "agent": "test-agent",
            "name": "local",
            "url": "http://127.0.0.1:3333/mcp",
            "auth_type": "oauth"
        }),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "body={body}");
    assert_eq!(body["tools_count"], 0);
}
```

- [ ] **Step 2: Run validation tests and verify they fail**

Run:

```bash
devenv shell -- cargo test -p right-mcp credentials::tests::validate_server_url_rejects_private_networks_but_allows_loopback
devenv shell -- cargo test -p right-mcp credentials::tests::is_public_url_remains_false_for_loopback_and_private_urls
devenv shell -- cargo test -p right-mcp credentials::tests::is_loopback_url_detects_localhost_and_loopback_ips
devenv shell -- cargo test -p right mcp_add_allows_loopback_oauth_registration
```

Expected: FAIL because loopback is still rejected and `is_loopback_url` does not exist.

- [ ] **Step 3: Implement loopback-aware validation**

In `crates/right-mcp/src/credentials.rs`, replace `validate_server_url` and `is_public_url`, and add these helpers near them:

```rust
fn parsed_url(url_str: &str) -> Result<Url, CredentialError> {
    Url::parse(url_str)
        .map_err(|e| CredentialError::InvalidServerUrl(format!("invalid URL: {e}")))
}

fn host_is_loopback(host: &url::Host<&str>) -> bool {
    match host {
        url::Host::Domain(domain) => domain.eq_ignore_ascii_case("localhost"),
        url::Host::Ipv4(v4) => v4.is_loopback(),
        url::Host::Ipv6(v6) => v6.is_loopback(),
    }
}

fn host_is_blocked_private(host: &url::Host<&str>) -> bool {
    match host {
        url::Host::Domain(_) => false,
        url::Host::Ipv4(v4) => v4.is_private() || v4.is_link_local(),
        url::Host::Ipv6(v6) => v6.is_unique_local() || v6.is_unicast_link_local(),
    }
}

/// Validate an MCP server URL for explicit user-managed registration.
///
/// Public servers must use HTTPS. Loopback development servers may use HTTP or
/// HTTPS. Broad private/link-local network ranges remain blocked by default.
pub fn validate_server_url(url_str: &str) -> Result<(), CredentialError> {
    let parsed = parsed_url(url_str)?;
    let host = parsed
        .host()
        .ok_or_else(|| CredentialError::InvalidServerUrl("URL has no host".to_string()))?;

    let loopback = host_is_loopback(&host);
    if parsed.scheme() != "https" && !(loopback && parsed.scheme() == "http") {
        return Err(CredentialError::InvalidServerUrl(format!(
            "only HTTPS URLs are allowed, got '{}'",
            parsed.scheme()
        )));
    }

    if !loopback && host_is_blocked_private(&host) {
        return Err(CredentialError::InvalidServerUrl(format!(
            "private/link-local host '{host}' is not allowed"
        )));
    }

    Ok(())
}

/// Check whether a URL is a valid public HTTPS URL, excluding loopback/private
/// addresses. This is for discovery/classification probes, not registration.
pub fn is_public_url(url: &str) -> bool {
    let Ok(parsed) = parsed_url(url) else {
        return false;
    };
    if parsed.scheme() != "https" {
        return false;
    }
    let Some(host) = parsed.host() else {
        return false;
    };
    !host_is_loopback(&host) && !host_is_blocked_private(&host)
}

/// Check whether a URL targets explicit loopback.
pub fn is_loopback_url(url: &str) -> bool {
    let Ok(parsed) = parsed_url(url) else {
        return false;
    };
    parsed.host().as_ref().is_some_and(host_is_loopback)
}
```

- [ ] **Step 4: Run validation tests and verify they pass**

Run:

```bash
devenv shell -- cargo test -p right-mcp credentials
devenv shell -- cargo test -p right mcp_add_validates_url_private_ip
devenv shell -- cargo test -p right mcp_add_allows_loopback_oauth_registration
```

Expected: PASS. `mcp_add_validates_url_private_ip` must still reject `https://192.168.1.1/mcp`.

- [ ] **Step 5: Commit validation split**

Run:

```bash
devenv shell -- git add crates/right-mcp/src/credentials.rs crates/right/src/internal_api.rs
devenv shell -- git commit -m "feat(mcp): allow loopback mcp registration"
```

## Task 3: Park Auth Choice Instead Of Registering During `/mcp add`

**Files:**
- Modify: `crates/bot/src/telegram/handler.rs`
- Modify: `crates/bot/src/telegram/dispatch.rs`

- [ ] **Step 1: Add failing supersession test for pending auth-choice state**

In `crates/bot/src/telegram/mcp_auth_choice.rs`, add this test to the existing test module:

```rust
#[tokio::test]
async fn supersession_cleanup_does_not_clobber_newer_auth_choice() {
    let slot = PendingMcpAuthChoiceSlot(Arc::new(Mutex::new(None)));
    let req_a_id = next_mcp_auth_choice_id();
    {
        let mut s = slot.0.lock().await;
        *s = Some(PendingMcpAuthChoiceRequest {
            id: req_a_id,
            chat_id: 100,
            thread_id: 0,
            agent_name: "agent".to_string(),
            server_name: "a".to_string(),
            original_url: "https://a.example/mcp".to_string(),
            bare_url: "https://a.example/mcp".to_string(),
            has_query: false,
            recommendation: McpAuthRecommendation {
                choice: McpAuthChoice::OAuth,
                header_auth_type: "bearer".to_string(),
                header_name: None,
            },
            expires_at: Instant::now() + MCP_AUTH_CHOICE_TTL,
        });
    }

    let req_b_id = next_mcp_auth_choice_id();
    {
        let mut s = slot.0.lock().await;
        *s = Some(PendingMcpAuthChoiceRequest {
            id: req_b_id,
            chat_id: 200,
            thread_id: 0,
            agent_name: "agent".to_string(),
            server_name: "b".to_string(),
            original_url: "https://b.example/mcp".to_string(),
            bare_url: "https://b.example/mcp".to_string(),
            has_query: false,
            recommendation: McpAuthRecommendation {
                choice: McpAuthChoice::Header,
                header_auth_type: "bearer".to_string(),
                header_name: None,
            },
            expires_at: Instant::now() + MCP_AUTH_CHOICE_TTL,
        });
    }

    {
        let mut s = slot.0.lock().await;
        if s.as_ref().map(|p| p.id) == Some(req_a_id) {
            s.take();
        }
    }

    let s = slot.0.lock().await;
    assert_eq!(s.as_ref().map(|p| p.id), Some(req_b_id));
}
```

- [ ] **Step 2: Run the choice test and verify it passes before integration**

Run:

```bash
devenv shell -- cargo test -p right-bot mcp_auth_choice::tests::supersession_cleanup_does_not_clobber_newer_auth_choice
```

Expected: PASS. This confirms the shared slot shape is usable before wiring it through Telegram.

- [ ] **Step 3: Wire `PendingMcpAuthChoiceSlot` through dispatch dependencies**

In `crates/bot/src/telegram/dispatch.rs`, update the imports from `handler` and add an import from the new module:

```rust
use super::handler::{
    AgentDir, AgentSettings, IdleTimestamp, InterceptSlots, InternalApi, PendingTokenSlot,
    RightHome, SshConfigPath, handle_bg_callback, handle_cron, handle_doctor, handle_list,
    handle_mcp, handle_mcp_auth_choice_callback, handle_message, handle_new, handle_start,
    handle_stop_callback, handle_switch, handle_thinking_toggle_callback, handle_usage,
};
use super::mcp_auth_choice::PendingMcpAuthChoiceSlot;
```

Near the existing pending token slot creation in `run_telegram`, add:

```rust
let pending_auth_choice_slot = Arc::new(PendingMcpAuthChoiceSlot(Arc::new(tokio::sync::Mutex::new(None))));
```

Change the `build_dispatcher` signature by inserting this parameter after
`pending_token_slot_arc`:

```rust
pending_auth_choice_slot_arc: Arc<PendingMcpAuthChoiceSlot>,
```

Update the `build_dispatcher(...)` call in `run_telegram` by inserting this
argument after `Arc::clone(&pending_token_slot_arc)`:

```rust
Arc::clone(&pending_auth_choice_slot),
```

Update the final dependency list inside `build_dispatcher` by inserting this
entry after `pending_token_slot_arc`:

```rust
pending_auth_choice_slot_arc,
```

In the dispatcher smoke test, create and inject the same dependency:

```rust
let pending_auth_choice_slot =
    Arc::new(PendingMcpAuthChoiceSlot(Arc::new(Mutex::new(None))));
```

In the smoke test's `build_dispatcher(...)` call, insert this argument after
`Arc::clone(&pending_token_slot)`:

```rust
Arc::clone(&pending_auth_choice_slot),
```

- [ ] **Step 4: Add callback route for `mcpauth:`**

In `crates/bot/src/telegram/dispatch.rs`, add this branch before `.endpoint(handle_stop_callback)`:

```rust
.branch(
    dptree::filter(|q: CallbackQuery| {
        q.data.as_deref().is_some_and(|d| d.starts_with("mcpauth:"))
    })
    .endpoint(handle_mcp_auth_choice_callback),
)
```

- [ ] **Step 5: Change handler signatures and add choice prompt helper**

In `crates/bot/src/telegram/handler.rs`, add imports:

```rust
use super::mcp_auth_choice::{
    AuthChoiceSignals, AuthDetectionResult, MCP_AUTH_CHOICE_TTL, McpAuthChoice,
    McpAuthRecommendation, PendingMcpAuthChoiceRequest, PendingMcpAuthChoiceSlot,
    next_mcp_auth_choice_id, parse_auth_choice_callback_data, render_auth_choice_keyboard,
    recommend_auth_choice,
};
```

Remove the old local `AuthDetectionResult` struct from `handler.rs`; use the module type.

Add `pending_auth_choice_slot: Arc<PendingMcpAuthChoiceSlot>` to `handle_mcp`, then pass it to `handle_mcp_add`.

Add `pending_auth_choice_slot: &PendingMcpAuthChoiceSlot` to `handle_mcp_add`.

Add this helper near `request_token_and_register`:

```rust
#[allow(clippy::too_many_arguments)]
async fn prompt_mcp_auth_choice(
    bot: &BotType,
    chat_id: teloxide::types::ChatId,
    eff_thread_id: i64,
    pending_auth_choice_slot: &PendingMcpAuthChoiceSlot,
    agent_name: String,
    server_name: String,
    original_url: String,
    bare_url: String,
    has_query: bool,
    recommendation: McpAuthRecommendation,
) -> Result<(), RequestError> {
    let request_id = next_mcp_auth_choice_id();
    let prev = {
        let mut slot = pending_auth_choice_slot.0.lock().await;
        let prev = slot.take();
        *slot = Some(PendingMcpAuthChoiceRequest {
            id: request_id,
            chat_id: chat_id.0,
            thread_id: eff_thread_id,
            agent_name,
            server_name: server_name.clone(),
            original_url,
            bare_url,
            has_query,
            recommendation: recommendation.clone(),
            expires_at: std::time::Instant::now() + MCP_AUTH_CHOICE_TTL,
        });
        prev
    };

    if let Some(prev) = prev {
        let mut send = bot.send_message(
            teloxide::types::ChatId(prev.chat_id),
            "Previous MCP auth choice request superseded by a new /mcp command.",
        );
        if prev.thread_id != 0 {
            send = send.message_thread_id(teloxide::types::ThreadId(teloxide::types::MessageId(
                prev.thread_id as i32,
            )));
        }
        send.await.ok();
    }

    let body = match recommendation.choice {
        McpAuthChoice::OAuth => format!(
            "Choose authentication method for {server_name}. Recommended: OAuth."
        ),
        McpAuthChoice::Header => {
            if let Some(header) = recommendation.header_name.as_deref() {
                format!(
                    "Choose authentication method for {server_name}. Recommended: Header ({header})."
                )
            } else {
                format!(
                    "Choose authentication method for {server_name}. Recommended: Header (Authorization: Bearer)."
                )
            }
        }
        McpAuthChoice::UrlAsIs => format!(
            "Choose authentication method for {server_name}. Recommended: URL as-is."
        ),
    };

    let keyboard = render_auth_choice_keyboard(request_id, recommendation.choice);
    let mut send = bot.send_message(chat_id, body).reply_markup(keyboard);
    if eff_thread_id != 0 {
        send = send.message_thread_id(teloxide::types::ThreadId(teloxide::types::MessageId(
            eff_thread_id as i32,
        )));
    }
    send.await?;
    Ok(())
}
```

- [ ] **Step 6: Replace immediate `/mcp add` registration with recommendation + pending choice**

In `handle_mcp_add`, keep URL parsing, `has_query`, `bare_url`, `agent_name`, and `eff_thread_id`.

Replace the current OAuth branch and non-OAuth registration branch with:

```rust
let is_loopback = right_mcp::credentials::is_loopback_url(original_url);
let is_public = right_mcp::credentials::is_public_url(&bare_url);

let http_client = reqwest::Client::builder()
    .connect_timeout(std::time::Duration::from_secs(10))
    .timeout(std::time::Duration::from_secs(15))
    .build()
    .unwrap_or_else(|_| reqwest::Client::new());

let oauth_discovered = if has_query || !is_public {
    false
} else {
    bot.send_chat_action(msg.chat.id, teloxide::types::ChatAction::Typing)
        .await
        .ok();
    tracing::info!(url = %bare_url, "mcp add: starting OAuth AS discovery");
    let result = right_mcp::oauth::discover_as(&http_client, &bare_url).await;
    tracing::info!(
        url = %bare_url,
        oauth_discovered = result.is_ok(),
        err = ?result.err(),
        "mcp add: OAuth AS discovery complete"
    );
    result.is_ok()
};

let detection = if !has_query && !oauth_discovered && is_public {
    bot.send_message(msg.chat.id, "Detecting authentication method...")
        .await?;
    let (auth_type, header_name) = detect_auth_with_typing_indicator(
        bot,
        msg.chat.id,
        &bare_url,
        agent_dir,
        ssh_config_path,
        resolved_sandbox,
    )
    .await;
    Some(AuthDetectionResult {
        auth_type,
        header_name,
    })
} else {
    None
};

let recommendation = recommend_auth_choice(AuthChoiceSignals {
    has_query,
    oauth_discovered,
    is_loopback,
    is_public,
    detection: detection.as_ref(),
});

prompt_mcp_auth_choice(
    bot,
    msg.chat.id,
    eff_thread_id,
    pending_auth_choice_slot,
    agent_name.to_string(),
    name.to_string(),
    original_url.to_string(),
    bare_url,
    has_query,
    recommendation,
)
.await
```

- [ ] **Step 7: Run dispatch smoke test and handler-adjacent tests**

Run:

```bash
devenv shell -- cargo test -p right-bot dispatcher_builds_without_panic
devenv shell -- cargo test -p right-bot mcp_auth_choice
```

Expected: PASS. If dptree reports a missing dependency type, add that exact dependency to `.dependencies(dptree::deps![...])`.

- [ ] **Step 8: Commit pending auth-choice wiring**

Run:

```bash
devenv shell -- git add crates/bot/src/telegram/dispatch.rs crates/bot/src/telegram/handler.rs crates/bot/src/telegram/mcp_auth_choice.rs
devenv shell -- git commit -m "feat(mcp): prompt for auth method choice"
```

## Task 4: Implement Auth Choice Callback Actions

**Files:**
- Modify: `crates/bot/src/telegram/handler.rs`
- Modify: `crates/bot/src/telegram/mcp_auth_choice.rs`

- [ ] **Step 1: Move token input parsing in `request_token_and_register` to the pure helper**

In `crates/bot/src/telegram/handler.rs`, add `parse_token_input` to the `mcp_auth_choice` import list.

Inside `request_token_and_register`, replace the existing `split_once(": ")` token parsing block with:

```rust
let parsed = parse_token_input(raw_input, initial_auth_type, initial_auth_header);
let token = parsed.token;
let auth_type = parsed.auth_type;
let auth_header = parsed.auth_header;
```

Keep the existing `tracing::info!(url = %bare_url, %auth_type, ...)` and `internal.mcp_add(...)` call below it.

- [ ] **Step 2: Add callback handler**

In `crates/bot/src/telegram/handler.rs`, add this public handler near the other callback handlers:

```rust
#[allow(clippy::too_many_arguments)]
pub async fn handle_mcp_auth_choice_callback(
    bot: BotType,
    q: CallbackQuery,
    internal: Arc<InternalApi>,
    pending_auth_choice_slot: Arc<PendingMcpAuthChoiceSlot>,
    pending_token_slot: Arc<PendingTokenSlot>,
) -> ResponseResult<()> {
    let qid = q.id.clone();
    let Some((request_id, choice)) = q
        .data
        .as_deref()
        .and_then(parse_auth_choice_callback_data)
    else {
        bot.answer_callback_query(qid).text("Invalid MCP choice").await?;
        return Ok(());
    };

    let pending = {
        let mut slot = pending_auth_choice_slot.0.lock().await;
        match slot.as_ref() {
            Some(p) if p.id == request_id && !p.is_expired(std::time::Instant::now()) => slot.take(),
            Some(p) if p.id == request_id => {
                slot.take();
                None
            }
            _ => None,
        }
    };

    let Some(pending) = pending else {
        bot.answer_callback_query(qid)
            .text("MCP choice expired")
            .await?;
        return Ok(());
    };

    if let Some(message) = q.message.as_ref() {
        if message.chat().id.0 != pending.chat_id {
            bot.answer_callback_query(qid).text("Not allowed").await?;
            return Ok(());
        }
    }

    bot.answer_callback_query(qid).text("Selected").await?;

    match choice {
        McpAuthChoice::OAuth => {
            match internal
                .0
                .mcp_add(
                    &pending.agent_name,
                    &pending.server_name,
                    &pending.bare_url,
                    Some("oauth"),
                    None,
                    None,
                )
                .await
            {
                Ok(resp) => {
                    let escaped = html_escape(&pending.server_name);
                    let mut reply = format!("Added MCP server <b>{escaped}</b> (OAuth).");
                    if let Some(ref w) = resp.warning {
                        reply.push_str(&format!("\n{}", html_escape(w)));
                    }
                    reply.push_str(&format!(
                        "\nRun <code>/mcp auth {}</code> to authenticate.",
                        pending.server_name
                    ));
                    send_html_reply(
                        &bot,
                        teloxide::types::ChatId(pending.chat_id),
                        pending.thread_id,
                        &reply,
                    )
                    .await?;
                }
                Err(e) => {
                    send_html_reply(
                        &bot,
                        teloxide::types::ChatId(pending.chat_id),
                        pending.thread_id,
                        &format!("Failed: {e:#}"),
                    )
                    .await?;
                }
            }
        }
        McpAuthChoice::Header => {
            request_token_and_register(
                bot.clone(),
                teloxide::types::ChatId(pending.chat_id),
                pending.thread_id,
                right_mcp::internal_client::InternalClient::new(internal.0.socket_path()),
                pending.agent_name,
                pending.server_name,
                pending.bare_url,
                pending.recommendation.header_auth_type,
                pending.recommendation.header_name,
                pending_token_slot.as_ref().clone(),
            )
            .await?;
        }
        McpAuthChoice::UrlAsIs => {
            let auth_type = if pending.has_query {
                Some("query_string")
            } else {
                None
            };
            match internal
                .0
                .mcp_add(
                    &pending.agent_name,
                    &pending.server_name,
                    &pending.original_url,
                    auth_type,
                    None,
                    None,
                )
                .await
            {
                Ok(resp) => {
                    let escaped = html_escape(&pending.server_name);
                    let mut reply = format!("Added MCP server <b>{escaped}</b>.");
                    if resp.tools_count > 0 {
                        reply.push_str(&format!(" {} tools available.", resp.tools_count));
                    }
                    if let Some(ref w) = resp.warning {
                        reply.push_str(&format!("\n{}", html_escape(w)));
                    }
                    send_html_reply(
                        &bot,
                        teloxide::types::ChatId(pending.chat_id),
                        pending.thread_id,
                        &reply,
                    )
                    .await?;
                }
                Err(e) => {
                    send_html_reply(
                        &bot,
                        teloxide::types::ChatId(pending.chat_id),
                        pending.thread_id,
                        &format!("Failed: {e:#}"),
                    )
                    .await?;
                }
            }
        }
    }

    Ok(())
}
```

- [ ] **Step 3: Run callback wiring tests**

Run:

```bash
devenv shell -- cargo test -p right-bot dispatcher_builds_without_panic
devenv shell -- cargo test -p right-bot mcp_auth_choice
```

Expected: PASS.

- [ ] **Step 4: Run a targeted compile check for handler changes**

Run:

```bash
devenv shell -- cargo test -p right-bot --no-run
```

Expected: PASS compilation. Fix any teloxide type mismatches directly; do not trust LSP diagnostics.

- [ ] **Step 5: Commit callback actions**

Run:

```bash
devenv shell -- git add crates/bot/src/telegram/handler.rs crates/bot/src/telegram/dispatch.rs crates/bot/src/telegram/mcp_auth_choice.rs
devenv shell -- git commit -m "feat(mcp): handle auth method callbacks"
```

## Task 5: Update MCP Auth Documentation

**Files:**
- Modify: `ARCHITECTURE.md`
- Modify: `docs/architecture/mcp.md`
- Modify: `docs/superpowers/specs/2026-05-18-mcp-auth-choice-design.md`

- [ ] **Step 1: Update `ARCHITECTURE.md` MCP auth section**

In `ARCHITECTURE.md`, replace the MCP auth type intro and detection table with:

```markdown
### MCP Auth Types

Four auth methods are supported. `/mcp add` runs discovery/classification
heuristics, then asks the user to choose `OAuth`, `Header`, or `URL as-is`.
The heuristic is a recommendation; the user's button choice is authoritative.

| auth_type | How token is injected | Selection |
|-----------|----------------------|-----------|
| `oauth` | `Authorization: Bearer` via DynamicAuthClient | User chooses `OAuth`; OAuth AS discovery recommends it |
| `bearer` | `Authorization: Bearer` header | User chooses `Header` with bearer recommendation/fallback |
| `header` | Custom header (e.g. `X-Api-Key`) | User chooses `Header`; Haiku may recommend the header name; user may override with `HeaderName: token` |
| `query_string` | Embedded in URL | User chooses `URL as-is` for a URL containing `?` query params |
```

Add this paragraph immediately below the table:

```markdown
`URL as-is` also covers no-auth and loopback development MCP servers. Public
servers still require HTTPS. Explicit loopback registration allows HTTP/HTTPS
for `localhost`, `127.0.0.1`, and `::1`; broad private/link-local ranges remain
blocked by default.
```

- [ ] **Step 2: Add auth-choice flow to `docs/architecture/mcp.md`**

In `docs/architecture/mcp.md`, add this section before `## MCP Token Refresh`:

```markdown
## MCP Auth Choice Flow

`/mcp add <name> <url>` treats detection as advice, not authority. The bot
parses the original URL, derives a bare URL for probes, runs OAuth discovery
when the bare URL is public and has no query string, and runs auth-header
classification only when OAuth was not discovered. It then shows inline buttons
for `OAuth`, `Header`, and `URL as-is`, marking the recommendation.

No upstream MCP server is registered until the user clicks a button. `OAuth`
registers the bare URL as `auth_type=oauth` and asks the user to run
`/mcp auth <server>`. `Header` prompts for a token using the detected bearer or
custom-header recommendation; the user can override with `HeaderName: token`.
`URL as-is` registers the exact original URL without token/header injection,
preserving query-string credentials.

URL validation has two modes. Public detection remains strict HTTPS and excludes
loopback/private/link-local hosts. Explicit user-managed registration allows
loopback HTTP/HTTPS for local development MCP servers, while broad
private/link-local ranges remain rejected.
```

- [ ] **Step 3: Verify the spec contains loopback-only local support**

Run:

```bash
devenv shell -- rg -n "Loopback support|broad private/link-local|URL as-is" docs/superpowers/specs/2026-05-18-mcp-auth-choice-design.md
```

Expected: output includes the loopback support paragraph and `URL as-is` references.

- [ ] **Step 4: Commit docs**

Run:

```bash
devenv shell -- git add ARCHITECTURE.md docs/architecture/mcp.md docs/superpowers/specs/2026-05-18-mcp-auth-choice-design.md
devenv shell -- git commit -m "docs(mcp): document auth method choice"
```

## Task 6: Final Targeted And Workspace Verification

**Files:**
- No code edits unless verification finds a bug.

- [ ] **Step 1: Run targeted MCP credentials tests**

Run:

```bash
devenv shell -- cargo test -p right-mcp credentials
```

Expected: PASS.

- [ ] **Step 2: Run targeted bot auth-choice tests**

Run:

```bash
devenv shell -- cargo test -p right-bot mcp_auth_choice
devenv shell -- cargo test -p right-bot dispatcher_builds_without_panic
```

Expected: PASS.

- [ ] **Step 3: Run targeted internal API tests**

Run:

```bash
devenv shell -- cargo test -p right mcp_add_validates_url_private_ip
devenv shell -- cargo test -p right mcp_add_allows_loopback_oauth_registration
```

Expected: PASS.

- [ ] **Step 4: Run final mandatory workspace test suite**

Run:

```bash
devenv shell -- cargo test --workspace
```

Expected: PASS. If failures are unrelated and pre-existing, record the exact failing test names and error summaries before stopping. If failures are caused by this work, fix them before claiming completion.

- [ ] **Step 5: Inspect git status and commit any verification fixes**

Run:

```bash
devenv shell -- git status --short
```

Expected: no uncommitted changes. If verification fixes were needed:

```bash
devenv shell -- git add <changed-files>
devenv shell -- git commit -m "fix(mcp): stabilize auth choice flow"
```

## Task 7: Claude Code Review Loop

**Files:**
- No planned code edits unless review finds a real issue.

- [ ] **Step 1: Run the CC review loop**

The user explicitly approved running `cca /review-loop` for this session.

Run:

```bash
devenv shell -- cc-review .
```

Expected: review completes. If the command is unavailable, inspect the CC
Review plugin instructions and use the repository's configured `cca
/review-loop` command.

- [ ] **Step 2: Apply only valid review fixes**

For each finding, verify it against the code before changing anything. Fix only
findings that are valid, in scope for the MCP auth-choice work, and not already
covered by passing tests.

- [ ] **Step 3: Re-run verification after review fixes**

If any files changed, run:

```bash
devenv shell -- cargo test --workspace
```

Expected: PASS.

- [ ] **Step 4: Commit review fixes if needed**

If review fixes were made:

```bash
devenv shell -- git add <changed-files>
devenv shell -- git commit -m "fix(mcp): address cc review findings"
```

If no fixes were needed, leave the worktree clean and record that the review
loop produced no required changes.
