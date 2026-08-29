//! Channel registration: bot promoted to channel admin → DM the first trusted
//! user with a single-use "Open channel" confirm button; the confirm callback
//! writes a `kind: channel` entry to allowlist.yaml. Bot demotion/removal
//! revokes the entry and drops any pending confirmation.

use std::sync::Arc;
use std::time::{Duration, Instant};

use dashmap::DashMap;
use frankenstein::types::{
    ChatMember, ChatMemberUpdated, ChatType, InlineKeyboardButton, InlineKeyboardMarkup,
};
use right_agent::agent::allowlist::{AddOutcome, AllowedGroup, GroupKind, RemoveOutcome};

use super::router::HandlerCtx;
use super::tg_bot::TgError;

pub(crate) const CHANCONF_PREFIX: &str = "chanconf:";

/// Pending confirmations expire after this long unconsumed. A fresh promotion
/// always replaces the pending entry (new nonce), so expiry only retires
/// buttons the user never saw or ignored.
const PENDING_CONFIRM_TTL: Duration = Duration::from_secs(24 * 60 * 60);

/// A pending channel-open confirmation: single-use, bound to the trusted user
/// the DM was sent to, and expiring after [`PENDING_CONFIRM_TTL`].
#[derive(Debug, Clone)]
pub(crate) struct PendingChannelConfirm {
    nonce: String,
    user_id: i64,
    /// Message id of the DM carrying the confirm button (for keyboard removal).
    message_id: i32,
    created_at: Instant,
}

/// In-memory pending confirmations keyed by channel chat id. Entries are lost
/// on bot restart — the stale button then answers "stale button" and the next
/// promotion re-issues a fresh DM.
#[derive(Debug, Clone, Default)]
pub(crate) struct ChannelConfirms {
    inner: Arc<DashMap<i64, PendingChannelConfirm>>,
}

impl ChannelConfirms {
    fn insert(&self, chat_id: i64, pending: PendingChannelConfirm) {
        self.inner.insert(chat_id, pending);
    }

    fn remove(&self, chat_id: i64) {
        self.inner.remove(&chat_id);
    }

    /// The pending entry for `chat_id` when its nonce matches and it has not
    /// expired. Expired entries are dropped as a side effect.
    fn get_valid(&self, chat_id: i64, nonce: &str) -> Option<PendingChannelConfirm> {
        let entry = self.inner.get(&chat_id)?;
        let pending = entry.value().clone();
        drop(entry);
        if pending.created_at.elapsed() > PENDING_CONFIRM_TTL {
            self.inner.remove(&chat_id);
            return None;
        }
        (pending.nonce == nonce).then_some(pending)
    }
}

/// Returns the channel chat id when this update means "the bot just became
/// administrator of a channel". Any other transition → None.
pub(crate) fn channel_admin_promotion(update: &ChatMemberUpdated) -> Option<i64> {
    if update.chat.type_field != ChatType::Channel {
        return None;
    }
    match (&update.old_chat_member, &update.new_chat_member) {
        (ChatMember::Administrator(_), ChatMember::Administrator(_)) => None,
        (_, ChatMember::Administrator(_)) => Some(update.chat.id),
        _ => None,
    }
}

/// Returns the channel chat id when this update means "the bot just lost
/// channel administrator status" (was admin, now anything else). Any other
/// transition → None.
pub(crate) fn channel_admin_demotion(update: &ChatMemberUpdated) -> Option<i64> {
    if update.chat.type_field != ChatType::Channel {
        return None;
    }
    match (&update.old_chat_member, &update.new_chat_member) {
        (ChatMember::Administrator(_), ChatMember::Administrator(_)) => None,
        (ChatMember::Administrator(_), _) => Some(update.chat.id),
        _ => None,
    }
}

/// Parses a channel-confirm callback payload into `(chat_id, nonce)`, returning
/// `None` when it is malformed or lacks the channel-confirm prefix.
pub(crate) fn parse_chanconf_data(data: &str) -> Option<(i64, String)> {
    let rest = data.strip_prefix(CHANCONF_PREFIX)?;
    let (id, nonce) = rest.split_once(':')?;
    Some((id.parse().ok()?, nonce.to_owned()))
}

