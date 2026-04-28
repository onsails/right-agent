use crate::ui::{Glyph, Theme};

pub fn status(_glyph: Glyph) -> Line { Line::default() }

#[derive(Default)]
pub struct Line;
impl Line {
    pub fn noun(self, _s: impl Into<String>) -> Self { self }
    pub fn verb(self, _s: impl Into<String>) -> Self { self }
    pub fn detail(self, _s: impl Into<String>) -> Self { self }
    pub fn fix(self, _s: impl Into<String>) -> Self { self }
    pub fn render(self, _theme: Theme) -> String { String::new() }
}

#[derive(Default)]
pub struct Block;
impl Block {
    pub fn new() -> Self { Block }
    pub fn push(&mut self, _line: Line) {}
    pub fn render(self, _theme: Theme) -> String { String::new() }
}
