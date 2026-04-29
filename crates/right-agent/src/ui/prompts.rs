//! Brand-conformant inquire `RenderConfig` builders.
//!
//! Inquire's `default_colored()` paints `?` LightGreen and answers/highlighted
//! options LightCyan — both clash with the rail-and-glyph palette. The brand
//! reads "interactive prompts stay plain" (spec Decision #1) literally: no
//! color injected into the prompt chrome. We use `RenderConfig::empty()` for
//! every theme.

use inquire::ui::RenderConfig;

use crate::ui::Theme;

/// Returns the brand `RenderConfig` for the given theme.
///
/// All themes get `RenderConfig::empty()` — terminal-default foreground for
/// every chrome element. (Color::DarkGrey was tried and rendered as pastel
/// blue on the macOS Terminal default palette, defeating the purpose.)
pub fn render_config(_theme: Theme) -> RenderConfig<'static> {
    RenderConfig::empty()
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
    fn all_themes_return_uncolored_config() {
        for theme in [Theme::Color, Theme::Mono, Theme::Ascii] {
            let cfg = render_config(theme);
            assert_eq!(cfg.prompt_prefix.content, "?");
            assert!(cfg.prompt_prefix.style.fg.is_none(), "theme {theme:?}");
            assert_eq!(cfg.answered_prompt_prefix.content, "?");
            assert!(
                cfg.answered_prompt_prefix.style.fg.is_none(),
                "theme {theme:?}"
            );
            assert_eq!(cfg.highlighted_option_prefix.content, ">");
            assert!(
                cfg.highlighted_option_prefix.style.fg.is_none(),
                "theme {theme:?}"
            );
            assert!(cfg.help_message.fg.is_none(), "theme {theme:?}");
            assert!(cfg.selected_option.is_none(), "theme {theme:?}");
        }
    }
}