fn confirm_keyboard(chat_id: i64, nonce: &str) -> InlineKeyboardMarkup {
    InlineKeyboardMarkup::builder()
        .inline_keyboard(vec![vec![
            InlineKeyboardButton::builder()
                .text("\u{2713} Open channel")
                .callback_data(format!("{CHANCONF_PREFIX}{chat_id}:{nonce}"))
                .build(),
        ]])
        .build()
}

/// MyChatMember intake. Promotions DM the first trusted user a single-use
/// confirm button; demotions revoke the channel entry and any pending
/// confirmation. No-op for non-channel transitions.
pub(crate) async fn handle_my_chat_member(
    ctx: &HandlerCtx,
    update: &ChatMemberUpdated,
) -> Result<(), TgError> {
    if let Some(chat_id) = channel_admin_demotion(update) {
        return handle_channel_demotion(ctx, chat_id).await;
    }
    let Some(chat_id) = channel_admin_promotion(update) else {
        return Ok(());
    };
    let trusted = {
        let allowlist = ctx.allowlist.0.read().expect("allowlist lock poisoned");
        if allowlist.is_channel_open(chat_id) {
            return Ok(());
        }
        allowlist.users().first().map(|u| u.id)
    };
    let Some(trusted) = trusted else {
        tracing::warn!(
            chat_id,
            "bot added to channel but no trusted users to confirm"
        );
        return Ok(());
    };
    // Fail-fast: a Telegram error propagates so the webhook fails and Telegram
    // retries the one-shot promotion update.
    let title = ctx.bot.get_chat_title(chat_id).await?;
    let nonce = uuid::Uuid::new_v4().simple().to_string();
    let text = format!(
        "I was added as admin to channel <b>{}</b>.\nOpen it for read + post access?",
        super::markdown::html_escape(&title)
    );
    let sent = ctx
        .bot
        .send_message_opts(
            trusted,
            &text,
            true,
            None,
            None,
            Some(confirm_keyboard(chat_id, &nonce)),
        )
        .await?;
    ctx.channel_confirms.insert(
        chat_id,
        PendingChannelConfirm {
            nonce,
            user_id: trusted,
            message_id: sent.message_id,
            created_at: Instant::now(),
        },
    );
    Ok(())
}

/// Bot lost channel admin: drop the pending confirmation and revoke the
/// `kind: channel` allowlist entry so a later re-add requires fresh approval.
async fn handle_channel_demotion(ctx: &HandlerCtx, chat_id: i64) -> Result<(), TgError> {
    ctx.channel_confirms.remove(chat_id);
    let outcome =
        super::allowlist_commands::update_locked(&ctx.allowlist, &ctx.agent_dir.0, move |next| {
            if next.is_channel_open(chat_id) {
                next.remove_group(chat_id)
            } else {
                RemoveOutcome::NotFound
            }
        })
        .await
        .map_err(TgError::Other)?;
    if outcome == RemoveOutcome::Removed {
        tracing::info!(
            chat_id,
            "channel allowlist entry revoked after bot lost admin status"
        );
    }
    Ok(())
}

/// Result of the locked allowlist mutation in the confirm callback.
enum ConfirmOutcome {
    Inserted,
    AlreadyPresent,
    /// The clicking user is not (or no longer) trusted.
    NotTrusted,
}

