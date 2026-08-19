//! Pure-part tests for `right agent migrate-sandbox`: what the command decides
//! and writes before it ever touches a sandbox.

use std::collections::HashSet;

use right_agent::sandbox_migrate::{entry_name, missing_entries};

use super::{
    MIGRATION_EXCLUDES, MigrationSource, SourcePlan, carried_entries, migration_recap,
    migration_source, plan_migration, rewrite_agent_yaml_for_migration,
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
        &[],
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
        &[],
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

/// Providers are the only credential signal the migration can hand back, so a
/// non-empty list must always reach the recap with the dashboard next step.
#[test]
fn recap_names_providers_that_need_their_credentials_re_entered() {
    let plan = recap_plan();
    let rendered = migration_recap(
        &plan,
        std::path::Path::new("/tmp/sandbox.tar.gz"),
        true,
        &["agent-a-provider".to_owned()],
        None,
    )
    .render(right_ui::Theme::Mono);
    assert!(
        rendered.contains("agent-a-provider"),
        "the provider must be named: {rendered}"
    );
    assert!(
        rendered.contains("/providers"),
        "the operator needs the dashboard step: {rendered}"
    );
}
