//! Watch agent.yaml for changes and trigger graceful restart.
//!
//! Uses `notify` with debouncing (2s) to avoid reacting to partial writes.
//! On change detection, sets a flag and cancels the provided `CancellationToken`.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio_util::sync::CancellationToken;

/// Spawn a blocking thread that watches `agent.yaml` for modifications.
///
/// When a change is detected (debounced 2s), sets `config_changed` to true
/// and cancels `token`, signalling all subsystems to begin graceful shutdown.
/// The caller checks `config_changed` after shutdown to decide the exit code.
pub fn spawn_config_watcher(
    agent_yaml: &Path,
    token: CancellationToken,
    config_changed: Arc<AtomicBool>,
) -> miette::Result<()> {
    use notify_debouncer_mini::{DebouncedEventKind, new_debouncer};
    use std::sync::mpsc;
    use std::time::Duration;

    let watch_dir = agent_yaml
        .parent()
        .ok_or_else(|| miette::miette!("agent.yaml has no parent directory"))?
        .to_path_buf();
    let yaml_filename = agent_yaml
        .file_name()
        .ok_or_else(|| miette::miette!("agent.yaml has no filename"))?
        .to_os_string();

    let (tx, rx) = mpsc::channel();

    let mut debouncer = new_debouncer(Duration::from_secs(2), tx)
        .map_err(|e| miette::miette!("failed to create file watcher: {e:#}"))?;

    debouncer
        .watcher()
        .watch(&watch_dir, notify::RecursiveMode::NonRecursive)
        .map_err(|e| miette::miette!("failed to watch {}: {e:#}", watch_dir.display()))?;

    std::thread::spawn(move || {
        // Move debouncer into thread to keep it alive.
        let _debouncer = debouncer;

        for result in rx {
            match result {
                Ok(events) => {
                    let relevant = events.iter().any(|e| {
                        e.kind == DebouncedEventKind::Any
                            && e.path.file_name() == Some(&yaml_filename)
                    });
                    if relevant {
                        tracing::info!("agent.yaml changed — initiating graceful restart");
                        config_changed.store(true, Ordering::Release);
                        token.cancel();
                        return;
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
