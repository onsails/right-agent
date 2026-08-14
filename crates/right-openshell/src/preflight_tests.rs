use super::{
    MIN_OPENSHELL_VERSION, PreflightError, cli_version_check_str, parse_openshell_cli_version,
};
use crate::openshell_proto::openshell::v1 as proto_v1;
use crate::test_mock_server::{MockOpenShell, mock_client, start_mock_server};
use semver::Version;

#[test]
fn min_openshell_version_is_v0_0_105() {
    assert_eq!(MIN_OPENSHELL_VERSION, Version::new(0, 0, 105));
}

#[test]
fn parse_openshell_cli_version_extracts_semver() {
    let v = parse_openshell_cli_version("openshell 0.0.50\n").unwrap();
    assert_eq!(v, Version::new(0, 0, 50));
}

#[test]
fn parse_openshell_cli_version_rejects_garbage() {
    let err = parse_openshell_cli_version("not a version line\n").unwrap_err();
    assert!(
        err.contains("could not parse"),
        "error should mention parse failure, got: {err}"
    );
}

#[test]
fn parse_openshell_cli_version_ignores_trailing_whitespace_and_lines() {
    let v = parse_openshell_cli_version("openshell 0.0.50  \n\nextra line\n").unwrap();
    assert_eq!(v, Version::new(0, 0, 50));
}

#[test]
fn cli_version_check_passes_on_exact_min() {
    let result = cli_version_check_str("openshell 0.0.105\n");
    assert!(result.is_ok(), "expected Ok, got: {result:?}");
}

#[test]
fn cli_version_check_passes_on_newer() {
    let result = cli_version_check_str("openshell 0.0.106\n");
    assert!(result.is_ok(), "expected Ok, got: {result:?}");
}

#[test]
fn cli_version_check_fails_on_too_old() {
    let result = cli_version_check_str("openshell 0.0.104\n");
    let err = result.unwrap_err();
    let found = match err {
        PreflightError::CliTooOld { found, required } => {
            assert_eq!(required, semver::Version::new(0, 0, 105));
            found
        }
        other => panic!("expected CliTooOld, got: {other:?}"),
    };
    assert_eq!(found, semver::Version::new(0, 0, 104));
}

#[test]
fn cli_version_check_fails_on_unparseable_output() {
    let result = cli_version_check_str("garbage\n");
    assert!(matches!(
        result,
        Err(PreflightError::CliVersionUnparseable(_))
    ));
}

// ---------------------------------------------------------------------------
// gateway_version_check tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn gateway_version_check_passes_on_exact_min() {
    let mock = MockOpenShell {
        mock_health: Some(Box::new(|| {
            Ok(proto_v1::HealthResponse {
                status: 0,
                version: "0.0.105".into(),
            })
        })),
        ..Default::default()
    };
    let (addr, _shutdown) = start_mock_server(mock).await;
    let mut client = mock_client(addr).await;
    let result = super::gateway_version_check(&mut client).await;
    assert!(result.is_ok(), "expected Ok, got: {result:?}");
}

#[tokio::test]
async fn gateway_version_check_fails_on_too_old() {
    let mock = MockOpenShell {
        mock_health: Some(Box::new(|| {
            Ok(proto_v1::HealthResponse {
                status: 0,
                version: "0.0.104".into(),
            })
        })),
        ..Default::default()
    };
    let (addr, _shutdown) = start_mock_server(mock).await;
    let mut client = mock_client(addr).await;
    let err = super::gateway_version_check(&mut client).await.unwrap_err();
    match err {
        PreflightError::GatewayTooOld { found, required } => {
            assert_eq!(found, semver::Version::new(0, 0, 104));
            assert_eq!(required, semver::Version::new(0, 0, 105));
        }
        other => panic!("expected GatewayTooOld, got: {other:?}"),
    }
}

#[tokio::test]
async fn gateway_version_check_fails_on_unparseable_version() {
    let mock = MockOpenShell {
        mock_health: Some(Box::new(|| {
            Ok(proto_v1::HealthResponse {
                status: 0,
                version: "garbage".into(),
            })
        })),
        ..Default::default()
    };
    let (addr, _shutdown) = start_mock_server(mock).await;
    let mut client = mock_client(addr).await;
    let result = super::gateway_version_check(&mut client).await;
    assert!(matches!(
        result,
        Err(PreflightError::GatewayVersionUnparseable(_))
    ));
}

#[tokio::test]
async fn gateway_version_check_fails_when_health_rpc_errors() {
    let mock = MockOpenShell {
        mock_health: Some(Box::new(|| Err(tonic::Status::internal("boom")))),
        ..Default::default()
    };
    let (addr, _shutdown) = start_mock_server(mock).await;
    let mut client = mock_client(addr).await;
    let result = super::gateway_version_check(&mut client).await;
    assert!(matches!(result, Err(PreflightError::GatewayUnreachable(_))));
}

// ---------------------------------------------------------------------------
// openshell_preflight_with tests
// ---------------------------------------------------------------------------

// Integration-style: spawn the mock server, hand a configured client to
// openshell_preflight, and assert it composes cli + gateway checks.
//
// The CLI half is exercised by spawning a fake `openshell --version`
// shim via env override. To keep this test hermetic without setting
// process env (forbidden by AGENTS.rust.md), we route via
// `openshell_preflight_with` which takes both a closure returning the
// CLI version string and the gRPC client.

#[tokio::test]
async fn openshell_preflight_with_succeeds_when_both_ok() {
    let mock = MockOpenShell {
        mock_health: Some(Box::new(|| {
            Ok(proto_v1::HealthResponse {
                status: 0,
                version: "0.0.105".into(),
            })
        })),
        ..Default::default()
    };
    let (addr, _shutdown) = start_mock_server(mock).await;
    let mut client = mock_client(addr).await;

    let result = super::openshell_preflight_with(
        || async { Ok("openshell 0.0.105\n".to_string()) },
        &mut client,
    )
    .await;
    assert!(result.is_ok(), "expected Ok, got: {result:?}");
}

#[tokio::test]
async fn openshell_preflight_with_fails_fast_on_cli_too_old() {
    let mock = MockOpenShell {
        mock_health: Some(Box::new(|| {
            Ok(proto_v1::HealthResponse {
                status: 0,
                version: "0.0.105".into(),
            })
        })),
        ..Default::default()
    };
    let (addr, _shutdown) = start_mock_server(mock).await;
    let mut client = mock_client(addr).await;

    let result = super::openshell_preflight_with(
        || async { Ok("openshell 0.0.104\n".to_string()) },
        &mut client,
    )
    .await;
    assert!(matches!(result, Err(PreflightError::CliTooOld { .. })));
}

#[tokio::test]
async fn openshell_preflight_with_fails_on_gateway_too_old() {
    let mock = MockOpenShell {
        mock_health: Some(Box::new(|| {
            Ok(proto_v1::HealthResponse {
                status: 0,
                version: "0.0.104".into(),
            })
        })),
        ..Default::default()
    };
    let (addr, _shutdown) = start_mock_server(mock).await;
    let mut client = mock_client(addr).await;

    let result = super::openshell_preflight_with(
        || async { Ok("openshell 0.0.105\n".to_string()) },
        &mut client,
    )
    .await;
    assert!(matches!(result, Err(PreflightError::GatewayTooOld { .. })));
}
