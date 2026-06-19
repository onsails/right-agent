//! Update routing. Replaces teloxide's Dispatcher/dptree. The pure
//! classification fns here are unit-tested; `route_update` + [`HandlerCtx`] map
//! an `UpdateContent` to a handler call.

use std::sync::Arc;

use dashmap::DashMap;
use right_agent::agent::allowlist::AllowlistHandle;
use tokio::sync::mpsc;

use super::BotType;
use super::handler::{
    AgentDir, AgentSettings, IdleTimestamp, InterceptSlots, InternalApi, RightHome, SshConfigPath,
};
use super::mention::BotIdentity;
use super::worker::{DebounceMsg, SessionKey};

/// Everything an update handler needs, replacing dptree's dependency injection.
///
/// One `HandlerCtx` is built in `setup_telegram` and shared (via `Arc`) by the
/// webhook handler; `route_update`/`on_message`/`on_callback` pass `&HandlerCtx`
/// to the migrated `handle_*` endpoints. Fields mirror the former
/// `dptree::deps![...]` list plus the resolved bot and identity.
#[derive(Clone)]
pub(crate) struct HandlerCtx {
    pub(crate) bot: BotType,
    pub(crate) allowlist: AllowlistHandle,
    pub(crate) identity: Arc<BotIdentity>,
    pub(crate) worker_map: Arc<DashMap<SessionKey, mpsc::Sender<DebounceMsg>>>,
    pub(crate) agent_dir: Arc<AgentDir>,
    pub(crate) home: Arc<RightHome>,
    pub(crate) ssh_config: Arc<SshConfigPath>,
    pub(crate) intercept_slots: Arc<InterceptSlots>,
    pub(crate) internal_api: Arc<InternalApi>,
    pub(crate) settings: Arc<AgentSettings>,
    pub(crate) idle_ts: Arc<IdleTimestamp>,
    pub(crate) worker_ctl: super::WorkerControlDeps,
}

/// Route one update to the matching handler. Best-effort: handler errors are
/// logged, never propagated — a single failed update must not stop the webhook
/// server.
///
/// Only fresh `Message` and `CallbackQuery` updates are routed. `EditedMessage`
/// is deliberately ignored (falls through to `_ => {}`): the former teloxide
/// dispatcher used `Update::filter_message()`, which matched only `Message` and
/// had no edited-message branch — so edits were received (they're in
/// `allowed_updates`) but silently dropped rather than starting a new agent
/// turn. We preserve that. `EditedMessage` stays in
/// `webhook::webhook_allowed_updates()` so `setWebhook` registration is
/// byte-identical to before.
pub(crate) async fn route_update(update: frankenstein::updates::Update, ctx: &HandlerCtx) {
    use frankenstein::updates::UpdateContent;
    match update.content {
        UpdateContent::Message(m) => {
            on_message(ctx, *m).await;
        }
        UpdateContent::CallbackQuery(q) => {
            on_callback(ctx, *q).await;
        }
        _ => {}
    }
}

