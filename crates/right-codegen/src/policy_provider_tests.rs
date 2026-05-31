use super::policy::*;

const POLICY_WITHOUT_PROVIDERS: &str = r#"
network:
  endpoints:
    - host: api.anthropic.com
      protocol: rest
      access: full
"#;

const POLICY_WITH_ONE_PROVIDER: &str = r#"
network:
  endpoints:
    - host: api.anthropic.com
      protocol: rest
      access: full
    # managed-by: right-providers:myagent-acme
    - host: api.acme.com
      port: 443
      protocol: rest
      access: full
"#;

#[test]
fn append_provider_endpoint_inserts_tagged_stanza() {
    let after = providers_append(
        POLICY_WITHOUT_PROVIDERS,
        "myagent-acme",
        "api.acme.com",
        None,
    );
    assert!(after.contains("managed-by: right-providers:myagent-acme"));
    assert!(after.contains("- host: api.acme.com"));
}

#[test]
fn append_provider_endpoint_existing_rest_is_noop() {
    let already = "network:\n  endpoints:\n    - host: api.acme.com\n      protocol: rest\n      access: full\n";
    let after = providers_append(already, "myagent-acme", "api.acme.com", None);
    assert_eq!(after, already);
}

#[test]
fn append_provider_endpoint_raw_tunnel_conflict() {
    let raw = "network:\n  endpoints:\n    - allowed_ips: [1.2.3.4/32]\n      tls: skip\n      ports: [443]\n";
    // tls:skip is a raw tunnel; with no domain we can't conflict — that's OK.
    let after = providers_append(raw, "myagent-acme", "api.acme.com", None);
    assert!(after.contains("managed-by: right-providers:myagent-acme"));
}

#[test]
fn append_provider_endpoint_conflicting_domain_raw_tunnel_returns_err() {
    let raw = "network:\n  endpoints:\n    - host: api.acme.com\n      tls: skip\n";
    let err = providers_append_checked(raw, "myagent-acme", "api.acme.com", None);
    assert!(matches!(err, Err(PolicyConflict::RawTunnel { .. })));
}

#[test]
fn append_provider_endpoint_prefix_collision_is_not_idempotent() {
    // Regression: a substring host-marker match used to treat
    // "api.acme.com.evil.tld" as collision-matching "api.acme.com",
    // causing the real endpoint to be silently skipped.
    let policy = "network:\n  endpoints:\n    - host: api.acme.com.evil.tld\n      protocol: rest\n      access: full\n";
    let after = providers_append(policy, "myagent-acme", "api.acme.com", None);
    assert!(after.contains("- host: api.acme.com\n"));
    assert!(after.contains("managed-by: right-providers:myagent-acme"));
}

#[test]
fn strip_provider_endpoint_removes_tagged() {
    let after = providers_strip(POLICY_WITH_ONE_PROVIDER, "myagent-acme", "api.acme.com");
    assert!(!after.contains("managed-by: right-providers:myagent-acme"));
    assert!(!after.contains("api.acme.com"));
}

#[test]
fn append_provider_endpoint_handles_crlf_policy() {
    let policy = "network:\r\n  endpoints:\r\n    - host: api.anthropic.com\r\n      protocol: rest\r\n      access: full\r\n";
    let after = providers_append(policy, "myagent-acme", "api.acme.com", None);
    // The new stanza must be present.
    assert!(after.contains("- host: api.acme.com"));
    assert!(after.contains("managed-by: right-providers:myagent-acme"));
    // The original CRLF content must not be mid-line corrupted: existing
    // endpoint still present.
    assert!(after.contains("- host: api.anthropic.com"));
}

