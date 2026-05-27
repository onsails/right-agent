use super::{
    MIN_OPENSHELL_VERSION, PreflightError, cli_version_check_str, parse_openshell_cli_version,
};
use crate::openshell_proto::openshell::v1 as proto_v1;
use crate::test_mock_server::{MockOpenShell, mock_client, start_mock_server};
use semver::Version;

#[test]
fn min_openshell_version_is_v0_0_50() {
    assert_eq!(MIN_OPENSHELL_VERSION, Version::new(0, 0, 50));
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
    let result = cli_version_check_str("openshell 0.0.50\n");
    assert!(result.is_ok(), "expected Ok, got: {result:?}");
}

#[test]
fn cli_version_check_passes_on_newer() {
    let result = cli_version_check_str("openshell 0.0.51\n");
    assert!(result.is_ok(), "expected Ok, got: {result:?}");
}

#[test]
fn cli_version_check_fails_on_too_old() {
    let result = cli_version_check_str("openshell 0.0.42\n");
    let err = result.unwrap_err();
    let found = match err {
        PreflightError::CliTooOld { found, required } => {
            assert_eq!(required, semver::Version::new(0, 0, 50));
            found
        }
        other => panic!("expected CliTooOld, got: {other:?}"),
    };
    assert_eq!(found, semver::Version::new(0, 0, 42));
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
                version: "0.0.50".into(),
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
                version: "0.0.49".into(),
            })
        })),
        ..Default::default()
    };
    let (addr, _shutdown) = start_mock_server(mock).await;
    let mut client = mock_client(addr).await;
    let err = super::gateway_version_check(&mut client).await.unwrap_err();
    match err {
        PreflightError::GatewayTooOld { found, required } => {
            assert_eq!(found, semver::Version::new(0, 0, 49));
            assert_eq!(required, semver::Version::new(0, 0, 50));
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
