//! `/model` command — inline-keyboard menu for switching the agent's Claude model.
//!
//! UI: 4 curated options (Default / Sonnet / Sonnet 1M / Haiku) matching the
//! Claude Code CLI `/model` picker.
//!
//! Persistence: writes `agent.yaml::model` via `right_agent::agent::types::write_agent_yaml_model`.
//! In-memory: stores into `AgentSettings.model: Arc<ArcSwap<Option<String>>>`.
//! Group chats are gated by the trusted-users allowlist (same gate as `/allow`).

/// One row in the curated model menu.
///
/// `model_id == None` represents the "Default" option — no `--model`
/// flag, CC chooses its own default. All other rows pin a specific
/// model via the exact model-ID string CC accepts on the command line.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ModelChoice {
    /// Short alias used in callback_data. Combined with the `model:` prefix
    /// (6 bytes) the total stays under Telegram's 64-byte limit.
    pub alias: &'static str,
    /// Button label (also row label in the body text).
    pub label: &'static str,
    /// Value written to `agent.yaml::model`. `None` = field absent.
    pub model_id: Option<&'static str>,
    /// One-line description shown in the menu body.
    pub description: &'static str,
}

/// Curated model menu — order is the order shown in the keyboard.
///
/// **Local registry, not a project-wide one.** Per the project memory
/// `feedback_no_central_registries`, this stays here rather than in a
/// shared types module.
pub(crate) const MODEL_CHOICES: &[ModelChoice] = &[
    ModelChoice {
        alias: "default",
        label: "Default",
        model_id: None,
        description: "Opus 4.7 (1M context) · Most capable",
    },
    ModelChoice {
        alias: "sonnet",
        label: "Sonnet",
        model_id: Some("claude-sonnet-4-6"),
        description: "Sonnet 4.6 · Best for everyday tasks",
    },
    ModelChoice {
        alias: "sonnet1m",
        label: "Sonnet 1M",
        model_id: Some("claude-sonnet-4-6[1m]"),
        description: "Sonnet 4.6 (1M context) · Extra usage billing",
    },
    ModelChoice {
        alias: "haiku",
        label: "Haiku",
        model_id: Some("claude-haiku-4-5"),
        description: "Haiku 4.5 · Fastest",
    },
];

/// Resolve a callback alias to a `ModelChoice`.
pub(crate) fn lookup(alias: &str) -> Option<&'static ModelChoice> {
    MODEL_CHOICES.iter().find(|c| c.alias == alias)
}

/// Find the choice that matches the given current `model_id` (from `agent.yaml`).
/// Returns `None` if the value is non-canonical (a "Custom" model).
pub(crate) fn active_choice(current: Option<&str>) -> Option<&'static ModelChoice> {
    MODEL_CHOICES.iter().find(|c| c.model_id == current)
}

/// Render the menu body text. Includes a "Current: ... (custom)" prefix line
/// when the active model is non-canonical.
pub(crate) fn render_menu_body(current: Option<&str>) -> String {
    let active = active_choice(current);
    let mut out = String::from("🤖 Choose Claude model\n\n");
    if let (None, Some(custom)) = (active, current) {
        out.push_str(&format!("Current: {custom} (custom)\n\n"));
    }
    for choice in MODEL_CHOICES {
        let mark = if active.map(|a| a.alias) == Some(choice.alias) {
            "✓ "
        } else {
            "   "
        };
        out.push_str(&format!("{}{} — {}\n", mark, choice.label, choice.description));
    }
    out
}

/// Render the inline keyboard — 2 columns × 2 rows, with `✓` prefix on the active button.
pub(crate) fn render_keyboard(current: Option<&str>) -> teloxide::types::InlineKeyboardMarkup {
    use teloxide::types::{InlineKeyboardButton, InlineKeyboardMarkup};
    let active = active_choice(current);
    let button = |c: &ModelChoice| -> InlineKeyboardButton {
        let label = if active.map(|a| a.alias) == Some(c.alias) {
            format!("✓ {}", c.label)
        } else {
            c.label.to_string()
        };
        InlineKeyboardButton::callback(label, format!("model:{}", c.alias))
    };
    InlineKeyboardMarkup::new(vec![
        vec![button(&MODEL_CHOICES[0]), button(&MODEL_CHOICES[1])],
        vec![button(&MODEL_CHOICES[2]), button(&MODEL_CHOICES[3])],
    ])
}

