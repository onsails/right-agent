use super::*;

#[cfg(test)]
mod provider_validation_tests {
    use super::*;

    #[test]
    fn name_must_match_agent_prefix() {
        // Legacy {agent}-{slug} form must still validate.
        assert!(validate_name("myagent", "myagent-anthropic").is_ok());
        // Agent-agnostic form (no agent prefix) is now also valid.
        assert!(validate_name("myagent", "other-anthropic").is_ok());
    }

    #[test]
    fn slug_pattern_enforced() {
        let err = validate_name("myagent", "myagent-Anthropic").unwrap_err();
        assert!(matches!(err, ProviderApiError::InvalidName { .. }));
        let err2 = validate_name("myagent", "myagent-").unwrap_err();
        assert!(matches!(err2, ProviderApiError::InvalidName { .. }));
    }

    #[test]
    fn claude_type_rejected() {
        assert!(matches!(
            validate_type_slug("claude"),
            Err(ProviderApiError::InvalidName { .. })
        ));
        assert!(validate_type_slug("anthropic").is_ok());
        assert!(validate_type_slug("generic").is_ok());
    }

    #[test]
    fn validate_type_slug_accepts_right_github() {
        // Regression: the dashboard offers `right-github` as the GitHub type, so
        // create-validation must accept it.
        assert!(validate_type_slug("right-github").is_ok());
    }

    #[test]
    fn validate_type_slug_in_sync_with_catalog() {
        // Every catalog type (except the reserved `claude` login slug, which is
        // never a catalog entry) must be creatable. Guards against the validator
        // and the catalog drifting apart again.
        for p in ProviderStore::catalog() {
            assert!(
                validate_type_slug(p.slug).is_ok(),
                "catalog type {} must pass create-validation",
                p.slug
            );
        }
    }

    // ── label validation against YAML-reserved tokens ─────────────────────

    fn assert_label_rejected(label: &str) {
        let err = validate_label(label)
            .expect_err(&format!("expected validate_label({label:?}) to reject"));
        assert!(
            matches!(err, ProviderApiError::InvalidName { .. }),
            "expected InvalidName for {label:?}, got {err:?}"
        );
    }

    #[test]
    fn validate_label_rejects_yaml_boolean_no() {
        assert_label_rejected("no");
    }

    #[test]
    fn validate_label_rejects_yaml_boolean_true() {
        assert_label_rejected("true");
    }

    #[test]
    fn validate_label_rejects_pure_numeric() {
        assert_label_rejected("123");
    }

    #[test]
    fn validate_label_rejects_tilde_null() {
        // `~` is not ASCII-alphanumeric so the scalar validator already
        // rejects it before the reserved-word check fires, but the field
        // still must be rejected (and classify as InvalidName).
        assert_label_rejected("~");
    }

    #[test]
    fn validate_label_rejects_case_variant_yes() {
        assert_label_rejected("Yes");
    }

    #[test]
    fn validate_label_rejects_all_keyword_case_variants() {
        for tok in [
            "y", "Y", "yes", "YES", "n", "N", "NO", "True", "TRUE", "False", "FALSE", "on", "On",
            "ON", "off", "Off", "OFF", "null", "Null", "NULL",
        ] {
            assert_label_rejected(tok);
        }
    }

    #[test]
    fn validate_label_accepts_hyphenated_keyword_like() {
        validate_label("no-thanks").expect("hyphenated label should be accepted");
    }

    #[test]
    fn validate_label_accepts_number_suffix() {
        validate_label("yes2").expect("'yes2' should be accepted");
    }

    #[test]
    fn validate_name_accepts_legacy_agent_prefixed() {
        validate_name("agent-a", "agent-a-provider").expect("legacy {agent}-{slug} must validate");
    }

    #[test]
    fn validate_name_accepts_agent_agnostic_uuid_form() {
        validate_name("agent-a", "fal-a1b2c3").expect("agent-agnostic name must validate");
    }

    #[test]
    fn validate_name_rejects_bad_agnostic_forms() {
        assert!(validate_name("agent-a", "Fal-a1b2c3").is_err()); // uppercase
        assert!(validate_name("agent-a", "1fal-a1b2c3").is_err()); // leading digit
        assert!(validate_name("agent-a", "").is_err()); // empty
        assert!(validate_name("agent-a", &"f".repeat(41)).is_err()); // over 40-char slug cap
    }

    #[test]
    fn new_record_name_has_type_slug_and_hex_suffix() {
        let n = right_providers::new_record_name("right-fal");
        assert!(n.starts_with("fal-"), "got {n}");
        let suffix = n.rsplit('-').next().unwrap();
        assert_eq!(suffix.len(), 6);
        assert!(suffix.chars().all(|c| c.is_ascii_hexdigit()), "got {n}");
    }

    /// Round-trip guard: serialize a provider entry with a label that YAML 1.1
    /// would otherwise coerce to a boolean, then re-parse the resulting YAML
    /// through `serde_saphyr` and confirm the label survives as a string.
    #[test]
    fn round_trip_quoted_label_survives_saphyr_parse() {
        let entry = right_agent_config::ProviderEntry {
            name: "agent-acme".to_string(),
            type_: right_agent_config::ProviderType::Generic,
            label: Some("acme".to_string()),
            generic: Some(right_agent_config::GenericProvider {
                env_var: "ACME_KEY".to_string(),
                upstream_hosts: vec!["api.acme.com".to_string()],
                upstream_path_prefix: Some("/v1".to_string()),
            }),
        };
        let serialized = serialize_provider_entry(&entry);
        assert!(serialized.contains("name: 'agent-acme'"));
        assert!(serialized.contains("label: 'acme'"));

        let yaml = format!("sandbox:\n  providers:\n{serialized}");
        let cfg: right_agent_config::AgentConfig = serde_saphyr::from_str(&yaml)
            .expect("serialized provider entry must round-trip through serde_saphyr");
        let entry_back = &cfg.sandbox.unwrap().providers[0];
        assert_eq!(entry_back.name, "agent-acme");
        assert_eq!(entry_back.label.as_deref(), Some("acme"));
        let g = entry_back.generic.as_ref().unwrap();
        assert_eq!(g.env_var, "ACME_KEY");
        assert_eq!(g.upstream_hosts, vec!["api.acme.com"]);
        assert_eq!(g.upstream_path_prefix.as_deref(), Some("/v1"));
    }

    /// If a value that YAML 1.1 would coerce to a non-string ever bypassed
    /// label validation, the on-disk YAML must still re-parse correctly
    /// because all scalars are single-quoted.
    #[test]
    fn round_trip_label_no_parses_as_string_when_quoted() {
        let entry = right_agent_config::ProviderEntry {
            name: "agent-no".to_string(),
            type_: right_agent_config::ProviderType::Generic,
            label: Some("no".to_string()),
            generic: Some(right_agent_config::GenericProvider {
                env_var: "NO_KEY".to_string(),
                upstream_hosts: vec!["api.example.com".to_string()],
                upstream_path_prefix: None,
            }),
        };
        let serialized = serialize_provider_entry(&entry);
        assert!(
            serialized.contains("label: 'no'"),
            "label must be single-quoted; got:\n{serialized}"
        );
        let yaml = format!("sandbox:\n  providers:\n{serialized}");
        let cfg: right_agent_config::AgentConfig = serde_saphyr::from_str(&yaml)
            .expect("single-quoted reserved-word label must still parse as Option<String>::Some");
        let entry_back = &cfg.sandbox.unwrap().providers[0];
        assert_eq!(entry_back.label.as_deref(), Some("no"));
    }

