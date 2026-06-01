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
    /// Only `model`, `debug`, and/or ignored legacy fields changed — apply
    /// in-memory runtime fields and continue running.
    HotReloadable {
        /// `Some(v)` = yaml field present with value `v`. `None` = field absent;
        /// watcher stores `None` into the ArcSwap, meaning "use default model".
        new_model: Option<String>,
        /// `Some(v)` = yaml field present with value `v`. `None` = field absent;
        /// watcher reverts the AtomicBool to its boot-time value (the CLI --debug flag).
        new_debug: Option<bool>,
    },
    /// Anything else — graceful restart.
    RestartRequired,
    /// Only `sandbox.providers` (optionally with model/debug) changed — apply
    /// model/debug in-memory and hot-reconcile providers without a restart.
    /// Carries the freshly parsed config so the reconcile reads new providers.
    ProvidersReload {
        new_model: Option<String>,
        new_debug: Option<bool>,
        new_config: Box<AgentConfig>,
    },
}

/// Decide whether a change can be hot-reloaded or requires a restart.
///
/// Compares old + new yaml as parsed `AgentConfig` values after nulling
/// hot-reloadable and ignored legacy fields on both sides. If the rest is
/// equal, hot-reload; else restart. Parse failure on either side fails-safe
/// to restart.
pub(crate) fn diff_classify(old_yaml: &str, new_yaml: &str) -> ChangeKind {
    if old_yaml == new_yaml {
        return ChangeKind::NoChange;
    }
    let old: AgentConfig = match serde_saphyr::from_str(old_yaml) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(error = %format!("{e:#}"),
                "config_watcher: failed to parse old agent.yaml — restart required");
            return ChangeKind::RestartRequired;
        }
    };
    let new: AgentConfig = match serde_saphyr::from_str(new_yaml) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(error = %format!("{e:#}"),
                "config_watcher: failed to parse new agent.yaml — restart required");
            return ChangeKind::RestartRequired;
        }
    };
    let new_model = new.model.clone();
    let new_debug = new.debug;

    // Stage A: only model/debug/learning differ → in-memory hot reload.
    let mut old_a = old.clone();
    let mut new_a = new.clone();
    normalize_for_reload_diff(&mut old_a);
    normalize_for_reload_diff(&mut new_a);
    if old_a == new_a {
        return ChangeKind::HotReloadable {
            new_model,
            new_debug,
        };
    }

    // Stage B: additionally ignore sandbox.providers → providers hot-reconcile.
    if let Some(s) = old_a.sandbox.as_mut() {
        s.providers.clear();
    }
    if let Some(s) = new_a.sandbox.as_mut() {
        s.providers.clear();
    }
    if old_a == new_a {
        return ChangeKind::ProvidersReload {
            new_model,
            new_debug,
            new_config: Box::new(new),
        };
    }

    ChangeKind::RestartRequired
}

fn normalize_for_reload_diff(config: &mut AgentConfig) {
    config.model = None;
    config.debug = None;
    config.learning.fork_probe_enabled = None;
    config.learning.fork_probe_model = None;
    config.learning.legacy_probe_model = None;
    config.learning.background_review_enabled = None;
    config.learning.episode_selector_model = None;
    config.learning.episode_selector_max_budget_usd = None;
    config.learning.episode_settle_seconds = None;
    config.learning.circuit_failure_threshold = None;
    config.learning.circuit_cooldown_minutes = None;
}

