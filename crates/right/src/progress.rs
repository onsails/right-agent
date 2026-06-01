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
    BackgroundReview,
    ProbeWriter,
    Curator,
    #[cfg(test)]
    NonForeground,
}

impl ProgressInvocationKind {
    pub(crate) fn is_learning_capable(self) -> bool {
        matches!(self, Self::Foreground | Self::ProbeWriter | Self::Curator)
    }

    pub(crate) fn sends_learning_messages(self) -> bool {
        matches!(self, Self::Foreground)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ConversationScope {
    pub(crate) chat_id: i64,
    pub(crate) thread_id: i64,
}

#[derive(Clone)]
pub(crate) struct ProgressRegistration {
    pub(crate) invocation_id: String,
    pub(crate) kind: ProgressInvocationKind,
    pub(crate) bot_socket_path: PathBuf,
    pub(crate) bot_send_token: String,
    pub(crate) conversation_scope: Option<ConversationScope>,
}

impl std::fmt::Debug for ProgressRegistration {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProgressRegistration")
            .field("invocation_id", &self.invocation_id)
            .field("kind", &self.kind)
            .field("bot_socket_path", &self.bot_socket_path)
            .field("bot_send_token", &"<redacted>")
            .field("conversation_scope", &self.conversation_scope)
            .finish()
    }
}

/// Everything the aggregator needs to perform a forum-topic operation for an
/// invocation: where to reach the bot, the shared send token, and the
/// server-resolved chat id (never agent-supplied).
#[derive(Clone)]
pub(crate) struct ForumTarget {
    pub(crate) bot_socket_path: PathBuf,
    pub(crate) bot_send_token: String,
    pub(crate) chat_id: i64,
}

impl std::fmt::Debug for ForumTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ForumTarget")
            .field("bot_socket_path", &self.bot_socket_path)
            .field("bot_send_token", &"<redacted>")
            .field("chat_id", &self.chat_id)
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
    conversation_scope: Option<ConversationScope>,
    last_sent_at: Option<Instant>,
}

