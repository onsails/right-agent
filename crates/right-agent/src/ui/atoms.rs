use crate::ui::theme::Theme;

pub struct Rail;
impl Rail {
    pub fn prefix(_theme: Theme) -> &'static str { "" }
    pub fn mark(_theme: Theme) -> &'static str { "" }
    pub fn blank(_theme: Theme) -> &'static str { "" }
}

#[derive(Debug, Clone, Copy)]
pub enum Glyph { Ok, Warn, Err, Info }
impl Glyph {
    pub fn render(self, _theme: Theme) -> String { String::new() }
}
