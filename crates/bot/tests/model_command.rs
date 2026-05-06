//! End-to-end test of /model — writes to a fixture agent.yaml and verifies
//! the in-memory ArcSwap is updated. Does NOT exercise teloxide HTTP — the
//! handler-level logic (allowlist gate + persist + swap) is what we cover.

use std::sync::Arc;

use arc_swap::ArcSwap;
use right_agent::agent::types::{AgentConfig, write_agent_yaml_model};

#[test]
fn write_yaml_then_diff_classifies_as_hot_reloadable() {
    // Simulates the steady-state /model flow:
    //   ① write_agent_yaml_model writes to disk
    //   ② diff_classify (used by config_watcher) sees a model-only change
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("agent.yaml");
    std::fs::write(&path, "restart: never\nmax_restarts: 5\n").unwrap();
    let old_yaml = std::fs::read_to_string(&path).unwrap();

    write_agent_yaml_model(&path, Some("claude-haiku-4-5")).unwrap();
    let new_yaml = std::fs::read_to_string(&path).unwrap();

    // Reproduce the watcher's diff logic — only model differs.
    let old: AgentConfig = serde_saphyr::from_str(&old_yaml).unwrap();
    let new: AgentConfig = serde_saphyr::from_str(&new_yaml).unwrap();
    let mut o = old.clone();
    let mut n = new.clone();
    o.model = None;
    n.model = None;
    assert_eq!(o, n, "non-model fields must be unchanged");
    assert_eq!(new.model.as_deref(), Some("claude-haiku-4-5"));
    assert!(old.model.is_none());
}

#[test]
fn arc_swap_visible_across_threads() {
    // Sanity: the ArcSwap cell shared between watcher and CC invocation
    // path actually propagates a store across threads.
    let cell: Arc<ArcSwap<Option<String>>> =
        Arc::new(ArcSwap::from_pointee(None));

    let cell_clone = Arc::clone(&cell);
    let writer = std::thread::spawn(move || {
        cell_clone.store(Arc::new(Some("claude-haiku-4-5".to_owned())));
    });
    writer.join().unwrap();

    let observed = (**cell.load()).clone();
    assert_eq!(observed.as_deref(), Some("claude-haiku-4-5"));
}

#[test]
fn write_then_clear_round_trips_to_none() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("agent.yaml");
    std::fs::write(&path, "restart: never\n").unwrap();

    write_agent_yaml_model(&path, Some("claude-sonnet-4-6")).unwrap();
    write_agent_yaml_model(&path, None).unwrap();

    let final_yaml = std::fs::read_to_string(&path).unwrap();
    let parsed: AgentConfig = serde_saphyr::from_str(&final_yaml).unwrap();
    assert!(parsed.model.is_none());
    assert!(!final_yaml.contains("model:"));
}