impl std::fmt::Debug for ProgressInvocation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProgressInvocation")
            .field("kind", &self.kind)
            .field("bot_socket_path", &self.bot_socket_path)
            .field("bot_send_token", &"<redacted>")
            .field("conversation_scope", &self.conversation_scope)
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
                conversation_scope: registration.conversation_scope,
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

    /// Atomic lookup returning both the invocation kind and its send target
    /// under a single mutex acquisition.
    ///
    /// `send_learning_message` calls this once instead of separately fetching
    /// kind and target — otherwise an unregister between the two `.await`
    /// points could flip the result from "send" to a confusing
    /// `Unavailable` error after the phase gate already approved the send.
    ///
    /// Gating on `is_learning_capable` (broader than `sends_learning_messages`)
    /// matches `learning_invocation_kind`'s semantics: the caller decides via
    /// `should_send_learning_message` whether the target is actually used.
    pub(crate) async fn learning_invocation_kind_and_target(
        &self,
        invocation_id: &str,
    ) -> Result<(ProgressInvocationKind, ProgressSendTarget), ProgressError> {
        let inner = self.inner.lock().await;
        let invocation = inner.get(invocation_id).ok_or(ProgressError::Unavailable)?;
        if !invocation.kind.is_learning_capable() {
            return Err(ProgressError::Forbidden);
        }
        Ok((
            invocation.kind,
            ProgressSendTarget {
                bot_socket_path: invocation.bot_socket_path.clone(),
                bot_send_token: invocation.bot_send_token.clone(),
            },
        ))
    }

    pub(crate) async fn learning_invocation_kind(
        &self,
        invocation_id: &str,
    ) -> Result<ProgressInvocationKind, ProgressError> {
        let inner = self.inner.lock().await;
        let invocation = inner.get(invocation_id).ok_or(ProgressError::Unavailable)?;
        if !invocation.kind.is_learning_capable() {
            return Err(ProgressError::Forbidden);
        }
        Ok(invocation.kind)
    }

    #[allow(dead_code)]
    pub(crate) async fn conversation_scope(
        &self,
        invocation_id: &str,
    ) -> Result<ConversationScope, ProgressError> {
        let inner = self.inner.lock().await;
        let invocation = inner.get(invocation_id).ok_or(ProgressError::Unavailable)?;
        if !matches!(invocation.kind, ProgressInvocationKind::Foreground) {
            return Err(ProgressError::Forbidden);
        }
        invocation
            .conversation_scope
            .ok_or(ProgressError::Unavailable)
    }

    /// Resolve the bot endpoint + token + chat id for a forum-topic
    /// operation. Foreground-only (like progress and conversation search):
    /// cron/delivery/reflection/background turns must not manage topics.
    pub(crate) async fn forum_target(
        &self,
        invocation_id: &str,
    ) -> Result<ForumTarget, ProgressError> {
        let inner = self.inner.lock().await;
        let invocation = inner.get(invocation_id).ok_or(ProgressError::Unavailable)?;
        if !matches!(invocation.kind, ProgressInvocationKind::Foreground) {
            return Err(ProgressError::Forbidden);
        }
        let scope = invocation
            .conversation_scope
            .ok_or(ProgressError::Unavailable)?;
        Ok(ForumTarget {
            bot_socket_path: invocation.bot_socket_path.clone(),
            bot_send_token: invocation.bot_send_token.clone(),
            chat_id: scope.chat_id,
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
            conversation_scope: None,
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

    #[tokio::test(start_paused = true)]
    async fn learning_invocation_kind_and_target_does_not_consume_progress_rate_limit() {
        let registry = ProgressRegistry::default();
        registry.register(foreground_registration()).await;

        let (kind, target) = registry
            .learning_invocation_kind_and_target("inv-1")
            .await
            .unwrap();
        assert_eq!(kind, ProgressInvocationKind::Foreground);
        assert_eq!(target.bot_socket_path, PathBuf::from("/tmp/bot.sock"));
        assert_eq!(target.bot_send_token, "send-token");

        registry.begin_send("inv-1").await.unwrap();
        let err = registry.begin_send("inv-1").await.unwrap_err();
        assert!(matches!(err, ProgressError::RateLimited { .. }));
    }

    #[tokio::test]
    async fn conversation_scope_available_for_foreground_invocation() {
        let registry = ProgressRegistry::default();
        registry
            .register(ProgressRegistration {
                conversation_scope: Some(ConversationScope {
                    chat_id: 100,
                    thread_id: 7,
                }),
                ..foreground_registration()
            })
            .await;

        let scope = registry.conversation_scope("inv-1").await.unwrap();

        assert_eq!(
            scope,
            ConversationScope {
                chat_id: 100,
                thread_id: 7
            }
        );
    }

    #[tokio::test]
    async fn conversation_scope_rejects_missing_or_nonforeground_invocation() {
        let registry = ProgressRegistry::default();
        registry.register(foreground_registration()).await;
        registry
            .register(ProgressRegistration {
                invocation_id: "background-inv".to_owned(),
                kind: ProgressInvocationKind::BackgroundReview,
                bot_socket_path: PathBuf::from("/tmp/bot.sock"),
                bot_send_token: "send-token".to_owned(),
                conversation_scope: Some(ConversationScope {
                    chat_id: 100,
                    thread_id: 7,
                }),
            })
            .await;

        let err = registry
            .conversation_scope("missing-inv")
            .await
            .unwrap_err();
        assert_eq!(err, ProgressError::Unavailable);

        let err = registry.conversation_scope("inv-1").await.unwrap_err();
        assert_eq!(err, ProgressError::Unavailable);

        let err = registry
            .conversation_scope("background-inv")
            .await
            .unwrap_err();
        assert_eq!(err, ProgressError::Forbidden);
    }

    #[test]
    fn progress_registration_debug_redacts_token() {
        let reg = ProgressRegistration {
            invocation_id: "inv-1".to_owned(),
            kind: ProgressInvocationKind::Foreground,
            bot_socket_path: PathBuf::from("/tmp/bot.sock"),
            bot_send_token: "supersecret".to_owned(),
            conversation_scope: None,
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

    #[tokio::test]
    async fn forum_target_returns_scope_for_foreground() {
        let reg = ProgressRegistry::default();
        reg.register(ProgressRegistration {
            invocation_id: "inv-1".to_owned(),
            kind: ProgressInvocationKind::Foreground,
            bot_socket_path: "/tmp/bot.sock".into(),
            bot_send_token: "tok".to_owned(),
            conversation_scope: Some(ConversationScope {
                chat_id: 42,
                thread_id: 7,
            }),
        })
        .await;
        let target = reg.forum_target("inv-1").await.unwrap();
        assert_eq!(target.chat_id, 42);
        assert_eq!(target.bot_send_token, "tok");
    }

    #[tokio::test]
    async fn forum_target_forbidden_for_non_foreground() {
        let reg = ProgressRegistry::default();
        reg.register(ProgressRegistration {
            invocation_id: "inv-2".to_owned(),
            kind: ProgressInvocationKind::NonForeground,
            bot_socket_path: "/tmp/bot.sock".into(),
            bot_send_token: "tok".to_owned(),
            conversation_scope: Some(ConversationScope {
                chat_id: 42,
                thread_id: 7,
            }),
        })
        .await;
        assert!(matches!(
            reg.forum_target("inv-2").await,
            Err(ProgressError::Forbidden)
        ));
    }

    #[test]
    fn forum_target_debug_redacts_token() {
        let target = ForumTarget {
            bot_socket_path: PathBuf::from("/tmp/bot.sock"),
            bot_send_token: "supersecret".to_owned(),
            chat_id: 42,
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
            conversation_scope: None,
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
                conversation_scope: None,
            })
            .await;

        let err = registry.begin_send("inv-1").await.unwrap_err();
        assert_eq!(err, ProgressError::Forbidden);
    }

    mod progress_invocation_kind {
        use super::*;

        fn registration(
            invocation_id: &str,
            kind: ProgressInvocationKind,
            conversation_scope: Option<ConversationScope>,
        ) -> ProgressRegistration {
            ProgressRegistration {
                invocation_id: invocation_id.to_owned(),
                kind,
                bot_socket_path: PathBuf::from("/tmp/bot.sock"),
                bot_send_token: "send-token".to_owned(),
                conversation_scope,
            }
        }

        #[tokio::test]
        async fn probe_writer_and_curator_are_learning_capable() {
            let registry = ProgressRegistry::default();
            let kinds = [
                ("probe-writer-inv", ProgressInvocationKind::ProbeWriter),
                ("curator-inv", ProgressInvocationKind::Curator),
            ];

            for (invocation_id, kind) in kinds {
                registry
                    .register(registration(invocation_id, kind, None))
                    .await;

                let actual = registry
                    .learning_invocation_kind(invocation_id)
                    .await
                    .unwrap();
                assert_eq!(actual, kind);
            }
        }

        #[tokio::test]
        async fn probe_writer_and_curator_do_not_have_conversation_scope() {
            let registry = ProgressRegistry::default();
            let scope = ConversationScope {
                chat_id: 100,
                thread_id: 7,
            };
            let kinds = [
                ("probe-writer-inv", ProgressInvocationKind::ProbeWriter),
                ("curator-inv", ProgressInvocationKind::Curator),
            ];

            for (invocation_id, kind) in kinds {
                registry
                    .register(registration(invocation_id, kind, Some(scope)))
                    .await;

                let err = registry
                    .conversation_scope(invocation_id)
                    .await
                    .unwrap_err();
                assert_eq!(err, ProgressError::Forbidden);
            }
        }

        #[tokio::test]
        async fn probe_writer_and_curator_do_not_send_telegram_learning_messages() {
            let kinds = [
                ProgressInvocationKind::ProbeWriter,
                ProgressInvocationKind::Curator,
            ];
            for kind in kinds {
                assert!(
                    !kind.sends_learning_messages(),
                    "{kind:?} must not send learning telegram messages"
                );
            }
        }

        #[tokio::test]
        async fn foreground_keeps_existing_learning_message_delivery() {
            let registry = ProgressRegistry::default();
            registry
                .register(registration(
                    "foreground-inv",
                    ProgressInvocationKind::Foreground,
                    None,
                ))
                .await;

            let (kind, target) = registry
                .learning_invocation_kind_and_target("foreground-inv")
                .await
                .unwrap();

            assert_eq!(kind, ProgressInvocationKind::Foreground);
            assert!(kind.sends_learning_messages());
            assert_eq!(target.bot_socket_path, PathBuf::from("/tmp/bot.sock"));
            assert_eq!(target.bot_send_token, "send-token");
        }
    }
}
