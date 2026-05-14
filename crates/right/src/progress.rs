use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use schemars::JsonSchema;
use serde::Deserialize;
use tokio::sync::Mutex;
use tokio::time::Instant;

pub(crate) use right_mcp::internal_client::{
    PROGRESS_INVOCATION_HEADER, PROGRESS_MESSAGE_MAX_CHARS, SEND_PROGRESS_TOOL,
};
pub(crate) const PROGRESS_RATE_LIMIT: Duration = Duration::from_secs(30);

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct SendProgressParams {
    /// `schemars` 1.x emits the `length(max = …)` value as a runtime
    /// expression, so a const path expands to its `usize` value in the
    /// generated schema. Keeping a single source of truth keeps `tools/list`,
    /// server-side validation, and bot-side validation locked together.
    #[schemars(length(max = PROGRESS_MESSAGE_MAX_CHARS))]
    pub(crate) message: String,
}

#[derive(Debug, Default, Clone)]
pub(crate) struct ToolCallContext {
    pub(crate) invocation_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProgressInvocationKind {
    Foreground,
    #[cfg(test)]
    NonForeground,
}

#[derive(Clone)]
pub(crate) struct ProgressRegistration {
    pub(crate) invocation_id: String,
    pub(crate) kind: ProgressInvocationKind,
    pub(crate) bot_socket_path: PathBuf,
    pub(crate) bot_send_token: String,
}

impl std::fmt::Debug for ProgressRegistration {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProgressRegistration")
            .field("invocation_id", &self.invocation_id)
            .field("kind", &self.kind)
            .field("bot_socket_path", &self.bot_socket_path)
            .field("bot_send_token", &"<redacted>")
            .finish()
    }
}

#[derive(Clone)]
pub(crate) struct ProgressSendTarget {
    pub(crate) bot_socket_path: PathBuf,
    pub(crate) bot_send_token: String,
}

impl std::fmt::Debug for ProgressSendTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProgressSendTarget")
            .field("bot_socket_path", &self.bot_socket_path)
            .field("bot_send_token", &"<redacted>")
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ProgressError {
    Unavailable,
    Forbidden,
    RateLimited { retry_after: Duration },
}

#[derive(Debug, Clone, Default)]
pub(crate) struct ProgressRegistry {
    inner: Arc<Mutex<HashMap<String, ProgressInvocation>>>,
}

#[derive(Clone)]
struct ProgressInvocation {
    kind: ProgressInvocationKind,
    bot_socket_path: PathBuf,
    bot_send_token: String,
    last_sent_at: Option<Instant>,
}

impl std::fmt::Debug for ProgressInvocation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProgressInvocation")
            .field("kind", &self.kind)
            .field("bot_socket_path", &self.bot_socket_path)
            .field("bot_send_token", &"<redacted>")
            .field("last_sent_at", &self.last_sent_at)
            .finish()
    }
}

impl ProgressRegistry {
    pub(crate) async fn register(&self, registration: ProgressRegistration) {
        let mut inner = self.inner.lock().await;
        inner.insert(
            registration.invocation_id,
            ProgressInvocation {
                kind: registration.kind,
                bot_socket_path: registration.bot_socket_path,
                bot_send_token: registration.bot_send_token,
                last_sent_at: None,
            },
        );
    }

    pub(crate) async fn unregister(&self, invocation_id: &str) {
        self.inner.lock().await.remove(invocation_id);
    }

    #[cfg(test)]
    pub(crate) async fn get(
        &self,
        invocation_id: &str,
    ) -> Result<ProgressSendTarget, ProgressError> {
        let inner = self.inner.lock().await;
        let invocation = inner.get(invocation_id).ok_or(ProgressError::Unavailable)?;
        Ok(ProgressSendTarget {
            bot_socket_path: invocation.bot_socket_path.clone(),
            bot_send_token: invocation.bot_send_token.clone(),
        })
    }

    pub(crate) async fn begin_send(
        &self,
        invocation_id: &str,
    ) -> Result<ProgressSendTarget, ProgressError> {
        let mut inner = self.inner.lock().await;
        let invocation = inner
            .get_mut(invocation_id)
            .ok_or(ProgressError::Unavailable)?;
        if !matches!(invocation.kind, ProgressInvocationKind::Foreground) {
            return Err(ProgressError::Forbidden);
        }

        let now = Instant::now();
        if let Some(last_sent_at) = invocation.last_sent_at {
            let elapsed = now.duration_since(last_sent_at);
            if elapsed < PROGRESS_RATE_LIMIT {
                return Err(ProgressError::RateLimited {
                    retry_after: PROGRESS_RATE_LIMIT - elapsed,
                });
            }
        }
        invocation.last_sent_at = Some(now);

        Ok(ProgressSendTarget {
            bot_socket_path: invocation.bot_socket_path.clone(),
            bot_send_token: invocation.bot_send_token.clone(),
        })
    }

