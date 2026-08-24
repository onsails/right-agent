use super::{
    SupervisorDeps, agent_sandbox_spec_for, degrade_decision, diagnose, resolve_named_provider,
    retryable_sync_diagnosis, right_managed_secret_env_vars, right_managed_secret_env_vars_with,
    secret_bindings, secret_bindings_with,
};
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

#[test]
fn transient_claude_acquisition_retries_but_invalid_config_remains_hard() {
    let transient = miette::Report::new(crate::claude_runtime::ClaudeRuntimeError::Retryable(
        miette::miette!("network timeout"),
    ));
    assert!(retryable_sync_diagnosis(&transient).is_some());

    let hard = miette::Report::new(crate::claude_runtime::ClaudeRuntimeError::Hard(
        miette::miette!("unsupported architecture"),
    ));
    assert!(retryable_sync_diagnosis(&hard).is_none());
    assert!(retryable_sync_diagnosis(&miette::miette!("malformed agent config")).is_none());
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

#[tokio::test]
async fn named_resolution_rejects_a_provider_absent_from_current_config() {
    let (_home, store) = store().await;
    store
        .create(
            declared_provider(),
            Credential::from("test-credential".to_owned()),
        )
        .await
        .expect("seed provider");
    let empty: AgentConfig = serde_saphyr::from_str("sandbox: {}\n").expect("config parses");

    let error = resolve_named_provider("riskoff", "riskoff-right-fal", &empty, &store)
        .await
        .expect_err("a stored provider absent from current agent config must fail");

    assert!(
        format!("{error:#}").contains("not declared"),
        "error must clearly name the config disagreement: {error:#}"
    );
}

#[tokio::test]
async fn named_resolution_rejects_a_missing_store_record() {
    let (_home, store) = store().await;
    let error = resolve_named_provider("riskoff", "riskoff-right-fal", &provider_config(), &store)
        .await
        .expect_err("a declared provider absent from the store must fail");

    let rendered = format!("{error:#}");
    assert!(
        rendered.contains("riskoff-right-fal") && rendered.contains("not found"),
        "error must identify the absent provider: {rendered}"
    );
}

#[tokio::test]
async fn named_resolution_turns_a_needs_value_record_into_a_store_backed_binding() {
    let (_home, store) = store().await;
    store
        .create(declared_provider(), Credential::absent())
        .await
        .expect("seed migrated provider");
    store
        .rotate(
            "riskoff",
            "riskoff-right-fal",
            Credential::from("test-credential".to_owned()),
        )
        .await
        .expect("dashboard-equivalent rotate");

    let binding =
        resolve_named_provider("riskoff", "riskoff-right-fal", &provider_config(), &store)
            .await
            .expect("rotated migrated provider resolves");

    assert_eq!(binding.env_var, "FAL_KEY");
    assert!(
        binding.source_env_var.starts_with("RIGHT_PROVIDER_"),
        "the seam carries an owner-scoped source identity rather than credential bytes"
    );
}

/// Startup uses the bulk resolver rather than the dashboard's named resolver.
/// A durable rotation made before restart must therefore be visible through
/// this exact seam before bring-up applies the binding to an attached sandbox.
#[tokio::test]
async fn startup_reconciliation_resolves_a_rotated_stored_credential() {
    let (_home, store) = store().await;
    store
        .create(
            declared_provider(),
            Credential::from("credential-before-restart".to_owned()),
        )
        .await
        .expect("seed the provider bound by the existing sandbox");
    store
        .rotate(
            "riskoff",
            "riskoff-right-fal",
            Credential::from("credential-after-rotation".to_owned()),
        )
        .await
        .expect("persist a dashboard-equivalent rotation");

    let bindings = secret_bindings("riskoff", &provider_config(), &store)
        .await
        .expect("startup reconciliation resolves the durable rotation");

    assert_eq!(bindings.len(), 1);
    assert_eq!(bindings[0].env_var, "FAL_KEY");
    assert!(
        bindings[0].source_env_var.starts_with("RIGHT_PROVIDER_"),
        "startup must apply an owner-scoped source identity rather than credential bytes"
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
            Credential::from("test-credential".to_owned()),
        )
        .await
        .expect("create");

    let spec = agent_sandbox_spec_for("riskoff", "right-riskoff", &provider_config(), &store)
        .await
        .expect("a credential-holding provider binds");

    assert_eq!(spec.secrets.len(), 1);
    assert_eq!(spec.secrets[0].env_var, "FAL_KEY");
}

#[tokio::test]
async fn borrower_resolution_uses_the_borrowed_entry_name_and_owner_definition() {
    let (_home, store) = store().await;
    store
        .create(
            declared_provider(),
            Credential::from("shared-value".to_owned()),
        )
        .await
        .expect("create owner record");
    store
        .share("riskoff", "riskoff-right-fal", "borrower")
        .await
        .expect("share record");
    let borrower_config: AgentConfig = serde_saphyr::from_str(
        "sandbox:\n  providers:\n    - name: riskoff-right-fal\n      type: right-fal\n",
    )
    .expect("borrower config parses");

    let binding = resolve_named_provider("borrower", "riskoff-right-fal", &borrower_config, &store)
        .await
        .expect("borrower resolves the owner's current credential");
    assert_eq!(binding.env_var, "FAL_KEY");
    assert_eq!(
        binding.source_env_var,
        right_providers::source_env_var("riskoff", "riskoff-right-fal")
    );
}

#[tokio::test]
async fn recovery_snapshots_the_latest_accepted_provider_config() {
    let (_temp, providers) = store().await;
    let startup = provider_config();
    let latest: AgentConfig = serde_saphyr::from_str(
        "sandbox:\n  providers:\n    - name: later-provider\n      type: right-fal\n",
    )
    .expect("latest config parses");
    let config = std::sync::Arc::new(arc_swap::ArcSwap::from_pointee(startup));
    let deps = SupervisorDeps::new(
        "riskoff".to_owned(),
        std::path::PathBuf::from("/tmp/riskoff"),
        "right-riskoff".to_owned(),
        std::sync::Arc::clone(&config),
        std::sync::Arc::new(providers),
        std::sync::Arc::new(tokio::sync::Mutex::new(())),
        tokio_util::sync::CancellationToken::new(),
    );

    config.store(std::sync::Arc::new(latest));
    let snapshot = deps.config_snapshot();
    let names = snapshot
        .providers()
        .iter()
        .map(|provider| provider.name.as_str())
        .collect::<Vec<_>>();
    assert_eq!(names, ["later-provider"]);
}

#[test]
fn removal_candidates_cover_current_and_previous_guest_bindings() {
    let previous: AgentConfig = serde_saphyr::from_str(
        "sandbox:\n  providers:\n    - name: old-generic\n      type: generic\n      generic:\n        env_var: OLD_TOKEN\n        upstream_hosts: [api.old.test]\n    - name: fal\n      type: right-fal\n",
    )
    .expect("previous config parses");
    let current: AgentConfig = serde_saphyr::from_str(
        "sandbox:\n  providers:\n    - name: github\n      type: right-github\n",
    )
    .expect("current config parses");

    let candidates =
        right_managed_secret_env_vars(&[previous], &current).expect("identities resolve");
    assert_eq!(
        candidates,
        ["OLD_TOKEN", "FAL_KEY", "GITHUB_TOKEN"]
            .into_iter()
            .map(str::to_owned)
            .collect()
    );
}

#[test]
fn duplicate_provider_types_collapse_to_one_guest_binding_identity() {
    let previous: AgentConfig = serde_saphyr::from_str(
        "sandbox:\n  providers:\n    - name: github-old\n      type: github\n",
    )
    .expect("previous config parses");
    let current: AgentConfig = serde_saphyr::from_str(
        "sandbox:\n  providers:\n    - name: github-new\n      type: right-github\n",
    )
    .expect("current config parses");

    assert_eq!(
        right_managed_secret_env_vars(&[previous], &current).expect("identities resolve"),
        [String::from("GITHUB_TOKEN")].into_iter().collect()
    );
}

#[tokio::test]
async fn duplicate_provider_env_bindings_fail_before_sandbox_mutation() {
    let (_home, store) = store().await;
    store
        .create(
            NewProvider {
                owner_agent: "riskoff".to_owned(),
                name: "github-a".to_owned(),
                kind: ProviderKind::Builtin("github".to_owned()),
                label: String::new(),
            },
            Credential::from("test-credential".to_owned()),
        )
        .await
        .expect("seed first binding provider");

    let error = store
        .create(
            NewProvider {
                owner_agent: "riskoff".to_owned(),
                name: "github-b".to_owned(),
                kind: ProviderKind::Builtin("right-github".to_owned()),
                label: String::new(),
            },
            Credential::from("test-credential".to_owned()),
        )
        .await
        .expect_err("duplicate guest binding identity must fail in the store");

    let rendered = format!("{error:#}");
    assert!(
        rendered.contains("GITHUB_TOKEN"),
        "unexpected error: {rendered}"
    );
}

#[tokio::test]
async fn tolerant_reconcile_skips_unknown_builtin_slug() {
    let (_home, store) = store().await;
    let config: AgentConfig = serde_saphyr::from_str(
        "sandbox:\n  providers:\n    - name: mystery\n      type: right-mystery\n",
    )
    .expect("config with unknown built-in slug parses");

    // Tolerant mode must not propagate the unknown-slug error, so revocation
    // of other obsolete bindings can still proceed.
    let bindings = secret_bindings_with("riskoff", &config, &store, true)
        .await
        .expect("tolerant reconcile must skip the unresolvable provider");
    assert!(
        bindings.is_empty(),
        "an unknown built-in slug resolves to no binding"
    );

    let managed =
        right_managed_secret_env_vars_with(&[], &config, true).expect("managed identities resolve");
    assert!(managed.is_empty(), "the skipped provider manages nothing");
}

#[tokio::test]
async fn tolerant_reconcile_skips_a_missing_store_record() {
    let (_home, store) = store().await;

    // The record is resolvable (a known built-in slug) but absent from the
    // store; tolerant mode downgrades the store error to a skip, while strict
    // mode (create-time) must still hard-fail.
    let strict_err = secret_bindings("riskoff", &provider_config(), &store)
        .await
        .expect_err("create-time resolution must still hard-fail on a missing record");
    let rendered = format!("{strict_err:#}");
    assert!(
        rendered.contains("riskoff-right-fal"),
        "error names the provider"
    );

    let bindings = secret_bindings_with("riskoff", &provider_config(), &store, true)
        .await
        .expect("tolerant reconcile must skip the unresolvable provider");
    assert!(
        bindings.is_empty(),
        "a missing store record resolves to no binding in tolerant mode"
    );
}
