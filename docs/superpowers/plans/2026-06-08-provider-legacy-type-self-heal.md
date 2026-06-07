# Provider Legacy Type Self-Heal Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Existing agents with pre-profile generic OpenShell providers (`type = "generic"`) self-heal to the current Right-managed profile type (`right-provider-*`) during startup and hot provider reconcile, and the bot reports provider composition failures instead of saying the sandbox is still starting.

**Architecture:** Keep the migration in `right_openshell::providers::reconcile_for_sandbox`, the single gateway provider reconcile path used by bot startup and hot-reconcile. The reconciler repairs only declared providers that already exist and still have the legacy generic type, using a sparse `UpdateProvider` request with empty credentials so gateway-held secret bytes are preserved. Bot bring-up keeps the same recoverable degraded retry behavior, but maps provider reconcile/composition failures to a provider-specific `GatewayCause`.

**Tech Stack:** Rust 2024, tonic OpenShell gRPC wrappers, Tokio tests, `devenv shell -- cargo test`, descriptive architecture docs in `docs/architecture/providers.md`.

---

## Constraints And Assumptions

- Do not manually edit `~/.right/agents/*/agent.yaml`, OpenShell provider records, or sandboxes while implementing this. The platform must repair already-deployed agents by code.
- All commands run from repo root and use `devenv shell --`.
- The Rust-specific local instruction asks for `rust-dev:rust-dev`, but that skill is not available in the current skills list. Record that in the execution notes and continue with repo tests.
- OpenShell `UpdateProvider` with an empty credentials map is treated as sparse credential update by existing dashboard code paths; this plan relies on that established behavior and keeps credentials out of logs/tests.
- `ARCHITECTURE.md` does not need a contract change unless implementation discovers a new review-blocking invariant. `docs/architecture/providers.md` must be updated because this subsystem is being touched and currently omits the legacy type repair.

## File Structure

- Modify `crates/right-openshell/src/providers.rs`: add legacy generic type repair helper, extend `ReconcileReport`, call `update_provider` before attach.
- Modify `crates/right-openshell/src/providers_tests.rs`: add regression tests for legacy `type = "generic"` repair and update failure behavior.
- Modify `crates/right-openshell/src/diagnosis.rs`: add provider composition/reconcile cause and user-facing diagnosis copy.
- Modify `crates/right-openshell/src/diagnosis_tests.rs`: pin provider-specific diagnosis copy and prevent the old "starting up" wording.
- Modify `crates/bot/src/sandbox_supervisor.rs`: map provider reconcile/composition failure to the new diagnosis.
- Modify `crates/bot/src/sandbox_supervisor_phase_tests.rs`: test the bot helper that creates the provider-specific diagnosis.
- Modify `docs/architecture/providers.md`: describe the startup/hot-reconcile self-heal step and clarify gateway provider type for generic providers.

---

### Task 0: Baseline Targeted Verification

**Files:**
- Read-only check: `crates/right-openshell/src/providers_tests.rs`
- Read-only check: `crates/bot/src/sandbox_supervisor_phase_tests.rs`

- [ ] **Step 1: Run the current provider reconcile test slice**

Run:

```bash
devenv shell -- cargo test -p right-openshell reconcile_skips_v2_when_nothing_declared
```

Expected: PASS.

- [ ] **Step 2: Run the current sandbox supervisor diagnosis test slice**

Run:

```bash
devenv shell -- cargo test -p right-bot sandbox_supervisor_phase_tests
```

Expected: PASS.

- [ ] **Step 3: Commit nothing**

Run:

```bash
git status --short
```

Expected: only pre-existing unrelated untracked files may appear. Do not commit baseline output.

---

### Task 1: Add Failing Provider Reconcile Regression Tests

**Files:**
- Modify: `crates/right-openshell/src/providers_tests.rs`

- [ ] **Step 1: Add the legacy repair tests**

In `crates/right-openshell/src/providers_tests.rs`, append these tests after `reconcile_skips_v2_when_nothing_declared` and before `get_sandbox_provider_environment_returns_map`:

