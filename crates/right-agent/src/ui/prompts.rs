//! Brand-conformant inquire `RenderConfig` builders.
//!
//! Inquire's `default_colored()` paints `?` LightGreen and answers/highlighted
//! options LightCyan — both clash with the rail-and-glyph palette. The brand
//! reads "interactive prompts stay plain" (spec Decision #1) literally: no
//! color injected into the prompt chrome — except the `>` highlighted-option
//! cursor, which uses brand orange so the active selection is the focal point.
//! (Color::DarkGrey was tried for the rest and rendered as pastel blue on the
//! macOS Terminal default palette, defeating the purpose.)

use inquire::ui::{Color, RenderConfig, Styled};

use crate::ui::Theme;
use crate::ui::atoms::ORANGE;

const BRAND_ORANGE: Color = Color::Rgb {
    r: ORANGE.0,
    g: ORANGE.1,
    b: ORANGE.2,
};

/// Returns the brand `RenderConfig` for the given theme.
///
/// `Color`: `empty()` chrome plus the orange `>` highlighted-option cursor.
/// `Mono` / `Ascii`: `RenderConfig::empty()` — no styling at all.
pub fn render_config(theme: Theme) -> RenderConfig<'static> {
    match theme {
        Theme::Mono | Theme::Ascii => RenderConfig::empty(),
        Theme::Color => RenderConfig::empty()
            .with_highlighted_option_prefix(Styled::new(">").with_fg(BRAND_ORANGE)),
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
    fn mono_and_ascii_use_empty_config() {
        for theme in [Theme::Mono, Theme::Ascii] {
            let cfg = render_config(theme);
            assert_eq!(cfg.prompt_prefix.content, "?");
            assert!(cfg.prompt_prefix.style.fg.is_none(), "theme {theme:?}");
            assert!(
                cfg.highlighted_option_prefix.style.fg.is_none(),
                "theme {theme:?}"
            );
        }
    }

    #[test]
    fn color_theme_only_colors_highlighted_cursor() {
        let cfg = render_config(Theme::Color);
        // Chrome stays uncolored — terminal-default fg keeps the prompt subtle.
        assert!(cfg.prompt_prefix.style.fg.is_none());
        assert!(cfg.answered_prompt_prefix.style.fg.is_none());
        assert!(cfg.help_message.fg.is_none());
        assert!(cfg.canceled_prompt_indicator.style.fg.is_none());
        // Only the highlighted cursor gets brand orange.
        assert_eq!(cfg.highlighted_option_prefix.content, ">");
        assert_eq!(cfg.highlighted_option_prefix.style.fg, Some(BRAND_ORANGE));
    }
}
