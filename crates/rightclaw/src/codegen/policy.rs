//! Generate OpenShell policy.yaml from agent configuration.

use crate::agent::types::NetworkPolicy;

/// Generate an OpenShell policy YAML string.
///
/// `right_mcp_port`: TCP port for the host-side right MCP HTTP server.
/// `network_policy`: Controls which outbound HTTPS domains are allowed.
///
/// Network policy allows outbound HTTPS (port 443) with TLS termination
/// so the OpenShell proxy can inspect traffic. The right MCP server on the host
/// is accessed via plain HTTP through the Docker bridge network.
pub fn generate_policy(right_mcp_port: u16, network_policy: &NetworkPolicy) -> String {
    let network_section = match network_policy {
        NetworkPolicy::Permissive => {
            r#"  outbound:
    endpoints:
      - host: "**.*"
        port: 443
        protocol: rest
        access: full
        tls: terminate
    binaries:
      - path: "**""#
                .to_owned()
        }
        NetworkPolicy::Restrictive => {
            r#"  anthropic:
    endpoints:
      - host: "*.anthropic.com"
        port: 443
        protocol: rest
        access: full
        tls: terminate
      - host: "anthropic.com"
        port: 443
        protocol: rest
        access: full
        tls: terminate
      - host: "*.claude.com"
        port: 443
        protocol: rest
        access: full
        tls: terminate
      - host: "claude.com"
        port: 443
        protocol: rest
        access: full
        tls: terminate
      - host: "*.claude.ai"
        port: 443
        protocol: rest
        access: full
        tls: terminate
    binaries:
      - path: "**""#
                .to_owned()
        }
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
          - "172.16.0.0/12"
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
        let policy = generate_policy(8100, &NetworkPolicy::Permissive);
        assert!(policy.contains("host.docker.internal"));
        assert!(policy.contains("8100"));
        assert!(policy.contains("172.16.0.0/12"));
        assert!(policy.contains("right:"));
        assert!(policy.contains("best_effort"));
        assert!(policy.contains("version: 1"));
    }

    #[test]
    fn allows_all_outbound_https() {
        let policy = generate_policy(8100, &NetworkPolicy::Permissive);
        assert!(policy.contains(r#"host: "**.*""#));
        assert!(policy.contains("port: 443"));
        assert!(policy.contains("tls: terminate"));
        assert!(policy.contains("outbound:"));
    }

    #[test]
    fn right_mcp_port_configurable() {
        let policy = generate_policy(9000, &NetworkPolicy::Permissive);
        assert!(policy.contains("9000"));
        assert!(!policy.contains("8100"));
    }

    /// OpenShell rejects bare `*` host wildcards — must use `*.example.com` or `*.*` patterns.
    #[test]
    fn no_bare_star_host_wildcards() {
        let policy = generate_policy(8100, &NetworkPolicy::Permissive);
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
        let policy = generate_policy(8100, &NetworkPolicy::Permissive);
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
        let policy = generate_policy(8100, &NetworkPolicy::Restrictive);
        assert!(policy.contains(r#"host: "*.anthropic.com""#));
        assert!(policy.contains(r#"host: "anthropic.com""#));
        assert!(policy.contains(r#"host: "*.claude.com""#));
        assert!(policy.contains(r#"host: "claude.com""#));
        assert!(policy.contains(r#"host: "*.claude.ai""#));
        assert!(!policy.contains(r#"host: "**.*""#), "restrictive must not contain wildcard");
    }

    #[test]
    fn permissive_policy_allows_all_https() {
        let policy = generate_policy(8100, &NetworkPolicy::Permissive);
        assert!(policy.contains(r#"host: "**.*""#));
        assert!(!policy.contains(r#"host: "*.anthropic.com""#), "permissive uses wildcard, not explicit domains");
    }

    #[test]
    fn restrictive_policy_is_valid_yaml() {
        let policy = generate_policy(8100, &NetworkPolicy::Restrictive);
        let parsed: serde_json::Value = serde_saphyr::from_str(&policy)
            .expect("restrictive policy must be valid YAML");
        let obj = parsed.as_object().expect("policy root must be a mapping");
        assert!(obj.contains_key("network_policies"));
    }

    #[test]
    fn restrictive_policy_has_no_bare_star_wildcards() {
        let policy = generate_policy(8100, &NetworkPolicy::Restrictive);
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
}
