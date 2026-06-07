use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::openshell_proto::openshell::datamodel::v1 as datamodel;
use crate::openshell_proto::openshell::sandbox::v1 as sandbox_v1;
use crate::openshell_proto::openshell::v1 as proto_v1;
use crate::providers::{
    PROVIDERS_V2_ENABLED_KEY, ProviderError, ProviderSpec, attach_to_sandbox, create_provider,
    delete_provider, detach_from_sandbox, ensure_v2_enabled, get_provider,
    get_sandbox_provider_environment, list_attached, list_providers_by_prefix,
    reconcile_for_sandbox, update_provider,
};
use crate::test_mock_server::{MockOpenShell, mock_client, start_mock_server};

#[tokio::test]
async fn create_provider_sends_typed_request() {
    let seen: Arc<Mutex<Option<proto_v1::CreateProviderRequest>>> = Arc::new(Mutex::new(None));
    let seen_clone = Arc::clone(&seen);
    let mock = MockOpenShell {
        mock_create_provider: Some(Box::new(move |req| {
            *seen_clone.lock().unwrap() = Some(req.clone());
            Ok(proto_v1::ProviderResponse {
                provider: req.provider,
            })
        })),
        ..Default::default()
    };
    let (addr, _shutdown) = start_mock_server(mock).await;
    let mut client = mock_client(addr).await;

    let mut creds = HashMap::new();
    creds.insert("MY_TOKEN".to_string(), "secret-value".to_string());
    let spec = ProviderSpec {
        name: "test-prov".into(),
        type_: "generic".into(),
        credentials: creds,
        config: HashMap::new(),
    };
    let created = create_provider(&mut client, &spec).await.unwrap();
    assert_eq!(created.name, "test-prov");

    let req = seen.lock().unwrap().clone().unwrap();
    let p = req.provider.unwrap();
    assert_eq!(p.metadata.unwrap().name, "test-prov");
    assert_eq!(p.r#type, "generic");
    assert_eq!(
        p.credentials.get("MY_TOKEN"),
        Some(&"secret-value".to_string())
    );
}

#[tokio::test]
async fn get_provider_maps_not_found() {
    let mock = MockOpenShell {
        mock_get_provider: Some(Box::new(|_| Err(tonic::Status::not_found("missing")))),
        ..Default::default()
    };
    let (addr, _shutdown) = start_mock_server(mock).await;
    let mut client = mock_client(addr).await;

    let err = get_provider(&mut client, "missing").await.unwrap_err();
    match err {
        ProviderError::NotFound(name) => assert_eq!(name, "missing"),
        other => panic!("expected NotFound, got: {other:?}"),
    }
}

#[tokio::test]
async fn get_provider_maps_other_status_to_grpc() {
    let mock = MockOpenShell {
        mock_get_provider: Some(Box::new(|_| Err(tonic::Status::internal("boom")))),
        ..Default::default()
    };
    let (addr, _shutdown) = start_mock_server(mock).await;
    let mut client = mock_client(addr).await;

    let err = get_provider(&mut client, "x").await.unwrap_err();
    match err {
        ProviderError::Grpc(msg) => {
            assert!(msg.contains("Internal"), "{msg}");
            assert!(msg.contains("boom"), "{msg}");
        }
        other => panic!("expected Grpc, got: {other:?}"),
    }
}

#[tokio::test]
async fn get_provider_decodes_provider_payload() {
    let mock = MockOpenShell {
        mock_get_provider: Some(Box::new(|_| {
            Ok(proto_v1::ProviderResponse {
                provider: Some(datamodel::Provider {
                    metadata: Some(datamodel::ObjectMeta {
                        name: "p1".into(),
                        ..Default::default()
                    }),
                    r#type: "generic".into(),
                    config: HashMap::new(),
                    credentials: HashMap::new(),
                    credential_expires_at_ms: HashMap::new(),
                }),
            })
        })),
        ..Default::default()
    };
    let (addr, _shutdown) = start_mock_server(mock).await;
    let mut client = mock_client(addr).await;
    let p = get_provider(&mut client, "p1").await.unwrap();
    assert_eq!(p.name, "p1");
    assert_eq!(p.type_, "generic");
}

#[tokio::test]
async fn update_provider_round_trip() {
    let seen: Arc<Mutex<Option<proto_v1::UpdateProviderRequest>>> = Arc::new(Mutex::new(None));
    let seen_clone = Arc::clone(&seen);
    let mock = MockOpenShell {
        mock_update_provider: Some(Box::new(move |req| {
            *seen_clone.lock().unwrap() = Some(req.clone());
            Ok(proto_v1::ProviderResponse {
                provider: req.provider,
            })
        })),
        ..Default::default()
    };
    let (addr, _shutdown) = start_mock_server(mock).await;
    let mut client = mock_client(addr).await;

    let mut creds = HashMap::new();
    creds.insert("ROT".into(), "v2".into());
    let spec = ProviderSpec {
        name: "rot".into(),
        type_: "generic".into(),
        credentials: creds,
        config: HashMap::new(),
    };
    let updated = update_provider(&mut client, &spec).await.unwrap();
    assert_eq!(updated.name, "rot");
    let req = seen.lock().unwrap().clone().unwrap();
    let p = req.provider.unwrap();
    assert_eq!(p.credentials.get("ROT"), Some(&"v2".to_string()));
}

#[tokio::test]
async fn delete_provider_returns_ok_on_success() {
    let mock = MockOpenShell {
        mock_delete_provider: Some(Box::new(|_| {
            Ok(proto_v1::DeleteProviderResponse { deleted: true })
        })),
        ..Default::default()
    };
    let (addr, _shutdown) = start_mock_server(mock).await;
    let mut client = mock_client(addr).await;
    delete_provider(&mut client, "x").await.unwrap();
}

#[tokio::test]
async fn delete_provider_maps_not_found() {
    let mock = MockOpenShell {
        mock_delete_provider: Some(Box::new(|_| Err(tonic::Status::not_found("absent")))),
        ..Default::default()
    };
    let (addr, _shutdown) = start_mock_server(mock).await;
    let mut client = mock_client(addr).await;
    let err = delete_provider(&mut client, "x").await.unwrap_err();
    assert!(matches!(err, ProviderError::NotFound(_)));
}

#[tokio::test]
async fn create_provider_request_debug_does_not_leak_credentials() {
    let mut creds = HashMap::new();
    creds.insert("MY_TOKEN".into(), "super-secret-value-xyz".into());
    let spec = ProviderSpec {
        name: "x".into(),
        type_: "generic".into(),
        credentials: creds,
        config: HashMap::new(),
    };
    let debug = format!("{spec:?}");
    assert!(
        !debug.contains("super-secret-value-xyz"),
        "credential value leaked through ProviderSpec Debug impl: {debug}"
    );
    assert!(debug.contains("redacted"), "debug should mention redaction");
}

#[tokio::test]
async fn list_providers_by_prefix_filters_client_side() {
    let mock = MockOpenShell {
        mock_list_providers: Some(Box::new(|_| {
            Ok(proto_v1::ListProvidersResponse {
                providers: vec![
                    datamodel::Provider {
                        metadata: Some(datamodel::ObjectMeta {
                            name: "agent1-acme".into(),
                            ..Default::default()
                        }),
                        r#type: "generic".into(),
                        ..Default::default()
                    },
                    datamodel::Provider {
                        metadata: Some(datamodel::ObjectMeta {
                            name: "agent2-acme".into(),
                            ..Default::default()
                        }),
                        r#type: "generic".into(),
                        ..Default::default()
                    },
                    datamodel::Provider {
                        metadata: Some(datamodel::ObjectMeta {
                            name: "agent1-other".into(),
                            ..Default::default()
                        }),
                        r#type: "generic".into(),
                        ..Default::default()
                    },
                ],
            })
        })),
        ..Default::default()
    };
    let (addr, _shutdown) = start_mock_server(mock).await;
    let mut client = mock_client(addr).await;
    let mut got = list_providers_by_prefix(&mut client, "agent1-")
        .await
        .unwrap();
    got.sort_by(|a, b| a.name.cmp(&b.name));
    assert_eq!(got.len(), 2);
    assert_eq!(got[0].name, "agent1-acme");
    assert_eq!(got[1].name, "agent1-other");
}

#[tokio::test]
async fn attach_to_sandbox_sends_typed_request() {
    let seen: Arc<Mutex<Option<proto_v1::AttachSandboxProviderRequest>>> =
        Arc::new(Mutex::new(None));
    let seen_clone = Arc::clone(&seen);
    let mock = MockOpenShell {
        mock_attach_sandbox_provider: Some(Box::new(move |req| {
            *seen_clone.lock().unwrap() = Some(req.clone());
            Ok(proto_v1::AttachSandboxProviderResponse {
                sandbox: None,
                attached: true,
            })
        })),
        ..Default::default()
    };
    let (addr, _shutdown) = start_mock_server(mock).await;
    let mut client = mock_client(addr).await;
    attach_to_sandbox(&mut client, "sbox1", "prov1")
        .await
        .unwrap();
    let req = seen.lock().unwrap().clone().unwrap();
    assert_eq!(req.sandbox_name, "sbox1");
    assert_eq!(req.provider_name, "prov1");
}

#[tokio::test]
async fn detach_from_sandbox_not_found() {
    let mock = MockOpenShell {
        mock_detach_sandbox_provider: Some(Box::new(|_| {
            Err(tonic::Status::not_found("not attached"))
        })),
        ..Default::default()
    };
    let (addr, _shutdown) = start_mock_server(mock).await;
    let mut client = mock_client(addr).await;
    let err = detach_from_sandbox(&mut client, "sbox", "prov")
        .await
        .unwrap_err();
    assert!(matches!(err, ProviderError::NotFound(_)));
}

#[tokio::test]
async fn list_attached_returns_names() {
    let mock = MockOpenShell {
        mock_list_sandbox_providers: Some(Box::new(|_| {
            Ok(proto_v1::ListSandboxProvidersResponse {
                providers: vec![
                    datamodel::Provider {
                        metadata: Some(datamodel::ObjectMeta {
                            name: "a".into(),
                            ..Default::default()
                        }),
                        ..Default::default()
                    },
                    datamodel::Provider {
                        metadata: Some(datamodel::ObjectMeta {
                            name: "b".into(),
                            ..Default::default()
                        }),
                        ..Default::default()
                    },
                ],
            })
        })),
        ..Default::default()
    };
    let (addr, _shutdown) = start_mock_server(mock).await;
    let mut client = mock_client(addr).await;
    let mut names = list_attached(&mut client, "sbox1").await.unwrap();
    names.sort();
    assert_eq!(names, vec!["a".to_string(), "b".to_string()]);
}

#[tokio::test]
async fn ensure_v2_enabled_upserts_global_bool_setting() {
    let seen: Arc<Mutex<Option<proto_v1::UpdateConfigRequest>>> = Arc::new(Mutex::new(None));
    let seen_clone = Arc::clone(&seen);
    let mock = MockOpenShell {
        mock_update_config: Some(Box::new(move |req| {
            *seen_clone.lock().unwrap() = Some(req.clone());
            Ok(proto_v1::UpdateConfigResponse {
                version: 0,
                policy_hash: String::new(),
                settings_revision: 1,
                deleted: false,
            })
        })),
        ..Default::default()
    };
    let (addr, _shutdown) = start_mock_server(mock).await;
    let mut client = mock_client(addr).await;

    ensure_v2_enabled(&mut client).await.unwrap();

    let req = seen.lock().unwrap().clone().unwrap();
    assert!(req.global, "must apply at gateway-global scope");
    assert_eq!(req.setting_key, PROVIDERS_V2_ENABLED_KEY);
    let value = req
        .setting_value
        .expect("setting_value present")
        .value
        .expect("oneof value present");
    assert_eq!(value, sandbox_v1::setting_value::Value::BoolValue(true));
}

#[tokio::test]
async fn ensure_v2_enabled_propagates_grpc_error() {
    let mock = MockOpenShell {
        mock_update_config: Some(Box::new(|_| Err(tonic::Status::internal("boom")))),
        ..Default::default()
    };
    let (addr, _shutdown) = start_mock_server(mock).await;
    let mut client = mock_client(addr).await;
    let err = ensure_v2_enabled(&mut client).await.unwrap_err();
    match err {
        ProviderError::Grpc(msg) => {
            assert!(msg.contains("boom"), "{msg}");
        }
        other => panic!("expected Grpc, got: {other:?}"),
    }
}

#[tokio::test]
async fn reconcile_ensures_v2_before_touching_providers_when_declared() {
    // update_config (ensure_v2) errors -> reconcile must surface that error,
    // proving ensure_v2 runs at the very top before list/attach.
    let mock = MockOpenShell {
        mock_update_config: Some(Box::new(|_| Err(tonic::Status::internal("v2-boom")))),
        ..Default::default()
    };
    let (addr, _shutdown) = start_mock_server(mock).await;
    let mut client = mock_client(addr).await;

    let err = reconcile_for_sandbox(&mut client, "sbx", "agent", &["agent-x".to_string()])
        .await
        .err()
        .expect("expected reconcile to fail before provider list/attach");
    match err {
        ProviderError::Grpc(msg) => assert!(msg.contains("v2-boom"), "{msg}"),
        other => panic!("expected Grpc from ensure_v2, got: {other:?}"),
    }
}

#[tokio::test]
async fn reconcile_skips_v2_when_nothing_declared() {
    let mock = MockOpenShell {
        mock_update_config: Some(Box::new(|_| Err(tonic::Status::internal("v2-boom")))),
        mock_list_sandbox_providers: Some(Box::new(|_| {
            Ok(proto_v1::ListSandboxProvidersResponse {
                providers: Vec::new(),
            })
        })),
        ..Default::default()
    };
    let (addr, _shutdown) = start_mock_server(mock).await;
    let mut client = mock_client(addr).await;

    let report = reconcile_for_sandbox(&mut client, "sbx", "agent", &[])
        .await
        .expect("empty declared reconcile must not require v2 enable");

    assert!(report.attached.is_empty());
    assert!(report.detached.is_empty());
    assert!(report.missing.is_empty());
    assert!(report.errors.is_empty());
}

#[tokio::test]
async fn reconcile_repairs_legacy_generic_provider_type_before_attaching() {
    let seen_update: Arc<Mutex<Option<proto_v1::UpdateProviderRequest>>> =
        Arc::new(Mutex::new(None));
    let seen_update_clone = Arc::clone(&seen_update);
    let seen_attach: Arc<Mutex<Option<proto_v1::AttachSandboxProviderRequest>>> =
        Arc::new(Mutex::new(None));
    let seen_attach_clone = Arc::clone(&seen_attach);
    let calls: Arc<Mutex<Vec<&'static str>>> = Arc::new(Mutex::new(Vec::new()));
    let calls_for_update = Arc::clone(&calls);
    let calls_for_attach = Arc::clone(&calls);

    let expected_type = crate::managed_profiles::generic_provider_profile_id("agent-acme");
    let expected_type_for_update = expected_type.clone();

    let mock = MockOpenShell {
        mock_update_config: Some(Box::new(|_| {
            Ok(proto_v1::UpdateConfigResponse {
                version: 0,
                policy_hash: String::new(),
                settings_revision: 1,
                deleted: false,
            })
        })),
        mock_list_sandbox_providers: Some(Box::new(|_| {
            Ok(proto_v1::ListSandboxProvidersResponse {
                providers: Vec::new(),
            })
        })),
        mock_get_provider: Some(Box::new(|req| {
            assert_eq!(req.name, "agent-acme");
            let mut legacy_config = HashMap::new();
            legacy_config.insert("upstream_host".into(), "api.acme.test".into());
            Ok(proto_v1::ProviderResponse {
                provider: Some(datamodel::Provider {
                    metadata: Some(datamodel::ObjectMeta {
                        name: req.name,
                        ..Default::default()
                    }),
                    r#type: "generic".into(),
                    config: legacy_config,
                    credentials: HashMap::new(),
                    credential_expires_at_ms: HashMap::new(),
                }),
            })
        })),
        mock_update_provider: Some(Box::new(move |req| {
            let provider = req.provider.clone().expect("provider update payload");
            assert_eq!(provider.r#type, expected_type_for_update);
            calls_for_update.lock().unwrap().push("update");
            *seen_update_clone.lock().unwrap() = Some(req);
            Ok(proto_v1::ProviderResponse {
                provider: Some(provider),
            })
        })),
        mock_attach_sandbox_provider: Some(Box::new(move |req| {
            calls_for_attach.lock().unwrap().push("attach");
            *seen_attach_clone.lock().unwrap() = Some(req);
            Ok(proto_v1::AttachSandboxProviderResponse {
                sandbox: None,
                attached: true,
            })
        })),
        ..Default::default()
    };
    let (addr, _shutdown) = start_mock_server(mock).await;
    let mut client = mock_client(addr).await;

    let report = reconcile_for_sandbox(&mut client, "sbx", "agent", &["agent-acme".to_string()])
        .await
        .unwrap();

    assert_eq!(report.repaired, vec!["agent-acme".to_string()]);
    assert_eq!(report.attached, vec!["agent-acme".to_string()]);
    assert!(report.detached.is_empty());
    assert!(report.missing.is_empty());
    assert!(report.errors.is_empty());
    assert_eq!(
        *calls.lock().unwrap(),
        vec!["update", "attach"],
        "legacy generic provider must be repaired before it is attached"
    );

    let update_req = seen_update
        .lock()
        .unwrap()
        .clone()
        .expect("legacy generic provider must be updated");
    let updated_provider = update_req.provider.expect("provider update payload");
    assert_eq!(
        updated_provider.metadata.unwrap().name,
        "agent-acme",
        "repair must update the existing gateway provider, not create a new one"
    );
    assert_eq!(updated_provider.r#type, expected_type);
    assert!(
        updated_provider.credentials.is_empty(),
        "repair must preserve existing gateway credential bytes via sparse credential update"
    );
    assert!(
        updated_provider.config.is_empty(),
        "new generic provider shape keeps upstream config in the authored profile, not Provider.config"
    );

    let attach_req = seen_attach
        .lock()
        .unwrap()
        .clone()
        .expect("repaired provider must still be attached");
    assert_eq!(attach_req.sandbox_name, "sbx");
    assert_eq!(attach_req.provider_name, "agent-acme");
}

#[tokio::test]
async fn reconcile_reports_legacy_generic_repair_errors_and_skips_attach() {
    let attach_calls = Arc::new(Mutex::new(0usize));
    let attach_calls_clone = Arc::clone(&attach_calls);

    let mock = MockOpenShell {
        mock_update_config: Some(Box::new(|_| {
            Ok(proto_v1::UpdateConfigResponse {
                version: 0,
                policy_hash: String::new(),
                settings_revision: 1,
                deleted: false,
            })
        })),
        mock_list_sandbox_providers: Some(Box::new(|_| {
            Ok(proto_v1::ListSandboxProvidersResponse {
                providers: Vec::new(),
            })
        })),
        mock_get_provider: Some(Box::new(|req| {
            Ok(proto_v1::ProviderResponse {
                provider: Some(datamodel::Provider {
                    metadata: Some(datamodel::ObjectMeta {
                        name: req.name,
                        ..Default::default()
                    }),
                    r#type: "generic".into(),
                    config: HashMap::new(),
                    credentials: HashMap::new(),
                    credential_expires_at_ms: HashMap::new(),
                }),
            })
        })),
        mock_update_provider: Some(Box::new(|_| Err(tonic::Status::internal("repair boom")))),
        mock_attach_sandbox_provider: Some(Box::new(move |_| {
            *attach_calls_clone.lock().unwrap() += 1;
            Ok(proto_v1::AttachSandboxProviderResponse {
                sandbox: None,
                attached: true,
            })
        })),
        ..Default::default()
    };
    let (addr, _shutdown) = start_mock_server(mock).await;
    let mut client = mock_client(addr).await;

    let report = reconcile_for_sandbox(&mut client, "sbx", "agent", &["agent-acme".to_string()])
        .await
        .unwrap();

    assert!(report.repaired.is_empty());
    assert!(report.attached.is_empty());
    assert!(report.detached.is_empty());
    assert!(report.missing.is_empty());
    assert_eq!(*attach_calls.lock().unwrap(), 0);
    assert_eq!(report.errors.len(), 1);
    assert_eq!(report.errors[0].0, "agent-acme");
    assert!(
        report.errors[0].1.contains("update:"),
        "repair failure must be classified as an update error: {:?}",
        report.errors
    );
    assert!(
        report.errors[0].1.contains("repair boom"),
        "repair failure detail must be preserved for logs: {:?}",
        report.errors
    );
}

#[tokio::test]
async fn get_sandbox_provider_environment_returns_map() {
    let mock = MockOpenShell {
        mock_get_sandbox_provider_environment: Some(Box::new(|req| {
            assert_eq!(req.sandbox_id, "sbox-id-xyz");
            let mut env = HashMap::new();
            env.insert(
                "MY_TOKEN".into(),
                "openshell:resolve:env:v1_MY_TOKEN".into(),
            );
            Ok(proto_v1::GetSandboxProviderEnvironmentResponse {
                environment: env,
                provider_env_revision: 1,
                credential_expires_at_ms: HashMap::new(),
            })
        })),
        ..Default::default()
    };
    let (addr, _shutdown) = start_mock_server(mock).await;
    let mut client = mock_client(addr).await;
    let env = get_sandbox_provider_environment(&mut client, "sbox-id-xyz")
        .await
        .unwrap();
    assert_eq!(
        env.get("MY_TOKEN"),
        Some(&"openshell:resolve:env:v1_MY_TOKEN".to_string())
    );
}
