use super::{RESTRICTIVE_EGRESS_ALLOW, diagnose, egress_for};
use right_agent_config::NetworkPolicy;
use right_sandbox::{Egress, SandboxCause, SandboxError, SandboxPhase};

#[test]
fn permissive_network_policy_opens_public_egress() {
    assert_eq!(egress_for(NetworkPolicy::Permissive), Egress::Permissive);
}

#[test]
fn restrictive_network_policy_allows_only_the_anthropic_suffixes() {
    let Egress::Restrictive { allow } = egress_for(NetworkPolicy::Restrictive) else {
        panic!("restrictive policy must not map to permissive egress");
    };
    assert_eq!(allow, RESTRICTIVE_EGRESS_ALLOW);
    assert!(
        !allow.iter().any(|domain| domain.starts_with("*.")),
        "entries are domain suffixes, not globs: {allow:?}"
    );
}

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