```rust
#[tokio::test]
async fn reconcile_repairs_legacy_generic_provider_type_before_attaching() {
    let seen_update: Arc<Mutex<Option<proto_v1::UpdateProviderRequest>>> =
        Arc::new(Mutex::new(None));
    let seen_update_clone = Arc::clone(&seen_update);
    let seen_attach: Arc<Mutex<Option<proto_v1::AttachSandboxProviderRequest>>> =
        Arc::new(Mutex::new(None));
    let seen_attach_clone = Arc::clone(&seen_attach);

    let expected_type = crate::managed_profiles::generic_provider_profile_id("agent-acme");
    let expected_type_for_update = expected_type.clone();

    let mock = MockOpenShell {
        mock_update_config: Some(Box::new(|_| {
            Ok(proto_v1::UpdateConfigResponse {
                version: 0,
                policy_hash: String::new(),
                settings_revision: 1,
                deleted: false,
            })
        })),
        mock_list_sandbox_providers: Some(Box::new(|_| {
            Ok(proto_v1::ListSandboxProvidersResponse {
                providers: Vec::new(),
            })
        })),
        mock_get_provider: Some(Box::new(|req| {
            assert_eq!(req.name, "agent-acme");
            let mut legacy_config = HashMap::new();
            legacy_config.insert("upstream_host".into(), "api.acme.test".into());
            Ok(proto_v1::ProviderResponse {
                provider: Some(datamodel::Provider {
                    metadata: Some(datamodel::ObjectMeta {
                        name: req.name,
                        ..Default::default()
                    }),
                    r#type: "generic".into(),
                    config: legacy_config,
                    credentials: HashMap::new(),
                    credential_expires_at_ms: HashMap::new(),
                }),
            })
        })),
        mock_update_provider: Some(Box::new(move |req| {
            let provider = req.provider.clone().expect("provider update payload");
            assert_eq!(provider.r#type, expected_type_for_update);
            *seen_update_clone.lock().unwrap() = Some(req);
            Ok(proto_v1::ProviderResponse {
                provider: Some(provider),
            })
        })),
        mock_attach_sandbox_provider: Some(Box::new(move |req| {
            *seen_attach_clone.lock().unwrap() = Some(req);
            Ok(proto_v1::AttachSandboxProviderResponse {
                sandbox: None,
                attached: true,
            })
        })),
        ..Default::default()
    };
    let (addr, _shutdown) = start_mock_server(mock).await;
    let mut client = mock_client(addr).await;

    let report = reconcile_for_sandbox(&mut client, "sbx", "agent", &["agent-acme".to_string()])
        .await
        .unwrap();

    assert_eq!(report.repaired, vec!["agent-acme".to_string()]);
    assert_eq!(report.attached, vec!["agent-acme".to_string()]);
    assert!(report.detached.is_empty());
    assert!(report.missing.is_empty());
    assert!(report.errors.is_empty());

    let update_req = seen_update
        .lock()
        .unwrap()
        .clone()
        .expect("legacy generic provider must be updated");
    let updated_provider = update_req.provider.expect("provider update payload");
    assert_eq!(
        updated_provider.metadata.unwrap().name,
        "agent-acme",
        "repair must update the existing gateway provider, not create a new one"
    );
    assert_eq!(updated_provider.r#type, expected_type);
    assert!(
        updated_provider.credentials.is_empty(),
        "repair must preserve existing gateway credential bytes via sparse credential update"
    );
    assert!(
        updated_provider.config.is_empty(),
        "new generic provider shape keeps upstream config in the authored profile, not Provider.config"
    );

    let attach_req = seen_attach
        .lock()
        .unwrap()
        .clone()
        .expect("repaired provider must still be attached");
    assert_eq!(attach_req.sandbox_name, "sbx");
    assert_eq!(attach_req.provider_name, "agent-acme");
}

#[tokio::test]
async fn reconcile_reports_legacy_generic_repair_errors_and_skips_attach() {
    let attach_calls = Arc::new(Mutex::new(0usize));
    let attach_calls_clone = Arc::clone(&attach_calls);

    let mock = MockOpenShell {
        mock_update_config: Some(Box::new(|_| {
            Ok(proto_v1::UpdateConfigResponse {
                version: 0,
                policy_hash: String::new(),
                settings_revision: 1,
                deleted: false,
            })
        })),
        mock_list_sandbox_providers: Some(Box::new(|_| {
            Ok(proto_v1::ListSandboxProvidersResponse {
                providers: Vec::new(),
            })
        })),
        mock_get_provider: Some(Box::new(|req| {
            Ok(proto_v1::ProviderResponse {
                provider: Some(datamodel::Provider {
                    metadata: Some(datamodel::ObjectMeta {
                        name: req.name,
                        ..Default::default()
                    }),
                    r#type: "generic".into(),
                    config: HashMap::new(),
                    credentials: HashMap::new(),
                    credential_expires_at_ms: HashMap::new(),
                }),
            })
        })),
        mock_update_provider: Some(Box::new(|_| Err(tonic::Status::internal("repair boom")))),
        mock_attach_sandbox_provider: Some(Box::new(move |_| {
            *attach_calls_clone.lock().unwrap() += 1;
            Ok(proto_v1::AttachSandboxProviderResponse {
                sandbox: None,
                attached: true,
            })
        })),
        ..Default::default()
    };
    let (addr, _shutdown) = start_mock_server(mock).await;
    let mut client = mock_client(addr).await;

    let report = reconcile_for_sandbox(&mut client, "sbx", "agent", &["agent-acme".to_string()])
        .await
        .unwrap();

    assert!(report.repaired.is_empty());
    assert!(report.attached.is_empty());
    assert!(report.detached.is_empty());
    assert!(report.missing.is_empty());
    assert_eq!(*attach_calls.lock().unwrap(), 0);
    assert_eq!(report.errors.len(), 1);
    assert_eq!(report.errors[0].0, "agent-acme");
    assert!(
        report.errors[0].1.contains("update:"),
        "repair failure must be classified as an update error: {:?}",
        report.errors
    );
    assert!(
        report.errors[0].1.contains("repair boom"),
        "repair failure detail must be preserved for logs: {:?}",
        report.errors
    );
}
```

