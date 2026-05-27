use super::policy::*;

const POLICY_WITHOUT_PROVIDERS: &str = r#"
network:
  endpoints:
    - domain: api.anthropic.com
      protocol: rest
      access: full
"#;

const POLICY_WITH_ONE_PROVIDER: &str = r#"
network:
  endpoints:
    - domain: api.anthropic.com
      protocol: rest
      access: full
    # managed-by: right-providers:myagent-acme
    - domain: api.acme.com
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
    assert!(after.contains("- domain: api.acme.com"));
}

#[test]
fn append_provider_endpoint_existing_rest_is_noop() {
    let already = "network:\n  endpoints:\n    - domain: api.acme.com\n      protocol: rest\n      access: full\n";
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
    let raw = "network:\n  endpoints:\n    - domain: api.acme.com\n      tls: skip\n";
    let err = providers_append_checked(raw, "myagent-acme", "api.acme.com", None);
    assert!(matches!(err, Err(PolicyConflict::RawTunnel { .. })));
}

#[test]
fn append_provider_endpoint_prefix_collision_is_not_idempotent() {
    // Regression: a substring host-marker match used to treat
    // "api.acme.com.evil.tld" as collision-matching "api.acme.com",
    // causing the real endpoint to be silently skipped.
    let policy = "network:\n  endpoints:\n    - domain: api.acme.com.evil.tld\n      protocol: rest\n      access: full\n";
    let after = providers_append(policy, "myagent-acme", "api.acme.com", None);
    assert!(after.contains("- domain: api.acme.com\n"));
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
    assert!(after.contains("- domain: api.acme.com"));
    assert!(after.contains("managed-by: right-providers:myagent-acme"));
    // The original CRLF content must not be mid-line corrupted: existing
    // endpoint still present.
    assert!(after.contains("- host: api.anthropic.com"));
}

#[test]
fn strip_one_of_two_adjacent_providers_does_not_touch_neighbor() {
    let policy = providers_append("network:\n  endpoints:\n", "myagent-a", "api.a.com", None);
    let policy = providers_append(&policy, "myagent-b", "api.b.com", None);
    let stripped = providers_strip(&policy, "myagent-a", "api.a.com");
    // A is gone
    assert!(!stripped.contains("right-providers:myagent-a"));
    assert!(!stripped.contains("- domain: api.a.com"));
    // B survives intact
    assert!(stripped.contains("right-providers:myagent-b"));
    assert!(stripped.contains("- domain: api.b.com"));
    assert!(stripped.contains("protocol: rest"));
}
