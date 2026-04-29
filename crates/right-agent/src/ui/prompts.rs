//! Brand-conformant inquire `RenderConfig` builders.
//!
//! Inquire's `default_colored()` paints `?` LightGreen and answers/highlighted
//! options LightCyan — both clash with the rail-and-glyph palette. We replace
//! them with `DarkGrey` for the `Color` theme and the empty (no-style) config
//! for `Mono` / `Ascii`.

use inquire::ui::{Color, RenderConfig, StyleSheet, Styled};

use crate::ui::Theme;

/// Returns a `RenderConfig` matching the brand for the given theme.
///
/// - `Color`: `empty()` base + DarkGrey for prefix/answered/help/highlighted/canceled.
/// - `Mono` / `Ascii`: `RenderConfig::empty()` (no styling at all).
pub fn render_config(theme: Theme) -> RenderConfig<'static> {
    match theme {
        Theme::Mono | Theme::Ascii => RenderConfig::empty(),
        Theme::Color => RenderConfig::empty()
            .with_prompt_prefix(Styled::new("?").with_fg(Color::DarkGrey))
            .with_answered_prompt_prefix(Styled::new(">").with_fg(Color::DarkGrey))
            .with_help_message(StyleSheet::empty().with_fg(Color::DarkGrey))
            .with_highlighted_option_prefix(Styled::new(">").with_fg(Color::DarkGrey))
            .with_canceled_prompt_indicator(
                Styled::new("<canceled>").with_fg(Color::DarkGrey),
            ),
    }
}

/// Install the brand-conformant `RenderConfig` for the detected theme via
/// `inquire::set_global_render_config`. Idempotent — safe to call repeatedly,
/// though one call early in `main` is sufficient.
pub fn install_global() {
    inquire::set_global_render_config(render_config(crate::ui::detect()));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_themes_use_empty_config() {
        let mono = render_config(Theme::Mono);
        let ascii = render_config(Theme::Ascii);
        // `empty()` keeps the "?" glyph but no foreground style.
        assert_eq!(mono.prompt_prefix.content, "?");
        assert_eq!(ascii.prompt_prefix.content, "?");
        assert!(mono.prompt_prefix.style.fg.is_none());
        assert!(ascii.prompt_prefix.style.fg.is_none());
    }

    #[test]
    fn color_theme_uses_dark_grey() {
        let cfg = render_config(Theme::Color);
        assert_eq!(cfg.prompt_prefix.content, "?");
        assert_eq!(cfg.prompt_prefix.style.fg, Some(Color::DarkGrey));
        assert_eq!(cfg.answered_prompt_prefix.content, ">");
        assert_eq!(cfg.answered_prompt_prefix.style.fg, Some(Color::DarkGrey));
        assert_eq!(cfg.highlighted_option_prefix.content, ">");
        assert_eq!(
            cfg.highlighted_option_prefix.style.fg,
            Some(Color::DarkGrey)
        );
        assert_eq!(cfg.help_message.fg, Some(Color::DarkGrey));
        assert_eq!(cfg.canceled_prompt_indicator.content, "<canceled>");
        assert_eq!(
            cfg.canceled_prompt_indicator.style.fg,
            Some(Color::DarkGrey)
        );
    }
}
