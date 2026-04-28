//! Theme tiers for brand-conformant CLI output.
//!
//! Detection order:
//! 1. `TERM=dumb` or non-TTY → `Ascii`
//! 2. `NO_COLOR` env var set (any non-empty value) → `Mono`
//! 3. Otherwise → `Color`
//!
//! Tests inject `EnvGet` + `IsTty` stubs to avoid `std::env::set_var`.
//! `IsTty` is a thin wrapper for `std::io::IsTerminal` (which is sealed and cannot be implemented outside std).

use std::sync::OnceLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Theme {
    Color,
    Mono,
    Ascii,
}

/// Pluggable env reader. Production uses `RealEnv`; tests use stubs.
pub(crate) trait EnvGet {
    fn get(&self, key: &str) -> Option<String>;
}

/// Pluggable TTY probe. Production uses `RealTty`; tests use stubs.
///
/// `std::io::IsTerminal` is sealed and cannot be implemented outside std.
/// We wrap it here so tests can inject a stub without touching global state.
pub(crate) trait IsTty {
    fn is_tty(&self) -> bool;
}

pub(crate) struct RealEnv;
impl EnvGet for RealEnv {
    fn get(&self, key: &str) -> Option<String> {
        // `.ok()` is intentional: VarError carries no useful info beyond presence/absence.
        std::env::var(key).ok()
    }
}

pub(crate) struct RealTty;
impl IsTty for RealTty {
    fn is_tty(&self) -> bool {
        use std::io::IsTerminal as _;
        std::io::stdout().is_terminal()
    }
}

static CACHED: OnceLock<Theme> = OnceLock::new();

/// Resolve the active theme once per process and cache it.
pub fn detect() -> Theme {
    *CACHED.get_or_init(|| detect_with(&RealTty, &RealEnv))
}

/// Pure detection — no caching, no globals. Used by tests and by callers
/// that want to override (e.g. `--no-color` flag passed in Step 6).
pub(crate) fn detect_with(tty: &impl IsTty, env: &impl EnvGet) -> Theme {
    if env.get("TERM").as_deref() == Some("dumb") || !tty.is_tty() {
        return Theme::Ascii;
    }
    if env.get("NO_COLOR").is_some_and(|v| !v.is_empty()) {
        return Theme::Mono;
    }
    Theme::Color
}

#[cfg(test)]
#[path = "theme_tests.rs"]
mod tests;