    #[test]
    fn serialize_provider_entry_emits_no_ownership_key() {
        // Ownership is a providers.db column: `ProviderEntry` has no
        // `shared_from` field and agent.yaml must never regrow one.
        let entry = right_agent_config::ProviderEntry {
            name: "fal-a1b2c3".into(),
            type_: right_agent_config::ProviderType::BuiltIn("right-fal".into()),
            label: None,
            generic: None,
        };
        let s = serialize_provider_entry(&entry);
        assert!(
            !s.contains("shared_from") && !s.contains("owner"),
            "agent.yaml must not carry ownership; got: {s}"
        );
    }
}

#[cfg(test)]
mod provider_view_tests {
    use super::*;

    fn record(kind: ProviderKind, status: right_providers::ProviderStatus) -> ProviderRecord {
        ProviderRecord {
            name: "fal-a1b2c3".into(),
            owner_agent: "agent-a".into(),
            kind,
            label: String::new(),
            env_var: "FAL_KEY".into(),
            updated_at: 1_755_000_000,
            borrower_agent: None,
            status,
        }
    }

    #[test]
    fn status_serializes_with_kind_tag_and_snake_case() {
        let ready = serde_json::to_value(ProviderStatus::Ready).unwrap();
        assert_eq!(ready, serde_json::json!({"kind": "ready"}));

        let needs = serde_json::to_value(ProviderStatus::NeedsValue).unwrap();
        assert_eq!(needs, serde_json::json!({"kind": "needs_value"}));

        let err = serde_json::to_value(ProviderStatus::Error {
            message: "boom".into(),
        })
        .unwrap();
        assert_eq!(err, serde_json::json!({"kind": "error", "message": "boom"}));
    }

    #[test]
    fn view_has_no_composed_field() {
        // The OpenShell-era `composed: bool|null` is replaced by the tri-state
        // status (design decision "Provider status"); the wire shape must not
        // resurrect it.
        let view = record_view(&record(
            ProviderKind::Builtin("right-fal".into()),
            right_providers::ProviderStatus::Ready,
        ));
        let json = serde_json::to_value(view).unwrap();
        assert!(json.get("composed").is_none(), "got: {json}");
        assert_eq!(json["status"]["kind"], "ready");
    }

    #[test]
    fn view_maps_store_status_and_omits_shared_from_for_owned() {
        let view = record_view(&record(
            ProviderKind::Builtin("right-fal".into()),
            right_providers::ProviderStatus::NeedsValue,
        ));
        let json = serde_json::to_value(view).unwrap();
        assert_eq!(json["status"]["kind"], "needs_value");
        assert!(json.get("shared_from").is_none(), "owned row: {json}");
    }

    #[test]
    fn view_sets_shared_from_to_true_owner_for_borrowed() {
        let mut rec = record(
            ProviderKind::Builtin("right-fal".into()),
            right_providers::ProviderStatus::Ready,
        );
        rec.borrower_agent = Some("right".into());
        let json = serde_json::to_value(record_view(&rec)).unwrap();
        assert_eq!(json["shared_from"], "agent-a");
    }

    #[test]
    fn view_error_status_names_unknown_builtin_slug() {
        let rec = record(
            ProviderKind::Builtin("not-a-real-slug".into()),
            right_providers::ProviderStatus::Error,
        );
        let json = serde_json::to_value(record_view(&rec)).unwrap();
        assert_eq!(json["status"]["kind"], "error");
        let msg = json["status"]["message"].as_str().unwrap();
        assert!(msg.contains("not-a-real-slug"), "got: {msg}");
    }

    #[test]
    fn view_generic_record_carries_generic_block() {
        let rec = record(
            ProviderKind::Generic(GenericSpec {
                env_var: "ACME_KEY".into(),
                upstream_hosts: vec!["api.acme.com".into()],
                upstream_path_prefix: Some("/v1".into()),
            }),
            right_providers::ProviderStatus::Ready,
        );
        let json = serde_json::to_value(record_view(&rec)).unwrap();
        assert_eq!(json["type"], "generic");
        assert_eq!(json["generic"]["env_var"], "ACME_KEY");
        assert_eq!(json["generic"]["upstream_hosts"][0], "api.acme.com");
        assert_eq!(json["generic"]["upstream_path_prefix"], "/v1");
    }

    #[test]
    fn view_updated_at_is_rfc3339_when_set() {
        let view = record_view(&record(
            ProviderKind::Builtin("right-fal".into()),
            right_providers::ProviderStatus::Ready,
        ));
        let json = serde_json::to_value(view).unwrap();
        assert!(
            json["updated_at"].as_str().unwrap().contains('T'),
            "updated_at must serialize RFC 3339: {json}"
        );
    }

    #[test]
    fn yaml_entry_from_borrowed_record_carries_no_ownership() {
        let mut rec = record(
            ProviderKind::Builtin("right-fal".into()),
            right_providers::ProviderStatus::Ready,
        );
        rec.borrower_agent = Some("right".into());
        let entry = record_yaml_entry(&rec);
        assert!(matches!(
            &entry.type_,
            right_agent_config::ProviderType::BuiltIn(s) if s == "right-fal"
        ));
        let serialized = serialize_provider_entry(&entry);
        assert!(
            !serialized.contains("shared_from") && !serialized.contains("agent-a"),
            "borrowed record must not leak ownership into agent.yaml; got: {serialized}"
        );
    }
}

#[cfg(test)]
mod store_err_mapping_tests {
    use super::*;
    use axum::http::StatusCode;
    use axum::response::IntoResponse;

    fn status_of(err: StoreError) -> StatusCode {
        store_err(err).into_response().status()
    }

    #[test]
    fn store_errors_map_to_dashboard_statuses() {
        assert_eq!(
            status_of(StoreError::NotFound { name: "x".into() }),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            status_of(StoreError::NameCollision { name: "x".into() }),
            StatusCode::CONFLICT
        );
        assert_eq!(
            status_of(StoreError::EnvVarCollision {
                env_var: "X".into()
            }),
            StatusCode::CONFLICT
        );
        assert_eq!(
            status_of(StoreError::InvalidName {
                name: "x".into(),
                reason: "y".into()
            }),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            status_of(StoreError::InvalidEnvVar {
                env_var: "x".into()
            }),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            status_of(StoreError::UnknownBuiltinSlug { slug: "x".into() }),
            StatusCode::INTERNAL_SERVER_ERROR
        );
        assert_eq!(
            status_of(StoreError::BorrowedReadOnly {
                name: "x".into(),
                owner: "y".into()
            }),
            StatusCode::CONFLICT
        );
        assert_eq!(
            status_of(StoreError::ShareConflict { reason: "x".into() }),
            StatusCode::CONFLICT
        );
        assert_eq!(
            status_of(StoreError::GenericEnvVarChangeRequiresCredential { name: "x".into() }),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            status_of(StoreError::SourceCredentialUnreadable {
                source_provider: "x".into()
            }),
            StatusCode::UNPROCESSABLE_ENTITY
        );
    }

