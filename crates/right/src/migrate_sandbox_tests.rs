//! Pure-part tests for `right agent migrate-sandbox`: what the command decides
//! and writes before it ever touches a sandbox.

use std::collections::HashSet;

use right_agent::sandbox_migrate::{entry_name, missing_entries};

use super::{
    MIGRATION_EXCLUDES, MigrationSource, SeededProviders, SourcePlan, carried_entries,
    migration_recap, migration_source, plan_migration, rewrite_agent_yaml_for_migration,
    seed_provider_records,
};

const OPENSHELL_YAML: &str = "\
network_policy: permissive
# keep this comment
sandbox:
  mode: openshell
  policy_file: policy.yaml
  name: right-finance
memory:
  provider: file
";

#[test]
fn openshell_agent_is_a_migration_source() {
    assert_eq!(
        migration_source(OPENSHELL_YAML).expect("parses"),
        MigrationSource::OpenShell {
            explicit_name: Some("right-finance".to_owned())
        }
    );
}

#[test]
fn migrated_agent_is_a_no_op() {
    let yaml = "name: finance\nsandbox:\n  name: right-finance\n";
    assert_eq!(
        migration_source(yaml).expect("parses"),
        MigrationSource::AlreadyMigrated
    );
    assert_eq!(
        migration_source("name: finance\n").expect("parses"),
        MigrationSource::AlreadyMigrated
    );
}

#[test]
fn sandboxless_and_unknown_modes_are_rejected() {
    for mode in ["none", "chroot"] {
        let yaml = format!("sandbox:\n  mode: {mode}\n");
        assert!(
            migration_source(&yaml).is_err(),
            "mode {mode} must not be treated as an OpenShell source"
        );
    }
}

#[test]
fn rewrite_drops_retired_keys_and_sets_the_new_name() {
    let rewritten = rewrite_agent_yaml_for_migration(OPENSHELL_YAML, "right-finance-msb");

    assert!(!rewritten.contains("mode:"), "mode: survived: {rewritten}");
    assert!(
        !rewritten.contains("policy_file:"),
        "policy_file: survived: {rewritten}"
    );
    assert!(rewritten.contains("  name: \"right-finance-msb\""));
    // Untouched lines keep their content and order.
    assert!(rewritten.contains("# keep this comment"));
    assert!(rewritten.starts_with("network_policy: permissive\n"));
    assert!(rewritten.ends_with("memory:\n  provider: file\n"));
}

#[test]
fn rewrite_preserves_other_sandbox_keys() {
    let yaml = "\
sandbox:
  mode: openshell
  providers:
    - name: finance-github
      type: github
";
    let rewritten = rewrite_agent_yaml_for_migration(yaml, "right-finance");
    assert!(rewritten.contains("  providers:"));
    assert!(rewritten.contains("    - name: finance-github"));
    assert!(!rewritten.contains("mode:"));
}

#[test]
fn rewrite_is_idempotent() {
    let once = rewrite_agent_yaml_for_migration(OPENSHELL_YAML, "right-finance-msb");
    let twice = rewrite_agent_yaml_for_migration(&once, "right-finance-msb");
    assert_eq!(once, twice);
}

#[test]
fn rewrite_adds_a_sandbox_block_when_there_is_none() {
    let rewritten = rewrite_agent_yaml_for_migration("name: finance\n", "right-finance");
    assert!(rewritten.ends_with("sandbox:\n  name: \"right-finance\"\n"));
}

#[test]
fn plan_resolves_both_names_and_validates_the_rewrite() {
    let plan = plan_migration("finance", OPENSHELL_YAML)
        .expect("plans")
        .expect("openshell agent is migratable");
    assert_eq!(plan.old_name, "right-finance");
    assert_eq!(plan.new_name, right_sandbox::sandbox_name("finance"));
    // The rewritten config is what the new sandbox spec is built from, so it
    // must parse through the real (openshell-rejecting) parser.
    assert_eq!(
        plan.migrated_config
            .sandbox
            .as_ref()
            .and_then(|s| s.name.as_deref()),
        Some(plan.new_name.as_str())
    );
    assert!(
        plan_migration("finance", "name: finance\n")
            .expect("plans")
            .is_none()
    );
}

#[test]
fn carried_entries_drop_only_excluded_top_level_names() {
    let listing = ".cache\n.claude\n.npm\n.platform\nCLAUDE.md\n.ssh\nprojects\n";
    assert_eq!(
        carried_entries(listing, MIGRATION_EXCLUDES),
        vec![".claude", ".platform", "CLAUDE.md", "projects"]
    );
}

