mod degrade_decision {
    use super::super::{ProbeOutcome, degrade_decision};
    use right_openshell::diagnosis::GatewayCause;

    #[test]
    fn ready_phase_does_not_degrade() {
        assert!(degrade_decision(ProbeOutcome::Ready, "right-him-1").is_none());
    }

    #[test]
    fn error_phase_degrades_with_sandbox_error_cause() {
        let diag = degrade_decision(
            ProbeOutcome::Error {
                detail: "phase=ERROR status=...".to_owned(),
            },
            "right-him-1",
        )
        .expect("Error phase must degrade");
        assert_eq!(
            diag.cause,
            GatewayCause::SandboxError {
                sandbox: "right-him-1".to_owned()
            }
        );
    }

    #[test]
    fn gateway_unreachable_degrades_with_gateway_diagnosis() {
        let diag = degrade_decision(
            ProbeOutcome::GatewayDiagnosis(GatewayCause::DockerDown.diagnose()),
            "right-him-1",
        )
        .expect("gateway failure must degrade");
        assert_eq!(diag.cause, GatewayCause::DockerDown);
    }

    #[test]
    fn transient_other_phase_does_not_degrade() {
        assert!(
            degrade_decision(
                ProbeOutcome::Other {
                    phase: "PROVISIONING".to_owned(),
                    detail: "prov".to_owned(),
                },
                "right-him-1"
            )
            .is_none()
        );
    }
}

mod bring_up_phase_diagnosis {
    use super::super::bring_up_phase_diagnosis;
    use right_openshell::diagnosis::GatewayCause;
    use right_openshell::openshell::SandboxPhaseStatus;

    #[test]
    fn ready_phase_continues_bring_up() {
        assert!(bring_up_phase_diagnosis(SandboxPhaseStatus::Ready, "right-him-1").is_none());
    }

    #[test]
    fn error_phase_preserves_sandbox_error_cause() {
        let diag = bring_up_phase_diagnosis(
            SandboxPhaseStatus::Error {
                detail: "phase=ERROR status=...".to_owned(),
            },
            "right-him-1",
        )
        .expect("Error phase must degrade bring-up");

        assert_eq!(
            diag.cause,
            GatewayCause::SandboxError {
                sandbox: "right-him-1".to_owned()
            }
        );
    }

    #[test]
    fn not_found_phase_reports_missing_sandbox() {
        let diag = bring_up_phase_diagnosis(SandboxPhaseStatus::NotFound, "right-him-1")
            .expect("NotFound phase must degrade bring-up");

        assert_eq!(
            diag.cause,
            GatewayCause::SandboxNotFound {
                sandbox: "right-him-1".to_owned()
            }
        );
    }

    #[test]
    fn other_phase_does_not_claim_sandbox_missing() {
        let diag = bring_up_phase_diagnosis(
            SandboxPhaseStatus::Other {
                phase: "PROVISIONING".to_owned(),
                detail: "phase=PROVISIONING status=...".to_owned(),
            },
            "right-him-1",
        )
        .expect("Other phase must degrade bring-up recoverably");

        assert_ne!(
            diag.cause,
            GatewayCause::SandboxNotFound {
                sandbox: "right-him-1".to_owned()
            }
        );
        assert_eq!(diag.cause, GatewayCause::Unreachable);
    }
}
