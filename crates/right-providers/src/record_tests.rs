use super::*;

fn generic_spec() -> GenericSpec {
    GenericSpec {
        env_var: "FAL_KEY".into(),
        upstream_hosts: vec!["fal.run".into()],
        upstream_path_prefix: Some("/v1".into()),
    }
}

#[test]
fn credential_debug_never_renders_the_value() {
    let cred = Credential::from("super-secret-value".to_string());
    let rendered = format!("{cred:?}");
    assert_eq!(rendered, "Credential(<redacted>)");
    assert!(!rendered.contains("super-secret-value"));
}

#[test]
fn record_debug_has_no_credential_field() {
    let record = ProviderRecord {
        name: "fal-a1b2c3".into(),
        owner_agent: "agent-a".into(),
        kind: ProviderKind::Builtin("right-fal".into()),
        label: "prod".into(),
        env_var: "FAL_KEY".into(),
        updated_at: 1_700_000_000,
        borrower_agent: None,
        status: ProviderStatus::Ready,
    };
    let rendered = format!("{record:?}");
    assert!(
        !rendered.to_lowercase().contains("credential"),
        "a read-side record must not carry a credential: {rendered}"
    );
}

#[test]
fn redacted_and_empty_credentials_are_unreadable() {
    for value in ["", REDACTION_SENTINEL] {
        let err = check_source_credential_readable(value, "fal-a1b2c3").unwrap_err();
        assert!(
            matches!(&err, StoreError::SourceCredentialUnreadable { source_provider }
                if source_provider == "fal-a1b2c3"),
            "got {err:?}"
        );
    }
    check_source_credential_readable("real-value", "fal-a1b2c3").expect("a real value is readable");
}

#[test]
fn builtin_kind_resolves_env_var_and_hosts_from_the_catalog() {
    let kind = ProviderKind::Builtin("right-fal".into());
    assert_eq!(kind.slug(), "right-fal");
    assert_eq!(kind.env_var().unwrap(), "FAL_KEY");
    assert_eq!(
        kind.allowed_hosts().unwrap(),
        vec!["fal.run", "queue.fal.run", "rest.fal.ai"]
    );
}

#[test]
fn unknown_builtin_slug_fails_loudly_rather_than_yielding_an_empty_env_var() {
    let kind = ProviderKind::Builtin("retired-slug".into());
    let err = kind.env_var().unwrap_err();
    assert!(
        matches!(&err, StoreError::UnknownBuiltinSlug { slug } if slug == "retired-slug"),
        "got {err:?}"
    );
    assert!(kind.allowed_hosts().is_err());
}

#[test]
fn generic_kind_reports_the_generic_slug_and_its_own_endpoints() {
    let kind = ProviderKind::Generic(generic_spec());
    assert_eq!(kind.slug(), "generic");
    assert_eq!(kind.env_var().unwrap(), "FAL_KEY");
    assert_eq!(kind.allowed_hosts().unwrap(), vec!["fal.run"]);
    assert_eq!(
        kind.generic().unwrap().upstream_path_prefix.as_deref(),
        Some("/v1")
    );
}

#[test]
fn status_serializes_kebab_case() {
    assert_eq!(
        serde_json::to_string(&ProviderStatus::NeedsValue).unwrap(),
        "\"needs-value\""
    );
    assert_eq!(
        serde_json::to_string(&ProviderStatus::Ready).unwrap(),
        "\"ready\""
    );
    assert_eq!(
        serde_json::to_string(&ProviderStatus::Error).unwrap(),
        "\"error\""
    );
}

#[test]
fn generic_spec_omits_an_absent_path_prefix() {
    let spec = GenericSpec {
        env_var: "FAL_KEY".into(),
        upstream_hosts: vec!["fal.run".into()],
        upstream_path_prefix: None,
    };
    assert_eq!(
        serde_json::to_string(&spec).unwrap(),
        r#"{"env_var":"FAL_KEY","upstream_hosts":["fal.run"]}"#
    );
}

#[test]
fn holder_is_the_borrower_when_borrowed() {
    let record = ProviderRecord {
        name: "fal-a1b2c3".into(),
        owner_agent: "agent-a".into(),
        kind: ProviderKind::Builtin("right-fal".into()),
        label: "prod".into(),
        env_var: "FAL_KEY".into(),
        updated_at: 0,
        borrower_agent: Some("right".into()),
        status: ProviderStatus::Ready,
    };
    assert_eq!(record.holder_agent(), "right");
    assert!(record.is_borrowed());
    assert!(!record.is_owned());
}