    /// Clear `last_sent_at` so the next attempt is not rate-limited.
    ///
    /// Why: `begin_send` optimistically reserves the rate-limit slot before the
    /// outgoing Telegram send. If delivery fails (bot UDS unreachable, Telegram
    /// 5xx, etc.) the agent would otherwise be locked out of progress for 30 s
    /// despite zero successful deliveries.
    pub(crate) async fn mark_send_failed(&self, invocation_id: &str) {
        let mut inner = self.inner.lock().await;
        if let Some(invocation) = inner.get_mut(invocation_id) {
            invocation.last_sent_at = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::time::Duration;

    fn foreground_registration() -> ProgressRegistration {
        ProgressRegistration {
            invocation_id: "inv-1".to_owned(),
            kind: ProgressInvocationKind::Foreground,
            bot_socket_path: PathBuf::from("/tmp/bot.sock"),
            bot_send_token: "send-token".to_owned(),
        }
    }

    #[tokio::test(start_paused = true)]
    async fn progress_registry_allows_first_send_and_rate_limits_second() {
        let registry = ProgressRegistry::default();
        registry.register(foreground_registration()).await;

        let target = registry.begin_send("inv-1").await.unwrap();
        assert_eq!(target.bot_socket_path, PathBuf::from("/tmp/bot.sock"));
        assert_eq!(target.bot_send_token, "send-token");

        let err = registry.begin_send("inv-1").await.unwrap_err();
        assert!(matches!(err, ProgressError::RateLimited { .. }));

        tokio::time::advance(Duration::from_secs(30)).await;
        registry.begin_send("inv-1").await.unwrap();
    }

    #[tokio::test]
    async fn progress_registry_unregister_removes_invocation() {
        let registry = ProgressRegistry::default();
        registry.register(foreground_registration()).await;

        registry.unregister("inv-1").await;

        let err = registry.get("inv-1").await.unwrap_err();
        assert_eq!(err, ProgressError::Unavailable);
    }

    #[tokio::test(start_paused = true)]
    async fn mark_send_failed_clears_rate_limit_slot() {
        let registry = ProgressRegistry::default();
        registry.register(foreground_registration()).await;

        registry.begin_send("inv-1").await.unwrap();
        registry.mark_send_failed("inv-1").await;
        // Without rollback this would be RateLimited; with rollback it passes.
        registry.begin_send("inv-1").await.unwrap();
    }

    #[test]
    fn progress_registration_debug_redacts_token() {
        let reg = ProgressRegistration {
            invocation_id: "inv-1".to_owned(),
            kind: ProgressInvocationKind::Foreground,
            bot_socket_path: PathBuf::from("/tmp/bot.sock"),
            bot_send_token: "supersecret".to_owned(),
        };
        let s = format!("{reg:?}");
        assert!(
            !s.contains("supersecret"),
            "Debug must redact bot_send_token: {s}"
        );
        assert!(s.contains("<redacted>"), "Debug must mark redaction: {s}");
    }

    #[test]
    fn progress_send_target_debug_redacts_token() {
        let target = ProgressSendTarget {
            bot_socket_path: PathBuf::from("/tmp/bot.sock"),
            bot_send_token: "supersecret".to_owned(),
        };
        let s = format!("{target:?}");
        assert!(
            !s.contains("supersecret"),
            "Debug must redact bot_send_token: {s}"
        );
        assert!(s.contains("<redacted>"), "Debug must mark redaction: {s}");
    }

    #[test]
    fn progress_invocation_debug_redacts_token() {
        let invocation = ProgressInvocation {
            kind: ProgressInvocationKind::Foreground,
            bot_socket_path: PathBuf::from("/tmp/bot.sock"),
            bot_send_token: "supersecret".to_owned(),
            last_sent_at: None,
        };
        let s = format!("{invocation:?}");
        assert!(
            !s.contains("supersecret"),
            "Debug must redact bot_send_token: {s}"
        );
        assert!(s.contains("<redacted>"), "Debug must mark redaction: {s}");
    }

    #[test]
    fn send_progress_params_schema_exposes_max_length() {
        // The agent-facing `tools/list` schema must advertise the same limit
        // the server enforces, so agents can plan their messages instead of
        // hitting `invalid_argument` blind.
        let schema = schemars::schema_for!(SendProgressParams);
        let value = serde_json::to_value(&schema).expect("schema serializes");
        let max_length = value
            .pointer("/properties/message/maxLength")
            .and_then(|v| v.as_u64())
            .expect("message.maxLength must be set on the schema");
        assert_eq!(max_length as usize, PROGRESS_MESSAGE_MAX_CHARS);
        assert_eq!(PROGRESS_MESSAGE_MAX_CHARS, 2000);
    }

    #[tokio::test]
    async fn progress_registry_rejects_non_foreground_kind() {
        let registry = ProgressRegistry::default();
        registry
            .register(ProgressRegistration {
                invocation_id: "inv-1".to_owned(),
                kind: ProgressInvocationKind::NonForeground,
                bot_socket_path: PathBuf::from("/tmp/bot.sock"),
                bot_send_token: "send-token".to_owned(),
            })
            .await;

        let err = registry.begin_send("inv-1").await.unwrap_err();
        assert_eq!(err, ProgressError::Forbidden);
    }
}