    #[test]
    fn copy_error_status_variants_map_to_expected_status() {
        assert_eq!(
            ProviderApiError::Unauthorized { agent: "a".into() }
                .into_response()
                .status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            ProviderApiError::CopyConflict { reason: "x".into() }
                .into_response()
                .status(),
            StatusCode::CONFLICT
        );
        assert_eq!(
            ProviderApiError::BorrowedProviderReadOnly {
                name: "n".into(),
                owner: "o".into()
            }
            .into_response()
            .status(),
            StatusCode::CONFLICT
        );
        assert_eq!(
            ProviderApiError::Internal("boom".into())
                .into_response()
                .status(),
            StatusCode::INTERNAL_SERVER_ERROR
        );
    }
}

#[cfg(test)]
mod plan_share_tests {
    use right_providers::{HeldProvider, plan_share, plan_unshare};

    use super::*;

    #[test]
    fn plan_share_rejects_self() {
        let e = plan_share("right", "right", "fal-a1b2c3", &[]).unwrap_err();
        assert!(matches!(e, StoreError::ShareConflict { .. }));
        // …and surfaces as 409 copy_conflict through the API mapping.
        assert!(matches!(
            store_err(e),
            ProviderApiError::CopyConflict { .. }
        ));
    }

    #[test]
    fn plan_share_rejects_dup_when_dest_already_has_record() {
        let existing = vec![HeldProvider::new("fal-a1b2c3", "agent-a")];
        let e = plan_share("agent-a", "right", "fal-a1b2c3", &existing).unwrap_err();
        assert!(matches!(e, StoreError::ShareConflict { .. }));
    }

    #[test]
    fn plan_share_accepts_new_record() {
        plan_share("agent-a", "right", "fal-a1b2c3", &[]).expect("share into a fresh dest is ok");
    }

    #[test]
    fn plan_share_rejects_dup_even_when_dest_borrows_same_name_from_elsewhere() {
        // Name uniqueness is per-holding-agent regardless of owner.
        let existing = vec![HeldProvider::new("fal-a1b2c3", "someone-else")];
        assert!(plan_share("agent-a", "right", "fal-a1b2c3", &existing).is_err());
    }

    #[test]
    fn plan_unshare_rejects_owned_entry() {
        let owned = HeldProvider::new("fal-a1b2c3", "right");
        assert!(matches!(
            plan_unshare("right", &owned).unwrap_err(),
            StoreError::ShareConflict { .. }
        ));
    }

    #[test]
    fn plan_unshare_accepts_borrowed_entry() {
        let borrowed = HeldProvider::new("fal-a1b2c3", "agent-a");
        plan_unshare("right", &borrowed).expect("borrowed entry can be unshared");
    }

    #[test]
    fn true_owner_points_past_the_intermediary() {
        let borrowed = HeldProvider::new("fal-a1b2c3", "agent-a");
        assert_eq!(right_providers::plan::true_owner(&borrowed), "agent-a");
    }
}

#[cfg(test)]
mod insert_tests {
    use super::*;

    #[test]
    fn insert_into_empty_sandbox() {
        let original = "name: foo\nsandbox:\n  name: foo\n";
        let entry = "    - name: foo-bar\n      type: anthropic\n";
        let out = insert_provider_entry(original, entry).unwrap();
        assert!(
            out.contains("providers:\n    - name: foo-bar"),
            "expected providers key followed by entry, got:\n{out}"
        );
    }

    #[test]
    fn insert_into_existing_providers() {
        let original = "sandbox:\n  providers:\n    - name: x\n      type: y\n";
        let entry = "    - name: foo-bar\n      type: anthropic\n";
        let out = insert_provider_entry(original, entry).unwrap();
        assert!(
            out.contains("- name: foo-bar"),
            "new entry missing from:\n{out}"
        );
        assert!(
            out.contains("- name: x"),
            "existing entry missing from:\n{out}"
        );
    }

    #[test]
    fn replace_existing_provider_swaps_in_place() {
        let original = "sandbox:\n  providers:\n    - name: foo-x\n      type: anthropic\n    - name: foo-y\n      type: github\n";
        let new_entry = "    - name: foo-x\n      type: openai\n";
        let out = replace_provider_entry(original, "foo-x", new_entry).unwrap();
        assert!(
            out.contains("- name: foo-x\n      type: openai"),
            "replaced entry not found in:\n{out}"
        );
        assert!(
            out.contains("- name: foo-y\n      type: github"),
            "sibling entry missing from:\n{out}"
        );
        assert!(
            !out.contains("type: anthropic"),
            "old entry still present in:\n{out}"
        );
    }

    #[test]
    fn remove_provider_drops_entry_only() {
        let original = "sandbox:\n  providers:\n    - name: foo-x\n      type: anthropic\n    - name: foo-y\n      type: github\n";
        let out = remove_provider_entry(original, "foo-x");
        assert!(!out.contains("foo-x"), "foo-x still present in:\n{out}");
        assert!(
            out.contains("- name: foo-y"),
            "sibling entry missing from:\n{out}"
        );
    }

    /// New writes single-quote names, so subsequent remove/replace operations
    /// must locate quoted entries too — not just legacy unquoted ones.
    #[test]
    fn remove_provider_handles_quoted_name() {
        let original = "sandbox:\n  providers:\n    - name: 'foo-x'\n      type: 'anthropic'\n    - name: 'foo-y'\n      type: 'github'\n";
        let out = remove_provider_entry(original, "foo-x");
        assert!(!out.contains("foo-x"), "foo-x still present in:\n{out}");
        assert!(
            out.contains("- name: 'foo-y'"),
            "sibling entry missing from:\n{out}"
        );
    }

    #[test]
    fn replace_provider_handles_quoted_name() {
        let original = "sandbox:\n  providers:\n    - name: 'foo-x'\n      type: 'anthropic'\n    - name: 'foo-y'\n      type: 'github'\n";
        let new_entry = "    - name: 'foo-x'\n      type: 'openai'\n";
        let out = replace_provider_entry(original, "foo-x", new_entry).unwrap();
        assert!(
            out.contains("- name: 'foo-x'\n      type: 'openai'"),
            "replaced entry not found in:\n{out}"
        );
        assert!(
            !out.contains("type: 'anthropic'"),
            "old entry still present in:\n{out}"
        );
    }

    /// Searching for an unquoted legacy entry whose name is a prefix of a
    /// longer name must not match the longer entry.
    #[test]
    fn find_provider_name_marker_unquoted_does_not_match_prefix() {
        let haystack =
            "sandbox:\n  providers:\n    - name: myagent-foo-bar\n      type: anthropic\n";
        assert_eq!(find_provider_name_marker(haystack, "myagent-foo"), None);
    }

