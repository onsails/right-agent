//! `/mode` (current topic) and `/mode_group` (whole group) — inline-keyboard
//! toggles for the per-scope response mode. Trusted-only, group-only.
//! Mirrors `model_command.rs`.

use std::sync::Arc;

use right_agent::agent::allowlist::{AllowlistHandle, AllowlistState, ResponseMode};
use teloxide::prelude::*;
use teloxide::types::{
    InlineKeyboardButton, InlineKeyboardMarkup, MaybeInaccessibleMessage, Message,
};

use super::BotType;
use super::handler::AgentDir;

/// Which scope a `/mode*` interaction targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ModeScope {
    Topic,
    Group,
}

/// A parsed callback: scope + requested change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ModeAction {
    Set(ResponseMode),
    /// Topic-only: clear the override, inherit the group default.
    ClearTopic,
}

/// Parse callback data. Topic prefix `mode:`, group prefix `modegroup:`.
/// Returns `None` for anything unrecognised.
pub(crate) fn parse_callback(data: &str) -> Option<(ModeScope, ModeAction)> {
    if let Some(rest) = data.strip_prefix("modegroup:") {
        let action = match rest {
            "addressed" => ModeAction::Set(ResponseMode::Addressed),
            "all" => ModeAction::Set(ResponseMode::All),
            _ => return None,
        };
        return Some((ModeScope::Group, action));
    }
    if let Some(rest) = data.strip_prefix("mode:") {
        let action = match rest {
            "addressed" => ModeAction::Set(ResponseMode::Addressed),
            "all" => ModeAction::Set(ResponseMode::All),
            "clear" => ModeAction::ClearTopic,
            _ => return None,
        };
        return Some((ModeScope::Topic, action));
    }
    None
}

fn mode_label(m: ResponseMode) -> &'static str {
    match m {
        ResponseMode::Addressed => "Addressed",
        ResponseMode::All => "All",
    }
}

/// Topic keyboard: [Addressed][All] then [Inherit group]. `✓` marks the
/// effective mode; the Inherit button is shown only when an override exists.
pub(crate) fn topic_keyboard(effective: ResponseMode, has_override: bool) -> InlineKeyboardMarkup {
    let btn = |m: ResponseMode, data: &str| {
        let label = if effective == m && has_override {
            format!("✓ {}", mode_label(m))
        } else {
            mode_label(m).to_string()
        };
        InlineKeyboardButton::callback(label, format!("mode:{data}"))
    };
    let mut rows = vec![vec![
        btn(ResponseMode::Addressed, "addressed"),
        btn(ResponseMode::All, "all"),
    ]];
    if has_override {
        rows.push(vec![InlineKeyboardButton::callback(
            "↩︎ Inherit group",
            "mode:clear",
        )]);
    }
    InlineKeyboardMarkup::new(rows)
}

/// Group keyboard: [Addressed][All], `✓` on the active default.
pub(crate) fn group_keyboard(current: ResponseMode) -> InlineKeyboardMarkup {
    let btn = |m: ResponseMode, data: &str| {
        let label = if current == m {
            format!("✓ {}", mode_label(m))
        } else {
            mode_label(m).to_string()
        };
        InlineKeyboardButton::callback(label, format!("modegroup:{data}"))
    };
    InlineKeyboardMarkup::new(vec![vec![
        btn(ResponseMode::Addressed, "addressed"),
        btn(ResponseMode::All, "all"),
    ]])
}

fn topic_body(effective: ResponseMode, has_override: bool) -> String {
    let src = if has_override {
        "topic override"
    } else {
        "inherited from group"
    };
    format!(
        "💬 Response mode — this topic\n\nCurrent: {} ({src})\n\nAddressed — reply only when @mentioned, replied-to, or commanded.\nAll — reply to every message from anyone here.",
        mode_label(effective)
    )
}

fn group_body(current: ResponseMode) -> String {
    format!(
        "💬 Response mode — whole group (default for topics without their own)\n\nCurrent: {}\n\nAddressed — reply only when addressed.\nAll — reply to every message from anyone.",
        mode_label(current)
    )
}

fn group_mode(state: &AllowlistState, chat_id: i64) -> Option<ResponseMode> {
    state
        .groups()
        .iter()
        .find(|g| g.id == chat_id)
        .map(|g| g.mode)
}

fn topic_mode_state(
    state: &AllowlistState,
    chat_id: i64,
    thread_id: i64,
) -> Option<(ResponseMode, bool)> {
    let group = state.groups().iter().find(|g| g.id == chat_id)?;
    let topic_mode = group
        .topics
        .iter()
        .find(|topic| topic.thread_id == thread_id)
        .map(|topic| topic.mode);
    Some((topic_mode.unwrap_or(group.mode), topic_mode.is_some()))
}

