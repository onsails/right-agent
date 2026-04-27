//! Shared MCP operation-error helper.
//!
//! Operation errors are agent-visible failures that fit the MCP convention
//! `is_error: true` on `CallToolResult` with a structured JSON body. This
//! module is the single entry point for constructing them. Protocol and
//! infrastructure failures must continue to bubble up as `Err`.

use rmcp::model::{CallToolResult, Content};
use serde_json::json;

use crate::mcp::proxy::ProxyError;

/// Build an MCP operation error.
///
/// Returns a `CallToolResult` with `is_error: Some(true)` and a single text
/// content body of JSON shape:
///
/// ```json
/// { "error": { "code": "<code>", "message": "<message>", "details": { ... } } }
/// ```
///
/// `details` is omitted from the JSON when `None`.
pub fn tool_error(
    code: &str,
    message: impl Into<String>,
    details: Option<serde_json::Value>,
) -> CallToolResult {
    let mut error = serde_json::Map::new();
    error.insert("code".to_string(), json!(code));
    error.insert("message".to_string(), json!(message.into()));
    if let Some(d) = details {
        error.insert("details".to_string(), d);
    }
    let payload = json!({ "error": serde_json::Value::Object(error) });
    let text = serde_json::to_string_pretty(&payload)
        .expect("serializing tool_error JSON cannot fail");
    CallToolResult::error(vec![Content::text(text)])
}

impl From<ProxyError> for CallToolResult {
    fn from(e: ProxyError) -> Self {
        match &e {
            ProxyError::NeedsAuth { .. } => tool_error("upstream_auth", format!("{e:#}"), None),
            ProxyError::Unreachable { .. } => {
                tool_error("upstream_unreachable", format!("{e:#}"), None)
            }
            ProxyError::NoSession { .. } => {
                tool_error("upstream_unreachable", format!("{e:#}"), None)
            }
            ProxyError::CallToolFailed { source, .. } => tool_error(
                "tool_failed",
                format!("{e:#}"),
                Some(json!({ "detail": format!("{source:#}") })),
            ),
            ProxyError::InitFailed { .. }
            | ProxyError::ListToolsFailed { .. }
            | ProxyError::InstructionsCacheFailed { .. } => {
                tool_error("upstream_unreachable", format!("{e:#}"), None)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rmcp::model::RawContent;
    use rmcp::service::ServiceError;

    fn extract_json(result: &CallToolResult) -> serde_json::Value {
        let RawContent::Text(t) = &result.content[0].raw else {
            panic!("expected text content, got {:?}", result.content[0].raw);
        };
        serde_json::from_str(&t.text).expect("body must be valid JSON")
    }

    #[test]
    fn tool_error_sets_is_error_and_basic_shape() {
        let r = tool_error("upstream_auth", "auth failed", None);
        assert_eq!(r.is_error, Some(true));
        let body = extract_json(&r);
        assert_eq!(body["error"]["code"], "upstream_auth");
        assert_eq!(body["error"]["message"], "auth failed");
        assert!(body["error"].get("details").is_none());
    }

    #[test]
    fn tool_error_includes_details_when_present() {
        let r = tool_error(
            "bootstrap_files_missing",
            "missing files",
            Some(json!({ "missing": ["IDENTITY.md"] })),
        );
        let body = extract_json(&r);
        assert_eq!(
            body["error"]["details"]["missing"][0].as_str(),
            Some("IDENTITY.md")
        );
    }

    #[test]
    fn from_proxy_error_needs_auth() {
        let e = ProxyError::NeedsAuth {
            server: "notion".into(),
        };
        let r: CallToolResult = e.into();
        assert_eq!(r.is_error, Some(true));
        let body = extract_json(&r);
        assert_eq!(body["error"]["code"], "upstream_auth");
    }

    #[test]
    fn from_proxy_error_unreachable() {
        let e = ProxyError::Unreachable {
            server: "notion".into(),
        };
        let r: CallToolResult = e.into();
        let body = extract_json(&r);
        assert_eq!(body["error"]["code"], "upstream_unreachable");
    }

    #[test]
    fn from_proxy_error_no_session() {
        let e = ProxyError::NoSession {
            server: "notion".into(),
        };
        let r: CallToolResult = e.into();
        let body = extract_json(&r);
        assert_eq!(body["error"]["code"], "upstream_unreachable");
    }

    #[test]
    fn from_proxy_error_call_tool_failed_includes_detail() {
        let e = ProxyError::CallToolFailed {
            server: "notion".into(),
            tool: "search".into(),
            source: ServiceError::Cancelled { reason: None },
        };
        let r: CallToolResult = e.into();
        let body = extract_json(&r);
        assert_eq!(body["error"]["code"], "tool_failed");
        assert!(body["error"]["details"]["detail"].is_string());
    }
}