    /// With both an exact unquoted entry and a longer-name entry present, the
    /// search must return the offset of the exact match.
    #[test]
    fn find_provider_name_marker_unquoted_matches_only_exact_followed_by_newline() {
        let haystack = "sandbox:\n  providers:\n    - name: myagent-foo\n      type: anthropic\n    - name: myagent-foo-bar\n      type: github\n";
        let foo_idx = find_provider_name_marker(haystack, "myagent-foo")
            .expect("exact unquoted match should resolve");
        let foo_bar_idx = find_provider_name_marker(haystack, "myagent-foo-bar")
            .expect("longer unquoted match should resolve");
        assert!(
            foo_idx < foo_bar_idx,
            "exact match must come before longer-name match: foo={foo_idx} foo-bar={foo_bar_idx}"
        );
        assert_eq!(
            &haystack[foo_idx..foo_idx + "    - name: myagent-foo".len()],
            "    - name: myagent-foo"
        );
        assert_eq!(
            &haystack[foo_bar_idx..foo_bar_idx + "    - name: myagent-foo-bar".len()],
            "    - name: myagent-foo-bar"
        );
    }

    /// Quoted form is bounded by the closing quote, so prefix collisions are
    /// impossible.
    #[test]
    fn find_provider_name_marker_quoted_matches() {
        let haystack =
            "sandbox:\n  providers:\n    - name: 'myagent-foo'\n      type: 'anthropic'\n";
        let idx = find_provider_name_marker(haystack, "myagent-foo")
            .expect("quoted match should resolve");
        assert_eq!(
            &haystack[idx..idx + "    - name: 'myagent-foo'".len()],
            "    - name: 'myagent-foo'"
        );
    }

    /// Removing the shorter name must drop the shorter row and leave the
    /// longer-name sibling untouched.
    #[test]
    fn remove_provider_entry_unquoted_does_not_remove_wrong_row() {
        let original = "sandbox:\n  providers:\n    - name: myagent-foo\n      type: anthropic\n    - name: myagent-foo-bar\n      type: github\n";
        let out = remove_provider_entry(original, "myagent-foo");
        assert!(
            out.contains("- name: myagent-foo-bar\n      type: github"),
            "longer-name sibling must be preserved in:\n{out}"
        );
        assert!(
            !out.contains("- name: myagent-foo\n      type: anthropic"),
            "exact entry should have been removed from:\n{out}"
        );
    }
}

#[cfg(test)]
mod handler_tests {
    use axum::body::Body;
    use axum::http::Request;
    use axum::http::StatusCode;
    use tower::ServiceExt;

    use super::{
        Credential, GenericSpec, NewProvider, ProviderApiError, ProviderConfigUpdateGeneric,
        ProviderConfigUpdateReq, ProviderKind, ProviderRemoveReq, ProviderRotateReq,
        handle_provider_config_update, handle_provider_remove, handle_provider_rotate,
    };

    /// Build a minimal internal router pointed at `agents_dir`, backed by a
    /// real `ProviderStore` in the temp home. Mirrors `make_test_router` from
    /// internal_api.rs.
    async fn make_provider_test_router(tmp: &std::path::Path) -> axum::Router {
        use crate::aggregator::{AgentInfo, BackendRegistry};
        use crate::right_backend::RightBackend;
        use dashmap::DashMap;
        use std::collections::HashMap;
        use std::sync::Arc;

        let agents_dir = tmp.join("agents");
        // Ensure the dispatcher has a known agent so token auth passes,
        // but the test agent ("hostagent") only needs the agent.yaml on disk.
        let placeholder_dir = agents_dir.join("hostagent");
        std::fs::create_dir_all(&placeholder_dir).unwrap();
        right_db::open_db(&placeholder_dir, true).await.unwrap();

        let right = RightBackend::new(agents_dir.clone(), None);
        let registry = BackendRegistry {
            right,
            proxies: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
            agent_dir: placeholder_dir.clone(),
            hindsight: None,
        };
        let agents = DashMap::new();
        agents.insert("hostagent".into(), registry);
        let dispatcher = Arc::new(crate::aggregator::ToolDispatcher { agents });

        let refresh_senders: crate::aggregator::RefreshSenders =
            Arc::new(std::collections::HashMap::new());
        let reconnect_managers: crate::aggregator::ReconnectManagers =
            Arc::new(std::collections::HashMap::new());

        let token_map_path = tmp.join("agent-tokens.json");
        std::fs::write(
            &token_map_path,
            serde_json::json!({"hostagent": "tok-test"}).to_string(),
        )
        .unwrap();
        let token_map: crate::aggregator::AgentTokenMap = {
            let mut map = std::collections::HashMap::new();
            map.insert(
                "tok-test".into(),
                AgentInfo {
                    name: "hostagent".into(),
                    dir: placeholder_dir,
                },
            );
            Arc::new(tokio::sync::RwLock::new(map))
        };

        let providers = right_providers::ProviderStore::open(tmp)
            .await
            .expect("temp provider store");
        crate::internal_api::internal_router(
            dispatcher,
            refresh_senders,
            reconnect_managers,
            token_map,
            token_map_path,
            agents_dir,
            providers,
        )
    }

    /// Build a minimal `InternalState` rooted at `tmp/agents` with a store in
    /// `tmp`, so unit tests can exercise the store's per-agent `agent_lock` and
    /// the agent.yaml RMW writer directly without going through the axum router.
    async fn make_provider_test_state(tmp: &std::path::Path) -> crate::internal_api::InternalState {
        use crate::aggregator::{AgentInfo, BackendRegistry};
        use crate::right_backend::RightBackend;
        use dashmap::DashMap;
        use std::collections::HashMap;
        use std::sync::Arc;

        let agents_dir = tmp.join("agents");
        let placeholder_dir = agents_dir.join("hostagent");
        std::fs::create_dir_all(&placeholder_dir).unwrap();
        right_db::open_db(&placeholder_dir, true).await.unwrap();

        let right = RightBackend::new(agents_dir.clone(), None);
        let registry = BackendRegistry {
            right,
            proxies: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
            agent_dir: placeholder_dir.clone(),
            hindsight: None,
        };
        let agents = DashMap::new();
        agents.insert("hostagent".into(), registry);
        let dispatcher = Arc::new(crate::aggregator::ToolDispatcher { agents });

        let refresh_senders: crate::aggregator::RefreshSenders =
            Arc::new(std::collections::HashMap::new());
        let reconnect_managers: crate::aggregator::ReconnectManagers =
            Arc::new(std::collections::HashMap::new());

        let token_map_path = tmp.join("agent-tokens.json");
        std::fs::write(
            &token_map_path,
            serde_json::json!({"hostagent": "tok-test"}).to_string(),
        )
        .unwrap();
        let token_map: crate::aggregator::AgentTokenMap = {
            let mut map = std::collections::HashMap::new();
            map.insert(
                "tok-test".into(),
                AgentInfo {
                    name: "hostagent".into(),
                    dir: placeholder_dir,
                },
            );
            Arc::new(tokio::sync::RwLock::new(map))
        };

        let providers = right_providers::ProviderStore::open(tmp)
            .await
            .expect("temp provider store");
        crate::internal_api::InternalState::new_for_test(
            dispatcher,
            refresh_senders,
            reconnect_managers,
            token_map,
            token_map_path,
            agents_dir,
            providers,
        )
    }

