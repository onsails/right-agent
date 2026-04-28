use crate::ui::Theme;
pub fn stdout(_theme: Theme, s: &str) { println!("{s}"); }
pub fn stderr(_theme: Theme, s: &str) { eprintln!("{s}"); }