/// Spawn a blocking thread that watches `agent.yaml` for modifications.
///
/// On change:
/// - `HotReloadable` → store new runtime fields, log info, do not cancel.
/// - `RestartRequired` → set `config_changed`, cancel `token` (existing path).
///
/// `initial_debug` is the value of the `--debug` CLI flag at process start.
/// When `debug:` is removed from `agent.yaml`, the watcher reverts to this value.
pub(crate) fn spawn_config_watcher(
    agent_yaml: &Path,
    token: CancellationToken,
    config_changed: Arc<AtomicBool>,
    model_swap: Arc<ArcSwap<Option<String>>>,
    debug_flag: Arc<AtomicBool>,
    initial_debug: bool,
    providers_tx: tokio::sync::mpsc::UnboundedSender<Box<AgentConfig>>,
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

    let initial_yaml = std::fs::read_to_string(&yaml_path).map_err(|e| {
        miette::miette!("failed to read {} for watcher: {e:#}", yaml_path.display())
    })?;

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
                        ChangeKind::HotReloadable {
                            new_model,
                            new_debug,
                        } => {
                            tracing::info!(
                                model = ?new_model.as_deref().unwrap_or("default"),
                                debug = ?new_debug,
                                "agent.yaml: model/debug or ignored legacy change — hot-reloading"
                            );
                            model_swap.store(Arc::new(new_model));
                            // yaml `debug:` present → use that value; absent → revert to boot-time CLI flag.
                            let debug_value = new_debug.unwrap_or(initial_debug);
                            debug_flag.store(debug_value, Ordering::Release);
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
                        ChangeKind::ProvidersReload {
                            new_model,
                            new_debug,
                            new_config,
                        } => {
                            tracing::info!(
                                providers = new_config
                                    .sandbox
                                    .as_ref()
                                    .map(|s| s.providers.len())
                                    .unwrap_or(0),
                                "agent.yaml: providers-only change — hot reconcile without restart"
                            );
                            model_swap.store(Arc::new(new_model));
                            debug_flag.store(new_debug.unwrap_or(initial_debug), Ordering::Release);
                            if let Err(e) = providers_tx.send(new_config) {
                                tracing::warn!(error = %format!("{e}"),
                                    "providers reconcile channel closed (consumer task gone, e.g. shutdown) — skipping hot reconcile");
                            }
                            last_yaml = new_yaml;
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

    #[tokio::test]
    async fn diff_model_only_is_hot_reloadable() {
        let old = "restart: never\nmax_restarts: 5\nmodel: \"claude-sonnet-4-6\"\n";
        let new = "restart: never\nmax_restarts: 5\nmodel: \"claude-haiku-4-5\"\n";
        match classify(old, new) {
            ChangeKind::HotReloadable {
                new_model,
                new_debug: _,
            } => {
                assert_eq!(new_model.as_deref(), Some("claude-haiku-4-5"));
            }
            other => panic!("expected HotReloadable, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn diff_model_added_is_hot_reloadable() {
        let old = "restart: never\nmax_restarts: 5\n";
        let new = "restart: never\nmax_restarts: 5\nmodel: \"claude-haiku-4-5\"\n";
        match classify(old, new) {
            ChangeKind::HotReloadable {
                new_model,
                new_debug: _,
            } => {
                assert_eq!(new_model.as_deref(), Some("claude-haiku-4-5"));
            }
            other => panic!("expected HotReloadable, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn diff_model_removed_is_hot_reloadable() {
        let old = "restart: never\nmax_restarts: 5\nmodel: \"claude-haiku-4-5\"\n";
        let new = "restart: never\nmax_restarts: 5\n";
        match classify(old, new) {
            ChangeKind::HotReloadable {
                new_model,
                new_debug: _,
            } => {
                assert!(new_model.is_none());
            }
            other => panic!("expected HotReloadable, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn diff_other_field_changed_is_restart_required() {
        let old = "restart: never\nmax_restarts: 5\nmodel: \"claude-sonnet-4-6\"\n";
        let new = "restart: always\nmax_restarts: 5\nmodel: \"claude-sonnet-4-6\"\n";
        assert!(matches!(classify(old, new), ChangeKind::RestartRequired));
    }

    #[tokio::test]
    async fn diff_deprecated_learning_change_is_ignored_for_reload() {
        let old = r#"restart: never
learning:
  fork_probe_enabled: true
  fork_probe_model: claude-sonnet-4-6
  probe_model: claude-haiku-4-5
  background_review_enabled: true
  episode_selector_model: claude-sonnet-4-6
  episode_selector_max_budget_usd: 1.0
  episode_settle_seconds: 90
  circuit_failure_threshold: 3
  circuit_cooldown_minutes: 20
"#;
        let new = r#"restart: never
learning:
  fork_probe_enabled: false
  fork_probe_model: claude-opus-4-1
  probe_model: claude-sonnet-4-6
  background_review_enabled: false
  episode_selector_model: claude-haiku-4-5
  episode_selector_max_budget_usd: 2.0
  episode_settle_seconds: 180
  circuit_failure_threshold: 9
  circuit_cooldown_minutes: 60
"#;
        assert!(matches!(
            classify(old, new),
            ChangeKind::HotReloadable { .. }
        ));
    }

    #[tokio::test]
    async fn diff_current_learning_change_requires_restart() {
        let old = "restart: never\nlearning:\n  max_daily_budget_usd: 2.0\n";
        let new = "restart: never\nlearning:\n  max_daily_budget_usd: 3.0\n";
        assert!(matches!(classify(old, new), ChangeKind::RestartRequired));
    }

    #[tokio::test]
    async fn diff_model_and_other_field_is_restart_required() {
        let old = "restart: never\nmodel: \"claude-sonnet-4-6\"\n";
        let new = "restart: always\nmodel: \"claude-haiku-4-5\"\n";
        assert!(matches!(classify(old, new), ChangeKind::RestartRequired));
    }

    #[tokio::test]
    async fn diff_parse_failure_is_restart_required() {
        let old = "restart: never\n";
        let new = "{ this is not yaml";
        assert!(matches!(classify(old, new), ChangeKind::RestartRequired));
    }

    #[tokio::test]
    async fn diff_identical_yaml_is_no_change() {
        let yaml = "restart: never\nmodel: \"claude-haiku-4-5\"\n";
        assert!(matches!(classify(yaml, yaml), ChangeKind::NoChange));
    }

    #[tokio::test]
    async fn agent_config_partial_eq_smoke_test() {
        let a: AgentConfig = serde_saphyr::from_str("restart: never\n").unwrap();
        let b: AgentConfig = serde_saphyr::from_str("restart: never\n").unwrap();
        assert_eq!(a, b);
    }

    #[tokio::test]
    async fn diff_debug_only_is_hot_reloadable() {
        let old = "restart: never\nmax_restarts: 5\ndebug: false\n";
        let new = "restart: never\nmax_restarts: 5\ndebug: true\n";
        match classify(old, new) {
            ChangeKind::HotReloadable {
                new_model,
                new_debug,
            } => {
                assert!(new_model.is_none());
                assert_eq!(new_debug, Some(true));
            }
            other => panic!("expected HotReloadable, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn diff_debug_added_is_hot_reloadable() {
        let old = "restart: never\nmax_restarts: 5\n";
        let new = "restart: never\nmax_restarts: 5\ndebug: true\n";
        match classify(old, new) {
            ChangeKind::HotReloadable {
                new_model,
                new_debug,
            } => {
                assert!(new_model.is_none());
                assert_eq!(new_debug, Some(true));
            }
            other => panic!("expected HotReloadable, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn diff_debug_removed_is_hot_reloadable() {
        let old = "restart: never\nmax_restarts: 5\ndebug: true\n";
        let new = "restart: never\nmax_restarts: 5\n";
        match classify(old, new) {
            ChangeKind::HotReloadable {
                new_model,
                new_debug,
            } => {
                assert!(new_model.is_none());
                assert!(new_debug.is_none());
            }
            other => panic!("expected HotReloadable, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn diff_debug_and_model_combined_is_hot_reloadable() {
        let old = "restart: never\nmodel: \"claude-sonnet-4-6\"\ndebug: false\n";
        let new = "restart: never\nmodel: \"claude-haiku-4-5\"\ndebug: true\n";
        match classify(old, new) {
            ChangeKind::HotReloadable {
                new_model,
                new_debug,
            } => {
                assert_eq!(new_model.as_deref(), Some("claude-haiku-4-5"));
                assert_eq!(new_debug, Some(true));
            }
            other => panic!("expected HotReloadable, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn diff_debug_plus_other_field_is_restart_required() {
        let old = "restart: never\nmax_restarts: 5\ndebug: false\n";
        let new = "restart: always\nmax_restarts: 5\ndebug: true\n";
        assert!(matches!(classify(old, new), ChangeKind::RestartRequired));
    }

    // diff_classify itself returns None on removal — the watcher handles the fallback to initial_debug.
    // The diff-level test below documents this contract.
    #[tokio::test]
    async fn diff_debug_removed_returns_none_for_watcher_to_handle() {
        let old = "restart: never\ndebug: true\n";
        let new = "restart: never\n";
        match classify(old, new) {
            ChangeKind::HotReloadable { new_debug, .. } => {
                assert!(
                    new_debug.is_none(),
                    "removal yields None — watcher uses initial_debug fallback"
                );
            }
            other => panic!("expected HotReloadable, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn diff_providers_only_is_providers_reload() {
        let old = "restart: never\nmax_restarts: 5\nsandbox:\n  mode: openshell\n";
        let new = "restart: never\nmax_restarts: 5\nsandbox:\n  mode: openshell\n  providers:\n    - name: right-typefully\n      type: generic\n      generic:\n        env_var: TYPEFULLY_API_KEY\n        upstream_host: api.typefully.com\n";
        match classify(old, new) {
            ChangeKind::ProvidersReload { new_config, .. } => {
                let provs = new_config
                    .sandbox
                    .as_ref()
                    .expect("sandbox")
                    .providers
                    .clone();
                assert_eq!(provs.len(), 1);
                assert_eq!(provs[0].name, "right-typefully");
            }
            other => panic!("expected ProvidersReload, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn diff_providers_plus_other_field_is_restart_required() {
        let old = "restart: never\nmax_restarts: 5\nsandbox:\n  mode: openshell\n";
        let new = "restart: always\nmax_restarts: 5\nsandbox:\n  mode: openshell\n  providers:\n    - name: right-typefully\n      type: generic\n      generic:\n        env_var: TYPEFULLY_API_KEY\n        upstream_host: api.typefully.com\n";
        assert!(matches!(classify(old, new), ChangeKind::RestartRequired));
    }

    #[tokio::test]
    async fn diff_model_only_still_hot_reloadable() {
        let old = "restart: never\nmax_restarts: 5\nmodel: opus\n";
        let new = "restart: never\nmax_restarts: 5\nmodel: sonnet\n";
        assert!(matches!(
            classify(old, new),
            ChangeKind::HotReloadable { .. }
        ));
    }
}