/// Confirms a channel-opening request from a trusted user.
///
/// The callback must carry a live pending nonce bound to the clicking user;
/// trust is rechecked against the locked on-disk allowlist; the bot's admin
/// status is re-verified before the write; consumed buttons are retired.
pub(crate) async fn handle_channel_confirm_callback(
    ctx: &HandlerCtx,
    q: &frankenstein::types::CallbackQuery,
) -> Result<(), TgError> {
    let data = q.data.as_deref().unwrap_or_default();
    let Some((chat_id, nonce)) = parse_chanconf_data(data) else {
        ctx.bot
            .answer_callback(&q.id, Some("stale button"), false)
            .await?;
        return Ok(());
    };
    let Some(pending) = ctx.channel_confirms.get_valid(chat_id, &nonce) else {
        ctx.bot
            .answer_callback(&q.id, Some("stale button"), false)
            .await?;
        return Ok(());
    };
    let clicker = q.from.id as i64;
    if clicker != pending.user_id {
        ctx.bot
            .answer_callback(&q.id, Some("not allowed"), true)
            .await?;
        return Ok(());
    }

    // Re-verify the bot is still a channel admin before committing anything.
    let still_admin = match ctx.bot.get_chat_member(chat_id, ctx.bot.me().id).await {
        Ok(member) => matches!(member, ChatMember::Administrator(_)),
        Err(e) => {
            ctx.bot
                .answer_callback(&q.id, Some("verification failed, try again"), true)
                .await
                .map_err(|answer_err| {
                    TgError::Other(format!(
                        "get_chat_member failed: {e}; callback answer failed: {answer_err}"
                    ))
                })?;
            return Err(e);
        }
    };
    if !still_admin {
        ctx.channel_confirms.remove(chat_id);
        ctx.bot
            .answer_callback(&q.id, Some("bot is no longer a channel admin"), true)
            .await?;
        return Ok(());
    }
    let title = match ctx.bot.get_chat_title(chat_id).await {
        Ok(title) => title,
        Err(e) => {
            ctx.bot
                .answer_callback(&q.id, Some("verification failed, try again"), true)
                .await
                .map_err(|answer_err| {
                    TgError::Other(format!(
                        "get_chat title lookup failed: {e}; callback answer failed: {answer_err}"
                    ))
                })?;
            return Err(e);
        }
    };

    let outcome = match super::allowlist_commands::update_locked(
        &ctx.allowlist,
        &ctx.agent_dir.0,
        move |next| {
            // Recheck trust against the same locked snapshot being written:
            // the clicker may have been revoked while we awaited Telegram.
            if !next.is_user_trusted(clicker) {
                return ConfirmOutcome::NotTrusted;
            }
            match next.add_group(AllowedGroup {
                id: chat_id,
                label: Some(title),
                opened_by: Some(clicker),
                opened_at: chrono::Utc::now(),
                mode: Default::default(),
                topics: Vec::new(),
                kind: GroupKind::Channel,
            }) {
                AddOutcome::Inserted => ConfirmOutcome::Inserted,
                AddOutcome::AlreadyPresent => ConfirmOutcome::AlreadyPresent,
            }
        },
    )
    .await
    {
        Ok(outcome) => outcome,
        Err(e) => {
            ctx.bot
                .answer_callback(&q.id, Some("save failed, try again"), true)
                .await
                .map_err(|answer_err| {
                    TgError::Other(format!(
                        "allowlist persist failed: {e}; callback answer failed: {answer_err}"
                    ))
                })?;
            return Err(TgError::Other(e));
        }
    };

    match outcome {
        ConfirmOutcome::Inserted | ConfirmOutcome::AlreadyPresent => {
            // The button's job is done either way: consume the pending entry,
            // ack, and retire the keyboard.
            ctx.channel_confirms.remove(chat_id);
            let ack = match outcome {
                ConfirmOutcome::Inserted => "channel opened",
                _ => "already opened",
            };
            ctx.bot.answer_callback(&q.id, Some(ack), false).await?;
            ctx.bot
                .remove_reply_keyboard(pending.user_id, pending.message_id)
                .await?;
        }
        ConfirmOutcome::NotTrusted => {
            ctx.channel_confirms.remove(chat_id);
            ctx.bot
                .answer_callback(&q.id, Some("not allowed"), true)
                .await?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chat_member_update(
        chat_type: &str,
        new_status: &str,
    ) -> frankenstein::types::ChatMemberUpdated {
        chat_member_update_with_old(chat_type, "left", new_status)
    }

    fn chat_member_update_with_old(
        chat_type: &str,
        old_status: &str,
        new_status: &str,
    ) -> frankenstein::types::ChatMemberUpdated {
        let bot_user = || {
            serde_json::json!({
                "id": 1,
                "is_bot": true,
                "first_name": "Bot"
            })
        };
        let chat_member = |status| match status {
            "administrator" => serde_json::json!({
                "status": "administrator",
                "user": bot_user(),
                "can_be_edited": false,
                "is_anonymous": false,
                "can_manage_chat": true,
                "can_delete_messages": true,
                "can_manage_video_chats": true,
                "can_restrict_members": true,
                "can_promote_members": false,
                "can_change_info": true,
                "can_invite_users": true,
                "can_send_welcome_messages": false
            }),
            "member" => serde_json::json!({
                "status": "member",
                "user": bot_user()
            }),
            "left" => serde_json::json!({
                "status": "left",
                "user": bot_user()
            }),
            "kicked" => serde_json::json!({
                "status": "kicked",
                "user": bot_user(),
                "until_date": 0
            }),
            _ => panic!("unsupported status fixture"),
        };
        serde_json::from_value(serde_json::json!({
            "chat": {"id": -100123, "type": chat_type},
            "from": {"id": 2, "is_bot": false, "first_name": "Owner"},
            "date": 0,
            "old_chat_member": chat_member(old_status),
            "new_chat_member": chat_member(new_status)
        }))
        .unwrap()
    }

    #[test]
    fn promotion_detection_requires_channel_and_new_admin() {
        assert_eq!(
            channel_admin_promotion(&chat_member_update("channel", "administrator")),
            Some(-100123)
        );
        assert_eq!(
            channel_admin_promotion(&chat_member_update("group", "member")),
            None
        );
        assert_eq!(
            channel_admin_promotion(&chat_member_update("channel", "left")),
            None
        );
        assert_eq!(
            channel_admin_promotion(&chat_member_update("supergroup", "administrator")),
            None
        );
    }

    #[test]
    fn promotion_detection_ignores_existing_administrator() {
        assert_eq!(
            channel_admin_promotion(&chat_member_update_with_old(
                "channel",
                "administrator",
                "administrator"
            )),
            None
        );
    }

    #[test]
    fn demotion_detection_requires_channel_and_old_admin() {
        for new_status in ["member", "left", "kicked"] {
            assert_eq!(
                channel_admin_demotion(&chat_member_update_with_old(
                    "channel",
                    "administrator",
                    new_status
                )),
                Some(-100123),
                "administrator -> {new_status} must be a demotion"
            );
        }
        assert_eq!(
            channel_admin_demotion(&chat_member_update_with_old(
                "channel",
                "administrator",
                "administrator"
            )),
            None
        );
        assert_eq!(
            channel_admin_demotion(&chat_member_update("channel", "administrator")),
            None
        );
        assert_eq!(
            channel_admin_demotion(&chat_member_update_with_old(
                "supergroup",
                "administrator",
                "left"
            )),
            None
        );
    }

    #[test]
    fn parse_chanconf_data_extracts_chat_id_and_nonce() {
        assert_eq!(
            parse_chanconf_data("chanconf:-100123:abc123"),
            Some((-100123, "abc123".to_owned()))
        );
        assert_eq!(parse_chanconf_data("chanconf:-100123"), None);
        assert_eq!(parse_chanconf_data("chanconf:abc:def"), None);
        assert_eq!(parse_chanconf_data("stop:1:2"), None);
    }

    #[test]
    fn pending_confirm_validates_nonce_and_expiry() {
        let confirms = ChannelConfirms::default();
        confirms.insert(
            -100,
            PendingChannelConfirm {
                nonce: "n1".to_owned(),
                user_id: 42,
                message_id: 7,
                created_at: Instant::now(),
            },
        );
        assert!(confirms.get_valid(-100, "n1").is_some());
        assert!(confirms.get_valid(-100, "wrong").is_none());
        assert!(confirms.get_valid(-999, "n1").is_none());

        confirms.insert(
            -200,
            PendingChannelConfirm {
                nonce: "n2".to_owned(),
                user_id: 42,
                message_id: 8,
                created_at: Instant::now() - PENDING_CONFIRM_TTL - Duration::from_secs(1),
            },
        );
        assert!(
            confirms.get_valid(-200, "n2").is_none(),
            "expired pending must not validate"
        );
        assert!(
            confirms.get_valid(-200, "n2").is_none(),
            "expired entry must be dropped"
        );
    }
}
