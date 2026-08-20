use super::*;

/// The catalog as it was in `right_openshell::providers::profile_catalog()`:
/// slug, env var, display name, and rendered category, in order. Any drift
/// here is a dashboard-visible change to `/provider-types`.
const EXPECTED: &[(&str, &str, &str, &str)] = &[
    (
        "anthropic",
        "ANTHROPIC_API_KEY",
        "Anthropic API",
        "inference",
    ),
    ("openai", "OPENAI_API_KEY", "OpenAI", "inference"),
    ("nvidia", "NVIDIA_API_KEY", "NVIDIA", "inference"),
    ("codex", "OPENAI_API_KEY", "Codex", "agent"),
    ("copilot", "COPILOT_GITHUB_TOKEN", "GitHub Copilot", "agent"),
    ("opencode", "OPENCODE_API_KEY", "OpenCode", "agent"),
    ("github", "GITHUB_TOKEN", "GitHub", "sourcecontrol"),
    ("right-github", "GITHUB_TOKEN", "GitHub", "sourcecontrol"),
    ("right-fal", "FAL_KEY", "fal.ai", "other"),
    ("gitlab", "GITLAB_TOKEN", "GitLab", "sourcecontrol"),
    ("generic", "", "Generic", "other"),
];

#[test]
fn catalog_matches_the_ported_gateway_profile_list() {
    let actual: Vec<(&str, &str, &str, &str)> = BUILTIN_CATALOG
        .iter()
        .map(|p| (p.slug, p.env_var, p.display_name, p.category.as_str()))
        .collect();
    assert_eq!(actual, EXPECTED);
}

#[test]
fn github_is_the_only_hidden_entry() {
    let hidden: Vec<&str> = BUILTIN_CATALOG
        .iter()
        .filter(|p| p.hidden)
        .map(|p| p.slug)
        .collect();
    assert_eq!(
        hidden,
        vec!["github"],
        "only the built-in superseded by right-github is hidden"
    );
}

#[test]
fn offered_catalog_hides_github_and_shows_right_github() {
    let offered = offered_catalog();
    assert!(
        offered.iter().all(|p| p.slug != "github"),
        "built-in read-only github is not offered"
    );
    assert!(
        offered
            .iter()
            .any(|p| p.slug == "right-github" && p.display_name == "GitHub"),
        "right-github is offered as GitHub"
    );
    assert!(
        offered.iter().any(|p| p.slug == "gitlab"),
        "the filter is narrow: other built-ins stay offered"
    );
}

#[test]
fn claude_is_never_in_the_catalog() {
    assert!(builtin(RESERVED_TYPE_SLUG).is_none());
}

#[test]
fn every_entry_but_generic_has_allowed_hosts() {
    for entry in BUILTIN_CATALOG {
        if entry.slug == GENERIC_SLUG {
            assert!(
                entry.allowed_hosts.is_empty(),
                "generic endpoints come from the record"
            );
            continue;
        }
        assert!(
            !entry.allowed_hosts.is_empty(),
            "{}: a secret with no allowed hosts can never substitute",
            entry.slug
        );
    }
}

#[test]
fn query_injection_is_opt_in_and_currently_unused() {
    assert!(
        BUILTIN_CATALOG.iter().all(|p| !p.query_injection),
        "headers inject by default; query params stay opt-in per entry"
    );
}

#[test]
fn category_renders_like_the_old_debug_lowercase() {
    assert_eq!(ProviderCategory::SourceControl.as_str(), "sourcecontrol");
    assert_eq!(ProviderCategory::Inference.to_string(), "inference");
    assert_eq!(ProviderCategory::Messaging.as_str(), "messaging");
    assert_eq!(ProviderCategory::Other.as_str(), "other");
    assert_eq!(ProviderCategory::Agent.as_str(), "agent");
}
