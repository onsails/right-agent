//! `/model` command — inline-keyboard menu for switching the agent's Claude model.
//!
//! UI: 4 curated options (Opus 1M / Sonnet / Sonnet 1M / Haiku).
//!
//! Persistence: writes `agent.yaml::model` via `right_agent::agent::types::write_agent_yaml_model`.
//! In-memory: stores into `AgentSettings.model: Arc<ArcSwap<Option<String>>>`.
//! Group chats are gated by the trusted-users allowlist (same gate as `/allow`).

/// One row in the curated model menu.
///
/// Curated rows pin a specific model via the exact model-ID string CC accepts
/// on the command line. `model_id == None` is reserved for pre-existing
/// absent config state and is not exposed as a selectable menu option.
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
        alias: "opus1m",
        label: "Opus 1M",
        model_id: Some("claude-opus-4-8[1m]"),
        description: "Opus 4.8 (1M context) · Most capable",
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
        out.push_str(&format!(
            "{}{} — {}\n",
            mark, choice.label, choice.description
        ));
    }
    out
}

/// Render the inline keyboard — 2 columns × 2 rows, with `✓` prefix on the active button.
pub(crate) fn render_keyboard(current: Option<&str>) -> frankenstein::types::InlineKeyboardMarkup {
    use frankenstein::types::{InlineKeyboardButton, InlineKeyboardMarkup};
    let active = active_choice(current);
    let button = |c: &ModelChoice| -> InlineKeyboardButton {
        let label = if active.map(|a| a.alias) == Some(c.alias) {
            format!("✓ {}", c.label)
        } else {
            c.label.to_string()
        };
        InlineKeyboardButton::builder()
            .text(label)
            .callback_data(format!("model:{}", c.alias))
            .build()
    };
    InlineKeyboardMarkup::builder()
        .inline_keyboard(vec![
            vec![button(&MODEL_CHOICES[0]), button(&MODEL_CHOICES[1])],
            vec![button(&MODEL_CHOICES[2]), button(&MODEL_CHOICES[3])],
        ])
        .build()
}

/// Open the `/model` menu. Allowlist-gated in groups.
pub(crate) async fn handle_model(
    ctx: &super::router::HandlerCtx,
    msg: &frankenstein::types::Message,
) -> Result<(), super::tg_bot::TgError> {
    let bot = &ctx.bot;
    let settings = &ctx.settings;
    if !super::msg_ext::is_private(&msg.chat)
        && !super::allowlist_commands::sender_is_trusted(msg, &ctx.allowlist)
    {
        tracing::debug!(
            chat_id = msg.chat.id,
            user_id = msg.from.as_ref().map(|u| u.id),
            "/model ignored: non-trusted sender in group"
        );
        return Ok(());
    }

    let current = settings.model.load();
    let current_str: Option<&str> = (*current).as_deref();
    let body = render_menu_body(current_str);
    let keyboard = render_keyboard(current_str);

    bot.send_message_opts(
        msg.chat.id,
        &body,
        false,
        msg.message_thread_id,
        None,
        Some(keyboard),
    )
    .await?;
    Ok(())
}