#[test]
fn nested_excludes_keep_their_parent_entry() {
    let listing = ".claude\n";
    assert_eq!(
        carried_entries(listing, &[".claude/settings.json"]),
        vec![".claude"]
    );
}

#[test]
fn entry_names_survive_both_listing_shapes() {
    assert_eq!(entry_name("/sandbox/.claude"), ".claude");
    assert_eq!(entry_name(".claude"), ".claude");
    assert_eq!(entry_name("/sandbox/projects/"), "projects");
}

#[test]
fn verification_reports_every_entry_that_did_not_arrive() {
    let expected = vec![
        ".claude".to_owned(),
        "CLAUDE.md".to_owned(),
        "projects".to_owned(),
    ];
    let present: HashSet<&str> = [".claude"].into_iter().collect();
    assert_eq!(
        missing_entries(&expected, &present),
        vec!["CLAUDE.md", "projects"]
    );
    let all: HashSet<&str> = [".claude", "CLAUDE.md", "projects"].into_iter().collect();
    assert!(missing_entries(&expected, &all).is_empty());
}

/// Build a plan good enough to render a recap; only the names are read.
fn recap_plan() -> SourcePlan {
    let yaml = "sandbox:\n  name: test-sandbox\n";
    SourcePlan {
        old_name: "test-sandbox-20260516-1649".to_owned(),
        new_name: "test-sandbox".to_owned(),
        migrated_yaml: yaml.to_owned(),
        migrated_config: serde_saphyr::from_str(yaml).expect("fixture parses"),
    }
}

/// The whole reason a failed delete may exit 0 is that the recap tells the
/// truth about it. A confirmed delete is the only path that may say "deleted".
#[test]
fn recap_claims_the_old_sandbox_is_deleted_only_when_it_was() {
    let plan = recap_plan();
    let rendered = migration_recap(
        &plan,
        std::path::Path::new("/tmp/sandbox.tar.gz"),
        true,
        &SeededProviders::default(),
        None,
    )
    .render(right_ui::Theme::Mono);
    assert!(
        rendered.contains("deleted"),
        "a confirmed delete must be reported: {rendered}"
    );
}

#[test]
fn recap_never_claims_a_deletion_that_failed() {
    let plan = recap_plan();
    let rendered = migration_recap(
        &plan,
        std::path::Path::new("/tmp/sandbox.tar.gz"),
        true,
        &SeededProviders::default(),
        Some(miette::miette!("gateway said no")),
    )
    .render(right_ui::Theme::Mono);
    assert!(
        !rendered.contains(&format!("{} deleted", plan.old_name)),
        "a failed delete must never be reported as done: {rendered}"
    );
    assert!(
        rendered.contains("could not be deleted"),
        "the failure must be surfaced: {rendered}"
    );
    assert!(
        rendered.contains(&format!("openshell sandbox delete {}", plan.old_name)),
        "the operator needs the exact manual command: {rendered}"
    );
}

/// Providers are the only credential signal the migration can hand back, so
/// every one awaiting a value must reach the recap with the dashboard next
/// step.
#[test]
fn recap_names_providers_that_need_their_credentials_re_entered() {
    let plan = recap_plan();
    let rendered = migration_recap(
        &plan,
        std::path::Path::new("/tmp/sandbox.tar.gz"),
        true,
        &SeededProviders {
            needs_credential: vec!["agent-a-provider".to_owned(), "agent-a-openai".to_owned()],
            ready: vec!["agent-a-shared".to_owned()],
            undeclared: vec!["agent-a-ghost".to_owned()],
        },
        None,
    )
    .render(right_ui::Theme::Mono);
    for name in ["agent-a-provider", "agent-a-openai"] {
        assert!(
            rendered.contains(name),
            "every provider awaiting a credential must be named: {rendered}"
        );
    }
    assert!(
        rendered.contains("agent-a-ghost"),
        "a provider the yaml never declared cannot be recorded, so it must be reported: {rendered}"
    );
    assert!(
        rendered.contains("/providers"),
        "the operator needs the dashboard step: {rendered}"
    );
    assert!(
        rendered.contains("agent-a-shared"),
        "a provider that already holds a credential must not be listed as needing one: {rendered}"
    );
}

/// A fresh store on a temp home, plus the directory that must outlive it.
async fn store() -> (tempfile::TempDir, right_providers::ProviderStore) {
    let home = tempfile::TempDir::new().expect("temp home");
    let store = right_providers::ProviderStore::open(home.path())
        .await
        .expect("open providers.db");
    (home, store)
}