    #[tokio::test]
    async fn provider_create_generic_rejected_in_restrictive_mode() {
        let tmp = tempfile::tempdir().unwrap();
        let agent_dir = tmp.path().join("agents").join("hostagent");
        std::fs::create_dir_all(&agent_dir).unwrap();
        // Generic providers are only supported under the permissive network
        // policy, so creation must be refused up-front.
        std::fs::write(
            agent_dir.join("agent.yaml"),
            "network_policy: restrictive\n\
             sandbox:\n  name: hostagent\n",
        )
        .unwrap();

        let app = make_provider_test_router(tmp.path()).await;

        let req = Request::builder()
            .method("POST")
            .uri("/provider-create")
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::to_vec(&serde_json::json!({
                    "agent": "hostagent",
                    "type": "generic",
                    "label": "acme",
                    "credential": "secret-value",
                    "generic": {
                        "env_var": "ACME_API_KEY",
                        "upstream_host": "api.acme.invalid",
                    },
                }))
                .unwrap(),
            ))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        let body_bytes = axum::body::to_bytes(resp.into_body(), 1_000_000)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
        assert_eq!(
            json["code"], "network_policy_forbids_generic",
            "expected code=network_policy_forbids_generic, got: {json}"
        );
    }

    #[tokio::test]
    async fn provider_rotate_fails_fast_on_unknown_builtin() {
        // If an agent.yaml has a BuiltIn(slug) that the catalog no longer
        // knows about (e.g. catalog renamed/dropped), rotating that provider
        // MUST fail with HTTP 500 + code=unknown_builtin_slug rather than
        // silently inserting "" as the credential key (AGENTS.rust.md §2
        // FAIL-FAST). The gateway-era code surfaced this from
        // `extract_env_var`; the store era surfaces it when the stored row's
        // slug fails to resolve.
        let tmp = tempfile::tempdir().unwrap();
        let agent_dir = tmp.path().join("agents").join("hostagent");
        std::fs::create_dir_all(&agent_dir).unwrap();
        std::fs::write(
            agent_dir.join("agent.yaml"),
            "sandbox:\n  name: hostagent\n",
        )
        .unwrap();

        // Seed the stale row directly into the store (the create path would
        // reject an unknown slug, so simulate a catalog drift).
        let store = right_providers::ProviderStore::open(tmp.path())
            .await
            .expect("temp provider store");
        seed_stale_builtin(
            &store,
            "hostagent",
            "hostagent-stale",
            "definitely-not-a-real-slug",
        )
        .await;

        let app = make_provider_test_router(tmp.path()).await;
        let req = Request::builder()
            .method("POST")
            .uri("/provider-rotate")
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::to_vec(&serde_json::json!({
                    "agent": "hostagent",
                    "name": "hostagent-stale",
                    "credential": "new-secret",
                }))
                .unwrap(),
            ))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::INTERNAL_SERVER_ERROR,
            "unknown built-in slug must surface as 500, not silent success"
        );
        let body_bytes = axum::body::to_bytes(resp.into_body(), 1_000_000)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
        assert_eq!(
            json["code"], "unknown_builtin_slug",
            "expected code=unknown_builtin_slug, got: {json}"
        );
    }

    /// Insert a builtin row whose slug is NOT in the catalog, bypassing
    /// `ProviderStore::create`'s catalog validation, to model a record that
    /// drifted out of the catalog after creation.
    async fn seed_stale_builtin(
        store: &right_providers::ProviderStore,
        agent: &str,
        name: &str,
        slug: &str,
    ) {
        store
            .seed_builtin_unchecked(agent, name, slug, "STALE_KEY")
            .await
            .expect("seed stale builtin row");
    }

    #[tokio::test]
    async fn provider_list_marks_unknown_builtin_as_status_error() {
        // List view must NOT abort when a single entry references an unknown
        // slug: the bad row is marked status.kind=error so the operator sees
        // it; rotation on that same row still fails fast (see above).
        let tmp = tempfile::tempdir().unwrap();
        let agent_dir = tmp.path().join("agents").join("hostagent");
        std::fs::create_dir_all(&agent_dir).unwrap();
        std::fs::write(
            agent_dir.join("agent.yaml"),
            "sandbox:\n  name: hostagent\n",
        )
        .unwrap();

        let store = right_providers::ProviderStore::open(tmp.path())
            .await
            .expect("temp provider store");
        seed_stale_builtin(
            &store,
            "hostagent",
            "hostagent-stale",
            "definitely-not-a-real-slug",
        )
        .await;

        let app = make_provider_test_router(tmp.path()).await;
        let req = Request::builder()
            .method("POST")
            .uri("/provider-list")
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::to_vec(&serde_json::json!({"agent": "hostagent"})).unwrap(),
            ))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body_bytes = axum::body::to_bytes(resp.into_body(), 1_000_000)
            .await
            .unwrap();
        let arr: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
        let entries = arr.as_array().expect("list returns an array");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0]["name"], "hostagent-stale");
        assert_eq!(
            entries[0]["status"]["kind"], "error",
            "stale row must be marked, not silently emptied: {entries:?}"
        );
        let msg = entries[0]["status"]["message"].as_str().unwrap();
        assert!(
            msg.contains("definitely-not-a-real-slug"),
            "message must name the slug: {msg}"
        );
        // The stored env var ("") is what the row reports; the record keeps
        // whatever was stored at creation rather than resolving to nothing.
        assert_eq!(entries[0]["env_var"], "STALE_KEY");
    }

    #[tokio::test]
    async fn provider_create_tolerates_unknown_builtin_row() {
        // If providers.db carries a stale row whose slug is no longer in the
        // catalog, the env-var collision check inside create must skip it
        // rather than locking the operator out of adding ANY new provider.
        let tmp = tempfile::tempdir().unwrap();
        let agent_dir = tmp.path().join("agents").join("hostagent");
        std::fs::create_dir_all(&agent_dir).unwrap();
        std::fs::write(
            agent_dir.join("agent.yaml"),
            "sandbox:\n  name: hostagent\n",
        )
        .unwrap();

        let store = right_providers::ProviderStore::open(tmp.path())
            .await
            .expect("temp provider store");
        seed_stale_builtin(
            &store,
            "hostagent",
            "hostagent-stale",
            "definitely-not-a-real-slug",
        )
        .await;

        let app = make_provider_test_router(tmp.path()).await;
        let req = Request::builder()
            .method("POST")
            .uri("/provider-create")
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::to_vec(&serde_json::json!({
                    "agent": "hostagent",
                    "type": "gitlab",
                    "credential": "glpat-test",
                }))
                .unwrap(),
            ))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        let status = resp.status();
        let body_bytes = axum::body::to_bytes(resp.into_body(), 1_000_000)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
        assert_eq!(
            status,
            StatusCode::OK,
            "stale unknown_builtin row must not block creating a different provider; body={json}"
        );
    }

    #[tokio::test]
    async fn provider_config_update_rejects_invalid_name() {
        // /provider-config-update must validate `name` before acquiring the
        // per-agent lock or touching agent.yaml.
        let tmp = tempfile::tempdir().unwrap();
        let agent_dir = tmp.path().join("agents").join("hostagent");
        std::fs::create_dir_all(&agent_dir).unwrap();
        std::fs::write(
            agent_dir.join("agent.yaml"),
            "sandbox:\n  name: hostagent\n",
        )
        .unwrap();

        let app = make_provider_test_router(tmp.path()).await;
        let req = Request::builder()
            .method("POST")
            .uri("/provider-config-update")
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::to_vec(&serde_json::json!({
                    "agent": "hostagent",
                    "name": "../bad",
                    "generic": {
                        "upstream_host": "api.acme.invalid",
                    },
                }))
                .unwrap(),
            ))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let body_bytes = axum::body::to_bytes(resp.into_body(), 1_000_000)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
        assert_eq!(
            json["code"], "invalid_name",
            "expected code=invalid_name, got: {json}"
        );
    }

    /// Many concurrent provider mutations for DISTINCT providers on the SAME
    /// agent must all end up in agent.yaml. Keying the store's `agent_lock` on
    /// `(agent, name)` would let different names take different locks, and
    /// last-write-wins RMW would silently drop entries (store already
    /// mutated, agent.yaml an orphan). The per-agent lock serializes the
    /// whole read+write window.
    #[tokio::test(flavor = "multi_thread", worker_threads = 8)]
    async fn concurrent_provider_create_serializes_on_same_agent() {
        use super::{load_agent_config, serialize_provider_entry};

        let tmp = tempfile::tempdir().unwrap();
        let state = make_provider_test_state(tmp.path()).await;
        let agent_dir = tmp.path().join("agents").join("hostagent");
        std::fs::write(
            agent_dir.join("agent.yaml"),
            "sandbox:\n  name: hostagent\n",
        )
        .unwrap();

        // N distinct provider entries for the SAME agent.
        const N: usize = 5;
        let entries: Vec<_> = (0..N)
            .map(|i| right_agent_config::ProviderEntry {
                name: format!("hostagent-p{i:02}"),
                type_: right_agent_config::ProviderType::Generic,
                label: Some(format!("p{i:02}")),
                generic: Some(right_agent_config::GenericProvider {
                    env_var: format!("KEY_{i:02}"),
                    upstream_hosts: vec![format!("api{i:02}.example.com")],
                    upstream_path_prefix: None,
                }),
            })
            .collect();

        // Spawn N tasks each performing a guarded RMW with a deliberate
        // sleep between read and write, widening the race window.
        let agent_yaml = state.agents_dir.join("hostagent").join("agent.yaml");
        let mut tasks = Vec::with_capacity(N);
        for entry in entries.iter().cloned() {
            let state = state.clone();
            let agent_yaml = agent_yaml.clone();
            tasks.push(tokio::spawn(async move {
                let _guard = state.providers.agent_lock("hostagent").await;
                let existing = tokio::fs::read_to_string(&agent_yaml).await.unwrap();
                // Hold open the RMW window: under a per-name lock every task
                // reaches this sleep concurrently; under the per-agent lock
                // the next task is still blocked on `agent_lock`.
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                let updated =
                    super::insert_provider_entry(&existing, &serialize_provider_entry(&entry))
                        .unwrap();
                tokio::fs::write(&agent_yaml, updated).await.unwrap();
            }));
        }
        for t in tasks {
            t.await.unwrap();
        }

        let cfg = load_agent_config(&state.agents_dir, "hostagent")
            .expect("agent.yaml must parse after concurrent appends");
        let names: std::collections::BTreeSet<_> = cfg
            .sandbox
            .as_ref()
            .expect("sandbox section present")
            .providers
            .iter()
            .map(|p| p.name.clone())
            .collect();
        for entry in &entries {
            assert!(
                names.contains(&entry.name),
                "{} dropped from agent.yaml (RMW race): {names:?}",
                entry.name
            );
        }
        assert_eq!(
            names.len(),
            N,
            "expected exactly {N} providers after concurrent create, got: {names:?}"
        );
    }

    #[tokio::test]
    async fn provider_remove_rejects_borrowed_entry() {
        let tmp = tempfile::tempdir().unwrap();
        let state = make_provider_test_state(tmp.path()).await;
        let agent_dir = tmp.path().join("agents").join("hostagent");
        std::fs::write(
            agent_dir.join("agent.yaml"),
            "sandbox:\n  name: hostagent\n",
        )
        .unwrap();

        // Seed an owned record on the true owner, then borrow it here.
        state
            .providers
            .create(
                NewProvider {
                    owner_agent: "owner-agent".into(),
                    name: "shared-key".into(),
                    kind: ProviderKind::Builtin("right-fal".into()),
                    label: "seeded".into(),
                },
                Credential::from("owner-secret".to_string()),
            )
            .await
            .expect("seed owner record");
        state
            .providers
            .share("owner-agent", "shared-key", "hostagent")
            .await
            .expect("seed borrow");

        let req = ProviderRemoveReq {
            agent: "hostagent".into(),
            name: "shared-key".into(),
        };
        let result = handle_provider_remove(axum::extract::State(state), axum::Json(req)).await;
        assert!(
            matches!(
                result,
                Err(ProviderApiError::BorrowedProviderReadOnly { .. })
            ),
            "borrowed entry must be rejected: {result:?}"
        );
    }

    #[tokio::test]
    async fn provider_rotate_rejects_borrowed_entry() {
        let tmp = tempfile::tempdir().unwrap();
        let state = make_provider_test_state(tmp.path()).await;
        let agent_dir = tmp.path().join("agents").join("hostagent");
        std::fs::write(
            agent_dir.join("agent.yaml"),
            "sandbox:\n  name: hostagent\n",
        )
        .unwrap();

        state
            .providers
            .create(
                NewProvider {
                    owner_agent: "owner-agent".into(),
                    name: "shared-key".into(),
                    kind: ProviderKind::Builtin("right-fal".into()),
                    label: "seeded".into(),
                },
                Credential::from("owner-secret".to_string()),
            )
            .await
            .expect("seed owner record");
        state
            .providers
            .share("owner-agent", "shared-key", "hostagent")
            .await
            .expect("seed borrow");

        let req = ProviderRotateReq {
            agent: "hostagent".into(),
            name: "shared-key".into(),
            credential: secrecy::SecretString::from("secret"),
        };
        let result = handle_provider_rotate(axum::extract::State(state), axum::Json(req)).await;
        assert!(
            matches!(
                result,
                Err(ProviderApiError::BorrowedProviderReadOnly { .. })
            ),
            "borrowed entry must be rejected: {result:?}"
        );
    }

    #[tokio::test]
    async fn provider_config_update_rejects_borrowed_entry() {
        let tmp = tempfile::tempdir().unwrap();
        let state = make_provider_test_state(tmp.path()).await;
        let agent_dir = tmp.path().join("agents").join("hostagent");
        std::fs::write(
            agent_dir.join("agent.yaml"),
            "sandbox:\n  name: hostagent\n",
        )
        .unwrap();

        state
            .providers
            .create(
                NewProvider {
                    owner_agent: "owner-agent".into(),
                    name: "shared-key".into(),
                    kind: ProviderKind::Generic(GenericSpec {
                        env_var: "SHARED_KEY".into(),
                        upstream_hosts: vec!["api.example.com".into()],
                        upstream_path_prefix: None,
                    }),
                    label: "seeded".into(),
                },
                Credential::from("owner-secret".to_string()),
            )
            .await
            .expect("seed owner record");
        state
            .providers
            .share("owner-agent", "shared-key", "hostagent")
            .await
            .expect("seed borrow");

        let req = ProviderConfigUpdateReq {
            agent: "hostagent".into(),
            name: "shared-key".into(),
            generic: ProviderConfigUpdateGeneric {
                env_var: None,
                upstream_host: None,
                upstream_hosts: None,
                upstream_path_prefix: None,
            },
        };
        let result =
            handle_provider_config_update(axum::extract::State(state), axum::Json(req)).await;
        assert!(
            matches!(
                result,
                Err(ProviderApiError::BorrowedProviderReadOnly { .. })
            ),
            "borrowed entry must be rejected: {result:?}"
        );
    }

    #[tokio::test]
    async fn provider_list_rejects_legacy_sandboxless_agent_yaml() {
        let tmp = tempfile::tempdir().unwrap();
        let agent_dir = tmp.path().join("agents").join("hostagent");
        std::fs::create_dir_all(&agent_dir).unwrap();
        // `sandbox: mode: none` is gone: the config parser rejects it outright,
        // so the route surfaces an agent.yaml failure instead of the retired
        // `sandbox_mode_none` code.
        std::fs::write(agent_dir.join("agent.yaml"), "sandbox:\n  mode: none\n").unwrap();

        let app = make_provider_test_router(tmp.path()).await;

        let req = Request::builder()
            .method("POST")
            .uri("/provider-list")
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::to_vec(&serde_json::json!({"agent": "hostagent"})).unwrap(),
            ))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);

        let body_bytes = axum::body::to_bytes(resp.into_body(), 1_000_000)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
        assert_eq!(
            json["code"], "agent_yaml_write",
            "expected code=agent_yaml_write, got: {json}"
        );
        let message = json["message"].as_str().unwrap_or_default();
        assert!(
            message.contains("no longer supported"),
            "error must explain that sandboxless mode is gone, got: {json}"
        );
    }

    #[tokio::test]
    async fn provider_list_empty_for_fresh_agent() {
        let tmp = tempfile::tempdir().unwrap();
        let agent_dir = tmp.path().join("agents").join("hostagent");
        std::fs::create_dir_all(&agent_dir).unwrap();
        std::fs::write(
            agent_dir.join("agent.yaml"),
            "sandbox:\n  name: hostagent\n",
        )
        .unwrap();

        let app = make_provider_test_router(tmp.path()).await;
        let req = Request::builder()
            .method("POST")
            .uri("/provider-list")
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::to_vec(&serde_json::json!({"agent": "hostagent"})).unwrap(),
            ))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body_bytes = axum::body::to_bytes(resp.into_body(), 1_000_000)
            .await
            .unwrap();
        let arr: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
        assert_eq!(arr.as_array().unwrap().len(), 0);
    }
}