/// Reproduce dispatch.rs's message branch: pre-filter log + group-archive, then
/// allow-list routing, then command parse → matching `handle_*`, else
/// `handle_message`. Errors are logged best-effort.
async fn on_message(ctx: &HandlerCtx, msg: frankenstein::types::Message) {
    use super::command::{self, BotCommand};
    use super::handler;

    let meta = super::dispatch::pre_filter_log_meta(&msg);
    tracing::info!(
        chat_id = meta.chat_id,
        chat_kind = meta.chat_kind,
        has_text = meta.has_text,
        has_caption = meta.has_caption,
        attachment_count = meta.attachment_count,
        entity_count = meta.entity_count,
        "message update received by webhook"
    );
    super::archive::archive_seen_group_message(&ctx.agent_dir.0, ctx.identity.as_ref(), &msg);

    // Allow-list / addressing filter (was dptree `filter_map`).
    let filter = super::filter::make_routing_filter(ctx.allowlist.clone(), (*ctx.identity).clone());
    let Some(decision) = filter(msg.clone()) else {
        return;
    };

    let bot_username = ctx.bot.me().username.as_deref().unwrap_or_default();
    let parsed = super::msg_ext::text_or_caption(&msg)
        .and_then(|text| command::parse(text, bot_username));

    let result = match parsed {
        Some(BotCommand::Start(payload)) => handler::handle_start(ctx, &msg, payload).await,
        Some(BotCommand::New(name)) => handler::handle_new(ctx, &msg, name).await,
        Some(BotCommand::List) => handler::handle_list(ctx, &msg).await,
        Some(BotCommand::Switch(uuid)) => handler::handle_switch(ctx, &msg, uuid).await,
        Some(BotCommand::Mcp(args)) => handler::handle_mcp(ctx, &msg, args).await,
        Some(BotCommand::Providers(args)) => handler::handle_providers(ctx, &msg, args).await,
        Some(BotCommand::SetFocus(args)) => handler::handle_set_focus(ctx, &msg, args).await,
        Some(BotCommand::Doctor) => handler::handle_doctor(ctx, &msg).await,
        Some(BotCommand::Model) => super::model_command::handle_model(ctx, &msg).await,
        Some(BotCommand::Mode) => super::mode_command::handle_mode(ctx, &msg).await,
        Some(BotCommand::ModeGroup) => super::mode_command::handle_mode_group(ctx, &msg).await,
        Some(BotCommand::Dashboard) => handler::handle_dashboard(ctx, &msg).await,
        Some(BotCommand::Debug(args)) => super::debug_command::handle_debug(ctx, &msg, args).await,
        Some(BotCommand::Cron(args)) => handler::handle_cron(ctx, &msg, args).await,
        Some(BotCommand::Usage(arg)) => handler::handle_usage(ctx, &msg, arg).await,
        Some(BotCommand::Allow(args)) => {
            super::allowlist_commands::handle_allow(ctx, &msg, args).await
        }
        Some(BotCommand::Deny(args)) => super::allowlist_commands::handle_deny(ctx, &msg, args).await,
        Some(BotCommand::Allowed) => super::allowlist_commands::handle_allowed(ctx, &msg).await,
        Some(BotCommand::AllowAll) => super::allowlist_commands::handle_allow_all(ctx, &msg).await,
        Some(BotCommand::DenyAll) => super::allowlist_commands::handle_deny_all(ctx, &msg).await,
        None => handler::handle_message(ctx, &msg, decision).await,
    };
    if let Err(e) = result {
        tracing::warn!(chat_id = meta.chat_id, "message handler failed: {e}");
    }
}

/// Reproduce dispatch.rs's callback branch: classify by data prefix → matching
/// callback handler. Errors are logged best-effort.
async fn on_callback(ctx: &HandlerCtx, q: frankenstein::types::CallbackQuery) {
    use super::handler;

    let result = match classify_callback(q.data.as_deref()) {
        CallbackRoute::Model => super::model_command::handle_model_callback(ctx, &q).await,
        CallbackRoute::Mode => super::mode_command::handle_mode_callback(ctx, &q).await,
        CallbackRoute::Thinking => handler::handle_thinking_toggle_callback(ctx, &q).await,
        CallbackRoute::Bg => handler::handle_bg_callback(ctx, &q).await,
        CallbackRoute::ErrorDetails => {
            super::error_details::handle_error_details_callback(ctx, &q).await
        }
        CallbackRoute::Stop => handler::handle_stop_callback(ctx, &q).await,
    };
    if let Err(e) = result {
        tracing::warn!(callback_id = %q.id, "callback handler failed: {e}");
    }
}

/// Which callback handler an inline-button `callback_query.data` routes to.
/// Mirrors the `dptree` callback branch order in `dispatch.rs`.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum CallbackRoute {
    Model,
    Mode,
    Thinking,
    Bg,
    ErrorDetails,
    Stop,
}

/// Classify inline-button callback data by prefix. The `Stop` fallthrough
/// matches the prior `.endpoint(handle_stop_callback)` default branch (also
/// covers `None` data).
pub(crate) fn classify_callback(data: Option<&str>) -> CallbackRoute {
    match data {
        Some(d) if d.starts_with("model:") => CallbackRoute::Model,
        Some(d) if d.starts_with("mode:") || d.starts_with("modegroup:") => CallbackRoute::Mode,
        Some(d) if d.starts_with("think:") => CallbackRoute::Thinking,
        Some(d) if d.starts_with("bg:") => CallbackRoute::Bg,
        Some(d) if d.starts_with("errdet:") => CallbackRoute::ErrorDetails,
        _ => CallbackRoute::Stop,
    }
}

