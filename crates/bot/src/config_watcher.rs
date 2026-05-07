//! Watch agent.yaml for changes. Model-only changes are hot-reloaded
//! into the in-memory ArcSwap cell; any other change triggers graceful
//! restart.
//!
//! Uses `notify` with debouncing (2s) to avoid reacting to partial writes.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use arc_swap::ArcSwap;
use right_agent::agent::types::AgentConfig;
use tokio_util::sync::CancellationToken;

/// Debounce window for filesystem events — long enough to coalesce editor
/// save bursts (write + rename + chmod), short enough that user-visible
/// hot-reload feels immediate.
const DEBOUNCE: std::time::Duration = std::time::Duration::from_secs(2);

/// Classification of a single agent.yaml change event.
#[derive(Debug)]
pub(crate) enum ChangeKind {
    /// File contents bytewise unchanged — fs noise (mtime touch, atomic
    /// rename, etc.). Skip silently.
    NoChange,
    /// Only `model` changed — apply in-memory and continue running.
    HotReloadable { new_model: Option<String> },
    /// Anything else — graceful restart.
    RestartRequired,
}

/// Decide whether a change can be hot-reloaded or requires a restart.
///
/// Compares old + new yaml as parsed `AgentConfig` values with `model`
/// nulled out on both sides. If the rest is equal, hot-reload; else
/// restart. Parse failure on either side fails-safe to restart.
/// `AgentConfig` derives `PartialEq` field-by-field, so HashMap-typed
/// fields like `env` compare order-insensitively.
pub(crate) fn diff_classify(old_yaml: &str, new_yaml: &str) -> ChangeKind {
    if old_yaml == new_yaml {
        return ChangeKind::NoChange;
    }
    let mut old: AgentConfig = match serde_saphyr::from_str(old_yaml) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(
                error = %format!("{e:#}"),
                "config_watcher: failed to parse old agent.yaml — restart required"
            );
            return ChangeKind::RestartRequired;
        }
    };
    let mut new: AgentConfig = match serde_saphyr::from_str(new_yaml) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(
                error = %format!("{e:#}"),
                "config_watcher: failed to parse new agent.yaml — restart required"
            );
            return ChangeKind::RestartRequired;
        }
    };
    let new_model = new.model.take();
    old.model = None;
    if old == new {
        ChangeKind::HotReloadable { new_model }
    } else {
        ChangeKind::RestartRequired
    }
}

