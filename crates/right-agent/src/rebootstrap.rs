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

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

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
}
