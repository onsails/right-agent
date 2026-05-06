use std::collections::HashMap;

use super::*;

struct EnvStub(HashMap<String, String>);
impl EnvStub {
    fn new() -> Self {
        EnvStub(HashMap::new())
    }
    fn with(mut self, k: &str, v: &str) -> Self {
        self.0.insert(k.into(), v.into());
        self
    }
}
impl EnvGet for EnvStub {
    fn get(&self, key: &str) -> Option<String> {
        self.0.get(key).cloned()
    }
}

struct TtyStub(bool);
impl IsTty for TtyStub {
    fn is_tty(&self) -> bool {
        self.0
    }
}

#[test]
fn detect_dumb_term_returns_ascii() {
    let env = EnvStub::new().with("TERM", "dumb");
    assert_eq!(detect_with(&TtyStub(true), &env), Theme::Ascii);
}

#[test]
fn detect_non_tty_returns_ascii() {
    let env = EnvStub::new();
    assert_eq!(detect_with(&TtyStub(false), &env), Theme::Ascii);
}

#[test]
fn detect_non_tty_overrides_no_color() {
    let env = EnvStub::new().with("NO_COLOR", "1");
    assert_eq!(detect_with(&TtyStub(false), &env), Theme::Ascii);
}

#[test]
fn detect_no_color_returns_mono() {
    let env = EnvStub::new().with("NO_COLOR", "1");
    assert_eq!(detect_with(&TtyStub(true), &env), Theme::Mono);
}

#[test]
fn detect_no_color_empty_value_falls_through_to_color() {
    let env = EnvStub::new().with("NO_COLOR", "");
    assert_eq!(detect_with(&TtyStub(true), &env), Theme::Color);
}

#[test]
fn detect_tty_no_env_returns_color() {
    let env = EnvStub::new();
    assert_eq!(detect_with(&TtyStub(true), &env), Theme::Color);
}
