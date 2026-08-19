use super::*;

fn reason(err: StoreError) -> String {
    match err {
        StoreError::InvalidName { reason, .. } => reason,
        other => panic!("expected InvalidName, got {other:?}"),
    }
}

#[test]
fn name_accepts_agent_prefixed_and_agent_agnostic_shapes() {
    validate_name("agent-a", "agent-a-provider").expect("legacy {agent}-{slug}");
    validate_name("agent-a", "fal-a1b2c3").expect("agent-agnostic {type}-{hex}");
}

#[test]
fn name_rejects_empty_slug_after_prefix() {
    let err = validate_name("agent-a", "agent-a-").unwrap_err();
    assert_eq!(reason(err), "1-40 chars after optional agent prefix");
}

#[test]
fn name_slug_cap_is_forty_after_the_prefix() {
    let slug = "a".repeat(40);
    validate_name("agent-a", &format!("agent-a-{slug}")).expect("40 chars is the boundary");

    let over = "a".repeat(41);
    let err = validate_name("agent-a", &format!("agent-a-{over}")).unwrap_err();
    assert_eq!(reason(err), "1-40 chars after optional agent prefix");
}

#[test]
fn name_full_length_cap_is_sixty_four() {
    // Long agent prefix keeps the slug within 40 while the whole name is 65.
    let agent = "a".repeat(30);
    let name = format!("{agent}-{}", "b".repeat(34));
    assert_eq!(name.len(), 65);
    let err = validate_name(&agent, &name).unwrap_err();
    assert_eq!(reason(err), "name too long (max 64)");
}

#[test]
fn name_alphabet_is_lowercase_digits_and_dash_starting_with_a_letter() {
    for bad in ["Fal-a1b2c3", "1fal", "-fal", "fal_a1b2c3", "fal.a1"] {
        let err = validate_name("agent-a", bad).unwrap_err();
        assert_eq!(
            reason(err),
            "lowercase a-z/0-9/'-', must start a-z",
            "{bad} must be rejected"
        );
    }
}

#[test]
fn record_name_strips_the_right_prefix_and_appends_six_hex() {
    let name = new_record_name("right-fal");
    assert!(name.starts_with("fal-"), "got {name}");
    let suffix = name.strip_prefix("fal-").unwrap();
    assert_eq!(suffix.len(), 6);
    assert!(suffix.chars().all(|c| c.is_ascii_hexdigit()));
}

#[test]
fn record_name_collapses_generic_shapes() {
    assert!(new_record_name("generic").starts_with("generic-"));
    assert!(new_record_name("right-generic-abc").starts_with("generic-"));
    assert!(new_record_name("").starts_with("generic-"));
}

#[test]
fn type_slug_rejects_the_reserved_claude_type() {
    let err = validate_type_slug("claude").unwrap_err();
    assert_eq!(
        reason(err),
        "type \"claude\" is reserved for the in-sandbox login flow"
    );
}

#[test]
fn type_slug_is_catalog_driven() {
    validate_type_slug("right-github").expect("managed profiles are valid types");
    validate_type_slug("generic").expect("the escape hatch is a valid type");
    let err = validate_type_slug("nope").unwrap_err();
    assert_eq!(reason(err), "unknown type \"nope\"");
}

#[test]
fn env_var_alphabet_and_length() {
    validate_env_var("FAL_KEY").expect("upper snake");
    validate_env_var("_PRIVATE1").expect("leading underscore, digits");
    for bad in ["", "fal_key", "1KEY", "FAL-KEY", "FAL KEY"] {
        assert!(validate_env_var(bad).is_err(), "{bad} must be rejected");
    }
    assert!(validate_env_var(&"A".repeat(64)).is_ok());
    assert!(validate_env_var(&"A".repeat(65)).is_err());
}

#[test]
fn label_rejects_yaml_reserved_words_and_pure_numbers() {
    validate_label("prod-key").expect("ordinary label");
    for bad in ["no", "YES", "off", "null", "123", "-0"] {
        let err = validate_label(bad).unwrap_err();
        assert_eq!(
            reason(err),
            "label must not be a YAML-reserved word or a pure number",
            "{bad} must be rejected"
        );
    }
    // `~` and `3.14` are YAML null/number too, but the scalar alphabet
    // rejects them first — same as the handler this was ported from.
    let err = validate_label("~").unwrap_err();
    assert_eq!(reason(err), "label contains disallowed character '~'");
    let err = validate_label("3.14").unwrap_err();
    assert_eq!(reason(err), "label contains disallowed character '.'");
}

#[test]
fn label_alphabet_and_length() {
    let err = validate_label("has space").unwrap_err();
    assert_eq!(reason(err), "label contains disallowed character ' '");
    let err = validate_label(&"a".repeat(33)).unwrap_err();
    assert_eq!(reason(err), "label must be 1-32 chars");
}

#[test]
fn upstream_host_and_path_prefix_alphabets() {
    validate_upstream_host("api.example.com:8443").expect("host with port");
    let err = validate_upstream_host("api/example").unwrap_err();
    assert_eq!(
        reason(err),
        "upstream_host contains disallowed character '/'"
    );

    validate_path_prefix("/v1/chat~completions").expect("path prefix alphabet");
    let err = validate_path_prefix("/v1?q=1").unwrap_err();
    assert_eq!(
        reason(err),
        "upstream_path_prefix contains disallowed character '?'"
    );
}

#[test]
fn generic_hosts_are_trimmed_deduped_and_order_preserved() {
    let extra = vec![
        " api.example.com ".to_string(),
        String::new(),
        "cdn.example.com".to_string(),
        "api.example.com".to_string(),
    ];
    let hosts = normalize_generic_hosts(Some(" api.example.com "), Some(&extra));
    assert_eq!(hosts, vec!["api.example.com", "cdn.example.com"]);
}

#[test]
fn generic_request_requires_at_least_one_host() {
    let err = validate_generic_request("FAL_KEY", None, Some(&[]), None).unwrap_err();
    assert_eq!(
        reason(err),
        "generic provider requires at least one upstream host"
    );
}

#[test]
fn generic_request_returns_normalized_hosts() {
    let hosts = validate_generic_request(
        "FAL_KEY",
        Some("fal.run"),
        Some(&["fal.run".into(), "rest.fal.ai".into()]),
        Some("/v1"),
    )
    .expect("valid generic definition");
    assert_eq!(hosts, vec!["fal.run", "rest.fal.ai"]);
}