- [ ] **Step 2: Run the new tests and verify red**

Run:

```bash
devenv shell -- cargo test -p right-openshell legacy_generic
```

Expected: FAIL. Accept either a compile failure about `ReconcileReport` lacking `repaired` or assertion failure because `UpdateProvider` is not called. Do not change production code before seeing this fail.

- [ ] **Step 3: Commit the failing tests only**

Run:

```bash
git add crates/right-openshell/src/providers_tests.rs
git commit -m "test(providers): cover legacy generic provider type repair"
```

Expected: commit succeeds with only `crates/right-openshell/src/providers_tests.rs`.

---

### Task 2: Implement Legacy Generic Provider Type Repair

**Files:**
- Modify: `crates/right-openshell/src/providers.rs`

- [ ] **Step 1: Extend `ReconcileReport`**

In `crates/right-openshell/src/providers.rs`, update `ReconcileReport` to include repaired providers:

```rust
/// Report from a provider reconcile pass.
pub struct ReconcileReport {
    /// Providers that were attached during this pass (were missing from sandbox).
    pub attached: Vec<String>,
    /// Providers that were detached during this pass (were attached but not declared).
    pub detached: Vec<String>,
    /// Declared existing providers whose legacy gateway type was repaired.
    pub repaired: Vec<String>,
    /// Declared providers that do not exist on the gateway (not yet created).
    pub missing: Vec<String>,
    /// Per-provider errors encountered during attach/detach/update. Each entry is
    /// `(provider_name, formatted_error)`. Reconcile continues past these so
    /// a single transient failure does not sink the whole pass.
    pub errors: Vec<(String, String)>,
}
```

- [ ] **Step 2: Add the repair helper**

In `crates/right-openshell/src/providers.rs`, insert this helper above `reconcile_for_sandbox`:

```rust
fn legacy_generic_provider_repair_spec(provider: &Provider) -> Option<ProviderSpec> {
    if provider.type_ != "generic" {
        return None;
    }

    Some(ProviderSpec {
        name: provider.name.clone(),
        type_: crate::managed_profiles::generic_provider_profile_id(&provider.name),
        credentials: HashMap::new(),
        config: HashMap::new(),
    })
}
```

- [ ] **Step 3: Initialize the new report field**

In `crates/right-openshell/src/providers.rs`, update the `ReconcileReport` initialization inside `reconcile_for_sandbox`:

```rust
let mut report = ReconcileReport {
    attached: vec![],
    detached: vec![],
    repaired: vec![],
    missing: vec![],
    errors: vec![],
};
```

