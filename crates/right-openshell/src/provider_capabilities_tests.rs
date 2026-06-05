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
    }
}

fn env_keys(keys: &[&str]) -> HashSet<String> {
    keys.iter().map(|key| (*key).into()).collect()
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
