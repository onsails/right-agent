//! `right agent rebootstrap` — re-enter bootstrap mode for an existing agent.
//!
//! Inverts the state mutations performed by bootstrap completion:
//! - Backs up `IDENTITY.md` / `SOUL.md` / `USER.md` from host and sandbox.
//! - Deletes those files from both sides.
//! - Recreates `BOOTSTRAP.md` on host (the bootstrap-mode flag).
//! - Deactivates all active `sessions` rows so the next message starts a
//!   new CC session.
//!
//! Sandbox, credentials, memory bank, and `data.db` rows are preserved.
//! Process-compose orchestration (stop bot → execute → start bot) is the
//! caller's responsibility (see `crates/right/src/main.rs::cmd_agent_rebootstrap`).
//!
//! See `docs/superpowers/specs/2026-04-29-rebootstrap-cmd-design.md`.

use std::path::{Path, PathBuf};

use crate::agent::types::{AgentConfig, SandboxMode};

/// Identity files that bootstrap (re)creates and that this command rewinds.
pub const IDENTITY_FILES: &[&str] = &["IDENTITY.md", "SOUL.md", "USER.md"];

/// Resolved inputs for a rebootstrap run. Cheap to compute; doesn't touch
/// the network or sandbox.
#[derive(Debug, Clone)]
pub struct RebootstrapPlan {
    pub agent_name: String,
    pub agent_dir: PathBuf,
    pub backup_dir: PathBuf,
    pub sandbox_mode: SandboxMode,
    /// `Some(name)` for openshell-mode agents; `None` for `sandbox.mode = none`.
    pub sandbox_name: Option<String>,
}

/// Outcome summary returned to the CLI for the final printed report.
#[derive(Debug, Default)]
pub struct RebootstrapReport {
    pub backup_dir: PathBuf,
    pub host_backed_up: Vec<&'static str>,
    pub sandbox_backed_up: Vec<&'static str>,
    pub sessions_deactivated: usize,
}

/// Build a `RebootstrapPlan` for `agent_name` under `home`.
///
/// Errors if the agent directory is missing.
pub fn plan(home: &Path, agent_name: &str) -> miette::Result<RebootstrapPlan> {
    let agents_dir = crate::config::agents_dir(home);
    let agent_dir = agents_dir.join(agent_name);
    if !agent_dir.exists() {
        return Err(miette::miette!(
            "Agent '{}' not found at {}",
            agent_name,
            agent_dir.display()
        ));
    }

    let config: Option<AgentConfig> = crate::agent::parse_agent_config(&agent_dir)?;

    let sandbox_mode = config
        .as_ref()
        .map(|c| *c.sandbox_mode())
        .unwrap_or(SandboxMode::Openshell);

    let sandbox_name = match sandbox_mode {
        SandboxMode::Openshell => Some(
            config
                .as_ref()
                .map(|c| crate::openshell::resolve_sandbox_name(agent_name, c))
                .unwrap_or_else(|| crate::openshell::sandbox_name(agent_name)),
        ),
        SandboxMode::None => None,
    };

    let timestamp = chrono::Local::now().format("%Y%m%d-%H%M").to_string();
    let backup_dir = crate::config::backups_dir(home, agent_name)
        .join(format!("rebootstrap-{timestamp}"));

    Ok(RebootstrapPlan {
        agent_name: agent_name.to_string(),
        agent_dir,
        backup_dir,
        sandbox_mode,
        sandbox_name,
    })
}

/// Run the full rebootstrap sequence (host + sandbox file ops + session
/// deactivation). Caller is responsible for stopping the bot before and
/// restarting it after.
pub async fn execute(_plan: &RebootstrapPlan) -> miette::Result<RebootstrapReport> {
    // Filled in by Task 7.
    miette::bail!("rebootstrap::execute not yet implemented")
}

