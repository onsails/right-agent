mod degrade_decision {
    use super::super::{ProbeOutcome, degrade_decision};
    use right_openshell::diagnosis::GatewayCause;

    #[test]
    fn ready_phase_does_not_degrade() {
        assert!(degrade_decision(ProbeOutcome::Ready, "test-sandbox-1").is_none());
    }

    #[test]
    fn error_phase_degrades_with_sandbox_error_cause() {
        let diag = degrade_decision(
            ProbeOutcome::Error {
                detail: "phase=ERROR status=...".to_owned(),
            },
            "test-sandbox-1",
        )
        .expect("Error phase must degrade");
        assert_eq!(
            diag.cause,
            GatewayCause::SandboxError {
                sandbox: "test-sandbox-1".to_owned()
            }
        );
    }

    #[test]
    fn gateway_unreachable_degrades_with_gateway_diagnosis() {
        let diag = degrade_decision(
            ProbeOutcome::GatewayDiagnosis(GatewayCause::DockerDown.diagnose()),
            "test-sandbox-1",
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
                "test-sandbox-1"
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
        assert!(bring_up_phase_diagnosis(SandboxPhaseStatus::Ready, "test-sandbox-1").is_none());
    }

    #[test]
    fn error_phase_preserves_sandbox_error_cause() {
        let diag = bring_up_phase_diagnosis(
            SandboxPhaseStatus::Error {
                detail: "phase=ERROR status=...".to_owned(),
            },
            "test-sandbox-1",
        )
        .expect("Error phase must degrade bring-up");

        assert_eq!(
            diag.cause,
            GatewayCause::SandboxError {
                sandbox: "test-sandbox-1".to_owned()
            }
        );
    }

    #[test]
    fn not_found_phase_reports_missing_sandbox() {
        let diag = bring_up_phase_diagnosis(SandboxPhaseStatus::NotFound, "test-sandbox-1")
            .expect("NotFound phase must degrade bring-up");

        assert_eq!(
            diag.cause,
            GatewayCause::SandboxNotFound {
                sandbox: "test-sandbox-1".to_owned()
            }
        );
    }

    #[test]
    fn other_phase_reports_not_ready_not_missing_or_unreachable() {
        let diag = bring_up_phase_diagnosis(
            SandboxPhaseStatus::Other {
                phase: "PROVISIONING".to_owned(),
                detail: "phase=PROVISIONING status=...".to_owned(),
            },
            "test-sandbox-1",
        )
        .expect("Other phase must degrade bring-up recoverably");

        // A provisioning sandbox is neither missing nor unreachable: it exists
        // and the gateway answered. Asserting SandboxNotReady pins that the copy
        // is "still starting up", not "create it with right init" / "check Docker".
        assert_eq!(
            diag.cause,
            GatewayCause::SandboxNotReady {
                sandbox: "test-sandbox-1".to_owned()
            }
        );
    }
}

mod provider_reconcile_diagnosis {
    use super::super::provider_reconcile_diagnosis;
    use right_openshell::diagnosis::GatewayCause;

    #[test]
    fn reports_provider_failure_not_sandbox_startup() {
        let diag = provider_reconcile_diagnosis(
            "right-right-1",
            "provider right-typefully attached but not composed".to_owned(),
        );

        assert_eq!(
            diag.cause,
            GatewayCause::ProviderComposition {
                sandbox: "right-right-1".to_owned(),
                detail: "provider right-typefully attached but not composed".to_owned()
            }
        );
        assert!(diag.summary.contains("provider access"));
        assert!(diag.summary.contains("right-typefully"));
        assert!(!diag.summary.contains("starting up"));
    }
}