- [ ] **Step 4: Repair before attach**

In `crates/right-openshell/src/providers.rs`, replace the current `Ok(_) => { ... }` arm in the declared-provider loop with:

```rust
Ok(provider) => {
    if let Some(repair_spec) = legacy_generic_provider_repair_spec(&provider) {
        match update_provider(client, &repair_spec).await {
            Ok(_) => report.repaired.push(name.clone()),
            Err(e) => {
                report
                    .errors
                    .push((name.clone(), format!("update: {e:#}")));
                continue;
            }
        }
    }

    if !attached_set.contains(name) {
        match attach_to_sandbox(client, sandbox_name, name).await {
            Ok(()) => report.attached.push(name.clone()),
            Err(e) => report.errors.push((name.clone(), format!("attach: {e:#}"))),
        }
    }
}
```

- [ ] **Step 5: Run the provider tests and verify green**

Run:

```bash
devenv shell -- cargo test -p right-openshell legacy_generic
```

Expected: PASS.

- [ ] **Step 6: Run the nearby reconcile tests**

Run:

```bash
devenv shell -- cargo test -p right-openshell reconcile_
```

Expected: PASS for all provider reconcile tests.

- [ ] **Step 7: Commit the implementation**

Run:

```bash
git add crates/right-openshell/src/providers.rs
git commit -m "fix(providers): repair legacy generic provider types"
```

Expected: commit succeeds with only `crates/right-openshell/src/providers.rs`.

---

### Task 3: Add Provider-Specific Gateway Diagnosis

**Files:**
- Modify: `crates/right-openshell/src/diagnosis.rs`
- Modify: `crates/right-openshell/src/diagnosis_tests.rs`

- [ ] **Step 1: Add the failing diagnosis test**

In `crates/right-openshell/src/diagnosis_tests.rs`, append this test after `sandbox_not_ready_diagnosis_names_the_sandbox_and_is_starting_oriented`:

```rust
#[test]
fn provider_composition_diagnosis_names_provider_without_startup_copy() {
    let detail =
        "provider right-typefully attached but not composed into sandbox right-right-1 with endpoint api.typefully.com"
            .to_owned();
    let d = GatewayCause::ProviderComposition {
        sandbox: "right-right-1".to_owned(),
        detail: detail.clone(),
    }
    .diagnose();

    assert_eq!(
        d.cause,
        GatewayCause::ProviderComposition {
            sandbox: "right-right-1".to_owned(),
            detail
        }
    );
    assert!(d.summary.contains("right-right-1"));
    assert!(d.summary.contains("provider access"));
    assert!(d.summary.contains("right-typefully"));
    assert!(
        !d.summary.contains("starting up"),
        "provider composition failure must not reuse sandbox startup copy"
    );
    let fixes = d.fixes.join(" ").to_lowercase();
    assert!(!fixes.contains("docker"));
    assert!(!fixes.contains("gateway start"));
}
```

- [ ] **Step 2: Run the new diagnosis test and verify red**

Run:

```bash
devenv shell -- cargo test -p right-openshell provider_composition_diagnosis_names_provider_without_startup_copy
```

Expected: FAIL with missing `GatewayCause::ProviderComposition`.

- [ ] **Step 3: Add the new cause variant**

In `crates/right-openshell/src/diagnosis.rs`, add this variant after `SandboxNotReady`:

```rust
    /// The sandbox and gateway are reachable, but OpenShell has not composed
    /// one or more attached provider profiles into the sandbox effective policy.
    ProviderComposition {
        sandbox: String,
        detail: String,
    },
```

- [ ] **Step 4: Add base diagnosis copy**

In `crates/right-openshell/src/diagnosis.rs`, add this match arm in `GatewayCause::diagnose` before `GatewayCause::Unreachable`:

```rust
            GatewayCause::ProviderComposition { .. } => (
                "provider access is not loaded into my sandbox",
                vec![
                    "I'll retry provider reconciliation automatically after OpenShell reloads provider composition.",
                ],
            ),
```

- [ ] **Step 5: Add enriched summary copy**

In `crates/right-openshell/src/diagnosis.rs`, add this arm in the summary enrichment `match &self` before `_ => summary.to_owned()`:

