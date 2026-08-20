use super::{agent_sandbox_spec_for, degrade_decision, diagnose};
use right_agent::agent::types::AgentConfig;
use right_providers::{Credential, NewProvider, ProviderKind, ProviderStore};
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

/// One built-in provider, declared exactly as `agent.yaml` declares it.
const AGENT_YAML: &str = "\
sandbox:
  name: right-riskoff
  providers:
    - name: riskoff-right-fal
      type: right-fal
";

fn provider_config() -> AgentConfig {
    serde_saphyr::from_str(AGENT_YAML).expect("fixture parses")
}

/// A fresh store on a temp home, plus the directory that must outlive it.
async fn store() -> (tempfile::TempDir, ProviderStore) {
    let home = tempfile::TempDir::new().expect("temp home");
    let store = ProviderStore::open(home.path())
        .await
        .expect("open providers.db");
    (home, store)
}

fn declared_provider() -> NewProvider {
    NewProvider {
        owner_agent: "riskoff".to_owned(),
        name: "riskoff-right-fal".to_owned(),
        kind: ProviderKind::Builtin("right-fal".to_owned()),
        label: String::new(),
    }
}

#[derive(Clone)]
struct SharedLogWriter(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

impl std::io::Write for SharedLogWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Capture this thread's tracing output for the guard's lifetime. `#[tokio::test]`
/// runs a current-thread runtime, so the future stays on the thread the
/// thread-local subscriber is installed on.
fn capture_log() -> (
    std::sync::Arc<std::sync::Mutex<Vec<u8>>>,
    tracing::subscriber::DefaultGuard,
) {
    let buffer = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let writer = SharedLogWriter(std::sync::Arc::clone(&buffer));
    let subscriber = tracing_subscriber::fmt()
        .with_writer(move || writer.clone())
        .with_ansi(false)
        .without_time()
        .finish();
    let guard = tracing::subscriber::set_default(subscriber);
    (buffer, guard)
}

fn captured(buffer: &std::sync::Arc<std::sync::Mutex<Vec<u8>>>) -> String {
    let bytes = buffer
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    String::from_utf8(bytes).expect("log is utf-8")
}

/// A migrated agent's providers exist with no credential. Bring-up must still
/// build a spec — refusing to start would leave the agent unrunnable forever,
/// since the credential is re-entered from the dashboard on a *running*
/// deployment — but it must not bind anything, and it must say so by name.
#[tokio::test]
async fn a_provider_awaiting_its_credential_is_skipped_with_a_warning() {
    let (_home, store) = store().await;
    store
        .create(declared_provider(), Credential::absent())
        .await
        .expect("seed the record the migration writes");

    let (buffer, guard) = capture_log();
    let spec = agent_sandbox_spec_for("riskoff", "right-riskoff", &provider_config(), &store)
        .await
        .expect("a provider awaiting its credential must not stop bring-up");
    drop(guard);

    assert!(
        spec.secrets.is_empty(),
        "an unbound provider must produce no binding: a bound placeholder with no \
         value would let the agent believe it is authenticated"
    );
    let log = captured(&buffer);
    assert!(
        log.contains("riskoff-right-fal"),
        "the operator needs the provider's name: {log}"
    );
    assert!(
        log.contains("/providers"),
        "the operator needs where to fix it: {log}"
    );
}

/// The distinction the migration depends on: no record at all is a disagreement
/// between `agent.yaml` and `providers.db`, not a pending operator action.
#[tokio::test]
async fn a_declared_provider_with_no_record_still_hard_fails() {
    let (_home, store) = store().await;

    let error = agent_sandbox_spec_for("riskoff", "right-riskoff", &provider_config(), &store)
        .await
        .expect_err("a declared provider the store has never heard of is a hard error");

    let rendered = format!("{error:#}");
    assert!(
        rendered.contains("riskoff-right-fal") && rendered.contains("not found"),
        "the error must name the provider and why: {rendered}"
    );
}

/// The skip is targeted: a provider that does hold a credential still binds,
/// so tolerating one awaiting its value cannot quietly unbind the rest.
#[tokio::test]
async fn a_provider_that_holds_a_credential_still_binds() {
    let (_home, store) = store().await;
    store
        .create(
            declared_provider(),
            Credential::from("real-value".to_owned()),
        )
        .await
        .expect("create");

    let spec = agent_sandbox_spec_for("riskoff", "right-riskoff", &provider_config(), &store)
        .await
        .expect("a credential-holding provider binds");

    assert_eq!(spec.secrets.len(), 1);
    assert_eq!(spec.secrets[0].env_var, "FAL_KEY");
    assert!(
        !format!("{:?}", spec.secrets[0]).contains("real-value"),
        "a binding carries references only"
    );
}