/// Handle a click on a `/model` keyboard button.
///
/// Callback data format: `model:<alias>` (e.g. `model:sonnet`).
/// Re-checks the allowlist on every click — the keyboard stays in the chat
/// and any group member could click it, not just the `/model` invoker.
pub(crate) async fn handle_model_callback(
    ctx: &super::router::HandlerCtx,
    q: &frankenstein::types::CallbackQuery,
) -> Result<(), super::tg_bot::TgError> {
    use frankenstein::types::MaybeInaccessibleMessage;
    let bot = &ctx.bot;
    let settings = &ctx.settings;
    let agent_dir = &ctx.agent_dir;
    let allowlist = &ctx.allowlist;

    let Some(data) = q.data.as_deref() else {
        // Ack so Telegram clears the loading spinner.
        bot.answer_callback(&q.id, None, false).await?;
        return Ok(());
    };
    let Some(alias) = data.strip_prefix("model:") else {
        bot.answer_callback(&q.id, None, false).await?;
        return Ok(());
    };

    let Some(choice) = lookup(alias) else {
        tracing::warn!(callback_data = data, "unknown /model alias");
        bot.answer_callback(&q.id, Some("Unknown option"), false)
            .await?;
        return Ok(());
    };

    // The accessible message (if any) carries the chat + message id for the
    // group-gate check and the menu edit.
    let message = match q.message.as_ref() {
        Some(MaybeInaccessibleMessage::Message(m)) => Some((m.chat.id, m.message_id, &m.chat)),
        _ => None,
    };

    // Re-check group gate on every click (the keyboard persists in chat,
    // any group member can click). Fail-secure: missing q.message → treat
    // as group, require trust.
    let in_group = message
        .map(|(_, _, chat)| !super::msg_ext::is_private(chat))
        .unwrap_or(true);
    if in_group {
        let user_id = q.from.id as i64;
        let trusted = allowlist
            .0
            .read()
            .expect("allowlist lock poisoned")
            .is_user_trusted(user_id);
        if !trusted {
            bot.answer_callback(&q.id, Some("Not allowed"), false)
                .await?;
            return Ok(());
        }
    }

    let agent_yaml_path = agent_dir.0.join("agent.yaml");
    let old_value: Option<String> = crate::snapshot_model(&settings.model);

    // Persist before swap: if disk write fails, in-memory stays untouched.
    if let Err(e) =
        right_agent::agent::types::write_agent_yaml_model(&agent_yaml_path, choice.model_id)
    {
        tracing::error!(error = %format!("{e:#}"), "/model: failed to write agent.yaml");
        bot.answer_callback(&q.id, Some("Failed to save model — see bot logs"), false)
            .await?;
        return Ok(());
    }

    // Two writers exist (this callback + config_watcher); both derive the value
    // from disk, so last-write-wins converges race-free.
    settings
        .model
        .store(std::sync::Arc::new(choice.model_id.map(str::to_owned)));

    tracing::info!(
        from = ?old_value.as_deref().unwrap_or("inherit"),
        to = ?choice.model_id.unwrap_or("inherit"),
        chat_id = message.map(|(chat_id, _, _)| chat_id),
        user_id = q.from.id,
        "model switched via /model"
    );

    // Best-effort menu refresh + toast in parallel. Edit failure is logged
    // but non-fatal: the persistent state and the toast are the source of
    // truth; the visible menu is a courtesy. Telegram requires
    // answerCallbackQuery within ~3s to clear the spinner — running it
    // concurrent with the edit avoids that timeout on slow networks.
    let toast_text = format!("Switched to {}", choice.label);
    if let Some((chat_id, message_id, _)) = message {
        let new_body = render_menu_body(choice.model_id);
        let new_kb = render_keyboard(choice.model_id);
        // Plain text (teloxide parity): the menu body is not HTML; edit_html
        // would force ParseMode::Html.
        let edit = bot.edit_text(chat_id, message_id, &new_body, Some(new_kb));
        let toast = bot.answer_callback(&q.id, Some(&toast_text), false);
        let (edit_result, toast_result) = tokio::join!(edit, toast);
        if let Err(e) = edit_result {
            tracing::warn!(error = %e, "failed to edit /model menu after switch");
        }
        toast_result?;
    } else {
        bot.answer_callback(&q.id, Some(&toast_text), false).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn aliases_unique() {
        let mut seen = std::collections::HashSet::new();
        for c in MODEL_CHOICES {
            assert!(seen.insert(c.alias), "duplicate alias: {}", c.alias);
        }
    }

    #[tokio::test]
    async fn aliases_short_enough_for_callback_data() {
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

    #[tokio::test]
    async fn lookup_known_alias() {
        let c = lookup("sonnet").unwrap();
        assert_eq!(c.model_id, Some("claude-sonnet-4-6"));
    }

    #[tokio::test]
    async fn lookup_unknown_alias_returns_none() {
        assert!(lookup("nonsense").is_none());
    }

    #[tokio::test]
    async fn active_choice_none_has_no_default() {
        assert!(active_choice(None).is_none());
    }

    #[tokio::test]
    async fn opus_1m_choice_is_explicit_model() {
        let c = lookup("opus1m").unwrap();
        assert_eq!(c.model_id, Some("claude-opus-4-8[1m]"));
    }

    #[tokio::test]
    async fn active_choice_canonical_model() {
        let c = active_choice(Some("claude-haiku-4-5")).unwrap();
        assert_eq!(c.alias, "haiku");
    }

    #[tokio::test]
    async fn active_choice_one_m_suffix() {
        let c = active_choice(Some("claude-sonnet-4-6[1m]")).unwrap();
        assert_eq!(c.alias, "sonnet1m");
    }

    #[tokio::test]
    async fn active_choice_custom_model_returns_none() {
        assert!(active_choice(Some("claude-opus-4-old")).is_none());
    }

    #[tokio::test]
    async fn menu_body_shows_checkmark_on_active() {
        let body = render_menu_body(Some("claude-sonnet-4-6"));
        assert!(
            body.contains("✓ Sonnet"),
            "expected checkmark on Sonnet:\n{body}"
        );
        assert!(
            !body.contains("✓ Opus 1M"),
            "no checkmark on Opus 1M:\n{body}"
        );
    }

    #[tokio::test]
    async fn menu_body_has_no_default_when_none() {
        let body = render_menu_body(None);
        assert!(
            !body.contains("Default"),
            "Default option must not be shown:\n{body}"
        );
        assert!(
            !body.contains("✓"),
            "no checkmark when no explicit model is configured:\n{body}"
        );
        assert!(
            body.contains("Opus 1M"),
            "Opus 1M option must be shown:\n{body}"
        );
    }

    #[tokio::test]
    async fn menu_body_shows_custom_prefix_for_non_canonical() {
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

    #[tokio::test]
    async fn render_keyboard_has_4_buttons_in_2_rows() {
        let kb = render_keyboard(None);
        assert_eq!(kb.inline_keyboard.len(), 2);
        assert_eq!(kb.inline_keyboard[0].len(), 2);
        assert_eq!(kb.inline_keyboard[1].len(), 2);
    }

    #[tokio::test]
    async fn render_keyboard_callback_data_format() {
        let kb = render_keyboard(None);
        let data: Vec<String> = kb
            .inline_keyboard
            .iter()
            .flatten()
            .filter_map(|b| b.callback_data.clone())
            .collect();
        assert_eq!(
            data,
            vec![
                "model:opus1m",
                "model:sonnet",
                "model:sonnet1m",
                "model:haiku"
            ]
        );
    }
}
