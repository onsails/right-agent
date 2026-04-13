//! Generate OpenShell policy.yaml from agent configuration.

use crate::agent::types::NetworkPolicy;

/// Domains allowed in restrictive mode (Anthropic/Claude only).
const RESTRICTIVE_DOMAINS: &[&str] = &[
    "*.anthropic.com",
    "anthropic.com",
    "*.claude.com",
    "claude.com",
    "*.claude.ai",
    "claude.ai",
];

fn restrictive_endpoints() -> String {
    RESTRICTIVE_DOMAINS
        .iter()
        .map(|host| {
            format!(
                r#"      - host: "{host}"
        port: 443
        protocol: rest
        access: full
        tls: terminate"#
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Generate an OpenShell policy YAML string.
///
/// `right_mcp_port`: TCP port for the host-side right MCP HTTP server.
/// `network_policy`: Controls which outbound HTTPS domains are allowed.
/// `host_ip`: Resolved IP of `host.docker.internal` from inside the sandbox.
///   When `Some`, uses the exact IP/32 in `allowed_ips`. When `None`, falls back
///   to common Docker network ranges (172.16.0.0/12 + 192.168.0.0/16).
///
/// Network policy allows outbound HTTPS (port 443) with TLS termination
/// so the OpenShell proxy can inspect traffic. The right MCP server on the host
/// is accessed via plain HTTP through the Docker bridge network.
pub fn generate_policy(
    right_mcp_port: u16,
    network_policy: &NetworkPolicy,
    host_ip: Option<std::net::IpAddr>,
) -> String {
    let network_section = match network_policy {
        NetworkPolicy::Permissive => r#"  outbound:
    endpoints:
      - host: "**.*"
        port: 443
        protocol: rest
        access: full
        tls: terminate
      - host: "**.*"
        port: 80
        protocol: rest
        access: full
    binaries:
      - path: "**""#
            .to_owned(),
        NetworkPolicy::Restrictive => {
            format!(
                "  anthropic:\n    endpoints:\n{}\n    binaries:\n      - path: \"**\"",
                restrictive_endpoints()
            )
        }
    };

    let allowed_ips = match host_ip {
        Some(ip) => format!("          - \"{ip}/32\""),
        None => "          - \"172.16.0.0/12\"\n          - \"192.168.0.0/16\"".to_owned(),
    };

    format!(
        r#"version: 1

filesystem_policy:
  include_workdir: true
  read_only:
    - /usr
    - /lib
    - /lib64
    - /etc
    - /proc
    - /dev/urandom
    - /dev/null
  read_write:
    - /tmp
    - /sandbox
    - /platform

landlock:
  compatibility: best_effort

process:
  run_as_user: sandbox
  run_as_group: sandbox

network_policies:
{network_section}

  right:
    endpoints:
      - host: "host.docker.internal"
        port: {right_mcp_port}
        allowed_ips:
{allowed_ips}
        protocol: rest
        access: full
    binaries:
      - path: "**"
"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generates_policy_with_right_mcp_port() {
        let policy = generate_policy(8100, &NetworkPolicy::Permissive, None);
        assert!(policy.contains("host.docker.internal"));
        assert!(policy.contains("8100"));
        assert!(policy.contains("172.16.0.0/12"));
        assert!(policy.contains("right:"));
        assert!(policy.contains("best_effort"));
        assert!(policy.contains("version: 1"));
    }

    #[test]
    fn allows_all_outbound_https_and_http() {
        let policy = generate_policy(8100, &NetworkPolicy::Permissive, None);
        assert!(policy.contains(r#"host: "**.*""#));
        assert!(policy.contains("port: 443"));
        assert!(policy.contains("port: 80"));
        assert!(policy.contains("tls: terminate"));
        assert!(policy.contains("outbound:"));
    }

    #[test]
    fn right_mcp_port_configurable() {
        let policy = generate_policy(9000, &NetworkPolicy::Permissive, None);
        assert!(policy.contains("9000"));
        assert!(!policy.contains("8100"));
    }

    /// OpenShell rejects bare `*` host wildcards — must use `*.example.com` or `*.*` patterns.
    #[test]
    fn no_bare_star_host_wildcards() {
        let policy = generate_policy(8100, &NetworkPolicy::Permissive, None);
        for line in policy.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("host:") {
                let host_val = trimmed.trim_start_matches("host:").trim().trim_matches('"');
                assert_ne!(
                    host_val, "*",
                    "bare '*' wildcard rejected by OpenShell — use '*.*' or '*.domain.com'"
                );
            }
        }
    }

    /// Policy YAML must be valid YAML and contain required OpenShell sections.
    #[test]
    fn policy_is_valid_yaml_with_required_sections() {
        let policy = generate_policy(8100, &NetworkPolicy::Permissive, None);
        let parsed: serde_json::Value = serde_saphyr::from_str(&policy)
            .expect("policy must be valid YAML");
        let obj = parsed.as_object().expect("policy root must be a mapping");
        assert!(obj.contains_key("version"), "missing 'version'");
        assert!(obj.contains_key("filesystem_policy"), "missing 'filesystem_policy'");
        assert!(obj.contains_key("network_policies"), "missing 'network_policies'");
        assert!(obj.contains_key("process"), "missing 'process'");
    }

    #[test]
    fn restrictive_policy_allows_only_anthropic_domains() {
        let policy = generate_policy(8100, &NetworkPolicy::Restrictive, None);
        assert!(policy.contains(r#"host: "*.anthropic.com""#));
        assert!(policy.contains(r#"host: "anthropic.com""#));
        assert!(policy.contains(r#"host: "*.claude.com""#));
        assert!(policy.contains(r#"host: "claude.com""#));
        assert!(policy.contains(r#"host: "*.claude.ai""#));
        assert!(policy.contains(r#"host: "claude.ai""#));
        assert!(!policy.contains(r#"host: "**.*""#), "restrictive must not contain wildcard");
    }

    #[test]
    fn permissive_policy_allows_all_https() {
        let policy = generate_policy(8100, &NetworkPolicy::Permissive, None);
        assert!(policy.contains(r#"host: "**.*""#));
        assert!(!policy.contains(r#"host: "*.anthropic.com""#), "permissive uses wildcard, not explicit domains");
    }

    #[test]
    fn restrictive_policy_is_valid_yaml() {
        let policy = generate_policy(8100, &NetworkPolicy::Restrictive, None);
        let parsed: serde_json::Value = serde_saphyr::from_str(&policy)
            .expect("restrictive policy must be valid YAML");
        let obj = parsed.as_object().expect("policy root must be a mapping");
        assert!(obj.contains_key("network_policies"));
    }

    #[test]
    fn restrictive_policy_has_no_bare_star_wildcards() {
        let policy = generate_policy(8100, &NetworkPolicy::Restrictive, None);
        for line in policy.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("host:") {
                let host_val = trimmed.trim_start_matches("host:").trim().trim_matches('"');
                assert_ne!(
                    host_val, "*",
                    "bare '*' wildcard rejected by OpenShell"
                );
            }
        }
    }

    #[test]
    fn host_ip_none_uses_fallback_ranges() {
        let policy = generate_policy(8100, &NetworkPolicy::Permissive, None);
        assert!(policy.contains("172.16.0.0/12"), "must include Docker bridge range");
        assert!(policy.contains("192.168.0.0/16"), "must include Docker Desktop range");
    }

    #[test]
    fn host_ip_some_uses_exact_ip() {
        let ip: std::net::IpAddr = "192.168.65.254".parse().unwrap();
        let policy = generate_policy(8100, &NetworkPolicy::Permissive, Some(ip));
        assert!(policy.contains("192.168.65.254/32"), "must use exact IP/32");
        assert!(!policy.contains("172.16.0.0/12"), "must not include fallback range");
    }

    #[test]
    fn host_ip_some_produces_valid_yaml() {
        let ip: std::net::IpAddr = "10.0.0.1".parse().unwrap();
        let policy = generate_policy(8100, &NetworkPolicy::Permissive, Some(ip));
        let _parsed: serde_json::Value = serde_saphyr::from_str(&policy)
            .expect("policy with dynamic IP must be valid YAML");
    }
}
