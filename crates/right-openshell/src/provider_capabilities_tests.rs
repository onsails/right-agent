use std::collections::HashSet;

use super::*;
use crate::openshell_proto::openshell::sandbox::v1::{
    NetworkBinary, NetworkEndpoint, NetworkPolicyRule, SandboxPolicy,
};

fn endpoint(host: &str) -> NetworkEndpoint {
    NetworkEndpoint {
        host: host.into(),
        port: 443,
        protocol: "rest".into(),
        ..Default::default()
    }
}

fn binary(path: &str) -> NetworkBinary {
    NetworkBinary {
        path: path.into(),
        ..Default::default()
    }
}

fn policy_with(rule_key: &str, bins: &[&str], hosts: &[&str]) -> SandboxPolicy {
    let rule = NetworkPolicyRule {
        name: rule_key.into(),
        endpoints: hosts.iter().map(|host| endpoint(host)).collect(),
        binaries: bins.iter().map(|path| binary(path)).collect(),
    };
    let mut policy = SandboxPolicy::default();
    policy.network_policies.insert(rule_key.into(), rule);
    policy
}

fn input(name: &str, display_name: &str, env_vars: &[&str]) -> ProviderCapabilityInput {
    ProviderCapabilityInput {
        name: name.into(),
        display_name: display_name.into(),
        candidate_env_vars: env_vars.iter().map(|v| (*v).into()).collect(),
        profile_binaries: Vec::new(),
        profile_endpoint_hosts: Vec::new(),
    }
}

fn input_with_profile(
    name: &str,
    display_name: &str,
    env_vars: &[&str],
    binaries: &[&str],
    hosts: &[&str],
) -> ProviderCapabilityInput {
    ProviderCapabilityInput {
        name: name.into(),
        display_name: display_name.into(),
        candidate_env_vars: env_vars.iter().map(|v| (*v).into()).collect(),
        profile_binaries: binaries.iter().map(|path| (*path).into()).collect(),
        profile_endpoint_hosts: hosts.iter().map(|host| (*host).into()).collect(),
    }
}

fn env_keys(keys: &[&str]) -> HashSet<String> {
    keys.iter().map(|key| (*key).into()).collect()
}

#[test]
fn env_keys_from_stdout_trims_empty_lines_and_dedupes() {
    let keys = env_keys_from_stdout("\nGITHUB_TOKEN\n  ANTHROPIC_API_KEY  \nGITHUB_TOKEN\n");

    assert_eq!(keys, env_keys(&["ANTHROPIC_API_KEY", "GITHUB_TOKEN"]));
}

#[test]
fn matched_provider_reports_policy_env_and_usage_hint() {
    let policy = policy_with(
        "_provider_right_right_github",
        &["/usr/bin/gh", "/usr/bin/git", "/usr/bin/gh"],
        &["uploads.github.com", "api.github.com", "api.github.com"],
    );
    let inputs = [
        input("right-zed", "Zed", &["ZED_API_KEY"]),
        input(
            "right-right-github",
            "GitHub",
            &["GITHUB_TOKEN", "GH_TOKEN"],
        ),
    ];
    let env = env_keys(&["GH_TOKEN", "IGNORED"]);

    let capabilities = correlate_provider_capabilities(&inputs, &policy, &env);

    assert_eq!(capabilities.len(), 2);
    assert_eq!(
        capabilities
            .iter()
            .map(|capability| capability.display_name.as_str())
            .collect::<Vec<_>>(),
        ["GitHub", "Zed"]
    );
    let github = capabilities
        .iter()
        .find(|capability| capability.display_name == "GitHub")
        .expect("GitHub capability should be present");
    assert_eq!(github.env_vars, ["GH_TOKEN"]);
    assert_eq!(github.allowed_binaries, ["/usr/bin/gh", "/usr/bin/git"]);
    assert_eq!(
        github.endpoint_hosts,
        ["api.github.com", "uploads.github.com"]
    );
    assert!(github.usage_hint.contains("gh"));
    assert!(github.usage_hint.contains("git"));
    assert!(github.usage_hint.contains("api.github.com"));
    assert!(github.usage_hint.contains("gateway"));
    assert!(github.usage_hint.contains("curl/fetch/python"));
    assert!(github.usage_hint.contains("401"));
}