/// Copy any present identity files from `agent_dir` into `backup_dir`.
/// Returns the list of files that were actually copied.
///
/// `backup_dir` must already exist. Missing source files are skipped at
/// DEBUG level (not errors).
#[allow(dead_code)] // called by execute() in Task 7
fn backup_host_files(
    agent_dir: &Path,
    backup_dir: &Path,
) -> miette::Result<Vec<&'static str>> {
    let mut copied = Vec::new();
    for &name in IDENTITY_FILES {
        let src = agent_dir.join(name);
        if !src.exists() {
            tracing::debug!(file = name, "rebootstrap: host file absent, skipping backup");
            continue;
        }
        let dst = backup_dir.join(name);
        std::fs::copy(&src, &dst).map_err(|e| {
            miette::miette!(
                "failed to back up host {} to {}: {e:#}",
                name,
                dst.display()
            )
        })?;
        copied.push(name);
    }
    Ok(copied)
}

/// Download identity files from sandbox into `<backup_dir>/sandbox/`.
/// Skipped entirely when `sandbox_name` is `None` (none-mode).
///
/// Returns the list of files that were actually downloaded. A missing
/// sandbox file is not an error; a download failure on a present file is.
#[allow(dead_code)] // called by execute() in Task 7
async fn backup_sandbox_files(
    sandbox_name: Option<&str>,
    backup_dir: &Path,
) -> miette::Result<Vec<&'static str>> {
    let Some(sandbox) = sandbox_name else {
        return Ok(Vec::new());
    };

    let mtls_dir = match crate::openshell::preflight_check() {
        crate::openshell::OpenShellStatus::Ready(d) => d,
        other => {
            tracing::info!(
                ?other,
                "rebootstrap: openshell not ready, skipping sandbox-side backup"
            );
            return Ok(Vec::new());
        }
    };

    let mut client = crate::openshell::connect_grpc(&mtls_dir).await?;

    // If the sandbox doesn't exist yet (never created), skip cleanly.
    if !crate::openshell::sandbox_exists(&mut client, sandbox).await? {
        tracing::info!(sandbox, "rebootstrap: sandbox absent, skipping sandbox-side backup");
        return Ok(Vec::new());
    }

    let sandbox_id = crate::openshell::resolve_sandbox_id(&mut client, sandbox).await?;
    let sandbox_backup_dir = backup_dir.join("sandbox");
    std::fs::create_dir_all(&sandbox_backup_dir).map_err(|e| {
        miette::miette!(
            "failed to create sandbox backup dir {}: {e:#}",
            sandbox_backup_dir.display()
        )
    })?;

    let mut copied = Vec::new();
    for &name in IDENTITY_FILES {
        let sandbox_path = format!("/sandbox/{name}");
        // Probe — exit 0 if present, 1 if absent.
        let (_stdout, exit) = crate::openshell::exec_in_sandbox(
            &mut client,
            &sandbox_id,
            &["test", "-f", &sandbox_path],
            crate::openshell::DEFAULT_EXEC_TIMEOUT_SECS,
        )
        .await?;
        if exit != 0 {
            tracing::debug!(file = name, "rebootstrap: sandbox file absent, skipping backup");
            continue;
        }
        let dst = sandbox_backup_dir.join(name);
        crate::openshell::download_file(sandbox, &sandbox_path, &dst).await?;
        copied.push(name);
    }
    Ok(copied)
}

/// Recreate `BOOTSTRAP.md` on host with the canonical bootstrap instructions.
/// Overwrites any existing file.
#[allow(dead_code)] // called by execute() in Task 7
fn write_bootstrap_md(agent_dir: &Path) -> miette::Result<()> {
    let path = agent_dir.join("BOOTSTRAP.md");
    std::fs::write(&path, crate::codegen::BOOTSTRAP_INSTRUCTIONS).map_err(|e| {
        miette::miette!("failed to write BOOTSTRAP.md at {}: {e:#}", path.display())
    })
}

/// Mark all active `sessions` rows in the agent's `data.db` as inactive.
/// Returns the number of rows updated. Skipped (returns 0) if `data.db`
/// is missing.
#[allow(dead_code)] // called by execute() in Task 7
fn deactivate_active_sessions(agent_dir: &Path) -> miette::Result<usize> {
    if !agent_dir.join("data.db").exists() {
        tracing::debug!("rebootstrap: data.db absent, skipping session deactivation");
        return Ok(0);
    }
    let conn = crate::memory::open_connection(agent_dir, false)
        .map_err(|e| miette::miette!("open data.db failed: {e:#}"))?;
    let n = conn
        .execute("UPDATE sessions SET is_active = 0 WHERE is_active = 1", [])
        .map_err(|e| miette::miette!("UPDATE sessions failed: {e:#}"))?;
    Ok(n)
}

