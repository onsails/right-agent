//! Generate OpenShell policy.yaml from agent configuration.

use std::net::IpAddr;

use right_agent_config::NetworkPolicy;

/// Domains allowed in restrictive mode (Anthropic/Claude only).
const RESTRICTIVE_DOMAINS: &[&str] = &[
    "*.anthropic.com",
    "anthropic.com",
    "*.claude.com",
    "claude.com",
    "*.claude.ai",
    "claude.ai",
    "storage.googleapis.com",
];

fn restrictive_endpoints() -> String {
    RESTRICTIVE_DOMAINS
        .iter()
        .map(|host| {
            format!(
                r#"      - host: "{host}"
        port: 443
        protocol: rest
        access: full"#
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Ipv4Range {
    start: u32,
    end: u32,
}

const NON_PUBLIC_IPV4_CIDRS: &[(&str, u8)] = &[
    ("0.0.0.0", 8),
    ("10.0.0.0", 8),
    ("100.64.0.0", 10),
    ("127.0.0.0", 8),
    ("169.254.0.0", 16),
    ("172.16.0.0", 12),
    ("192.0.0.0", 24),
    ("192.0.2.0", 24),
    ("192.88.99.0", 24),
    ("192.168.0.0", 16),
    ("198.18.0.0", 15),
    ("198.51.100.0", 24),
    ("203.0.113.0", 24),
    ("224.0.0.0", 4),
    ("240.0.0.0", 4),
];

const PUBLIC_IPV6_CIDRS: &[&str] = &["2000::/3"];

fn ipv4_cidr_to_range(base: &str, prefix: u8) -> Ipv4Range {
    let base = u32::from(
        base.parse::<std::net::Ipv4Addr>()
            .expect("static IPv4 CIDR base must parse"),
    );
    let mask = if prefix == 0 {
        0
    } else {
        u32::MAX << (32 - prefix)
    };
    let start = base & mask;
    Ipv4Range {
        start,
        end: start | !mask,
    }
}

fn range_to_ipv4_cidrs(start: u32, end: u32) -> Vec<String> {
    let mut cidrs = Vec::new();
    let mut cursor = u64::from(start);
    let end = u64::from(end);

    while cursor <= end {
        let alignment = cursor & cursor.wrapping_neg();
        let mut block_size = if alignment == 0 {
            1_u64 << 32
        } else {
            alignment
        };
        let remaining = end - cursor + 1;
        while block_size > remaining {
            block_size >>= 1;
        }

        let prefix = 32 - block_size.trailing_zeros();
        cidrs.push(format!(
            "{}/{}",
            std::net::Ipv4Addr::from(cursor as u32),
            prefix
        ));
        cursor += block_size;
    }

    cidrs
}

fn public_ipv4_cidrs() -> Vec<String> {
    let mut ranges = NON_PUBLIC_IPV4_CIDRS
        .iter()
        .map(|(base, prefix)| ipv4_cidr_to_range(base, *prefix))
        .collect::<Vec<_>>();
    ranges.sort_by_key(|range| range.start);

    let mut merged = Vec::<Ipv4Range>::new();
    for range in ranges {
        if let Some(last) = merged.last_mut()
            && range.start <= last.end.saturating_add(1)
        {
            last.end = last.end.max(range.end);
            continue;
        }
        merged.push(range);
    }

    let mut cidrs = Vec::new();
    let mut cursor = 0_u32;
    for range in merged {
        if cursor < range.start {
            cidrs.extend(range_to_ipv4_cidrs(cursor, range.start - 1));
        }
        if range.end == u32::MAX {
            return cidrs;
        }
        cursor = cursor.max(range.end + 1);
    }
    cidrs.extend(range_to_ipv4_cidrs(cursor, u32::MAX));
    cidrs
}

pub fn public_web_allowed_ip_cidrs() -> Vec<String> {
    public_ipv4_cidrs()
        .into_iter()
        .chain(PUBLIC_IPV6_CIDRS.iter().map(|cidr| (*cidr).to_owned()))
        .collect()
}

fn public_web_allowed_ips_yaml(indent: usize) -> String {
    let pad = " ".repeat(indent);
    public_web_allowed_ip_cidrs()
        .into_iter()
        .map(|cidr| format!("{pad}- \"{cidr}\""))
        .collect::<Vec<_>>()
        .join("\n")
}

fn permissive_endpoints() -> String {
    let allowed_ips = public_web_allowed_ips_yaml(10);
    // The trailing `# right-providers: insert-above` line is the anchor used
    // by `providers_append_checked` to locate the correct insertion point.
    // Without it, the heuristic "find first endpoints:" picks whichever
    // network_policies sub-section is rendered first (`outbound` in
    // permissive mode, `anthropic` in restrictive mode) — and the latter
    // smuggles generic provider stanzas into the Anthropic-gated allowlist.
    format!(
        r#"      - port: 443
        allowed_ips:
{allowed_ips}
        tls: skip
      - port: 80
        allowed_ips:
{allowed_ips}
        tls: skip
      # right-providers: insert-above"#
    )
}

/// Right MCP host access mode for the rendered `right:` policy endpoint.
///
/// The two variants form a lifecycle pair: `BootstrapUnresolved` renders a
/// `right:` endpoint without `allowed_ips:` so the sandbox can be created
/// before `host.openshell.internal` is resolvable; `Resolved` carries the
/// concrete host IPs and is hot-applied via
/// `apply_exact_right_mcp_policy_for_sandbox` (or the inline flow in
/// `bot/src/lib.rs`) once the sandbox is READY. See ARCHITECTURE.md →
/// "OpenShell Policy Gotchas" → "Right MCP policy lifecycle".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostMcpAccess {
    /// Bootstrap mode used before sandbox creation: the rendered `right:`
    /// endpoint has no `allowed_ips:` block, so OpenShell's SSRF guard blocks
    /// sandbox-to-host MCP traffic. Callers MUST hot-apply a `Resolved`
    /// policy via `apply_exact_right_mcp_policy_for_sandbox` after the
    /// sandbox reaches READY and before any `claude -p` invocation;
    /// otherwise MCP is silently unreachable from inside the sandbox.
    /// See ARCHITECTURE.md → "OpenShell Policy Gotchas" → "Right MCP policy
    /// lifecycle".
    BootstrapUnresolved,
    Resolved(Vec<IpAddr>),
}

fn host_mcp_ip_cidr(ip: IpAddr) -> String {
    match ip {
        IpAddr::V4(ip) => format!("{ip}/32"),
        IpAddr::V6(ip) => format!("{ip}/128"),
    }
}

/// Render the `allowed_ips:` block for the Right MCP `right:` endpoint.
///
/// `BootstrapUnresolved` deliberately returns an empty string — no
/// `allowed_ips:` block is emitted, leaving the bootstrap policy
/// sandbox-creation-only. It is paired with `Resolved`, which carries the
/// concrete IPs and is hot-applied via
/// `apply_exact_right_mcp_policy_for_sandbox` once the sandbox is READY.
fn right_mcp_allowed_ips_yaml(host_mcp_access: HostMcpAccess) -> String {
    match host_mcp_access {
        HostMcpAccess::BootstrapUnresolved => String::new(),
        HostMcpAccess::Resolved(ips) => {
            assert!(
                !ips.is_empty(),
                "resolved Right MCP host access requires at least one IP"
            );
            let allowed_ips = ips
                .into_iter()
                .map(host_mcp_ip_cidr)
                .map(|cidr| format!("          - \"{cidr}\""))
                .collect::<Vec<_>>()
                .join("\n");
            format!("        allowed_ips:\n{allowed_ips}\n")
        }
    }
}

/// Generate an OpenShell policy YAML string.
///
/// `right_mcp_port`: TCP port for the host-side right MCP HTTP server.
/// `network_policy`: Controls which outbound public web or HTTPS domains are allowed.
/// `host_mcp_access`: Right MCP host access mode. Bootstrap mode is used before
///   the sandbox exists and omits guessed private ranges. Resolved mode is used
///   after resolving `host.openshell.internal` from inside the target sandbox.
///
/// Network policy allows outbound HTTP/HTTPS. Since OpenShell v0.0.30
/// L7 endpoints auto-detect TLS via ClientHello peek; `tls: terminate` and
/// `tls: passthrough` are deprecated and no longer written. Permissive public
/// web endpoints intentionally use `tls: skip` raw tunnels so normal public
/// internet request targets such as scoped npm metadata (`%2F`) bypass L7
/// parsing. The right MCP server on the host is accessed via plain HTTP
/// through the Docker bridge.
pub fn generate_policy(
    right_mcp_port: u16,
    network_policy: &NetworkPolicy,
    host_mcp_access: HostMcpAccess,
) -> String {
    let network_section = match network_policy {
        NetworkPolicy::Permissive => {
            format!(
                "  outbound:\n    endpoints:\n{}\n    binaries:\n      - path: \"**\"",
                permissive_endpoints()
            )
        }
        NetworkPolicy::Restrictive => {
            format!(
                "  anthropic:\n    endpoints:\n{}\n    binaries:\n      - path: \"**\"",
                restrictive_endpoints()
            )
        }
    };

    let right_mcp_allowed_ips = right_mcp_allowed_ips_yaml(host_mcp_access);

    // `/var/log` is in `read_only` because the OpenShell server appends it to
    // every stored policy. Omitting it makes `filesystem_policy_changed` flag
    // every fresh sandbox as drifted at bot startup.
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
    - /var/log
  read_write:
    - /dev/null
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
      - host: "host.openshell.internal"
        port: {right_mcp_port}
{right_mcp_allowed_ips}        protocol: rest
        access: full
    binaries:
      - path: "**"
"#
    )
}

/// Rewrite legacy generated permissive endpoints that OpenShell v0.0.37+
/// rejects (`host: "**.*"`) into public-web `allowed_ips` endpoints.
///
/// Returns `Ok(Some(yaml))` when it changed the document and `Ok(None)` when
/// the input has no legacy public-web endpoint.
pub fn migrate_legacy_permissive_policy_yaml(yaml: &str) -> miette::Result<Option<String>> {
    let mut doc: serde_json::Value = serde_saphyr::from_str(yaml)
        .map_err(|e| miette::miette!("failed to parse policy.yaml for migration: {e:#}"))?;

    let replacement_ips = serde_json::Value::Array(
        public_web_allowed_ip_cidrs()
            .into_iter()
            .map(serde_json::Value::String)
            .collect(),
    );

    let Some(policies) = doc
        .get_mut("network_policies")
        .and_then(serde_json::Value::as_object_mut)
    else {
        return Ok(None);
    };

    let mut changed = false;
    for (name, policy) in policies.iter_mut() {
        if name != "outbound" {
            continue;
        }

        let Some(endpoints) = policy
            .get_mut("endpoints")
            .and_then(serde_json::Value::as_array_mut)
        else {
            continue;
        };

        for endpoint in endpoints {
            let is_legacy_host =
                endpoint.get("host").and_then(serde_json::Value::as_str) == Some("**.*");
            let is_public_web_port = matches!(
                endpoint.get("port").and_then(serde_json::Value::as_u64),
                Some(80 | 443)
            );
            let is_generated_legacy_shape =
                endpoint.get("protocol").and_then(serde_json::Value::as_str) == Some("rest")
                    && endpoint.get("access").and_then(serde_json::Value::as_str) == Some("full");

            if is_legacy_host && is_public_web_port && is_generated_legacy_shape {
                let endpoint = endpoint.as_object_mut().ok_or_else(|| {
                    miette::miette!("policy.yaml contains a non-mapping endpoint")
                })?;
                endpoint.remove("host");
                endpoint
                    .entry("allowed_ips".to_owned())
                    .or_insert_with(|| replacement_ips.clone());
                changed = true;
            }
        }
    }

    if !changed {
        return Ok(None);
    }

    serde_saphyr::to_string(&doc)
        .map(Some)
        .map_err(|e| miette::miette!("failed to serialize migrated policy.yaml: {e:#}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bootstrap_right_mcp_policy_omits_broad_private_ranges() {
        let policy = generate_policy(
            8100,
            &NetworkPolicy::Permissive,
            HostMcpAccess::BootstrapUnresolved,
        );
        let parsed: serde_json::Value =
            serde_saphyr::from_str(&policy).expect("bootstrap policy must be valid YAML");
        let endpoint = &parsed["network_policies"]["right"]["endpoints"][0];

        assert_eq!(endpoint["host"].as_str(), Some("host.openshell.internal"));
        assert!(
            endpoint.get("allowed_ips").is_none(),
            "bootstrap Right MCP endpoint must not emit guessed private ranges"
        );
        assert!(!policy.contains("172.16.0.0/12"));
        assert!(!policy.contains("192.168.0.0/16"));
        assert!(!policy.contains("fc00::/7"));
    }

    #[test]
    fn resolved_right_mcp_policy_emits_ipv4_and_ipv6_exact_prefixes() {
        let policy = generate_policy(
            8100,
            &NetworkPolicy::Permissive,
            HostMcpAccess::Resolved(vec![
                "192.168.65.254".parse().unwrap(),
                "fdc4:f303:9324::254".parse().unwrap(),
            ]),
        );
        let parsed: serde_json::Value =
            serde_saphyr::from_str(&policy).expect("resolved policy must be valid YAML");
        let allowed_ips = parsed["network_policies"]["right"]["endpoints"][0]["allowed_ips"]
            .as_array()
            .expect("resolved Right MCP endpoint must include allowed_ips");

        assert!(
            allowed_ips
                .iter()
                .any(|cidr| cidr.as_str() == Some("192.168.65.254/32")),
            "IPv4 host aliases must be exact /32 CIDRs"
        );
        assert!(
            allowed_ips
                .iter()
                .any(|cidr| cidr.as_str() == Some("fdc4:f303:9324::254/128")),
            "IPv6 host aliases must be exact /128 CIDRs"
        );
        assert!(
            !allowed_ips
                .iter()
                .any(|cidr| cidr.as_str() == Some("172.16.0.0/12"))
        );
        assert!(
            !allowed_ips
                .iter()
                .any(|cidr| cidr.as_str() == Some("192.168.0.0/16"))
        );
    }

    #[test]
    #[should_panic(expected = "resolved Right MCP host access requires at least one IP")]
    fn resolved_right_mcp_policy_rejects_empty_ip_list() {
        let _ = generate_policy(
            8100,
            &NetworkPolicy::Permissive,
            HostMcpAccess::Resolved(Vec::new()),
        );
    }

    #[test]
    fn generates_policy_with_right_mcp_port() {
        let policy = generate_policy(
            8100,
            &NetworkPolicy::Permissive,
            HostMcpAccess::BootstrapUnresolved,
        );
        assert!(policy.contains("host.openshell.internal"));
        assert!(policy.contains("8100"));
        assert!(policy.contains("right:"));
        assert!(policy.contains("best_effort"));
        assert!(policy.contains("version: 1"));
    }

    #[test]
    fn permissive_policy_uses_public_allowed_ips() {
        let policy = generate_policy(
            8100,
            &NetworkPolicy::Permissive,
            HostMcpAccess::BootstrapUnresolved,
        );
        let parsed: serde_json::Value =
            serde_saphyr::from_str(&policy).expect("policy must be valid YAML");
        let outbound = &parsed["network_policies"]["outbound"];
        let endpoints = outbound["endpoints"]
            .as_array()
            .expect("outbound endpoints must be a list");

        assert_eq!(
            endpoints.len(),
            2,
            "permissive policy has HTTP and HTTPS endpoints"
        );
        assert!(
            !policy.contains(r#"host: "**.*""#),
            "OpenShell v0.0.37+ rejects TLD wildcard endpoints"
        );

        for port in [443_u64, 80] {
            let endpoint = endpoints
                .iter()
                .find(|endpoint| endpoint["port"].as_u64() == Some(port))
                .unwrap_or_else(|| panic!("missing permissive endpoint for port {port}"));
            assert!(
                endpoint.get("host").is_none(),
                "public-web endpoint must be hostless so DNS names are not filtered by TLD wildcard"
            );
            let allowed_ips = endpoint["allowed_ips"]
                .as_array()
                .expect("public-web endpoint must use allowed_ips");
            assert!(
                allowed_ips
                    .iter()
                    .any(|cidr| cidr.as_str() == Some("1.0.0.0/8")),
                "public-web IPv4 CIDRs must include normal public ranges"
            );
            assert!(
                allowed_ips
                    .iter()
                    .any(|cidr| cidr.as_str() == Some("2000::/3")),
                "public-web IPv6 CIDRs must include global unicast"
            );
            for forbidden in [
                "0.0.0.0/0",
                "10.0.0.0/8",
                "127.0.0.0/8",
                "169.254.0.0/16",
                "172.16.0.0/12",
                "192.168.0.0/16",
            ] {
                assert!(
                    !allowed_ips
                        .iter()
                        .any(|cidr| cidr.as_str() == Some(forbidden)),
                    "public-web endpoint must not allow {forbidden}"
                );
            }
        }
    }

    #[test]
    fn permissive_public_web_endpoints_are_l4_only() {
        let policy = generate_policy(
            8100,
            &NetworkPolicy::Permissive,
            HostMcpAccess::BootstrapUnresolved,
        );
        let parsed: serde_json::Value =
            serde_saphyr::from_str(&policy).expect("policy must be valid YAML");
        let endpoints = parsed["network_policies"]["outbound"]["endpoints"]
            .as_array()
            .expect("outbound endpoints must be a list");

        for port in [443_u64, 80] {
            let endpoint = endpoints
                .iter()
                .find(|endpoint| endpoint["port"].as_u64() == Some(port))
                .unwrap_or_else(|| panic!("missing permissive endpoint for port {port}"));
            assert!(
                endpoint.get("protocol").is_none(),
                "permissive public-web endpoint on port {port} must be L4-only; \
                 L7 REST inspection rejects scoped npm request targets containing %2F",
            );
            assert!(
                endpoint.get("access").is_none(),
                "permissive public-web endpoint on port {port} must not set L7 access presets",
            );
            assert_eq!(
                endpoint["tls"].as_str(),
                Some("skip"),
                "permissive public-web endpoint on port {port} must skip TLS termination; \
                 OpenShell auto-termination still routes encoded scoped npm paths through L7",
            );
        }
    }

    /// Regression: OpenShell v0.0.30 deprecated `tls: terminate` and
    /// `tls: passthrough`. Restrictive L7 endpoints rely on auto-detect and
    /// must not emit `tls`; permissive public-web endpoints intentionally use
    /// `tls: skip` to bypass L7 request-target parsing for raw internet access.
    #[test]
    fn does_not_emit_deprecated_tls_modes() {
        let permissive = generate_policy(
            8100,
            &NetworkPolicy::Permissive,
            HostMcpAccess::BootstrapUnresolved,
        );
        let restrictive = generate_policy(
            8100,
            &NetworkPolicy::Restrictive,
            HostMcpAccess::BootstrapUnresolved,
        );
        for (name, policy) in [("permissive", &permissive), ("restrictive", &restrictive)] {
            assert!(
                !policy.contains("tls: terminate") && !policy.contains("tls: passthrough"),
                "{name} policy must not emit deprecated tls modes",
            );
        }
        assert!(
            !restrictive.contains("tls:"),
            "restrictive L7 policy must use TLS auto-detect rather than explicit tls fields",
        );
    }

    #[test]
    fn right_mcp_port_configurable() {
        let policy = generate_policy(
            9000,
            &NetworkPolicy::Permissive,
            HostMcpAccess::BootstrapUnresolved,
        );
        assert!(policy.contains("9000"));
        assert!(!policy.contains("8100"));
    }

    /// OpenShell v0.0.37+ rejects TLD-wide wildcard hosts.
    #[test]
    fn no_tld_wide_host_wildcards() {
        let policy = generate_policy(
            8100,
            &NetworkPolicy::Permissive,
            HostMcpAccess::BootstrapUnresolved,
        );
        for line in policy.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("host:") {
                let host_val = trimmed.trim_start_matches("host:").trim().trim_matches('"');
                assert_ne!(host_val, "*", "bare '*' wildcard rejected by OpenShell");
                assert_ne!(
                    host_val, "**.*",
                    "TLD-wide '**.*' wildcard rejected by OpenShell v0.0.37+"
                );
            }
        }
    }

    /// Policy YAML must be valid YAML and contain required OpenShell sections.
    #[test]
    fn policy_is_valid_yaml_with_required_sections() {
        let policy = generate_policy(
            8100,
            &NetworkPolicy::Permissive,
            HostMcpAccess::BootstrapUnresolved,
        );
        let parsed: serde_json::Value =
            serde_saphyr::from_str(&policy).expect("policy must be valid YAML");
        let obj = parsed.as_object().expect("policy root must be a mapping");
        assert!(obj.contains_key("version"), "missing 'version'");
        assert!(
            obj.contains_key("filesystem_policy"),
            "missing 'filesystem_policy'"
        );
        assert!(
            obj.contains_key("network_policies"),
            "missing 'network_policies'"
        );
        assert!(obj.contains_key("process"), "missing 'process'");
    }

    #[test]
    fn restrictive_policy_allows_only_anthropic_domains() {
        let policy = generate_policy(
            8100,
            &NetworkPolicy::Restrictive,
            HostMcpAccess::BootstrapUnresolved,
        );
        assert!(policy.contains(r#"host: "*.anthropic.com""#));
        assert!(policy.contains(r#"host: "anthropic.com""#));
        assert!(policy.contains(r#"host: "*.claude.com""#));
        assert!(policy.contains(r#"host: "claude.com""#));
        assert!(policy.contains(r#"host: "*.claude.ai""#));
        assert!(policy.contains(r#"host: "claude.ai""#));
        assert!(policy.contains(r#"host: "storage.googleapis.com""#));
        assert!(
            !policy.contains(r#"host: "**.*""#),
            "restrictive must not contain top-level wildcard"
        );
    }

    #[test]
    fn permissive_policy_allows_public_web_without_domain_wildcard() {
        let policy = generate_policy(
            8100,
            &NetworkPolicy::Permissive,
            HostMcpAccess::BootstrapUnresolved,
        );
        assert!(
            !policy.contains(r#"host: "**.*""#),
            "permissive policy must not emit OpenShell-rejected TLD wildcard"
        );
        assert!(
            !policy.contains(r#"host: "*.anthropic.com""#),
            "permissive policy uses public allowed_ips, not restrictive domain entries"
        );
        assert!(policy.contains("allowed_ips:"));
        assert!(policy.contains("port: 443"));
        assert!(policy.contains("port: 80"));
    }

    #[test]
    fn restrictive_policy_is_valid_yaml() {
        let policy = generate_policy(
            8100,
            &NetworkPolicy::Restrictive,
            HostMcpAccess::BootstrapUnresolved,
        );
        let parsed: serde_json::Value =
            serde_saphyr::from_str(&policy).expect("restrictive policy must be valid YAML");
        let obj = parsed.as_object().expect("policy root must be a mapping");
        assert!(obj.contains_key("network_policies"));
    }

    #[test]
    fn restrictive_policy_has_no_bare_star_wildcards() {
        let policy = generate_policy(
            8100,
            &NetworkPolicy::Restrictive,
            HostMcpAccess::BootstrapUnresolved,
        );
        for line in policy.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("host:") {
                let host_val = trimmed.trim_start_matches("host:").trim().trim_matches('"');
                assert_ne!(host_val, "*", "bare '*' wildcard rejected by OpenShell");
            }
        }
    }

    #[test]
    fn migrates_legacy_permissive_wildcard_policy() {
        let legacy = r#"version: 1
network_policies:
  outbound:
    endpoints:
      - host: "**.*"
        port: 443
        protocol: rest
        access: full
      - host: "**.*"
        port: 80
        protocol: rest
        access: full
    binaries:
      - path: "**"
  right:
    endpoints:
      - host: "host.openshell.internal"
        port: 8100
        allowed_ips:
          - "192.168.65.254/32"
        protocol: rest
        access: full
    binaries:
      - path: "**"
"#;

        let migrated = migrate_legacy_permissive_policy_yaml(legacy)
            .expect("migration must parse")
            .expect("legacy wildcard must be migrated");

        assert!(
            !migrated.contains(r#"host: "**.*""#),
            "legacy TLD wildcard must be removed"
        );

        let parsed: serde_json::Value =
            serde_saphyr::from_str(&migrated).expect("migrated policy must be valid YAML");
        let endpoints = parsed["network_policies"]["outbound"]["endpoints"]
            .as_array()
            .expect("outbound endpoints must remain a list");

        for port in [443_u64, 80] {
            let endpoint = endpoints
                .iter()
                .find(|endpoint| endpoint["port"].as_u64() == Some(port))
                .unwrap_or_else(|| panic!("missing migrated endpoint for port {port}"));
            assert!(endpoint.get("host").is_none());
            assert!(
                endpoint["allowed_ips"]
                    .as_array()
                    .expect("allowed_ips must be a list")
                    .iter()
                    .any(|cidr| cidr.as_str() == Some("1.0.0.0/8"))
            );
        }

        assert_eq!(
            parsed["network_policies"]["right"]["endpoints"][0]["host"].as_str(),
            Some("host.openshell.internal"),
            "non-public-web host endpoint must be preserved"
        );
    }

    #[test]
    fn migration_is_noop_for_current_policy() {
        let policy = generate_policy(
            8100,
            &NetworkPolicy::Permissive,
            HostMcpAccess::BootstrapUnresolved,
        );
        let migrated =
            migrate_legacy_permissive_policy_yaml(&policy).expect("current policy must parse");
        assert!(migrated.is_none(), "current policy must not be rewritten");
    }

    #[test]
    fn migration_removes_legacy_host_and_preserves_existing_allowed_ips() {
        let custom = r#"version: 1
network_policies:
  outbound:
    endpoints:
      - host: "**.*"
        port: 443
        allowed_ips:
          - "8.8.8.8/32"
        protocol: rest
        access: full
    binaries:
      - path: "**"
"#;

        let migrated = migrate_legacy_permissive_policy_yaml(custom)
            .expect("custom policy must parse")
            .expect("legacy wildcard host must be removed");

        let parsed: serde_json::Value =
            serde_saphyr::from_str(&migrated).expect("migrated policy must be valid YAML");
        let endpoint = &parsed["network_policies"]["outbound"]["endpoints"][0];
        assert!(endpoint.get("host").is_none());
        assert!(
            endpoint["allowed_ips"]
                .as_array()
                .expect("allowed_ips must remain a list")
                .iter()
                .any(|cidr| cidr.as_str() == Some("8.8.8.8/32")),
            "migration must preserve existing allowed_ips"
        );
        assert!(
            !endpoint["allowed_ips"]
                .as_array()
                .expect("allowed_ips must remain a list")
                .iter()
                .any(|cidr| cidr.as_str() == Some("1.0.0.0/8")),
            "migration must not replace existing allowed_ips"
        );
    }

    #[test]
    fn public_web_ipv4_cidrs_do_not_overlap_non_public_ranges() {
        fn overlaps(left: Ipv4Range, right: Ipv4Range) -> bool {
            left.start <= right.end && right.start <= left.end
        }

        let denied_ranges = NON_PUBLIC_IPV4_CIDRS
            .iter()
            .map(|(base, prefix)| ipv4_cidr_to_range(base, *prefix))
            .collect::<Vec<_>>();

        for cidr in public_web_allowed_ip_cidrs() {
            if cidr.contains(':') {
                continue;
            }
            let (base, prefix) = cidr
                .split_once('/')
                .unwrap_or_else(|| panic!("CIDR must contain prefix: {cidr}"));
            let public_range = ipv4_cidr_to_range(
                base,
                prefix
                    .parse::<u8>()
                    .unwrap_or_else(|_| panic!("CIDR prefix must parse: {cidr}")),
            );

            for denied_range in &denied_ranges {
                assert!(
                    !overlaps(public_range, *denied_range),
                    "public CIDR {cidr} overlaps denied range {denied_range:?}"
                );
            }
        }
    }

    #[test]
    fn bootstrap_host_access_omits_fallback_ranges() {
        let policy = generate_policy(
            8100,
            &NetworkPolicy::Permissive,
            HostMcpAccess::BootstrapUnresolved,
        );
        assert!(
            !policy.contains("172.16.0.0/12"),
            "bootstrap policy must not include Docker bridge range"
        );
        assert!(
            !policy.contains("192.168.0.0/16"),
            "bootstrap policy must not include Docker Desktop range"
        );
    }

    #[test]
    fn resolved_host_access_uses_exact_ipv4_ip() {
        let ip: std::net::IpAddr = "192.168.65.254".parse().unwrap();
        let policy = generate_policy(
            8100,
            &NetworkPolicy::Permissive,
            HostMcpAccess::Resolved(vec![ip]),
        );
        assert!(policy.contains("192.168.65.254/32"), "must use exact IP/32");
        assert!(
            !policy.contains("172.16.0.0/12"),
            "must not include fallback range"
        );
    }

    #[test]
    fn resolved_host_access_produces_valid_yaml() {
        let ip: std::net::IpAddr = "10.0.0.1".parse().unwrap();
        let policy = generate_policy(
            8100,
            &NetworkPolicy::Permissive,
            HostMcpAccess::Resolved(vec![ip]),
        );
        let _parsed: serde_json::Value =
            serde_saphyr::from_str(&policy).expect("policy with dynamic IP must be valid YAML");
    }
}

// ---------------------------------------------------------------------------
// Provider-managed endpoint append/strip
// ---------------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum PolicyConflict {
    #[error(
        "host {host} is configured as raw tunnel (tls: skip) — cannot terminate for substitution"
    )]
    RawTunnel { host: String },
}

/// Returns `true` when `line` opens a YAML key at the same indentation as
/// `endpoints:` (4 spaces) or a shallower outer level (2 spaces, 0 spaces),
/// i.e. the line marks the end of the current endpoints list.
///
/// Examples that match:
///   `    binaries:`        — 4-space sibling of `endpoints:`
///   `  right:`             — 2-space sibling of `outbound:`
///   `network_policies:`    — top-level key
///
/// Examples that do NOT match:
///   `      - port: 443`    — list item (leading dash)
///   `        tls: skip`    — 8-space sub-key inside a list item
///   `      # comment`      — comment line (not a YAML key)
///   `    - host: ...`      — list item at 4-space indent (legacy stanza form)
///
/// This is the load-bearing stop condition for legacy policies that pre-date
/// the `# right-providers: insert-above` anchor. Without it, both
/// `providers_append`'s fallback (looking for end-of-list) and
/// `providers_strip`'s walker (consuming the stanza) would treat sibling
/// keys like `    binaries:` as continuation of the endpoints list and
/// corrupt the surrounding YAML structure.
fn is_endpoints_sibling_or_shallower_key(line: &str) -> bool {
    let body = line.trim_end_matches(['\r', '\n']);
    if body.is_empty() {
        return false;
    }
    let indent = body.bytes().take_while(|b| *b == b' ').count();
    // endpoints: itself sits at 4-space indent in generate_policy output.
    // A "sibling or shallower" line has indent ≤ 4.
    if indent > 4 {
        return false;
    }
    let rest = &body[indent..];
    // Reject list items, comments, document separators, anything that is
    // clearly not an identifier-keyed map entry.
    let first = match rest.chars().next() {
        Some(c) => c,
        None => return false,
    };
    if !(first.is_ascii_alphabetic() || first == '_') {
        return false;
    }
    // Identifier ends at the first non-[A-Za-z0-9_-] char; require ':' to
    // follow immediately after.
    let id_end = rest
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '-'))
        .unwrap_or(rest.len());
    if id_end == 0 {
        return false;
    }
    rest[id_end..].starts_with(':')
}

/// Append a TLS-terminated endpoint for `host` tagged with the provider name,
/// unless an entry for the same domain already exists.
///
/// Panics on conflict — use [`providers_append_checked`] when you need to
/// handle conflicts explicitly.
pub fn providers_append(
    policy: &str,
    provider_name: &str,
    host: &str,
    path_prefix: Option<&str>,
) -> String {
    providers_append_checked(policy, provider_name, host, path_prefix)
        .unwrap_or_else(|e| panic!("policy conflict: {e:#}"))
}

/// Fold an agent's generic-provider host stanzas onto a rendered policy.
///
/// For each `ProviderType::Generic` entry with a `generic` config, inserts a
/// TLS-terminating REST endpoint above the `# right-providers: insert-above`
/// anchor via [`providers_append_checked`]. Idempotent; a no-op when the policy
/// has no anchor (restrictive mode) or the list is empty. This is what makes
/// every full policy regeneration provider-aware so the network policy is
/// reconstructable from `agent.yaml` on every regen.
pub fn apply_provider_stanzas(
    policy: &str,
    providers: &[right_agent_config::ProviderEntry],
) -> Result<String, PolicyConflict> {
    // Restrictive policies have no anchor — folding is a no-op there.
    // The provider API already rejects generic + restrictive
    // (`NetworkPolicyForbidsGeneric`), so nothing to insert.
    const PROVIDERS_ANCHOR: &str = "# right-providers: insert-above";
    if !policy.contains(PROVIDERS_ANCHOR) {
        return Ok(policy.to_string());
    }
    let mut out = policy.to_string();
    for entry in providers {
        if !matches!(entry.type_, right_agent_config::ProviderType::Generic) {
            continue;
        }
        let Some(g) = entry.generic.as_ref() else {
            continue;
        };
        out = providers_append_checked(
            &out,
            &entry.name,
            &g.upstream_host,
            g.upstream_path_prefix.as_deref(),
        )?;
    }
    Ok(out)
}

/// Like [`providers_append`] but returns `Err(PolicyConflict)` instead of
/// panicking when the host is already configured as a raw tunnel.
pub fn providers_append_checked(
    policy: &str,
    provider_name: &str,
    host: &str,
    path_prefix: Option<&str>,
) -> Result<String, PolicyConflict> {
    // Line-anchored match so a prefix-collision host (e.g. existing
    // "api.openai.com.evil.tld") doesn't satisfy the idempotency check
    // for "api.openai.com" and cause us to silently skip the real add.
    let host_marker = format!("- host: {host}");
    let found_match = policy.match_indices(&host_marker).find(|(idx, marker)| {
        let after = idx + marker.len();
        after == policy.len() || matches!(policy.as_bytes().get(after), Some(b'\n' | b'\r'))
    });
    if let Some((idx, _)) = found_match {
        let window_end = (idx + 400).min(policy.len());
        let window = &policy[idx..window_end];
        if window.contains("tls: skip") {
            return Err(PolicyConflict::RawTunnel {
                host: host.to_string(),
            });
        }
        // Domain already present as a TLS-terminated endpoint — idempotent.
        return Ok(policy.to_string());
    }

    let path_line = path_prefix
        .map(|p| format!("      path: {p}\n"))
        .unwrap_or_default();
    let stanza = format!(
        "    # managed-by: right-providers:{provider_name}\n    - host: {host}\n      port: 443\n      protocol: rest\n      access: full\n{path_line}"
    );

    // Preferred path: insert immediately above the sentinel anchor emitted
    // by `generate_policy` inside `network_policies.outbound.endpoints`.
    // Generic providers are only ever appended to permissive policies — the
    // provider API rejects generic + restrictive (`NetworkPolicyForbidsGeneric`),
    // and restrictive mode renders no anchor, so this branch is the production
    // path. The anchor line's leading whitespace defines the marker column;
    // YAML list items ("- host: ...") share that column, value lines sit two
    // columns deeper, matching the real policy's 6-space-indented list-item
    // style. Endpoints use OpenShell's `host`/`port` keys (not `domain`, which
    // v0.0.50 rejects as an unknown field).
    const PROVIDERS_ANCHOR: &str = "# right-providers: insert-above";
    if let Some(anchor_idx) = policy.find(PROVIDERS_ANCHOR) {
        let line_start = policy[..anchor_idx].rfind('\n').map(|i| i + 1).unwrap_or(0);
        let marker_indent_len = anchor_idx - line_start;
        let marker_indent = " ".repeat(marker_indent_len);
        let value_indent = " ".repeat(marker_indent_len + 2);
        let path_line_anchored = path_prefix
            .map(|p| format!("{value_indent}path: {p}\n"))
            .unwrap_or_default();
        let stanza_anchored = format!(
            "{marker_indent}# managed-by: right-providers:{provider_name}\n\
             {marker_indent}- host: {host}\n\
             {value_indent}port: 443\n\
             {value_indent}protocol: rest\n\
             {value_indent}access: full\n\
             {path_line_anchored}"
        );
        let mut out = String::with_capacity(policy.len() + stanza_anchored.len());
        out.push_str(&policy[..line_start]);
        out.push_str(&stanza_anchored);
        out.push_str(&policy[line_start..]);
        return Ok(out);
    }

    let Some(endpoints_idx) = policy.find("endpoints:") else {
        // No endpoints section at all — synthesise a minimal network block.
        return Ok(format!("{policy}\nnetwork:\n  endpoints:\n{stanza}"));
    };

    // Find the byte offset of the end of the endpoints list so we can append
    // inside it rather than after the whole section. For legacy policies on
    // disk that pre-date the `# right-providers: insert-above` anchor, we
    // must stop at a sibling key (e.g. `    binaries:`) or any shallower
    // top-level key — otherwise the stanza is appended at end-of-file (or
    // worse, the end-of-list marker is conflated with non-endpoint content).
    // Use split_inclusive('\n') so each chunk includes its actual terminator
    // ('\n' or '\r\n'), giving correct byte lengths for both LF and CRLF files.
    let after_endpoints = &policy[endpoints_idx..];
    let list_end_line = after_endpoints
        .split_inclusive('\n')
        .enumerate()
        .skip(1) // skip the "endpoints:" line itself
        .find(|(_, l)| {
            let body = l.trim_end_matches(['\r', '\n']);
            if body.is_empty() {
                return false;
            }
            if !body.starts_with(' ') {
                // Un-indented line — left the network section entirely.
                return true;
            }
            // A line that opens a sibling/shallower YAML key (e.g.
            // `    binaries:` next to `    endpoints:`, or `  right:` next
            // to `  outbound:`) marks the end of the endpoints list.
            is_endpoints_sibling_or_shallower_key(body)
        })
        .map(|(i, _)| i)
        .unwrap_or_else(|| after_endpoints.split_inclusive('\n').count());

    let mut byte_offset = endpoints_idx;
    let mut i = 0;
    for line in after_endpoints.split_inclusive('\n') {
        if i == list_end_line {
            break;
        }
        byte_offset += line.len();
        i += 1;
    }

    let mut out = String::with_capacity(policy.len() + stanza.len());
    out.push_str(&policy[..byte_offset]);
    out.push_str(&stanza);
    out.push_str(&policy[byte_offset..]);
    Ok(out)
}

/// Remove the managed-by tag comment and its associated endpoint stanza for
/// `provider_name` from the policy YAML string.
pub fn providers_strip(policy: &str, provider_name: &str, _host: &str) -> String {
    let tag = format!("# managed-by: right-providers:{provider_name}");
    let Some(tag_idx) = policy.find(&tag) else {
        return policy.to_string();
    };

    // Start of the comment line (include leading indentation).
    let line_start = policy[..tag_idx].rfind('\n').map(|i| i + 1).unwrap_or(0);

    // Walk forward past the comment line and all indented continuation lines
    // that belong to this stanza (lines that start with spaces, i.e. deeper
    // indented YAML content following the `- host:` entry).
    let mut end_byte = tag_idx + tag.len();
    // Consume the rest of the tag line.
    if let Some(nl) = policy[end_byte..].find('\n') {
        end_byte += nl + 1;
    }

    // Consume subsequent lines that are part of this stanza (indented ≥ 4 spaces
    // or blank), but stop as soon as we see another `# managed-by:` marker so
    // that adjacent provider stanzas are not consumed as collateral.
    loop {
        let remaining = &policy[end_byte..];
        let next_line = remaining.lines().next().unwrap_or("");
        if next_line.is_empty() {
            // Blank line — stop here to avoid eating unrelated blank separators.
            break;
        }
        if !next_line.starts_with("    ") {
            // Un-indented line — left the network/sandbox block entirely.
            break;
        }
        // Another managed-by marker means the next provider's stanza starts here.
        if next_line.trim_start().starts_with("# managed-by:") {
            break;
        }
        // The sentinel anchor emitted by `generate_policy` must not be
        // consumed as part of a provider stanza, otherwise future appends
        // lose their insertion point.
        if next_line.trim_start().starts_with("# right-providers:") {
            break;
        }
        // A line that opens a sibling/shallower YAML key (e.g.
        // `    binaries:` next to `    endpoints:`) is NOT part of the
        // endpoints list, even if it is 4-space indented. Without this
        // stop, legacy policies (generated before the anchor existed) lose
        // their `outbound.binaries` block on strip, which OpenShell then
        // rejects or treats as zero-egress. See AGENTS.md →
        // "Upgrade-friendly design".
        if is_endpoints_sibling_or_shallower_key(next_line) {
            break;
        }
        end_byte += next_line.len() + 1;
    }

    // `host` is available for caller context; the strip boundary is determined
    // by the next managed-by marker rather than by matching the domain value.

    let mut out = String::with_capacity(policy.len());
    out.push_str(&policy[..line_start]);
    out.push_str(&policy[end_byte..]);
    out
}
