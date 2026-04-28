use crate::ui::Theme;

pub struct Recap;
impl Recap {
    pub fn new(_title: &str) -> Self { Recap }
    pub fn ok(self, _noun: &str, _detail: &str) -> Self { self }
    pub fn warn(self, _noun: &str, _detail: &str) -> Self { self }
    pub fn next(self, _hint: &str) -> Self { self }
    pub fn render(self, _theme: Theme) -> String { String::new() }
}