#[cfg(test)]
mod provider_types_tests {
    use super::*;

    #[tokio::test]
    async fn provider_types_hides_builtin_github_and_shows_right_github() {
        let axum::Json(types) = handle_provider_types().await;
        assert!(
            types.iter().all(|t| t.type_slug != "github"),
            "built-in read-only github is hidden from the dashboard"
        );
        assert!(
            types
                .iter()
                .any(|t| t.type_slug == "right-github" && t.display_name == "GitHub"),
            "right-github offered as GitHub"
        );
        assert!(
            types.iter().any(|t| t.type_slug == "gitlab"),
            "filter is narrow — other built-ins (gitlab) still offered"
        );
    }

    #[tokio::test]
    async fn every_hidden_catalog_entry_stays_offered_only_to_existing_records() {
        // Invariant: hidden catalog entries resolve (so existing records keep
        // working) but are never offered as new provider types.
        let axum::Json(types) = handle_provider_types().await;
        for p in ProviderStore::catalog() {
            if p.hidden {
                assert!(
                    types.iter().all(|t| t.type_slug != p.slug),
                    "hidden catalog entry {} must not be offered",
                    p.slug
                );
            } else {
                assert!(
                    types.iter().any(|t| t.type_slug == p.slug),
                    "visible catalog entry {} must be offered",
                    p.slug
                );
            }
        }
    }

