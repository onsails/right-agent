use super::{degrade_decision, diagnose};
use right_sandbox::{SandboxCause, SandboxError, SandboxPhase};

// The `network_policy` → egress mapping now lives with the shared spec
// builder in `right-sandbox` (`agent::tests`), where the CLI's creator sees
// the same assertions.

#[test]
fn a_stopped_sandbox_diagnoses_as_not_running() {
    let diagnosis = diagnose(&SandboxError::NotRunning {
        name: "right-agent".to_owned(),
        phase: SandboxPhase::Stopped,
    });

    assert_eq!(
        diagnosis.cause,
        SandboxCause::SandboxNotRunning {
            sandbox: "right-agent".to_owned()
        }
    );
    assert!(!diagnosis.fixes.is_empty(), "every diagnosis offers a fix");
}

#[test]
fn a_command_level_failure_is_inconclusive_not_a_backend_verdict() {
    // `ExecSpawn` says nothing about backend health; degrading on it would
    // take the agent down for a typo in a guest command.
    let diagnosis = diagnose(&SandboxError::ExecSpawn {
        name: "right-agent".to_owned(),
        cmd: "/bin/sh".to_owned(),
        kind: "NotFound".to_owned(),
        message: "no such file".to_owned(),
    });

    assert_eq!(diagnosis.cause, SandboxCause::Unreachable);
}

/// Restores the property the deleted `sandbox_supervisor_phase_tests.rs`
/// guarded under the gateway taxonomy ("a transient provisioning phase does
/// not degrade"), now expressed in microsandbox phases.
#[test]
fn a_still_booting_sandbox_does_not_degrade() {
    for phase in [SandboxPhase::Created, SandboxPhase::Starting] {
        let decision = degrade_decision(&SandboxError::NotRunning {
            name: "right-agent".to_owned(),
            phase,
        });
        assert!(
            decision.is_none(),
            "{phase} is a sandbox on its way up, not a failed one"
        );
    }
}

#[test]
fn a_terminal_phase_degrades() {
    for phase in [SandboxPhase::Stopped, SandboxPhase::Crashed] {
        let diagnosis = degrade_decision(&SandboxError::NotRunning {
            name: "right-agent".to_owned(),
            phase,
        })
        .unwrap_or_else(|| panic!("{phase} must degrade"));
        assert_eq!(
            diagnosis.cause,
            SandboxCause::SandboxNotRunning {
                sandbox: "right-agent".to_owned()
            }
        );
    }
}

/// A sandbox that cannot be reached at all is a failure regardless of phase:
/// the transient-phase exemption must not swallow runtime errors.
#[test]
fn an_unreachable_runtime_degrades() {
    let diagnosis = degrade_decision(&SandboxError::NotFound {
        name: "right-agent".to_owned(),
    })
    .expect("a missing sandbox must degrade");
    assert_eq!(
        diagnosis.cause,
        SandboxCause::SandboxNotFound {
            sandbox: "right-agent".to_owned()
        }
    );
}