```rust
            GatewayCause::ProviderComposition { sandbox, detail } => {
                format!("provider access for sandbox '{sandbox}' is not loaded: {detail}")
            }
```

- [ ] **Step 6: Run the diagnosis tests and verify green**

Run:

```bash
devenv shell -- cargo test -p right-openshell diagnosis_tests
```

Expected: PASS.

- [ ] **Step 7: Commit the diagnosis change**

Run:

```bash
git add crates/right-openshell/src/diagnosis.rs crates/right-openshell/src/diagnosis_tests.rs
git commit -m "fix(openshell): diagnose provider composition failures"
```

Expected: commit succeeds with only the two diagnosis files.

---

### Task 4: Wire Bot Bring-Up To Provider Diagnosis

**Files:**
- Modify: `crates/bot/src/sandbox_supervisor.rs`
- Modify: `crates/bot/src/sandbox_supervisor_phase_tests.rs`

- [ ] **Step 1: Add the failing bot helper test**

In `crates/bot/src/sandbox_supervisor_phase_tests.rs`, append this module after `mod bring_up_phase_diagnosis`:

```rust
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
```

- [ ] **Step 2: Run the bot helper test and verify red**

Run:

```bash
devenv shell -- cargo test -p right-bot provider_reconcile_diagnosis
```

Expected: FAIL with missing `provider_reconcile_diagnosis`.

- [ ] **Step 3: Add the helper**

In `crates/bot/src/sandbox_supervisor.rs`, add this function after `bring_up_phase_diagnosis`:

```rust
fn provider_reconcile_diagnosis(sandbox: &str, detail: String) -> GatewayDiagnosis {
    GatewayCause::ProviderComposition {
        sandbox: sandbox.to_owned(),
        detail,
    }
    .diagnose()
}
```

- [ ] **Step 4: Replace the misleading `SandboxNotReady` mapping**

In `crates/bot/src/sandbox_supervisor.rs`, replace the provider reconcile failure return inside `bring_up_sandbox`:

```rust
return Ok(Err(GatewayCause::SandboxNotReady {
    sandbox: sandbox.clone(),
}
.diagnose()));
```

with:

```rust
return Ok(Err(provider_reconcile_diagnosis(&sandbox, format!("{e:#}"))));
```

- [ ] **Step 5: Include repaired providers in bot reconcile logs**

In `crates/bot/src/sandbox_supervisor.rs`, update both reconcile log calls so they include the new report field.

For startup reconcile, use:

```rust
tracing::info!(
    agent = %agent,
    attached = ?report.attached,
    detached = ?report.detached,
    repaired = ?report.repaired,
    missing = ?report.missing,
    "provider reconcile complete"
);
```

For hot-reconcile, use:

```rust
tracing::info!(
    agent = %agent,
    attached = ?report.attached,
    detached = ?report.detached,
    repaired = ?report.repaired,
    missing = ?report.missing,
    profile_outcomes = ?profile_outcomes,
    "providers hot-reconcile complete"
);
```

- [ ] **Step 6: Run the bot tests and verify green**

Run:

```bash
devenv shell -- cargo test -p right-bot sandbox_supervisor_phase_tests
```

Expected: PASS.

- [ ] **Step 7: Commit the bot wiring**

Run:

```bash
git add crates/bot/src/sandbox_supervisor.rs crates/bot/src/sandbox_supervisor_phase_tests.rs
git commit -m "fix(bot): report provider composition degradation"
```

Expected: commit succeeds with only the two bot files.

---

### Task 5: Update Provider Architecture Doc

**Files:**
- Modify: `docs/architecture/providers.md`

- [ ] **Step 1: Clarify gateway provider type in the overview**

In `docs/architecture/providers.md`, replace this overview sentence:

```markdown
gateway-unique name, a type slug (`anthropic`, `openai`, `github`,
`gitlab`, `nvidia`, `codex`, `copilot`, `opencode`, or `generic`), a
credentials map, and an optional non-secret config map. Right Agent
```

with:

```markdown
gateway-unique name, a gateway type, a credentials map, and an optional
non-secret config map. Built-in providers use upstream type slugs such as
`anthropic`, `openai`, `right-github`, or `gitlab`; generic providers are
displayed as `generic` in Right's dashboard and `agent.yaml`, but the
gateway provider `type` is the Right-authored profile ID
(`right-provider-*`). Right Agent
```