/// Outcome of authenticating + parsing an inbound webhook request.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum WebhookOutcome {
    Unauthorized,
    AckIgnore,
    Routed,
}

/// Decide how to handle a webhook POST: a missing or mismatched secret header
/// is `Unauthorized`; a valid secret with an unparseable body is `AckIgnore`
/// (200 to stop Telegram retries); a valid secret with a parseable body is
/// `Routed`.
pub(crate) fn webhook_outcome(
    secret_header: Option<&str>,
    expected_secret: &str,
    body_parses: bool,
) -> WebhookOutcome {
    match secret_header {
        Some(s) if s == expected_secret => {
            if body_parses {
                WebhookOutcome::Routed
            } else {
                WebhookOutcome::AckIgnore
            }
        }
        _ => WebhookOutcome::Unauthorized,
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::*;
    use std::path::PathBuf;

    use dashmap::DashMap;
    use right_agent::agent::allowlist::{AllowlistHandle, AllowlistState};

    use super::super::handler::{
        AgentDir, AgentSettings, IdleTimestamp, InterceptSlots, InternalApi, RightHome,
        SshConfigPath,
    };

    /// Build a `HandlerCtx` with dummy dependencies for handler-free tests
    /// (e.g. the webhook secret-rejection paths, which short-circuit before any
    /// handler runs). The bot is built via the sync `RightBot::new` (no network).
    pub(crate) fn placeholder_ctx() -> HandlerCtx {
        placeholder_ctx_with_allowlist(AllowlistHandle::new(AllowlistState::default()))
    }

    /// Like [`placeholder_ctx`] but with `user_id` in the trusted-users
    /// allowlist, so a DM from that user survives `make_routing_filter` and
    /// reaches `handle_message` (used by the edited-message routing test).
    pub(crate) fn placeholder_ctx_trusting(user_id: i64) -> HandlerCtx {
        use chrono::Utc;
        use right_agent::agent::allowlist::{AllowedUser, AllowlistFile};
        let allowlist = AllowlistHandle::new(AllowlistState::from_file(AllowlistFile {
            version: right_agent::agent::allowlist::CURRENT_VERSION,
            users: vec![AllowedUser {
                id: user_id,
                label: None,
                added_by: None,
                added_at: Utc::now(),
            }],
            groups: vec![],
        }));
        placeholder_ctx_with_allowlist(allowlist)
    }

    fn placeholder_ctx_with_allowlist(allowlist: AllowlistHandle) -> HandlerCtx {
        let settings = Arc::new(AgentSettings {
            show_thinking: false,
            model: Arc::new(arc_swap::ArcSwap::from_pointee(None)),
            resolved_sandbox: None,
            hindsight: None,
            prefetch_cache: None,
            upgrade_lock: Arc::new(tokio::sync::RwLock::new(())),
            debug: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            stt: None,
            learning: right_agent::agent::types::LearningConfig::default(),
            claude_health: crate::keepalive::ClaudeHealth::new(
                "test".to_owned(),
                PathBuf::from("/tmp/router-test"),
                None,
                None,
                None,
                None,
            ),
            shutdown: tokio_util::sync::CancellationToken::new(),
            sandbox_runtime: {
                let (h, _rx) = crate::sandbox_runtime::SandboxRuntimeHandle::new(
                    crate::sandbox_runtime::SandboxHealth::Ready,
                );
                h
            },
        });
        HandlerCtx {
            bot: super::super::bot::build_bot("0:fake_token_for_router_tests".to_owned()),
            allowlist,
            identity: Arc::new(BotIdentity {
                username: "test_bot".to_owned(),
                user_id: 1,
            }),
            worker_map: Arc::new(DashMap::new()),
            agent_dir: Arc::new(AgentDir(PathBuf::from("/tmp/router-test"))),
            home: Arc::new(RightHome(PathBuf::from("/tmp/router-test"))),
            ssh_config: Arc::new(SshConfigPath(None)),
            intercept_slots: Arc::new(InterceptSlots {
                auth_code: Arc::new(tokio::sync::Mutex::new(None)),
                auth_watcher: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            }),
            internal_api: Arc::new(InternalApi(Arc::new(
                right_mcp::internal_client::InternalClient::new("/tmp/router-test.sock"),
            ))),
            settings,
            idle_ts: Arc::new(IdleTimestamp(Arc::new(std::sync::atomic::AtomicI64::new(0)))),
            worker_ctl: super::super::WorkerControlDeps {
                stop_tokens: Arc::new(DashMap::new()),
                session_locks: Arc::new(DashMap::new()),
                bg_requests: Arc::new(DashMap::new()),
                bg_handoff_gates: Arc::new(DashMap::new()),
                thinking_visibility: Arc::new(DashMap::new()),
                progress: super::super::progress::ProgressState::default(),
                compact_timers: Arc::new(DashMap::new()),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_callback_routes_each_prefix() {
        assert_eq!(classify_callback(Some("model:opus")), CallbackRoute::Model);
        assert_eq!(classify_callback(Some("mode:all")), CallbackRoute::Mode);
        assert_eq!(classify_callback(Some("modegroup:x")), CallbackRoute::Mode);
        assert_eq!(classify_callback(Some("think:on")), CallbackRoute::Thinking);
        assert_eq!(classify_callback(Some("bg:123")), CallbackRoute::Bg);
        assert_eq!(
            classify_callback(Some("errdet:1")),
            CallbackRoute::ErrorDetails
        );
    }

    /// Build a JSON update envelope of the given `content_key` ("message" or
    /// "edited_message") carrying a content-less DM from `user_id` — content-less
    /// so a routed `Message` reaches `handle_message`, sets `idle_ts`, and
    /// returns early without spawning a worker.
    #[cfg(test)]
    fn dm_update(content_key: &str, user_id: i64) -> frankenstein::updates::Update {
        serde_json::from_value(serde_json::json!({
            "update_id": 1,
            content_key: {
                "message_id": 7,
                "date": 0,
                "chat": {"id": user_id, "type": "private", "first_name": "U"},
                "from": {"id": user_id, "is_bot": false, "first_name": "U"}
            }
        }))
        .unwrap()
    }

    /// A fresh `Message` from a trusted DM sender reaches `handle_message`, which
    /// stamps `idle_ts` before its empty-content early return.
    #[tokio::test]
    async fn route_update_routes_fresh_message() {
        use std::sync::atomic::Ordering;
        let ctx = test_support::placeholder_ctx_trusting(42);
        assert_eq!(ctx.idle_ts.0.load(Ordering::Relaxed), 0);
        route_update(dm_update("message", 42), &ctx).await;
        assert!(
            ctx.idle_ts.0.load(Ordering::Relaxed) > 0,
            "a fresh Message must be routed to handle_message"
        );
    }

    /// An `EditedMessage` is ignored (former teloxide `filter_message()` had no
    /// edited-message branch): it must NOT reach `handle_message`, so `idle_ts`
    /// stays at its initial 0.
    #[tokio::test]
    async fn route_update_ignores_edited_message() {
        use std::sync::atomic::Ordering;
        let ctx = test_support::placeholder_ctx_trusting(42);
        route_update(dm_update("edited_message", 42), &ctx).await;
        assert_eq!(
            ctx.idle_ts.0.load(Ordering::Relaxed),
            0,
            "an EditedMessage must NOT be routed to a handler"
        );
    }

    #[test]
    fn classify_callback_falls_through_to_stop() {
        assert_eq!(classify_callback(Some("stop:1")), CallbackRoute::Stop);
        assert_eq!(classify_callback(None), CallbackRoute::Stop);
    }

    #[test]
    fn webhook_outcome_unauthorized_without_matching_secret() {
        assert_eq!(
            webhook_outcome(None, "s", true),
            WebhookOutcome::Unauthorized
        );
        assert_eq!(
            webhook_outcome(Some("nope"), "s", true),
            WebhookOutcome::Unauthorized
        );
    }

    #[test]
    fn webhook_outcome_routes_valid_secret_and_body() {
        assert_eq!(
            webhook_outcome(Some("s"), "s", true),
            WebhookOutcome::Routed
        );
    }

    #[test]
    fn webhook_outcome_acks_valid_secret_unparseable_body() {
        assert_eq!(
            webhook_outcome(Some("s"), "s", false),
            WebhookOutcome::AckIgnore
        );
    }
}
