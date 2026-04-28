//! Brand voice regression: every prompt label must be lowercase-first and
//! must not contain `!` (we never use exclamation marks). Allowed proper-noun
//! prefixes (env var names, `@handles`, file names) are exempt.

use right_agent::init::PROMPT_LABELS as INIT_LABELS;

const ALLOWED_PROPER_NOUNS: &[&str] = &[
    "HINDSIGHT_API_KEY",
    "RIGHT_TG_TOKEN",
    "MEMORY.md",
    "@BotFather",
    "@userinfobot",
];

fn first_visible_char(s: &str) -> char {
    s.chars().next().expect("non-empty label")
}

fn starts_with_allowed_proper_noun(s: &str) -> bool {
    ALLOWED_PROPER_NOUNS.iter().any(|p| s.starts_with(p))
}

#[test]
fn init_labels_are_lowercase_first() {
    for label in INIT_LABELS {
        let first = first_visible_char(label);
        assert!(
            !first.is_uppercase() || starts_with_allowed_proper_noun(label),
            "init prompt has uppercase first letter: {label:?}"
        );
    }
}

#[test]
fn init_labels_have_no_exclamation_marks() {
    for label in INIT_LABELS {
        assert!(
            !label.contains('!'),
            "init prompt contains '!': {label:?}"
        );
    }
}
