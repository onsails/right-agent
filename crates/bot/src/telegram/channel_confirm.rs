//! Channel registration: bot promoted to channel admin → DM the first trusted
//! user with an "Open channel" confirm button; the confirm callback writes a
//! `kind: channel` entry to allowlist.yaml.

use frankenstein::types::{
    ChatMemberUpdated, ChatType, InlineKeyboardButton, InlineKeyboardMarkup,
};
use right_agent::agent::allowlist::{AddOutcome, AllowedGroup, GroupKind};

use super::router::HandlerCtx;
use super::tg_bot::TgError;

pub(crate) const CHANCONF_PREFIX: &str = "chanconf:";

/// Returns the channel chat id when this update means "the bot just became
/// administrator of a channel". Any other transition → None.
pub(crate) fn channel_admin_promotion(update: &ChatMemberUpdated) -> Option<i64> {
    if update.chat.type_field != ChatType::Channel {
        return None;
    }
    match (&update.old_chat_member, &update.new_chat_member) {
        (
            frankenstein::types::ChatMember::Administrator(_),
            frankenstein::types::ChatMember::Administrator(_),
        ) => None,
        (_, frankenstein::types::ChatMember::Administrator(_)) => Some(update.chat.id),
        _ => None,
    }
}

pub(crate) fn parse_chanconf_data(data: &str) -> Option<i64> {
    data.strip_prefix(CHANCONF_PREFIX)?.parse().ok()
}

fn confirm_keyboard(chat_id: i64) -> InlineKeyboardMarkup {
    InlineKeyboardMarkup::builder()
        .inline_keyboard(vec![vec![
            InlineKeyboardButton::builder()
                .text("\u{2713} Open channel")
                .callback_data(format!("{CHANCONF_PREFIX}{chat_id}"))
                .build(),
        ]])
        .build()
}

/// MyChatMember intake. Notifies the first trusted user; no-op when the
/// transition isn't a channel-admin promotion or nobody is trusted yet.
pub(crate) async fn handle_my_chat_member(
    ctx: &HandlerCtx,
    update: &ChatMemberUpdated,
) -> Result<(), TgError> {
    let Some(chat_id) = channel_admin_promotion(update) else {
        return Ok(());
    };
    let Some(trusted) = ctx
        .allowlist
        .0
        .read()
        .expect("allowlist lock poisoned")
        .users()
        .first()
        .map(|u| u.id)
    else {
        tracing::warn!(
            chat_id,
            "bot added to channel but no trusted users to confirm"
        );
        return Ok(());
    };
    let title = ctx
        .bot
        .get_chat_title(chat_id)
        .await
        .unwrap_or_else(|_| chat_id.to_string());
    let text = format!(
        "I was added as admin to channel <b>{}</b>.\nOpen it for read + post access?",
        super::markdown::html_escape(&title)
    );
    ctx.bot
        .send_message_opts(
            trusted,
            &text,
            true,
            None,
            None,
            Some(confirm_keyboard(chat_id)),
        )
        .await?;
    Ok(())
}

pub(crate) async fn handle_channel_confirm_callback(
    ctx: &HandlerCtx,
    q: &frankenstein::types::CallbackQuery,
) -> Result<(), TgError> {
    let data = q.data.as_deref().unwrap_or_default();
    let Some(chat_id) = parse_chanconf_data(data) else {
        ctx.bot
            .answer_callback(&q.id, Some("stale button"), false)
            .await?;
        return Ok(());
    };

    if !ctx
        .allowlist
        .0
        .read()
        .expect("allowlist lock poisoned")
        .is_user_trusted(q.from.id as i64)
    {
        ctx.bot
            .answer_callback(&q.id, Some("not allowed"), true)
            .await?;
        return Ok(());
    }

    let title = ctx.bot.get_chat_title(chat_id).await.ok();
    let opened_by = q.from.id as i64;
    let outcome = match super::allowlist_commands::update_locked(
        &ctx.allowlist,
        &ctx.agent_dir.0,
        move |next| {
            next.add_group(AllowedGroup {
                id: chat_id,
                label: title,
                opened_by: Some(opened_by),
                opened_at: chrono::Utc::now(),
                mode: Default::default(),
                topics: Vec::new(),
                kind: GroupKind::Channel,
            })
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
        AddOutcome::Inserted => {
            ctx.bot
                .answer_callback(&q.id, Some("channel opened"), false)
                .await?;
        }
        AddOutcome::AlreadyPresent => {
            ctx.bot
                .answer_callback(&q.id, Some("already opened"), false)
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
                "can_invite_users": true
            }),
            "member" => serde_json::json!({
                "status": "member",
                "user": bot_user()
            }),
            "left" => serde_json::json!({
                "status": "left",
                "user": bot_user()
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
    fn parse_chanconf_data_extracts_chat_id() {
        assert_eq!(parse_chanconf_data("chanconf:-100123"), Some(-100123));
        assert_eq!(parse_chanconf_data("chanconf:abc"), None);
        assert_eq!(parse_chanconf_data("stop:1:2"), None);
    }
}
