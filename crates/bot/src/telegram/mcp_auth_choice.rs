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
    pub recommendation: McpAuthRecommendation,
    pub expires_at: Instant,
}

impl PendingMcpAuthChoiceRequest {
    pub(crate) fn is_expired(&self, now: Instant) -> bool {
        now >= self.expires_at
    }

    /// True when `original_url` carried a query string.
    pub(crate) fn has_query(&self) -> bool {
        reqwest::Url::parse(&self.original_url)
            .ok()
            .and_then(|url| url.query().map(|_| ()))
            .is_some()
    }
}

#[derive(Clone)]
pub struct PendingMcpAuthChoiceSlot(pub(crate) Arc<Mutex<Option<PendingMcpAuthChoiceRequest>>>);

pub(crate) const MCP_AUTH_CHOICE_TTL: Duration = Duration::from_secs(120);

static NEXT_MCP_AUTH_CHOICE_ID: AtomicU64 = AtomicU64::new(1);

pub(crate) fn next_mcp_auth_choice_id() -> u64 {
    NEXT_MCP_AUTH_CHOICE_ID.fetch_add(1, Ordering::Relaxed)
}

pub(crate) fn parse_auth_choice_callback_data(data: &str) -> Option<(u64, McpAuthChoice)> {
    let mut parts = data.splitn(3, ':');
    if parts.next()? != "mcpauth" {
        return None;
    }
    let id = parts.next()?.parse::<u64>().ok()?;
    let choice = match parts.next()? {
        "oauth" => McpAuthChoice::OAuth,
        "header" => McpAuthChoice::Header,
        "url" => McpAuthChoice::UrlAsIs,
        _ => return None,
    };
    Some((id, choice))
}

#[derive(Debug)]
pub(crate) enum PendingMcpAuthChoiceTake {
    Ready(PendingMcpAuthChoiceRequest),
    Missing,
    Expired,
    ChatMismatch,
}

pub(crate) async fn take_pending_auth_choice(
    slot: &PendingMcpAuthChoiceSlot,
    request_id: u64,
    callback_chat_id: Option<i64>,
    callback_thread_id: Option<i64>,
    now: Instant,
) -> PendingMcpAuthChoiceTake {
    let mut slot = slot.0.lock().await;
    let Some(current) = slot.as_ref() else {
        return PendingMcpAuthChoiceTake::Missing;
    };

    if current.id != request_id {
        return PendingMcpAuthChoiceTake::Missing;
    }

    if current.is_expired(now) {
        slot.take();
        return PendingMcpAuthChoiceTake::Expired;
    }

    let Some(callback_chat_id) = callback_chat_id else {
        return PendingMcpAuthChoiceTake::ChatMismatch;
    };
    if callback_chat_id != current.chat_id {
        return PendingMcpAuthChoiceTake::ChatMismatch;
    }

    let Some(callback_thread_id) = callback_thread_id else {
        return PendingMcpAuthChoiceTake::ChatMismatch;
    };
    if callback_thread_id != current.thread_id {
        return PendingMcpAuthChoiceTake::ChatMismatch;
    }

    PendingMcpAuthChoiceTake::Ready(
        slot.take()
            .expect("slot contains matching pending MCP auth choice"),
    )
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
        return default_recommendation(McpAuthChoice::UrlAsIs);
    }

    if signals.oauth_discovered {
        return default_recommendation(McpAuthChoice::OAuth);
    }

    if signals.is_loopback || !signals.is_public {
        return default_recommendation(McpAuthChoice::UrlAsIs);
    }

    let Some(detection) = signals.detection else {
        return default_recommendation(McpAuthChoice::Header);
    };

    let Some(header_name) = detection.header_name.as_deref().map(str::trim) else {
        return default_recommendation(McpAuthChoice::Header);
    };

    if detection.auth_type == "header" && !header_name.is_empty() {
        McpAuthRecommendation {
            choice: McpAuthChoice::Header,
            header_auth_type: "header".to_string(),
            header_name: Some(header_name.to_string()),
        }
    } else {
        default_recommendation(McpAuthChoice::Header)
    }
}