    #[tokio::test]
    async fn provider_types_categories_render_lowercase() {
        let axum::Json(types) = handle_provider_types().await;
        for t in &types {
            assert_eq!(
                t.category,
                t.category.to_lowercase(),
                "category must render lowercase: {t:?}"
            );
            assert!(!t.category.is_empty());
        }
    }
}

#[cfg(test)]
mod peers_tests {
    use super::*;

    async fn open_store(dir: &std::path::Path) -> ProviderStore {
        ProviderStore::open(dir).await.expect("temp provider store")
    }

    fn write_agent(dir: &std::path::Path, name: &str, allow_ids: &[i64], providers_yaml: &str) {
        let agent_dir = dir.join(name);
        std::fs::create_dir_all(&agent_dir).unwrap();
        let users = allow_ids
            .iter()
            .map(|id| format!("  - id: {id}\n    added_at: 2026-01-01T00:00:00Z"))
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(
            agent_dir.join("allowlist.yaml"),
            format!("version: 2\nusers:\n{users}\n"),
        )
        .unwrap();
        std::fs::write(
            agent_dir.join("agent.yaml"),
            format!("sandbox:\n{providers_yaml}"),
        )
        .unwrap();
    }

    async fn create_builtin(store: &ProviderStore, agent: &str, name: &str, slug: &str) {
        store
            .create(
                NewProvider {
                    owner_agent: agent.into(),
                    name: name.into(),
                    kind: ProviderKind::Builtin(slug.into()),
                    label: "seeded".into(),
                },
                Credential::from("peer-secret".to_string()),
            )
            .await
            .expect("seed peer provider");
    }

    #[test]
    fn require_trusted_accepts_member_rejects_others() {
        let tmp = tempfile::tempdir().unwrap();
        write_agent(tmp.path(), "agent-a", &[7], "  providers: []\n");
        assert!(require_trusted(tmp.path(), "agent-a", 7).is_ok());
        let err = require_trusted(tmp.path(), "agent-a", 99).unwrap_err();
        assert!(matches!(err, ProviderApiError::Unauthorized { .. }));
    }