/// Remove identity files from `agent_dir`. Infallible — "not found" and
/// I/O errors are logged at DEBUG/WARN respectively but never returned.
#[allow(dead_code)] // called by execute() in Task 7
fn delete_identity_from_host(agent_dir: &Path) {
    for &name in IDENTITY_FILES {
        let p = agent_dir.join(name);
        match std::fs::remove_file(&p) {
            Ok(()) => tracing::debug!(file = name, "rebootstrap: removed host file"),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => tracing::warn!(
                file = name,
                "rebootstrap: failed to remove host file: {e:#}"
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn backup_host_files_copies_present_files() {
        let home = tempfile::tempdir().unwrap();
        let agent_dir = home.path().join("agents").join("c");
        std::fs::create_dir_all(&agent_dir).unwrap();
        // Two of three identity files present on host
        std::fs::write(agent_dir.join("IDENTITY.md"), "id\n").unwrap();
        std::fs::write(agent_dir.join("USER.md"), "user\n").unwrap();
        // SOUL.md intentionally missing

        let backup_dir = home.path().join("backup");
        std::fs::create_dir_all(&backup_dir).unwrap();

        let copied = backup_host_files(&agent_dir, &backup_dir).unwrap();

        assert_eq!(copied, vec!["IDENTITY.md", "USER.md"]);
        assert_eq!(
            std::fs::read_to_string(backup_dir.join("IDENTITY.md")).unwrap(),
            "id\n"
        );
        assert_eq!(
            std::fs::read_to_string(backup_dir.join("USER.md")).unwrap(),
            "user\n"
        );
        assert!(!backup_dir.join("SOUL.md").exists());
    }

    #[test]
    fn backup_host_files_no_files_returns_empty() {
        let home = tempfile::tempdir().unwrap();
        let agent_dir = home.path().join("agents").join("d");
        std::fs::create_dir_all(&agent_dir).unwrap();
        let backup_dir = home.path().join("backup");
        std::fs::create_dir_all(&backup_dir).unwrap();
        let copied = backup_host_files(&agent_dir, &backup_dir).unwrap();
        assert!(copied.is_empty());
    }

    fn make_home_with_agent(name: &str, agent_yaml: Option<&str>) -> TempDir {
        let home = tempfile::tempdir().unwrap();
        let agent_dir = home.path().join("agents").join(name);
        std::fs::create_dir_all(&agent_dir).unwrap();
        // discover_agents requires IDENTITY.md OR BOOTSTRAP.md present;
        // parse_agent_config tolerates missing agent.yaml.
        std::fs::write(agent_dir.join("IDENTITY.md"), format!("# {name}\n")).unwrap();
        if let Some(y) = agent_yaml {
            std::fs::write(agent_dir.join("agent.yaml"), y).unwrap();
        }
        home
    }

    #[test]
    fn plan_errors_when_agent_missing() {
        let home = tempfile::tempdir().unwrap();
        let err = plan(home.path(), "ghost").unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("ghost"), "error should name the agent: {msg}");
    }

    #[test]
    fn plan_defaults_to_openshell_when_no_agent_yaml() {
        let home = make_home_with_agent("alice", None);
        let p = plan(home.path(), "alice").unwrap();
        assert_eq!(p.agent_name, "alice");
        assert_eq!(p.sandbox_mode, SandboxMode::Openshell);
        assert!(p.sandbox_name.is_some());
        assert!(
            p.backup_dir.starts_with(home.path().join("backups").join("alice")),
            "backup_dir under <home>/backups/alice/: {}",
            p.backup_dir.display()
        );
        let leaf = p.backup_dir.file_name().unwrap().to_string_lossy();
        assert!(
            leaf.starts_with("rebootstrap-"),
            "backup leaf should start with 'rebootstrap-': {leaf}"
        );
    }

    #[test]
    fn plan_respects_sandbox_mode_none() {
        let yaml = "sandbox:\n  mode: none\n";
        let home = make_home_with_agent("bob", Some(yaml));
        let p = plan(home.path(), "bob").unwrap();
        assert_eq!(p.sandbox_mode, SandboxMode::None);
        assert!(p.sandbox_name.is_none());
    }

    #[test]
    fn delete_identity_from_host_removes_present_files() {
        let home = tempfile::tempdir().unwrap();
        let agent_dir = home.path().join("agents").join("e");
        std::fs::create_dir_all(&agent_dir).unwrap();
        std::fs::write(agent_dir.join("IDENTITY.md"), "x").unwrap();
        std::fs::write(agent_dir.join("SOUL.md"), "x").unwrap();
        // USER.md absent on purpose

        delete_identity_from_host(&agent_dir);

        for &f in IDENTITY_FILES {
            assert!(
                !agent_dir.join(f).exists(),
                "{f} should be gone after delete_identity_from_host"
            );
        }
    }

    #[test]
    fn delete_identity_from_host_idempotent() {
        let home = tempfile::tempdir().unwrap();
        let agent_dir = home.path().join("agents").join("f");
        std::fs::create_dir_all(&agent_dir).unwrap();
        // No identity files at all
        delete_identity_from_host(&agent_dir);
        delete_identity_from_host(&agent_dir);
        // No panic, no assertion — just don't error.
    }

    #[test]
    fn write_bootstrap_md_writes_canonical_content() {
        let home = tempfile::tempdir().unwrap();
        let agent_dir = home.path().join("agents").join("g");
        std::fs::create_dir_all(&agent_dir).unwrap();

        write_bootstrap_md(&agent_dir).unwrap();

        let path = agent_dir.join("BOOTSTRAP.md");
        assert!(path.exists());
        let content = std::fs::read_to_string(&path).unwrap();
        assert_eq!(content, crate::codegen::BOOTSTRAP_INSTRUCTIONS);
    }

    #[test]
    fn write_bootstrap_md_overwrites_existing() {
        let home = tempfile::tempdir().unwrap();
        let agent_dir = home.path().join("agents").join("h");
        std::fs::create_dir_all(&agent_dir).unwrap();
        std::fs::write(agent_dir.join("BOOTSTRAP.md"), "stale").unwrap();

        write_bootstrap_md(&agent_dir).unwrap();

        let content = std::fs::read_to_string(agent_dir.join("BOOTSTRAP.md")).unwrap();
        assert_eq!(content, crate::codegen::BOOTSTRAP_INSTRUCTIONS);
        assert_ne!(content, "stale");
    }

    #[test]
    fn deactivate_active_sessions_flips_all_active_rows() {
        let dir = tempfile::tempdir().unwrap();
        let conn = crate::memory::open_connection(dir.path(), true).unwrap();
        // Two active sessions for two distinct (chat_id, thread_id),
        // and one already-inactive session that must stay untouched.
        conn.execute(
            "INSERT INTO sessions (chat_id, thread_id, root_session_id, is_active) \
             VALUES (1, 0, 'uuid-a', 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO sessions (chat_id, thread_id, root_session_id, is_active) \
             VALUES (2, 0, 'uuid-b', 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO sessions (chat_id, thread_id, root_session_id, is_active) \
             VALUES (3, 0, 'uuid-c', 0)",
            [],
        )
        .unwrap();
        drop(conn);

        let n = deactivate_active_sessions(dir.path()).unwrap();
        assert_eq!(n, 2);

        let conn = crate::memory::open_connection(dir.path(), true).unwrap();
        let active_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sessions WHERE is_active = 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(active_count, 0, "no rows should remain active");
        let total: i64 = conn
            .query_row("SELECT COUNT(*) FROM sessions", [], |r| r.get(0))
            .unwrap();
        assert_eq!(total, 3, "no rows should be deleted");
    }

    #[test]
    fn deactivate_active_sessions_skips_when_db_missing() {
        let dir = tempfile::tempdir().unwrap();
        // No data.db
        let n = deactivate_active_sessions(dir.path()).unwrap();
        assert_eq!(n, 0);
    }

    #[test]
    fn deactivate_active_sessions_no_active_returns_zero() {
        let dir = tempfile::tempdir().unwrap();
        let _ = crate::memory::open_connection(dir.path(), true).unwrap();
        let n = deactivate_active_sessions(dir.path()).unwrap();
        assert_eq!(n, 0);
    }
}
