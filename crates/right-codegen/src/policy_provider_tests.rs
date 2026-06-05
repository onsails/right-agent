use super::policy::providers_strip;

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

const LEGACY_POLICY_WITH_PROVIDER_BEFORE_BINARIES: &str = r#"network_policies:
  outbound:
    endpoints:
      - port: 443
        allowed_ips:
          - "1.0.0.0/8"
        tls: skip
      # managed-by: right-providers:myagent-acme
      - host: api.acme.invalid
        port: 443
        protocol: rest
        access: full
    binaries:
      - path: "**"
"#;

const LEGACY_POLICY_WITHOUT_PROVIDER_BEFORE_BINARIES: &str = r#"network_policies:
  outbound:
    endpoints:
      - port: 443
        allowed_ips:
          - "1.0.0.0/8"
        tls: skip
    binaries:
      - path: "**"
"#;

#[test]
fn providers_strip_absent_provider_is_noop() {
    let after = providers_strip(POLICY_WITH_ONE_PROVIDER, "myagent-other", "api.other.com");
    assert_eq!(after, POLICY_WITH_ONE_PROVIDER);
}

#[test]
fn providers_strip_removes_tagged_endpoint() {
    let after = providers_strip(POLICY_WITH_ONE_PROVIDER, "myagent-acme", "api.acme.com");
    assert!(!after.contains("managed-by: right-providers:myagent-acme"));
    assert!(!after.contains("api.acme.com"));
    assert!(after.contains("api.anthropic.com"));
}

#[test]
fn providers_strip_does_not_consume_outbound_binaries_in_legacy_policy() {
    let stripped = providers_strip(
        LEGACY_POLICY_WITH_PROVIDER_BEFORE_BINARIES,
        "myagent-acme",
        "api.acme.invalid",
    );

    let parsed: serde_json::Value =
        serde_saphyr::from_str(&stripped).expect("stripped legacy policy must be valid YAML");
    let outbound = &parsed["network_policies"]["outbound"];
    assert!(
        outbound.get("binaries").is_some(),
        "outbound.binaries must survive strip"
    );
    assert_eq!(outbound["binaries"][0]["path"].as_str(), Some("**"));

    let endpoints = outbound["endpoints"]
        .as_array()
        .expect("outbound.endpoints must remain a list");
    assert!(
        endpoints.iter().any(|e| e["port"].as_u64() == Some(443)),
        "original public-web endpoint must survive strip"
    );
    assert!(!stripped.contains("api.acme.invalid"));
    assert!(!stripped.contains("right-providers:myagent-acme"));
}

#[test]
fn providers_strip_from_legacy_policy_yields_expected_base() {
    let stripped = providers_strip(
        LEGACY_POLICY_WITH_PROVIDER_BEFORE_BINARIES,
        "myagent-acme",
        "api.acme.invalid",
    );
    assert_eq!(stripped, LEGACY_POLICY_WITHOUT_PROVIDER_BEFORE_BINARIES);
}

#[test]
fn providers_strip_one_of_two_adjacent_providers_does_not_touch_neighbor() {
    let policy = r#"network:
  endpoints:
    # managed-by: right-providers:myagent-a
    - host: api.a.com
      port: 443
      protocol: rest
      access: full
    # managed-by: right-providers:myagent-b
    - host: api.b.com
      port: 443
      protocol: rest
      access: full
"#;

    let stripped = providers_strip(policy, "myagent-a", "api.a.com");
    assert!(!stripped.contains("right-providers:myagent-a"));
    assert!(!stripped.contains("- host: api.a.com"));
    assert!(stripped.contains("right-providers:myagent-b"));
    assert!(stripped.contains("- host: api.b.com"));
    assert!(stripped.contains("protocol: rest"));
}