/// Spawn a blocking thread that watches `agent.yaml` for modifications.
///
/// On change:
/// - `HotReloadable` → store new model into `model_swap`, log info, do not cancel.
/// - `RestartRequired` → set `config_changed`, cancel `token` (existing path).
pub(crate) fn spawn_config_watcher(
    agent_yaml: &Path,
    token: CancellationToken,
    config_changed: Arc<AtomicBool>,
    model_swap: Arc<ArcSwap<Option<String>>>,
) -> miette::Result<()> {
    use notify_debouncer_mini::{DebouncedEventKind, new_debouncer};
    use std::sync::mpsc;

    let watch_dir = agent_yaml
        .parent()
        .ok_or_else(|| miette::miette!("agent.yaml has no parent directory"))?
        .to_path_buf();
    let yaml_filename = agent_yaml
        .file_name()
        .ok_or_else(|| miette::miette!("agent.yaml has no filename"))?
        .to_os_string();
    let yaml_path: PathBuf = agent_yaml.to_path_buf();

    let initial_yaml = std::fs::read_to_string(&yaml_path)
        .map_err(|e| miette::miette!("failed to read {} for watcher: {e:#}", yaml_path.display()))?;

    let (tx, rx) = mpsc::channel();

    let mut debouncer = new_debouncer(DEBOUNCE, tx)
        .map_err(|e| miette::miette!("failed to create file watcher: {e:#}"))?;

    debouncer
        .watcher()
        .watch(&watch_dir, notify::RecursiveMode::NonRecursive)
        .map_err(|e| miette::miette!("failed to watch {}: {e:#}", watch_dir.display()))?;

    std::thread::spawn(move || {
        let _debouncer = debouncer;
        let mut last_yaml = initial_yaml;

        for result in rx {
            match result {
                Ok(events) => {
                    let relevant = events.iter().any(|e| {
                        e.kind == DebouncedEventKind::Any
                            && e.path.file_name() == Some(&yaml_filename)
                    });
                    if !relevant {
                        continue;
                    }

                    let new_yaml = match std::fs::read_to_string(&yaml_path) {
                        Ok(s) => s,
                        Err(e) => {
                            tracing::warn!(
                                error = %e,
                                "config_watcher: failed to read {} after change — restart",
                                yaml_path.display()
                            );
                            config_changed.store(true, Ordering::Release);
                            token.cancel();
                            return;
                        }
                    };

                    match diff_classify(&last_yaml, &new_yaml) {
                        ChangeKind::NoChange => {
                            last_yaml = new_yaml;
                        }
                        ChangeKind::HotReloadable { new_model } => {
                            tracing::info!(
                                model = ?new_model.as_deref().unwrap_or("default"),
                                "agent.yaml: model-only change — hot-reloading"
                            );
                            // Two writers exist (this watcher + /model callback); both derive
                            // the value from disk, so last-write-wins converges race-free.
                            model_swap.store(Arc::new(new_model));
                            last_yaml = new_yaml;
                        }
                        ChangeKind::RestartRequired => {
                            tracing::info!(
                                "agent.yaml changed (non-model) — initiating graceful restart"
                            );
                            config_changed.store(true, Ordering::Release);
                            token.cancel();
                            return;
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!("file watcher error: {e:#}");
                }
            }
        }
    });

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn classify(old: &str, new: &str) -> ChangeKind {
        diff_classify(old, new)
    }

    #[test]
    fn diff_model_only_is_hot_reloadable() {
        let old = "restart: never\nmax_restarts: 5\nmodel: \"claude-sonnet-4-6\"\n";
        let new = "restart: never\nmax_restarts: 5\nmodel: \"claude-haiku-4-5\"\n";
        match classify(old, new) {
            ChangeKind::HotReloadable { new_model } => {
                assert_eq!(new_model.as_deref(), Some("claude-haiku-4-5"));
            }
            other => panic!("expected HotReloadable, got {other:?}"),
        }
    }

    #[test]
    fn diff_model_added_is_hot_reloadable() {
        let old = "restart: never\nmax_restarts: 5\n";
        let new = "restart: never\nmax_restarts: 5\nmodel: \"claude-haiku-4-5\"\n";
        match classify(old, new) {
            ChangeKind::HotReloadable { new_model } => {
                assert_eq!(new_model.as_deref(), Some("claude-haiku-4-5"));
            }
            other => panic!("expected HotReloadable, got {other:?}"),
        }
    }

    #[test]
    fn diff_model_removed_is_hot_reloadable() {
        let old = "restart: never\nmax_restarts: 5\nmodel: \"claude-haiku-4-5\"\n";
        let new = "restart: never\nmax_restarts: 5\n";
        match classify(old, new) {
            ChangeKind::HotReloadable { new_model } => {
                assert!(new_model.is_none());
            }
            other => panic!("expected HotReloadable, got {other:?}"),
        }
    }

    #[test]
    fn diff_other_field_changed_is_restart_required() {
        let old = "restart: never\nmax_restarts: 5\nmodel: \"claude-sonnet-4-6\"\n";
        let new = "restart: always\nmax_restarts: 5\nmodel: \"claude-sonnet-4-6\"\n";
        assert!(matches!(classify(old, new), ChangeKind::RestartRequired));
    }

    #[test]
    fn diff_model_and_other_field_is_restart_required() {
        let old = "restart: never\nmodel: \"claude-sonnet-4-6\"\n";
        let new = "restart: always\nmodel: \"claude-haiku-4-5\"\n";
        assert!(matches!(classify(old, new), ChangeKind::RestartRequired));
    }

    #[test]
    fn diff_parse_failure_is_restart_required() {
        let old = "restart: never\n";
        let new = "{ this is not yaml";
        assert!(matches!(classify(old, new), ChangeKind::RestartRequired));
    }

    #[test]
    fn diff_identical_yaml_is_no_change() {
        let yaml = "restart: never\nmodel: \"claude-haiku-4-5\"\n";
        assert!(matches!(classify(yaml, yaml), ChangeKind::NoChange));
    }

    #[test]
    fn agent_config_partial_eq_smoke_test() {
        let a: AgentConfig = serde_saphyr::from_str("restart: never\n").unwrap();
        let b: AgentConfig = serde_saphyr::from_str("restart: never\n").unwrap();
        assert_eq!(a, b);
    }
}