#[test]
fn wildcard_binary_yields_any_binary_usage_hint() {
    let policy = policy_with("_provider_right_example", &["**"], &["api.example.com"]);
    let inputs = [input("right-example", "Example", &["EXAMPLE_API_KEY"])];
    let env = env_keys(&["EXAMPLE_API_KEY"]);

    let capabilities = correlate_provider_capabilities(&inputs, &policy, &env);

    assert_eq!(capabilities[0].allowed_binaries, ["**"]);
    assert!(capabilities[0].usage_hint.contains("Any binary"));
    assert!(capabilities[0].usage_hint.contains("api.example.com"));
}

#[test]
fn provider_profile_constraints_apply_when_placeholder_materialized_and_policy_exposes_no_rule() {
    let inputs = [input_with_profile(
        "right-example",
        "Example",
        &["EXAMPLE_API_KEY"],
        &["**"],
        &["api.example.com"],
    )];
    let env = env_keys(&["EXAMPLE_API_KEY"]);

    let capabilities = correlate_provider_capabilities(&inputs, &SandboxPolicy::default(), &env);

    assert_eq!(capabilities[0].allowed_binaries, ["**"]);
    assert_eq!(capabilities[0].endpoint_hosts, ["api.example.com"]);
    assert!(capabilities[0].usage_hint.contains("Any binary"));
}

#[test]
fn provider_profile_constraints_do_not_apply_without_materialized_placeholder() {
    let inputs = [input_with_profile(
        "right-example",
        "Example",
        &["EXAMPLE_API_KEY"],
        &["**"],
        &["api.example.com"],
    )];

    let capabilities =
        correlate_provider_capabilities(&inputs, &SandboxPolicy::default(), &HashSet::new());

    assert!(capabilities[0].allowed_binaries.is_empty());
    assert!(capabilities[0].endpoint_hosts.is_empty());
    assert!(capabilities[0].usage_hint.contains("not currently active"));
}

#[test]
fn matched_rule_with_no_binaries_reports_no_allowed_binaries() {
    let policy = policy_with("_provider_right_example", &[], &["api.example.com"]);
    let inputs = [input("right-example", "Example", &["EXAMPLE_API_KEY"])];
    let env = env_keys(&["EXAMPLE_API_KEY"]);

    let capabilities = correlate_provider_capabilities(&inputs, &policy, &env);

    assert!(capabilities[0].allowed_binaries.is_empty());
    assert_eq!(capabilities[0].endpoint_hosts, ["api.example.com"]);
    assert!(
        capabilities[0]
            .usage_hint
            .contains("No binaries are currently allowed")
    );
    assert!(!capabilities[0].usage_hint.contains("configured binaries"));
}

#[test]
fn matched_rule_with_no_endpoints_reports_no_allowed_endpoint_hosts() {
    let policy = policy_with("_provider_right_example", &["/usr/bin/example"], &[]);
    let inputs = [input("right-example", "Example", &["EXAMPLE_API_KEY"])];
    let env = env_keys(&["EXAMPLE_API_KEY"]);

    let capabilities = correlate_provider_capabilities(&inputs, &policy, &env);

    assert_eq!(capabilities[0].allowed_binaries, ["/usr/bin/example"]);
    assert!(capabilities[0].endpoint_hosts.is_empty());
    assert!(
        capabilities[0]
            .usage_hint
            .contains("No endpoint hosts are currently allowed")
    );
    assert!(!capabilities[0].usage_hint.contains("configured hosts"));
}

#[test]
fn attached_but_uncomposed_provider_is_inactive() {
    let inputs = [input("right-example", "Example", &["EXAMPLE_API_KEY"])];
    let env = env_keys(&["EXAMPLE_API_KEY"]);

    let capabilities = correlate_provider_capabilities(&inputs, &SandboxPolicy::default(), &env);

    assert_eq!(capabilities[0].env_vars, ["EXAMPLE_API_KEY"]);
    assert!(capabilities[0].allowed_binaries.is_empty());
    assert!(capabilities[0].endpoint_hosts.is_empty());
    assert!(capabilities[0].usage_hint.contains("not currently active"));
}

#[test]
fn provider_is_composed_true_when_rule_present() {
    // Provider gateway name `right-example` composes under rule key
    // `_provider_right_example` (the `_provider_` prefix + sanitized name).
    let policy = policy_with("_provider_right_example", &["**"], &["api.example.com"]);
    assert!(crate::provider_capabilities::provider_is_composed(
        &policy,
        "right-example"
    ));
}

#[test]
fn provider_is_composed_false_on_empty_policy() {
    let policy = crate::openshell_proto::openshell::sandbox::v1::SandboxPolicy::default();
    assert!(!crate::provider_capabilities::provider_is_composed(
        &policy,
        "right-example"
    ));
}