- [ ] **Step 2: Document startup/hot-reconcile repair**

In `docs/architecture/providers.md`, replace the `Reconciler walkthrough` numbered list with:

```markdown
For each entry in `agent.yaml::sandbox::providers`:

1. `GetProvider` against the gateway.
   - **Ok** → continue.
   - **NotFound** → mark the entry as `Status::Missing` (a "ghost"
     provider). Do not auto-heal: Right does not have the credential
     bytes. The dashboard surfaces these with a *Resolve* action.
2. If the provider exists with the legacy generic gateway type
   `type = "generic"`, call `UpdateProvider` with
   `type = right_openshell::managed_profiles::generic_provider_profile_id(name)`,
   empty credentials, and empty config. This preserves the gateway-held
   credential bytes while moving already-deployed agents to the current
   provider-profile shape. Built-in providers and already-migrated generic
   providers are left unchanged.
3. If not currently attached to the sandbox, call
   `Sandbox.provider.attach`.

Then for each provider currently attached to the sandbox whose name
starts with `<agent>-` but is absent from `agent.yaml`: call
`Sandbox.provider.detach`.

The reconciler returns a `ReconcileReport { attached, detached,
repaired, missing, errors }` per agent which is surfaced in logs and to
callers.
```

- [ ] **Step 3: Run a docs diff check**

Run:

```bash
git diff -- docs/architecture/providers.md
```

Expected: the diff only clarifies gateway type and reconcile repair. It must not add a new `ARCHITECTURE.md` import or a long mechanism walkthrough to `ARCHITECTURE.md`.

- [ ] **Step 4: Commit the docs update**

Run:

```bash
git add docs/architecture/providers.md
git commit -m "docs(providers): describe legacy generic type repair"
```

Expected: commit succeeds with only `docs/architecture/providers.md`.

---

### Task 6: Final Verification And Live Read-Only Sanity Check

**Files:**
- Verification only.

- [ ] **Step 1: Run targeted package tests**

Run:

```bash
devenv shell -- cargo test -p right-openshell providers_tests
devenv shell -- cargo test -p right-openshell diagnosis_tests
```

Expected: PASS.

- [ ] **Step 2: Run targeted bot tests**

Run:

```bash
devenv shell -- cargo test -p right-bot sandbox_supervisor_phase_tests
```

Expected: PASS.

- [ ] **Step 3: Run the mandatory full workspace test suite**

Run:

```bash
devenv shell -- cargo test --workspace
```

Expected: PASS. If pre-existing failures appear, capture the failing test names and confirm they are unrelated before stopping.

- [ ] **Step 4: Build the workspace in debug mode**

Run:

```bash
devenv shell -- cargo build --workspace
```

Expected: PASS. This satisfies the project Rust final build requirement.

- [ ] **Step 5: Inspect live Right provider state without mutating it**

Run:

```bash
devenv shell -- openshell provider list
```

Expected before deploying/running the new binary on the live bot: `right-typefully` and `right-twitterapi` may still show legacy `generic`. Do not manually update them.

- [ ] **Step 6: Verify the built binary can report status**

Run:

```bash
devenv shell -- cargo run --bin right -- status
```

Expected: process-compose status prints without using a stale installed `right` binary.

- [ ] **Step 7: Commit any final verification-only doc adjustment**

Run:

```bash
git status --short
```

Expected: clean except pre-existing unrelated untracked files. If only this plan file remains uncommitted because earlier task commits were made before the plan existed, commit it:

```bash
git add docs/superpowers/plans/2026-06-08-provider-legacy-type-self-heal.md
git commit -m "docs(superpowers): plan provider legacy type self-heal"
```

---

## Self-Review

- Spec coverage: legacy generic `type = "generic"` provider records are repaired in the shared reconcile path; startup and hot-reconcile both benefit; missing providers still remain ghost rows; provider composition failure copy no longer says the sandbox is starting; docs explain the upgrade path.
- Placeholder scan: no deferred implementation text remains in this plan; every code-changing step includes concrete Rust or Markdown content.
- Type consistency: `ReconcileReport::repaired`, `GatewayCause::ProviderComposition`, and `provider_reconcile_diagnosis` are named consistently across tests, implementation, logging, and docs.
