use super::{GatewayCause, classify_doctor_output};

const DOCTOR_DOCKER_FAILED: &str = "Checking system prerequisites...\n\n  Docker ............. FAILED\n\nError:   × docker info failed: Cannot connect to the Docker daemon";
const DOCTOR_OK: &str = "Checking system prerequisites...\n\n  Docker ............. OK\n";
const STATUS_REFUSED: &str = "Server Status\n\n  Gateway: openshell\n  Server: https://127.0.0.1:17670\nError:   × client error (Connect)\n  ╰─▶ Connection refused (os error 61)";
const STATUS_OK: &str =
    "Server Status\n\n  Gateway: openshell\n  Server: https://127.0.0.1:17670\n  Version: 0.0.50";

#[test]
fn docker_failed_classifies_as_docker_down() {
    assert_eq!(
        classify_doctor_output(DOCTOR_DOCKER_FAILED, STATUS_REFUSED),
        GatewayCause::DockerDown
    );
}

#[test]
fn docker_ok_but_server_refused_classifies_as_gateway_not_started() {
    assert_eq!(
        classify_doctor_output(DOCTOR_OK, STATUS_REFUSED),
        GatewayCause::GatewayNotStarted
    );
}

#[test]
fn docker_ok_and_server_ok_classifies_as_unreachable() {
    // Connect failed yet probes look healthy — race or transient; generic advice.
    assert_eq!(
        classify_doctor_output(DOCTOR_OK, STATUS_OK),
        GatewayCause::Unreachable
    );
}

#[test]
fn empty_probe_output_classifies_as_unreachable() {
    assert_eq!(classify_doctor_output("", ""), GatewayCause::Unreachable);
}

#[test]
fn docker_down_diagnosis_summary_and_fixes_are_actionable() {
    let d = GatewayCause::DockerDown.diagnose();
    assert!(d.summary.to_lowercase().contains("docker"));
    assert!(!d.fixes.is_empty());
    assert!(d.fixes[0].to_lowercase().contains("docker"));
}

#[test]
fn sandbox_error_diagnosis_names_the_sandbox_and_is_recovery_oriented() {
    let d = GatewayCause::SandboxError {
        sandbox: "test-sandbox-1".to_owned(),
    }
    .diagnose();
    assert_eq!(
        d.cause,
        GatewayCause::SandboxError {
            sandbox: "test-sandbox-1".to_owned()
        }
    );
    assert!(d.summary.contains("test-sandbox-1"));
    assert!(!d.fixes.is_empty());
}
