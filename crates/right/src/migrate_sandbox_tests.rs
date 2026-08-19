//! Pure-part tests for `right agent migrate-sandbox`: what the command decides
//! and writes before it ever touches a sandbox.

use std::collections::HashSet;

use right_agent::sandbox_migrate::{entry_name, missing_entries};

use super::{
    MIGRATION_EXCLUDES, MigrationSource, carried_entries, migration_source, plan_migration,
    rewrite_agent_yaml_for_migration,
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