    #[test]
    fn require_trusted_rejects_when_no_allowlist() {
        let tmp = tempfile::tempdir().unwrap();
        let agent_dir = tmp.path().join("nolst");
        std::fs::create_dir_all(&agent_dir).unwrap();
        std::fs::write(agent_dir.join("agent.yaml"), "sandbox:\n  providers: []\n").unwrap();
        // Missing allowlist = secure default: deny all
        let err = require_trusted(tmp.path(), "nolst", 7).unwrap_err();
        assert!(matches!(err, ProviderApiError::Unauthorized { .. }));
    }

    #[tokio::test]
    async fn build_peers_skips_peer_with_legacy_sandboxless_agent_yaml() {
        let tmp = tempfile::tempdir().unwrap();
        write_agent(tmp.path(), "current", &[7], "  providers: []\n");
        // Trusted peer, but its agent.yaml still carries the removed
        // `mode: none`: it no longer parses, so discovery skips it instead of
        // aborting the whole listing.
        let agent_dir = tmp.path().join("hostmode");
        std::fs::create_dir_all(&agent_dir).unwrap();
        std::fs::write(
            agent_dir.join("allowlist.yaml"),
            "version: 2\nusers:\n  - id: 7\n    added_at: 2026-01-01T00:00:00Z\n",
        )
        .unwrap();
        std::fs::write(
            agent_dir.join("agent.yaml"),
            "sandbox:\n  mode: none\n  providers: []\n",
        )
        .unwrap();

        let store = open_store(tmp.path()).await;
        let peers = build_peers(&store, tmp.path(), 7, "current").await.unwrap();
        assert!(peers.iter().all(|p| p.agent != "hostmode"));
    }

    #[tokio::test]
    async fn build_peers_includes_peer_without_sandbox_section() {
        let tmp = tempfile::tempdir().unwrap();
        write_agent(tmp.path(), "current", &[7], "  providers: []\n");
        // No `sandbox:` section at all: every agent is sandboxed now, so the
        // peer is a normal share target rather than an excluded host agent.
        let agent_dir = tmp.path().join("plain");
        std::fs::create_dir_all(&agent_dir).unwrap();
        std::fs::write(
            agent_dir.join("allowlist.yaml"),
            "version: 2\nusers:\n  - id: 7\n    added_at: 2026-01-01T00:00:00Z\n",
        )
        .unwrap();
        std::fs::write(agent_dir.join("agent.yaml"), "model: sonnet\n").unwrap();

        let store = open_store(tmp.path()).await;
        let peers = build_peers(&store, tmp.path(), 7, "current").await.unwrap();
        assert!(
            peers.iter().any(|p| p.agent == "plain"),
            "agent without a sandbox section must still be offered as a peer: {peers:?}"
        );
    }

    #[tokio::test]
    async fn build_peers_excludes_agent_with_no_allowlist() {
        let tmp = tempfile::tempdir().unwrap();
        write_agent(tmp.path(), "current", &[7], "  providers: []\n");
        // Peer with no allowlist.yaml at all
        let no_allow_dir = tmp.path().join("nolst");
        std::fs::create_dir_all(&no_allow_dir).unwrap();
        std::fs::write(
            no_allow_dir.join("agent.yaml"),
            "sandbox:\n  providers:\n    - name: nolst-fal\n      type: right-fal\n",
        )
        .unwrap();

        let store = open_store(tmp.path()).await;
        let peers = build_peers(&store, tmp.path(), 7, "current").await.unwrap();
        assert!(peers.is_empty(), "peer with no allowlist must be excluded");
    }

    #[tokio::test]
    async fn build_peers_skips_peer_with_corrupt_allowlist_not_fatal() {
        let tmp = tempfile::tempdir().unwrap();
        write_agent(tmp.path(), "current", &[7], "  providers: []\n");
        // A healthy, trusted peer that must still be returned.
        write_agent(tmp.path(), "healthy", &[7], "  providers: []\n");
        // A peer whose allowlist.yaml is corrupt (users is not a sequence):
        // it must be skipped, never abort the whole listing.
        let bad_dir = tmp.path().join("corrupt");
        std::fs::create_dir_all(&bad_dir).unwrap();
        std::fs::write(
            bad_dir.join("allowlist.yaml"),
            "version: 2\nusers: not-a-list\n",
        )
        .unwrap();
        std::fs::write(bad_dir.join("agent.yaml"), "sandbox:\n  providers: []\n").unwrap();

        let store = open_store(tmp.path()).await;
        let peers = build_peers(&store, tmp.path(), 7, "current").await.unwrap();
        assert_eq!(
            peers.len(),
            1,
            "corrupt-allowlist peer skipped, healthy kept"
        );
        assert_eq!(peers[0].agent, "healthy");
    }

    #[tokio::test]
    async fn build_peers_excludes_self_and_untrusted_and_reports_providers() {
        let tmp = tempfile::tempdir().unwrap();
        write_agent(tmp.path(), "current", &[7], "  providers: []\n");
        write_agent(tmp.path(), "agent-a", &[7], "  providers: []\n");
        write_agent(tmp.path(), "secret", &[42], "  providers: []\n");

        let store = open_store(tmp.path()).await;
        create_builtin(&store, "agent-a", "agent-a-provider", "right-fal").await;

        let peers = build_peers(&store, tmp.path(), 7, "current").await.unwrap();
        let names: Vec<&str> = peers.iter().map(|p| p.agent.as_str()).collect();
        assert_eq!(names, vec!["agent-a"]); // self + untrusted filtered
        assert_eq!(peers[0].providers.len(), 1);
        assert_eq!(peers[0].providers[0].name, "agent-a-provider");
        assert_eq!(peers[0].providers[0].env_var, "FAL_KEY");
        assert_eq!(peers[0].network_policy, "permissive");
    }

    #[tokio::test]
    async fn build_peers_reads_borrowed_rows_without_credentials() {
        // A peer whose provider is BORROWED still lists it (name/env/type
        // only); the credential value never crosses the read path.
        let tmp = tempfile::tempdir().unwrap();
        write_agent(tmp.path(), "current", &[7], "  providers: []\n");
        write_agent(tmp.path(), "agent-a", &[7], "  providers: []\n");
        write_agent(tmp.path(), "borrower", &[7], "  providers: []\n");

        let store = open_store(tmp.path()).await;
        create_builtin(&store, "agent-a", "agent-a-provider", "right-fal").await;
        store
            .share("agent-a", "agent-a-provider", "borrower")
            .await
            .expect("share to borrower");

        let peers = build_peers(&store, tmp.path(), 7, "current").await.unwrap();
        let borrower = peers.iter().find(|p| p.agent == "borrower").unwrap();
        assert_eq!(borrower.providers.len(), 1);
        assert_eq!(borrower.providers[0].name, "agent-a-provider");
        assert_eq!(borrower.providers[0].env_var, "FAL_KEY");
        // PeerProvider has no credential field at all — structural redaction.
    }
}