const PROVIDER_YAML: &str = "\
sandbox:
  name: test-sandbox-a
  providers:
    - name: agent-a-provider
      type: right-fal
      label: prod
    - name: agent-a-acme
      type: generic
      generic:
        env_var: ACME_KEY
        upstream_hosts:
          - api.acme.com
        upstream_path_prefix: /v1
";

fn provider_config() -> right_agent::agent::types::AgentConfig {
    serde_saphyr::from_str(PROVIDER_YAML).expect("fixture parses")
}

/// The whole point of seeding: a migrated provider exists in `providers.db`
/// with its definition and no value, which is what the bot's bring-up skips
/// and the dashboard renders as awaiting a credential.
#[tokio::test]
async fn seeding_lands_every_declared_provider_as_needs_value() {
    let (_home, store) = store().await;
    let config = provider_config();

    let seeded = seed_provider_records(&store, "agent-a", config.providers(), &[])
        .await
        .expect("seeding a fresh store");

    assert_eq!(
        seeded.needs_credential,
        vec!["agent-a-provider".to_owned(), "agent-a-acme".to_owned()]
    );
    assert!(seeded.ready.is_empty());

    let builtin = store
        .get("agent-a", "agent-a-provider")
        .await
        .expect("built-in record was written");
    assert_eq!(builtin.status, right_providers::ProviderStatus::NeedsValue);
    assert_eq!(builtin.env_var, "FAL_KEY");
    assert_eq!(builtin.label, "prod");

    let generic = store
        .get("agent-a", "agent-a-acme")
        .await
        .expect("generic record was written");
    assert_eq!(generic.status, right_providers::ProviderStatus::NeedsValue);
    assert_eq!(generic.env_var, "ACME_KEY");
    assert_eq!(
        generic.kind,
        right_providers::ProviderKind::Generic(right_providers::GenericSpec {
            env_var: "ACME_KEY".to_owned(),
            upstream_hosts: vec!["api.acme.com".to_owned()],
            upstream_path_prefix: Some("/v1".to_owned()),
        }),
        "the generic endpoints must survive the migration: they are unrecoverable otherwise"
    );
}

/// A rolled-back migration is re-run from the same state, so seeding must not
/// duplicate, clobber, or downgrade a record the operator has since filled in.
#[tokio::test]
async fn seeding_leaves_an_existing_record_alone() {
    let (_home, store) = store().await;
    let config = provider_config();
    seed_provider_records(&store, "agent-a", config.providers(), &[])
        .await
        .expect("first run");
    store
        .rotate(
            "agent-a",
            "agent-a-provider",
            right_providers::Credential::from("re-entered".to_owned()),
        )
        .await
        .expect("operator adds the credential from the dashboard");

    let seeded = seed_provider_records(&store, "agent-a", config.providers(), &[])
        .await
        .expect("re-run");

    assert_eq!(seeded.ready, vec!["agent-a-provider".to_owned()]);
    assert_eq!(seeded.needs_credential, vec!["agent-a-acme".to_owned()]);
    assert_eq!(
        store.list("agent-a").await.expect("list").len(),
        2,
        "a re-run must not duplicate records"
    );
}

/// The OpenShell sandbox's own view is the cross-check: a provider it had
/// attached that `agent.yaml` never declared has no definition to seed from,
/// so it must be reported rather than silently dropped.
#[tokio::test]
async fn attached_providers_the_yaml_never_declared_are_reported() {
    let (_home, store) = store().await;
    let config = provider_config();
    let attached = vec!["agent-a-provider".to_owned(), "agent-a-ghost".to_owned()];

    let seeded = seed_provider_records(&store, "agent-a", config.providers(), &attached)
        .await
        .expect("seeding");

    assert_eq!(seeded.undeclared, vec!["agent-a-ghost".to_owned()]);
}

/// A definition the store cannot accept would leave the agent unstartable
/// after the migration, so it aborts while everything is still untouched.
#[tokio::test]
async fn a_provider_definition_the_store_rejects_aborts_the_migration() {
    let (_home, store) = store().await;
    let config: right_agent::agent::types::AgentConfig = serde_saphyr::from_str(
        "sandbox:\n  name: test-sandbox-a\n  providers:\n    - name: agent-a-gone\n      type: no-such-provider\n",
    )
    .expect("fixture parses");

    let error = seed_provider_records(&store, "agent-a", config.providers(), &[])
        .await
        .expect_err("an unknown provider type cannot be recorded");

    assert!(
        format!("{error:#}").contains("agent-a-gone"),
        "the failure must name the provider: {error:#}"
    );
    assert!(
        store.list("agent-a").await.expect("list").is_empty(),
        "a rejected definition must leave the store as it was"
    );
}