fn default_recommendation(choice: McpAuthChoice) -> McpAuthRecommendation {
    McpAuthRecommendation {
        choice,
        header_auth_type: "bearer".to_string(),
        header_name: None,
    }
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
    if let Some((header, value_raw)) = raw_input.split_once(':') {
        let header = header.trim();
        let value = value_raw.trim();
        let looks_like_header =
            header.contains('-') || value_raw.starts_with(|c: char| c.is_ascii_whitespace());
        if looks_like_header && is_header_name(header) && !value.is_empty() {
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

fn is_header_name(value: &str) -> bool {
    !value.is_empty()
        && !value.contains(' ')
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

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

    #[tokio::test]
    async fn parse_auth_choice_callback_data_accepts_valid_choices() {
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

    #[tokio::test]
    async fn parse_auth_choice_callback_data_rejects_invalid_data() {
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

    #[tokio::test]
    async fn render_auth_choice_keyboard_marks_recommended_button() {
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

    #[tokio::test]
    async fn recommend_auth_choice_prefers_query_string_over_oauth() {
        let rec = recommend_auth_choice(AuthChoiceSignals {
            has_query: true,
            oauth_discovered: true,
            is_loopback: false,
            is_public: true,
            detection: Some(&AuthDetectionResult {
                auth_type: "header".to_string(),
                header_name: Some("X-Api-Key".to_string()),
            }),
        });

        assert_eq!(rec.choice, McpAuthChoice::UrlAsIs);
        assert_eq!(rec.header_auth_type, "bearer");
        assert_eq!(rec.header_name, None);
    }

    #[tokio::test]
    async fn recommend_auth_choice_uses_oauth_when_discovered_without_query() {
        let rec = recommend_auth_choice(AuthChoiceSignals {
            has_query: false,
            oauth_discovered: true,
            is_loopback: false,
            is_public: true,
            detection: None,
        });

        assert_eq!(rec.choice, McpAuthChoice::OAuth);
    }

    #[tokio::test]
    async fn recommend_auth_choice_uses_detected_custom_header() {
        let rec = recommend_auth_choice(AuthChoiceSignals {
            has_query: false,
            oauth_discovered: false,
            is_loopback: false,
            is_public: true,
            detection: Some(&AuthDetectionResult {
                auth_type: "header".to_string(),
                header_name: Some("X-Api-Key".to_string()),
            }),
        });

        assert_eq!(rec.choice, McpAuthChoice::Header);
        assert_eq!(rec.header_auth_type, "header");
        assert_eq!(rec.header_name.as_deref(), Some("X-Api-Key"));
    }

    #[tokio::test]
    async fn recommend_auth_choice_uses_header_for_public_detection_failure() {
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

    #[tokio::test]
    async fn recommend_auth_choice_uses_url_as_is_for_loopback() {
        let rec = recommend_auth_choice(AuthChoiceSignals {
            has_query: false,
            oauth_discovered: false,
            is_loopback: true,
            is_public: false,
            detection: None,
        });

        assert_eq!(rec.choice, McpAuthChoice::UrlAsIs);
    }

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

    #[tokio::test]
    async fn take_pending_auth_choice_consumes_matching_request() {
        let slot = PendingMcpAuthChoiceSlot(Arc::new(Mutex::new(Some(pending_request(
            42,
            100,
            Instant::now() + MCP_AUTH_CHOICE_TTL,
        )))));

        let taken = take_pending_auth_choice(&slot, 42, Some(100), Some(0), Instant::now()).await;

        assert!(matches!(taken, PendingMcpAuthChoiceTake::Ready(_)));
        assert!(slot.0.lock().await.is_none());
    }

    #[tokio::test]
    async fn take_pending_auth_choice_rejects_stale_request_without_clearing_current() {
        let slot = PendingMcpAuthChoiceSlot(Arc::new(Mutex::new(Some(pending_request(
            43,
            100,
            Instant::now() + MCP_AUTH_CHOICE_TTL,
        )))));

        let taken = take_pending_auth_choice(&slot, 42, Some(100), Some(0), Instant::now()).await;

        assert!(matches!(taken, PendingMcpAuthChoiceTake::Missing));
        assert_eq!(slot.0.lock().await.as_ref().map(|p| p.id), Some(43));
    }

    #[tokio::test]
    async fn take_pending_auth_choice_clears_expired_matching_request() {
        let slot = PendingMcpAuthChoiceSlot(Arc::new(Mutex::new(Some(pending_request(
            42,
            100,
            Instant::now() - Duration::from_secs(1),
        )))));

        let taken = take_pending_auth_choice(&slot, 42, Some(100), Some(0), Instant::now()).await;

        assert!(matches!(taken, PendingMcpAuthChoiceTake::Expired));
        assert!(slot.0.lock().await.is_none());
    }

    #[tokio::test]
    async fn take_pending_auth_choice_rejects_chat_mismatch_without_clearing_request() {
        let slot = PendingMcpAuthChoiceSlot(Arc::new(Mutex::new(Some(pending_request(
            42,
            100,
            Instant::now() + MCP_AUTH_CHOICE_TTL,
        )))));

        let taken = take_pending_auth_choice(&slot, 42, Some(200), Some(0), Instant::now()).await;

        assert!(matches!(taken, PendingMcpAuthChoiceTake::ChatMismatch));
        assert_eq!(slot.0.lock().await.as_ref().map(|p| p.id), Some(42));
    }

    #[tokio::test]
    async fn take_pending_auth_choice_rejects_missing_chat_context_without_clearing_request() {
        let slot = PendingMcpAuthChoiceSlot(Arc::new(Mutex::new(Some(pending_request(
            42,
            100,
            Instant::now() + MCP_AUTH_CHOICE_TTL,
        )))));

        let taken = take_pending_auth_choice(&slot, 42, None, Some(0), Instant::now()).await;

        assert!(matches!(taken, PendingMcpAuthChoiceTake::ChatMismatch));
        assert_eq!(slot.0.lock().await.as_ref().map(|p| p.id), Some(42));
    }

    #[tokio::test]
    async fn take_pending_auth_choice_rejects_missing_thread_context_without_clearing_request() {
        let slot = PendingMcpAuthChoiceSlot(Arc::new(Mutex::new(Some(pending_request(
            42,
            100,
            Instant::now() + MCP_AUTH_CHOICE_TTL,
        )))));

        let taken = take_pending_auth_choice(&slot, 42, Some(100), None, Instant::now()).await;

        assert!(matches!(taken, PendingMcpAuthChoiceTake::ChatMismatch));
        assert_eq!(slot.0.lock().await.as_ref().map(|p| p.id), Some(42));
    }

    #[tokio::test]
    async fn take_pending_auth_choice_rejects_thread_mismatch_without_clearing_request() {
        let mut request = pending_request(42, 100, Instant::now() + MCP_AUTH_CHOICE_TTL);
        request.thread_id = 10;
        let slot = PendingMcpAuthChoiceSlot(Arc::new(Mutex::new(Some(request))));

        let taken = take_pending_auth_choice(&slot, 42, Some(100), Some(11), Instant::now()).await;

        assert!(matches!(taken, PendingMcpAuthChoiceTake::ChatMismatch));
        assert_eq!(slot.0.lock().await.as_ref().map(|p| p.id), Some(42));
    }

    fn pending_request(id: u64, chat_id: i64, expires_at: Instant) -> PendingMcpAuthChoiceRequest {
        PendingMcpAuthChoiceRequest {
            id,
            chat_id,
            thread_id: 0,
            agent_name: "agent".to_string(),
            server_name: "server".to_string(),
            original_url: "https://example.com/mcp?api_key=secret".to_string(),
            bare_url: "https://example.com/mcp".to_string(),
            recommendation: McpAuthRecommendation {
                choice: McpAuthChoice::UrlAsIs,
                header_auth_type: "bearer".to_string(),
                header_name: None,
            },
            expires_at,
        }
    }

    #[tokio::test]
    async fn pending_request_has_query_ignores_url_normalization() {
        let pending = PendingMcpAuthChoiceRequest {
            original_url: "https://example.com".to_string(),
            bare_url: "https://example.com/".to_string(),
            ..pending_request(42, 100, Instant::now() + MCP_AUTH_CHOICE_TTL)
        };

        assert!(!pending.has_query());
    }

    #[tokio::test]
    async fn parse_token_input_supports_custom_header_override() {
        let parsed = parse_token_input("X-Api-Key: secret".to_string(), "bearer".to_string(), None);

        assert_eq!(parsed.token, "secret");
        assert_eq!(parsed.auth_type, "header");
        assert_eq!(parsed.auth_header.as_deref(), Some("X-Api-Key"));
    }

    #[tokio::test]
    async fn parse_token_input_supports_custom_header_without_space() {
        let parsed = parse_token_input("X-Api-Key:secret".to_string(), "bearer".to_string(), None);

        assert_eq!(parsed.token, "secret");
        assert_eq!(parsed.auth_type, "header");
        assert_eq!(parsed.auth_header.as_deref(), Some("X-Api-Key"));
    }

    #[tokio::test]
    async fn parse_token_input_keeps_raw_token_when_header_value_empty() {
        let parsed = parse_token_input("X-Api-Key:   ".to_string(), "bearer".to_string(), None);

        assert_eq!(parsed.token, "X-Api-Key:   ");
        assert_eq!(parsed.auth_type, "bearer");
        assert_eq!(parsed.auth_header, None);
    }

    #[tokio::test]
    async fn parse_token_input_keeps_raw_token_when_prefix_is_not_header() {
        let parsed = parse_token_input(
            "Bearer token with spaces".to_string(),
            "bearer".to_string(),
            None,
        );

        assert_eq!(parsed.token, "Bearer token with spaces");
        assert_eq!(parsed.auth_type, "bearer");
        assert_eq!(parsed.auth_header, None);
    }

    #[tokio::test]
    async fn parse_token_input_keeps_raw_token_when_value_contains_colon_no_space() {
        // Tokens like Stripe `sk_live_abc:def` or Basic auth `user:pass` look like
        // `is_header_name(prefix)` matches but no hyphen and no space-after-colon
        // means the user is pasting a single token, not a header/value pair.
        let parsed = parse_token_input("sk_live_abc:def".to_string(), "bearer".to_string(), None);
        assert_eq!(parsed.token, "sk_live_abc:def");
        assert_eq!(parsed.auth_type, "bearer");
        assert_eq!(parsed.auth_header, None);

        let parsed = parse_token_input("user:pass".to_string(), "bearer".to_string(), None);
        assert_eq!(parsed.token, "user:pass");
        assert_eq!(parsed.auth_type, "bearer");
        assert_eq!(parsed.auth_header, None);
    }
}