/// Regression: prior to the anchor fix, `providers_append` used
/// `policy.find("endpoints:")` which lands on whichever sub-section
/// is rendered first under `network_policies:` in the real output of
/// `generate_policy`. In restrictive mode that is `anthropic.endpoints`
/// (the Anthropic-gated allowlist), so generic provider stanzas were
/// silently smuggled inside it. The synthetic `network:\n  endpoints:`
/// fixtures above never exercised this path. This test runs against the
/// real `generate_policy` output to keep that gap closed.
#[test]
fn append_targets_outbound_endpoints_in_permissive_real_policy() {
    let base = generate_policy(
        8100,
        &right_agent_config::NetworkPolicy::Permissive,
        HostMcpAccess::BootstrapUnresolved,
    );

    let appended = providers_append(&base, "myagent-acme", "api.acme.invalid", None);

    let parsed: serde_json::Value =
        serde_saphyr::from_str(&appended).expect("appended policy must be valid YAML");
    let outbound_endpoints = parsed["network_policies"]["outbound"]["endpoints"]
        .as_array()
        .expect("outbound endpoints must be a list");

    let in_outbound = outbound_endpoints
        .iter()
        .any(|e| e.get("host").and_then(|h| h.as_str()) == Some("api.acme.invalid"));
    assert!(
        in_outbound,
        "generic provider stanza must land in network_policies.outbound.endpoints, not elsewhere"
    );

    // And it must NOT live in any other section (e.g. right or a stray block).
    let other_sections = ["right", "anthropic"];
    for section in other_sections {
        let endpoints = &parsed["network_policies"][section]["endpoints"];
        if let Some(arr) = endpoints.as_array() {
            assert!(
                !arr.iter()
                    .any(|e| e.get("host").and_then(|h| h.as_str()) == Some("api.acme.invalid")),
                "provider stanza leaked into network_policies.{section}.endpoints"
            );
        }
    }
}

/// Round-trip: append then strip a provider against a real
/// `generate_policy` output and confirm the policy is byte-identical
/// to the original. This guards both the anchor-aware append and
/// strip-stops-at-anchor logic.
#[test]
fn append_then_strip_round_trips_against_real_policy() {
    let base = generate_policy(
        8100,
        &right_agent_config::NetworkPolicy::Permissive,
        HostMcpAccess::BootstrapUnresolved,
    );

    let appended = providers_append(&base, "myagent-acme", "api.acme.invalid", None);
    assert_ne!(appended, base, "append must change the policy");
    assert!(
        appended.contains("# right-providers: insert-above"),
        "anchor must survive append"
    );

    let stripped = providers_strip(&appended, "myagent-acme", "api.acme.invalid");
    assert_eq!(
        stripped, base,
        "strip after append must restore the byte-for-byte original policy"
    );
}

/// Bug regression: the appended provider endpoint must use OpenShell's real
/// endpoint keys (`host` + `port`), not the legacy `domain` key that OpenShell
/// v0.0.50 rejects as an unknown field. Generic providers are only ever
/// appended to permissive policies (the API rejects generic + restrictive via
/// `NetworkPolicyForbidsGeneric`), so this asserts the production path against
/// the real permissive `generate_policy` output. Shipped broken because no live
/// test ever applied a successfully-appended policy.
#[test]
fn appended_provider_endpoint_uses_host_and_port_not_domain() {
    let base = generate_policy(
        8100,
        &right_agent_config::NetworkPolicy::Permissive,
        HostMcpAccess::BootstrapUnresolved,
    );

    let appended = providers_append(&base, "myagent-acme", "api.acme.invalid", None);
    let parsed: serde_json::Value =
        serde_saphyr::from_str(&appended).expect("appended policy must be valid YAML");

    let stanza = parsed["network_policies"]["outbound"]["endpoints"]
        .as_array()
        .expect("outbound endpoints must be a list")
        .iter()
        .find(|e| e.get("host").and_then(|h| h.as_str()) == Some("api.acme.invalid"))
        .expect("provider stanza must carry a `host` key (OpenShell rejects `domain`)");
    assert!(
        stanza.get("port").is_some(),
        "provider endpoint must carry a `port` (OpenShell expects it)"
    );
    assert!(
        !appended.contains("- domain: api.acme.invalid"),
        "the legacy `domain:` key must not be emitted"
    );
}

/// A faithful reproduction of a permissive `generate_policy` output as it
/// existed BEFORE the `# right-providers: insert-above` anchor was added.
/// Per AGENTS.md "Upgrade-friendly design", agents deployed against that
/// version still have on-disk policies in this exact shape, and any new
/// append/strip code must handle them without sandbox recreation.
fn legacy_permissive_policy_without_anchor() -> String {
    // Mirrors generate_policy(NetworkPolicy::Permissive, BootstrapUnresolved)
    // with the anchor line stripped from permissive_endpoints(). All other
    // indentation and surrounding structure is preserved verbatim, including
    // the sibling `binaries:` key and the trailing `right:` block.
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
  outbound:
    endpoints:
      - port: 443
        allowed_ips:
          - "1.0.0.0/8"
          - "2000::/3"
        tls: skip
      - port: 80
        allowed_ips:
          - "1.0.0.0/8"
          - "2000::/3"
        tls: skip
    binaries:
      - path: "**"

  right:
    endpoints:
      - host: "host.openshell.internal"
        port: 8100
        protocol: rest
        access: full
    binaries:
      - path: "**"
"#
    .to_string()
}

