use std::path::{Path, PathBuf};

/// Options for destroying an agent (resolved by caller — no TTY interaction).
pub struct DestroyOptions {
    pub agent_name: String,
    pub backup: bool,
    pub pc_port: u16,
}

/// Result of a destroy operation — booleans reflect what actually happened.
pub struct DestroyResult {
    /// Whether the agent process was stopped via process-compose.
    pub agent_stopped: bool,
    /// Whether an OpenShell sandbox was deleted.
    pub sandbox_deleted: bool,
    /// Path to backup if one was created.
    pub backup_path: Option<PathBuf>,
    /// Whether the agent directory was removed.
    pub dir_removed: bool,
    /// Whether process-compose was reloaded.
    pub pc_reloaded: bool,
}

/// Destroy an agent: stop process, optionally backup, delete sandbox, remove directory, reload PC.
///
/// Non-fatal steps (stop, sandbox delete, PC reload) warn and continue.
/// Fatal steps (backup if requested, directory removal) propagate errors.
pub async fn destroy_agent(home: &Path, options: &DestroyOptions) -> miette::Result<DestroyResult> {
    todo!()
}
