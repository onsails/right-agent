#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Theme { Color, Mono, Ascii }

pub fn detect() -> Theme { Theme::Color }