/// Regression for the upgrade-path bug described in
/// `Fix: providers_strip over-eats on legacy policy`:
///
/// Legacy permissive policies on disk do NOT contain the
/// `# right-providers: insert-above` anchor that `generate_policy` now emits.
/// When `providers_append` falls back to the no-anchor path on such a policy
/// and then `providers_strip` removes the stanza, the walker MUST stop at
/// `    binaries:` (the sibling key of `endpoints:`). Otherwise it consumes
/// the `outbound.binaries` block, which OpenShell then rejects (or worse,
/// silently restricts all egress).
#[test]
fn providers_strip_does_not_consume_outbound_binaries_in_legacy_policy() {
    let legacy = legacy_permissive_policy_without_anchor();
    assert!(
        !legacy.contains("# right-providers:"),
        "legacy fixture must not contain the anchor sentinel"
    );

    let appended = providers_append(&legacy, "myagent-acme", "api.acme.invalid", None);
    assert!(
        appended.contains("managed-by: right-providers:myagent-acme"),
        "append must produce a tagged stanza even without the anchor"
    );

    let stripped = providers_strip(&appended, "myagent-acme", "api.acme.invalid");

    // The post-strip policy must remain valid YAML and must preserve both
    // `outbound.endpoints` and `outbound.binaries` exactly. The bug was that
    // `binaries:` was being walked as a continuation of the endpoints list
    // and removed from the document.
    let parsed: serde_json::Value =
        serde_saphyr::from_str(&stripped).expect("stripped legacy policy must be valid YAML");
    let outbound = &parsed["network_policies"]["outbound"];
    assert!(
        outbound.get("binaries").is_some(),
        "outbound.binaries must survive strip (was eaten by walker pre-fix)"
    );
    let binaries = outbound["binaries"]
        .as_array()
        .expect("outbound.binaries must be a list");
    assert_eq!(
        binaries.len(),
        1,
        "outbound.binaries must still contain its single `**` entry"
    );
    assert_eq!(
        binaries[0]["path"].as_str(),
        Some("**"),
        "outbound.binaries[0].path must be the unchanged `**` wildcard"
    );

    let endpoints = outbound["endpoints"]
        .as_array()
        .expect("outbound.endpoints must remain a list");
    let ports: Vec<u64> = endpoints
        .iter()
        .filter_map(|e| e["port"].as_u64())
        .collect();
    assert!(
        ports.contains(&443) && ports.contains(&80),
        "both original public-web endpoints (ports 80 + 443) must survive strip, got {ports:?}"
    );

    // And the provider stanza itself must be gone.
    assert!(!stripped.contains("api.acme.invalid"));
    assert!(!stripped.contains("right-providers:myagent-acme"));
}

/// Stronger guarantee: appending into a legacy (no-anchor) policy and then
/// stripping that same provider must yield a byte-identical original. If
/// any whitespace, sibling key, or trailing block is disturbed, this test
/// fails.
#[test]
fn providers_append_into_legacy_policy_then_strip_yields_byte_identical_original() {
    let legacy = legacy_permissive_policy_without_anchor();

    let appended = providers_append(&legacy, "myagent-acme", "api.acme.invalid", None);
    assert_ne!(appended, legacy, "append must change the policy");

    let stripped = providers_strip(&appended, "myagent-acme", "api.acme.invalid");
    assert_eq!(
        stripped, legacy,
        "strip after append on a legacy (no-anchor) policy must restore byte-for-byte"
    );
}

#[test]
fn strip_one_of_two_adjacent_providers_does_not_touch_neighbor() {
    let policy = providers_append("network:\n  endpoints:\n", "myagent-a", "api.a.com", None);
    let policy = providers_append(&policy, "myagent-b", "api.b.com", None);
    let stripped = providers_strip(&policy, "myagent-a", "api.a.com");
    // A is gone
    assert!(!stripped.contains("right-providers:myagent-a"));
    assert!(!stripped.contains("- host: api.a.com"));
    // B survives intact
    assert!(stripped.contains("right-providers:myagent-b"));
    assert!(stripped.contains("- host: api.b.com"));
    assert!(stripped.contains("protocol: rest"));
}
