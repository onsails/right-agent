//! Generate OpenShell policy.yaml from agent configuration.

/// Generate an OpenShell policy YAML string.
///
/// `rightmemory_port`: TCP port for the host-side rightmemory HTTP server.
/// `external_mcp_servers`: (name, domain) pairs for external MCP servers the agent needs.
pub fn generate_policy(rightmemory_port: u16, external_mcp_servers: &[(&str, &str)]) -> String {
    let mut policy = format!(
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
  anthropic:
    endpoints:
      - host: "*.anthropic.com"
        port: 443
        protocol: rest
        access: full
      - host: "anthropic.com"
        port: 443
        protocol: rest
        access: full
      - host: "*.claude.ai"
        port: 443
        protocol: rest
        access: full
      - host: "claude.com"
        port: 443
        protocol: rest
        access: full
      - host: "*.claude.com"
        port: 443
        protocol: rest
        access: full
    binaries:
      - path: "/sandbox/**"

  rightmemory:
    endpoints:
      - host: "host.docker.internal"
        port: {rightmemory_port}
        protocol: rest
        access: full
    binaries:
      - path: "/sandbox/**"
"#
    );

    for (name, domain) in external_mcp_servers {
        policy.push_str(&format!(
            r#"
  {name}:
    endpoints:
      - host: "{domain}"
        port: 443
        protocol: rest
        access: full
    binaries:
      - path: "/sandbox/**"
"#
        ));
    }

    policy
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generates_policy_with_rightmemory_port() {
        let policy = generate_policy(8100, &[]);
        assert!(policy.contains("host.docker.internal"));
        assert!(policy.contains("8100"));
        assert!(policy.contains("*.anthropic.com"));
        assert!(policy.contains("*.claude.ai"));
        assert!(policy.contains("best_effort"));
        assert!(policy.contains("version: 1"));
    }

    #[test]
    fn adds_external_mcp_domains() {
        let policy = generate_policy(
            8100,
            &[("notion", "mcp.notion.com"), ("linear", "mcp.linear.app")],
        );
        assert!(policy.contains("mcp.notion.com"));
        assert!(policy.contains("mcp.linear.app"));
        assert!(policy.contains("notion:"));
        assert!(policy.contains("linear:"));
    }

    #[test]
    fn base_policy_has_no_extra_domains() {
        let policy = generate_policy(9000, &[]);
        assert!(!policy.contains("mcp.notion.com"));
        assert!(policy.contains("9000"));
    }
}
