use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::openshell_proto::openshell::datamodel::v1 as datamodel;
use crate::openshell_proto::openshell::v1 as proto_v1;
use crate::providers::{
    ProviderError, ProviderSpec, create_provider, delete_provider, get_provider, update_provider,
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
