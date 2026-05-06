//! Completion-frame builder: section header + status block + `next:` pointer.

use crate::ui::atoms::{Glyph, Rail};
use crate::ui::header::section;
use crate::ui::line::{Block, status};
use crate::ui::theme::Theme;

/// Completion-frame builder. Wraps a section header, a column-aligned status
/// block, and an optional `next: <hint>` pointer.
pub struct Recap {
    title: String,
    block: Block,
    next: Option<String>,
}

impl Recap {
    pub fn new(title: &str) -> Self {
        Recap { title: title.into(), block: Block::new(), next: None }
    }

    pub fn ok(mut self, noun: &str, detail: &str) -> Self {
        self.block.push(status(Glyph::Ok).noun(noun).verb(detail));
        self
    }

    pub fn warn(mut self, noun: &str, detail: &str) -> Self {
        self.block.push(status(Glyph::Warn).noun(noun).verb(detail));
        self
    }

    pub fn next(mut self, hint: &str) -> Self {
        self.next = Some(hint.into());
        self
    }

    pub fn render(&self, theme: Theme) -> String {
        let mut out = String::new();
        out.push_str(&section(theme, &self.title));
        out.push('\n');
        out.push_str(&Rail::blank(theme));
        out.push('\n');
        out.push_str(&self.block.render(theme));
        if !self.block.is_empty() {
            out.push('\n');
        }
        out.push_str(&Rail::blank(theme));
        if let Some(ref hint) = self.next {
            out.push('\n');
            out.push_str(&Rail::prefix(theme));
            out.push_str("next: ");
            out.push_str(hint);
        }
        out
    }
}

#[cfg(test)]
#[path = "recap_tests.rs"]
mod tests;