/// Open the `/model` menu. Allowlist-gated in groups.
pub(crate) async fn handle_model(
    bot: super::BotType,
    msg: teloxide::types::Message,
    settings: std::sync::Arc<super::handler::AgentSettings>,
    allowlist: right_agent::agent::allowlist::AllowlistHandle,
) -> teloxide::prelude::ResponseResult<()> {
    use teloxide::prelude::*;
    if !super::handler::is_private_chat(&msg.chat.kind)
        && !super::allowlist_commands::sender_is_trusted(&msg, &allowlist)
    {
        tracing::debug!(
            chat_id = msg.chat.id.0,
            user_id = msg.from.as_ref().map(|u| u.id.0),
            "/model ignored: non-trusted sender in group"
        );
        return Ok(());
    }

    let current = settings.model.load();
    let current_str: Option<&str> = (*current).as_deref();
    let body = render_menu_body(current_str);
    let keyboard = render_keyboard(current_str);

    let mut send = bot.send_message(msg.chat.id, body).reply_markup(keyboard);
    if let Some(thread_id) = msg.thread_id {
        send = send.message_thread_id(thread_id);
    }
    send.await?;
    Ok(())
}

/// Handle a click on a `/model` keyboard button.
///
/// Callback data format: `model:<alias>` (e.g. `model:sonnet`).
/// Re-checks the allowlist on every click — the keyboard stays in the chat
/// and any group member could click it, not just the `/model` invoker.
pub(crate) async fn handle_model_callback(
    bot: super::BotType,
    q: teloxide::types::CallbackQuery,
    settings: std::sync::Arc<super::handler::AgentSettings>,
    agent_dir: std::sync::Arc<super::handler::AgentDir>,
    allowlist: right_agent::agent::allowlist::AllowlistHandle,
) -> teloxide::prelude::ResponseResult<()> {
    use teloxide::prelude::*;

    let Some(data) = q.data.as_deref() else {
        // Ack so Telegram clears the loading spinner.
        bot.answer_callback_query(q.id).await?;
        return Ok(());
    };
    let Some(alias) = data.strip_prefix("model:") else {
        bot.answer_callback_query(q.id).await?;
        return Ok(());
    };

    let Some(choice) = lookup(alias) else {
        tracing::warn!(callback_data = data, "unknown /model alias");
        bot.answer_callback_query(q.id)
            .text("Unknown option")
            .await?;
        return Ok(());
    };

    // Re-check group gate on every click (the keyboard persists in chat,
    // any group member can click). Fail-secure: missing q.message → treat
    // as group, require trust.
    let in_group = q
        .message
        .as_ref()
        .map(|m| !super::handler::is_private_chat(&m.chat().kind))
        .unwrap_or(true);
    if in_group {
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
    }

    let agent_yaml_path = agent_dir.0.join("agent.yaml");
    let old_value: Option<String> = crate::snapshot_model(&settings.model);

    // Persist before swap: if disk write fails, in-memory stays untouched.
    if let Err(e) = right_agent::agent::types::write_agent_yaml_model(
        &agent_yaml_path,
        choice.model_id,
    ) {
        tracing::error!(error = %format!("{e:#}"), "/model: failed to write agent.yaml");
        bot.answer_callback_query(q.id)
            .text("Failed to save model — see bot logs")
            .await?;
        return Ok(());
    }

    // Two writers exist (this callback + config_watcher); both derive the value
    // from disk, so last-write-wins converges race-free.
    settings
        .model
        .store(std::sync::Arc::new(choice.model_id.map(str::to_owned)));

    tracing::info!(
        from = ?old_value.as_deref().unwrap_or("default"),
        to = ?choice.model_id.unwrap_or("default"),
        chat_id = q.message.as_ref().map(|m| m.chat().id.0),
        user_id = q.from.id.0,
        "model switched via /model"
    );

    // Best-effort menu refresh + toast in parallel. Edit failure is logged
    // but non-fatal: the persistent state and the toast are the source of
    // truth; the visible menu is a courtesy. Telegram requires
    // answerCallbackQuery within ~3s to clear the spinner — running it
    // concurrent with the edit avoids that timeout on slow networks.
    let toast = bot
        .answer_callback_query(q.id)
        .text(format!("Switched to {}", choice.label));
    if let Some(message) = q.message.as_ref() {
        let new_body = render_menu_body(choice.model_id);
        let new_kb = render_keyboard(choice.model_id);
        let edit = bot
            .edit_message_text(message.chat().id, message.id(), new_body)
            .reply_markup(new_kb);
        let (edit_result, toast_result) = tokio::join!(edit.send(), toast.send());
        if let Err(e) = edit_result {
            tracing::warn!(error = %e, "failed to edit /model menu after switch");
        }
        toast_result?;
    } else {
        toast.await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aliases_unique() {
        let mut seen = std::collections::HashSet::new();
        for c in MODEL_CHOICES {
            assert!(seen.insert(c.alias), "duplicate alias: {}", c.alias);
        }
    }

    #[test]
    fn aliases_short_enough_for_callback_data() {
        // "model:" prefix = 6 bytes; Telegram limit = 64.
        for c in MODEL_CHOICES {
            assert!(
                c.alias.len() <= 32,
                "alias {} too long ({} bytes)",
                c.alias,
                c.alias.len()
            );
        }
    }

    #[test]
    fn lookup_known_alias() {
        let c = lookup("sonnet").unwrap();
        assert_eq!(c.model_id, Some("claude-sonnet-4-6"));
    }

    #[test]
    fn lookup_unknown_alias_returns_none() {
        assert!(lookup("nonsense").is_none());
    }

    #[test]
    fn active_choice_default_for_none() {
        let c = active_choice(None).unwrap();
        assert_eq!(c.alias, "default");
    }

    #[test]
    fn active_choice_canonical_model() {
        let c = active_choice(Some("claude-haiku-4-5")).unwrap();
        assert_eq!(c.alias, "haiku");
    }

    #[test]
    fn active_choice_one_m_suffix() {
        let c = active_choice(Some("claude-sonnet-4-6[1m]")).unwrap();
        assert_eq!(c.alias, "sonnet1m");
    }

    #[test]
    fn active_choice_custom_model_returns_none() {
        assert!(active_choice(Some("claude-opus-4-old")).is_none());
    }

    #[test]
    fn menu_body_shows_checkmark_on_active() {
        let body = render_menu_body(Some("claude-sonnet-4-6"));
        assert!(body.contains("✓ Sonnet"), "expected checkmark on Sonnet:\n{body}");
        assert!(!body.contains("✓ Default"), "no checkmark on Default:\n{body}");
    }

    #[test]
    fn menu_body_shows_default_when_none() {
        let body = render_menu_body(None);
        assert!(body.contains("✓ Default"), "expected checkmark on Default:\n{body}");
    }

    #[test]
    fn menu_body_shows_custom_prefix_for_non_canonical() {
        let body = render_menu_body(Some("claude-opus-4-old"));
        assert!(
            body.contains("Current: claude-opus-4-old (custom)"),
            "custom prefix:\n{body}"
        );
        assert!(
            !body.contains("✓"),
            "no checkmark anywhere when custom:\n{body}"
        );
    }

    #[test]
    fn render_keyboard_has_4_buttons_in_2_rows() {
        let kb = render_keyboard(None);
        assert_eq!(kb.inline_keyboard.len(), 2);
        assert_eq!(kb.inline_keyboard[0].len(), 2);
        assert_eq!(kb.inline_keyboard[1].len(), 2);
    }

    #[test]
    fn render_keyboard_callback_data_format() {
        let kb = render_keyboard(None);
        let data: Vec<String> = kb
            .inline_keyboard
            .iter()
            .flatten()
            .filter_map(|b| match &b.kind {
                teloxide::types::InlineKeyboardButtonKind::CallbackData(d) => Some(d.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(
            data,
            vec!["model:default", "model:sonnet", "model:sonnet1m", "model:haiku"]
        );
    }
}
