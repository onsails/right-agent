//! Status-line builder. Canonical shape: `▐  {glyph} {noun:<width}  {verb} [({detail})]`.

use crate::ui::atoms::{Glyph, Rail};
use crate::ui::theme::Theme;

/// Start a new status line for the given glyph.
pub fn status(glyph: Glyph) -> Line {
    Line {
        glyph,
        noun: String::new(),
        verb: String::new(),
        detail: None,
        fix: None,
    }
}

/// A single status line. Construct via [`status`] and chain builder methods
/// (`noun`, `verb`, `detail`, `fix`) before calling [`Line::render`].
#[derive(Clone)]
pub struct Line {
    glyph: Glyph,
    noun: String,
    verb: String,
    detail: Option<String>,
    fix: Option<String>,
}

impl Line {
    pub fn noun(mut self, s: impl Into<String>) -> Self { self.noun = s.into(); self }
    pub fn verb(mut self, s: impl Into<String>) -> Self { self.verb = s.into(); self }
    pub fn detail(mut self, s: impl Into<String>) -> Self { self.detail = Some(s.into()); self }
    pub fn fix(mut self, s: impl Into<String>) -> Self { self.fix = Some(s.into()); self }

    /// Render as a single string. May contain `\n` if `fix` is set.
    pub fn render(&self, theme: Theme) -> String {
        self.render_with_pad(theme, self.noun.len())
    }

    fn render_with_pad(&self, theme: Theme, noun_pad: usize) -> String {
        let mut out = String::new();
        out.push_str(&Rail::prefix(theme));
        out.push_str(&self.glyph.render(theme));
        out.push(' ');
        if noun_pad > 0 {
            out.push_str(&format!("{:<width$}", self.noun, width = noun_pad));
        } else {
            out.push_str(&self.noun);
        }
        if !self.verb.is_empty() {
            out.push_str("  ");
            out.push_str(&self.verb);
        }
        if let Some(ref d) = self.detail {
            out.push(' ');
            out.push('(');
            out.push_str(d);
            out.push(')');
        }
        if let Some(ref f) = self.fix {
            out.push('\n');
            out.push_str(&Rail::blank(theme));
            out.push_str("    fix: ");
            out.push_str(f);
        }
        out
    }
}

/// A vertical group of `Line`s with column-aligned noun widths.
#[derive(Default)]
pub struct Block {
    lines: Vec<Line>,
}

impl Block {
    pub fn new() -> Self { Block { lines: Vec::new() } }
    pub fn push(&mut self, line: Line) { self.lines.push(line); }
    pub fn is_empty(&self) -> bool { self.lines.is_empty() }
    pub fn len(&self) -> usize { self.lines.len() }

    pub fn render(&self, theme: Theme) -> String {
        let pad = self.lines.iter().map(|l| l.noun.len()).max().unwrap_or(0);
        self.lines
            .iter()
            .map(|l| l.render_with_pad(theme, pad))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

#[cfg(test)]
#[path = "line_tests.rs"]
mod tests;