fn apply_mode_action(
    state: &mut AllowlistState,
    chat_id: i64,
    thread_id: i64,
    scope: ModeScope,
    action: ModeAction,
) -> bool {
    match (scope, action) {
        (ModeScope::Group, ModeAction::Set(mode)) => state.set_group_mode(chat_id, mode),
        (ModeScope::Topic, ModeAction::Set(mode)) => state.set_topic_mode(chat_id, thread_id, mode),
        (ModeScope::Topic, ModeAction::ClearTopic) => {
            if !state.is_group_open(chat_id) {
                return false;
            }
            let _removed = state.clear_topic_mode(chat_id, thread_id);
            true
        }
        (ModeScope::Group, ModeAction::ClearTopic) => false,
    }
}

fn render_scope_state(
    state: &AllowlistState,
    chat_id: i64,
    thread_id: i64,
    scope: ModeScope,
) -> Option<(String, InlineKeyboardMarkup)> {
    match scope {
        ModeScope::Topic => {
            topic_mode_state(state, chat_id, thread_id).map(|(effective, has_override)| {
                (
                    topic_body(effective, has_override),
                    topic_keyboard(effective, has_override),
                )
            })
        }
        ModeScope::Group => {
            group_mode(state, chat_id).map(|current| (group_body(current), group_keyboard(current)))
        }
    }
}

fn callback_target_from_regular_message(
    scope: ModeScope,
    message: Option<&Message>,
    group_chat_id: i64,
) -> Result<(i64, i64), &'static str> {
    match scope {
        ModeScope::Group => Ok((group_chat_id, 0)),
        ModeScope::Topic => {
            let message = message.ok_or("Mode menu unavailable")?;
            Ok((
                message.chat.id.0,
                super::session::effective_thread_id(message),
            ))
        }
    }
}

fn callback_target(
    scope: ModeScope,
    message: &MaybeInaccessibleMessage,
) -> Result<(i64, i64), &'static str> {
    match scope {
        ModeScope::Group => callback_target_from_regular_message(
            scope,
            message.regular_message(),
            message.chat().id.0,
        ),
        ModeScope::Topic => {
            callback_target_from_regular_message(scope, message.regular_message(), 0)
        }
    }
}

async fn send_in_thread(
    bot: &BotType,
    msg: &Message,
    text: impl Into<String>,
    reply_markup: Option<InlineKeyboardMarkup>,
) -> ResponseResult<()> {
    let mut send = bot.send_message(msg.chat.id, text);
    if let Some(reply_markup) = reply_markup {
        send = send.reply_markup(reply_markup);
    }
    if let Some(thread_id) = msg.thread_id {
        send = send.message_thread_id(thread_id);
    }
    send.await?;
    Ok(())
}

/// Open the current-topic `/mode` menu.
pub(crate) async fn handle_mode(
    bot: BotType,
    msg: Message,
    allowlist: AllowlistHandle,
) -> ResponseResult<()> {
    if super::handler::is_private_chat(&msg.chat.kind) {
        send_in_thread(&bot, &msg, "/mode is only valid in group chats", None).await?;
        return Ok(());
    }

    if !super::allowlist_commands::sender_is_trusted(&msg, &allowlist) {
        tracing::debug!(
            chat_id = msg.chat.id.0,
            user_id = msg.from.as_ref().map(|u| u.id.0),
            "/mode ignored: non-trusted sender in group"
        );
        return Ok(());
    }

    let chat_id = msg.chat.id.0;
    let thread_id = super::session::effective_thread_id(&msg);
    let Some((effective, has_override)) = ({
        let state = allowlist.0.read().expect("allowlist lock poisoned");
        topic_mode_state(&state, chat_id, thread_id)
    }) else {
        send_in_thread(
            &bot,
            &msg,
            "Open the group first with /allow_all, then set a mode",
            None,
        )
        .await?;
        return Ok(());
    };

    send_in_thread(
        &bot,
        &msg,
        topic_body(effective, has_override),
        Some(topic_keyboard(effective, has_override)),
    )
    .await?;
    Ok(())
}

/// Open the whole-group `/mode_group` menu.
pub(crate) async fn handle_mode_group(
    bot: BotType,
    msg: Message,
    allowlist: AllowlistHandle,
) -> ResponseResult<()> {
    if super::handler::is_private_chat(&msg.chat.kind) {
        send_in_thread(&bot, &msg, "/mode_group is only valid in group chats", None).await?;
        return Ok(());
    }

    if !super::allowlist_commands::sender_is_trusted(&msg, &allowlist) {
        tracing::debug!(
            chat_id = msg.chat.id.0,
            user_id = msg.from.as_ref().map(|u| u.id.0),
            "/mode_group ignored: non-trusted sender in group"
        );
        return Ok(());
    }

    let chat_id = msg.chat.id.0;
    let Some(current) = ({
        let state = allowlist.0.read().expect("allowlist lock poisoned");
        group_mode(&state, chat_id)
    }) else {
        send_in_thread(
            &bot,
            &msg,
            "Open the group first with /allow_all, then set a mode",
            None,
        )
        .await?;
        return Ok(());
    };

    send_in_thread(
        &bot,
        &msg,
        group_body(current),
        Some(group_keyboard(current)),
    )
    .await?;
    Ok(())
}