#[test]
fn provider_is_composed_with_endpoint_matches_host_and_path() {
    let mut policy = policy_with("_provider_right_example", &["**"], &["api.example.com"]);
    policy
        .network_policies
        .get_mut("_provider_right_example")
        .unwrap()
        .endpoints[0]
        .path = "/v1".into();

    assert!(
        crate::provider_capabilities::provider_is_composed_with_endpoint(
            &policy,
            "right-example",
            "api.example.com",
            "/v1"
        )
    );
    assert!(
        crate::provider_capabilities::provider_is_composed_with_endpoint(
            &policy,
            "right-example",
            "API.EXAMPLE.COM",
            "/v1"
        )
    );
}

#[test]
fn all_endpoints_present_requires_every_host() {
    let mut policy = policy_with("_provider_right_example", &["**"], &["fal.run"]);
    policy
        .network_policies
        .get_mut("_provider_right_example")
        .unwrap()
        .endpoints[0]
        .path = "".into();

    assert!(
        crate::provider_capabilities::provider_is_composed_with_all_endpoints(
            &policy,
            "right-example",
            &[("FAL.RUN".into(), "".into())]
        )
    );
    assert!(
        !crate::provider_capabilities::provider_is_composed_with_all_endpoints(
            &policy,
            "right-example",
            &[
                ("fal.run".into(), "".into()),
                ("queue.fal.run".into(), "".into())
            ]
        )
    );
}

#[test]
fn provider_is_composed_with_all_endpoints_rejects_empty_expected() {
    let policy = policy_with("_provider_right_example", &["**"], &["fal.run"]);

    assert!(
        !crate::provider_capabilities::provider_is_composed_with_all_endpoints(
            &policy,
            "right-example",
            &[]
        )
    );
}

#[test]
fn provider_is_composed_with_all_endpoints_rejects_missing_rule() {
    let policy = SandboxPolicy::default();

    assert!(
        !crate::provider_capabilities::provider_is_composed_with_all_endpoints(
            &policy,
            "right-example",
            &[("fal.run".into(), "".into())]
        )
    );
}

#[test]
fn provider_is_composed_with_all_endpoints_rejects_rule_with_no_endpoints() {
    let policy = policy_with("_provider_right_example", &["**"], &[]);

    assert!(
        !crate::provider_capabilities::provider_is_composed_with_all_endpoints(
            &policy,
            "right-example",
            &[("fal.run".into(), "".into())]
        )
    );
}

#[test]
fn provider_is_composed_with_all_endpoints_rejects_wrong_path() {
    let mut policy = policy_with("_provider_right_example", &["**"], &["fal.run"]);
    policy
        .network_policies
        .get_mut("_provider_right_example")
        .unwrap()
        .endpoints[0]
        .path = "/v1".into();

    assert!(
        !crate::provider_capabilities::provider_is_composed_with_all_endpoints(
            &policy,
            "right-example",
            &[("fal.run".into(), "/v2".into())]
        )
    );
}

#[test]
fn provider_is_composed_with_all_endpoints_allows_extra_rule_endpoints() {
    let mut policy = policy_with(
        "_provider_right_example",
        &["**"],
        &["fal.run", "queue.fal.run", "cdn.fal.run"],
    );
    let endpoints = &mut policy
        .network_policies
        .get_mut("_provider_right_example")
        .unwrap()
        .endpoints;
    endpoints[0].path = "".into();
    endpoints[1].path = "/queue".into();
    endpoints[2].path = "/cdn".into();

    assert!(
        crate::provider_capabilities::provider_is_composed_with_all_endpoints(
            &policy,
            "right-example",
            &[
                ("FAL.RUN".into(), "".into()),
                ("queue.fal.run".into(), "/queue".into())
            ]
        )
    );
}

#[test]
fn provider_is_composed_with_endpoint_rejects_stale_host_or_path() {
    let mut policy = policy_with("_provider_right_example", &["**"], &["old.example.com"]);
    policy
        .network_policies
        .get_mut("_provider_right_example")
        .unwrap()
        .endpoints[0]
        .path = "/old".into();

    assert!(
        !crate::provider_capabilities::provider_is_composed_with_endpoint(
            &policy,
            "right-example",
            "api.example.com",
            "/old"
        )
    );
    assert!(
        !crate::provider_capabilities::provider_is_composed_with_endpoint(
            &policy,
            "right-example",
            "old.example.com",
            "/v1"
        )
    );
}