/// Handle a click on a `/mode` or `/mode_group` keyboard button.
pub(crate) async fn handle_mode_callback(
    bot: BotType,
    q: teloxide::types::CallbackQuery,
    agent_dir: Arc<AgentDir>,
    allowlist: AllowlistHandle,
) -> ResponseResult<()> {
    let Some(data) = q.data.as_deref() else {
        bot.answer_callback_query(q.id).await?;
        return Ok(());
    };
    let Some((scope, action)) = parse_callback(data) else {
        bot.answer_callback_query(q.id).await?;
        return Ok(());
    };

    let user_id = q.from.id.0 as i64;
    let trusted = allowlist
        .0
        .read()
        .expect("allowlist lock poisoned")
        .is_user_trusted(user_id);
    if !trusted {
        bot.answer_callback_query(q.id).text("Not allowed").await?;
        return Ok(());
    }

    let Some(message) = q.message.as_ref() else {
        bot.answer_callback_query(q.id)
            .text("Mode menu unavailable")
            .await?;
        return Ok(());
    };
    let (chat_id, thread_id) = match callback_target(scope, message) {
        Ok(target) => target,
        Err(text) => {
            bot.answer_callback_query(q.id).text(text).await?;
            return Ok(());
        }
    };

    let updated =
        match super::allowlist_commands::update_locked(&allowlist, &agent_dir.0, move |state| {
            apply_mode_action(state, chat_id, thread_id, scope, action)
        })
        .await
        {
            Ok(updated) => updated,
            Err(e) => {
                tracing::error!(error = %e, "/mode: failed to persist allowlist");
                bot.answer_callback_query(q.id)
                    .text("Failed to save — see bot logs")
                    .await?;
                return Ok(());
            }
        };

    if !updated {
        bot.answer_callback_query(q.id)
            .text("Open the group first with /allow_all, then set a mode")
            .await?;
        return Ok(());
    }

    let rendered = {
        let state = allowlist.0.read().expect("allowlist lock poisoned");
        render_scope_state(&state, chat_id, thread_id, scope)
    };

    let Some((body, keyboard)) = rendered else {
        bot.answer_callback_query(q.id)
            .text("Open the group first with /allow_all, then set a mode")
            .await?;
        return Ok(());
    };

    let edit = bot
        .edit_message_text(message.chat().id, message.id(), body)
        .reply_markup(keyboard);
    let toast = bot.answer_callback_query(q.id).text("Mode updated");
    let (edit_result, toast_result) = tokio::join!(edit.send(), toast.send());
    if let Err(e) = edit_result {
        tracing::warn!(error = %e, "failed to edit /mode menu after update");
    }
    toast_result?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use right_agent::agent::allowlist::{AllowedGroup, AllowlistFile};
    use teloxide::types::InlineKeyboardButtonKind;

    fn opened_state(chat_id: i64, mode: ResponseMode) -> AllowlistState {
        AllowlistState::from_file(AllowlistFile {
            groups: vec![AllowedGroup {
                id: chat_id,
                label: Some("group".to_string()),
                opened_by: Some(1),
                opened_at: Utc::now(),
                mode,
                topics: Vec::new(),
            }],
            ..Default::default()
        })
    }

    fn topic_message(chat_id: i64, thread_id: i32) -> Message {
        serde_json::from_value(serde_json::json!({
            "message_id": 10,
            "date": 0,
            "chat": {"id": chat_id, "type": "supergroup", "title": "Group"},
            "message_thread_id": thread_id,
            "is_topic_message": true,
            "from": {"id": 1, "is_bot": false, "first_name": "User"},
            "text": "/mode"
        }))
        .unwrap()
    }

    fn callback_data(keyboard: &InlineKeyboardMarkup) -> Vec<String> {
        keyboard
            .inline_keyboard
            .iter()
            .flatten()
            .filter_map(|button| match &button.kind {
                InlineKeyboardButtonKind::CallbackData(data) => Some(data.clone()),
                _ => None,
            })
            .collect()
    }

    fn labels(keyboard: &InlineKeyboardMarkup) -> Vec<String> {
        keyboard
            .inline_keyboard
            .iter()
            .flatten()
            .map(|button| button.text.clone())
            .collect()
    }

    #[tokio::test]
    async fn parse_topic_set_callbacks() {
        assert_eq!(
            parse_callback("mode:addressed"),
            Some((ModeScope::Topic, ModeAction::Set(ResponseMode::Addressed)))
        );
        assert_eq!(
            parse_callback("mode:all"),
            Some((ModeScope::Topic, ModeAction::Set(ResponseMode::All)))
        );
    }

    #[tokio::test]
    async fn parse_topic_clear_callback() {
        assert_eq!(
            parse_callback("mode:clear"),
            Some((ModeScope::Topic, ModeAction::ClearTopic))
        );
    }

    #[tokio::test]
    async fn parse_group_set_callbacks() {
        assert_eq!(
            parse_callback("modegroup:addressed"),
            Some((ModeScope::Group, ModeAction::Set(ResponseMode::Addressed)))
        );
        assert_eq!(
            parse_callback("modegroup:all"),
            Some((ModeScope::Group, ModeAction::Set(ResponseMode::All)))
        );
    }

    #[tokio::test]
    async fn parse_rejects_unknown_callbacks() {
        assert_eq!(parse_callback("model:all"), None);
        assert_eq!(parse_callback("modegroup:clear"), None);
        assert_eq!(parse_callback("mode:nonsense"), None);
    }

    #[tokio::test]
    async fn apply_mode_action_mutates_only_open_scopes() {
        let chat_id = -100;
        let thread_id = 42;
        let mut state = opened_state(chat_id, ResponseMode::Addressed);

        assert!(apply_mode_action(
            &mut state,
            chat_id,
            thread_id,
            ModeScope::Group,
            ModeAction::Set(ResponseMode::All)
        ));
        assert_eq!(group_mode(&state, chat_id), Some(ResponseMode::All));

        assert!(apply_mode_action(
            &mut state,
            chat_id,
            thread_id,
            ModeScope::Topic,
            ModeAction::Set(ResponseMode::Addressed)
        ));
        assert_eq!(
            topic_mode_state(&state, chat_id, thread_id),
            Some((ResponseMode::Addressed, true))
        );

        assert!(apply_mode_action(
            &mut state,
            chat_id,
            thread_id,
            ModeScope::Topic,
            ModeAction::ClearTopic
        ));
        assert_eq!(
            topic_mode_state(&state, chat_id, thread_id),
            Some((ResponseMode::All, false))
        );

        let mut closed = AllowlistState::default();
        assert!(!apply_mode_action(
            &mut closed,
            chat_id,
            thread_id,
            ModeScope::Group,
            ModeAction::Set(ResponseMode::All)
        ));
    }

    #[tokio::test]
    async fn callback_target_requires_accessible_message_for_topic_scope() {
        let msg = topic_message(-100, 42);

        assert_eq!(
            callback_target_from_regular_message(ModeScope::Topic, Some(&msg), -100),
            Ok((-100, 42))
        );
        assert_eq!(
            callback_target_from_regular_message(ModeScope::Group, None, -100),
            Ok((-100, 0))
        );
        assert_eq!(
            callback_target_from_regular_message(ModeScope::Topic, None, -100),
            Err("Mode menu unavailable")
        );
    }

    #[tokio::test]
    async fn topic_keyboard_without_override_has_two_unchecked_buttons() {
        let keyboard = topic_keyboard(ResponseMode::All, false);

        assert_eq!(keyboard.inline_keyboard.len(), 1);
        assert_eq!(callback_data(&keyboard), vec!["mode:addressed", "mode:all"]);
        assert_eq!(labels(&keyboard), vec!["Addressed", "All"]);
    }

    #[tokio::test]
    async fn topic_keyboard_with_override_marks_effective_and_can_clear() {
        let keyboard = topic_keyboard(ResponseMode::All, true);

        assert_eq!(keyboard.inline_keyboard.len(), 2);
        assert_eq!(
            callback_data(&keyboard),
            vec!["mode:addressed", "mode:all", "mode:clear"]
        );
        assert_eq!(
            labels(&keyboard),
            vec!["Addressed", "✓ All", "↩︎ Inherit group"]
        );
    }

    #[tokio::test]
    async fn group_keyboard_marks_current_default() {
        let keyboard = group_keyboard(ResponseMode::Addressed);

        assert_eq!(keyboard.inline_keyboard.len(), 1);
        assert_eq!(
            callback_data(&keyboard),
            vec!["modegroup:addressed", "modegroup:all"]
        );
        assert_eq!(labels(&keyboard), vec!["✓ Addressed", "All"]);
    }

    #[tokio::test]
    async fn bodies_describe_scope_and_current_mode() {
        let topic = topic_body(ResponseMode::Addressed, false);
        assert!(topic.contains("this topic"));
        assert!(topic.contains("Current: Addressed (inherited from group)"));

        let group = group_body(ResponseMode::All);
        assert!(group.contains("whole group"));
        assert!(group.contains("Current: All"));
    }
}
