//! `right agent rebootstrap` — re-enter bootstrap mode for an existing agent.
//!
//! Inverts the state mutations performed by bootstrap completion:
//! - Backs up `IDENTITY.md` / `SOUL.md` / `USER.md` from host and sandbox.
//! - For configured sandboxes, requires authoritative sandbox identity deletion.
//! - Deletes the host identity only after sandbox deletion succeeds.
//! - Recreates `BOOTSTRAP.md` on host (the bootstrap-mode flag).
//! - Deactivates all active `sessions` rows so the next message starts a
//!   new CC session.
//!
//! Sandbox, credentials, memory bank, and other `data.db` rows are preserved.
//! Process-compose orchestration (stop bot → execute → start bot) is the
//! caller's responsibility (see `crates/right/src/main.rs::cmd_agent_rebootstrap`).
//!
//! See `docs/superpowers/specs/2026-04-29-rebootstrap-cmd-design.md`.

use std::path::{Path, PathBuf};

use tonic::transport::Channel;

use crate::agent::types::AgentConfig;
use right_openshell::openshell_proto::openshell::v1::open_shell_client::OpenShellClient;

/// Identity files that bootstrap (re)creates and that this command rewinds.
pub const IDENTITY_FILES: &[&str] = &["IDENTITY.md", "SOUL.md", "USER.md"];

/// Runtime-owned crash-recovery record written by the bot while committing
/// bootstrap completion. It is transient state, not a codegen-managed file.
pub const BOOTSTRAP_FINALIZATION_INTENT_FILE: &str = ".bootstrap-finalization.json";

/// Resolved inputs for a rebootstrap run. Cheap to compute; doesn't touch
/// the network or sandbox.
#[derive(Debug, Clone)]
pub struct RebootstrapPlan {
    pub agent_name: String,
    pub agent_dir: PathBuf,
    pub backup_dir: PathBuf,
    pub sandbox_name: String,
}

/// Outcome summary returned to the CLI for the final printed report.
#[derive(Debug)]
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
    let agents_dir = right_config::agents_dir(home);
    let agent_dir = agents_dir.join(agent_name);
    if !agent_dir.exists() {
        return Err(miette::miette!(
            "Agent '{}' not found at {}",
            agent_name,
            agent_dir.display()
        ));
    }

    let config: Option<AgentConfig> = crate::agent::parse_agent_config(&agent_dir)?;

    let explicit_sandbox_name = config
        .as_ref()
        .and_then(|c| c.sandbox.as_ref())
        .and_then(|s| s.name.as_deref());
    let sandbox_name =
        right_openshell::openshell::resolve_sandbox_name(agent_name, explicit_sandbox_name);

    let timestamp = chrono::Local::now().format("%Y%m%d-%H%M").to_string();
    let backup_dir =
        right_config::backups_dir(home, agent_name).join(format!("rebootstrap-{timestamp}"));

    Ok(RebootstrapPlan {
        agent_name: agent_name.to_string(),
        agent_dir,
        backup_dir,
        sandbox_name,
    })
}

/// Run the full rebootstrap sequence (host + sandbox file ops + session
/// deactivation). For configured sandboxes, host state is reset only after
/// authoritative sandbox identity deletion succeeds. Caller is responsible
/// for stopping the bot before and restarting it after.
pub async fn execute(plan: &RebootstrapPlan) -> miette::Result<RebootstrapReport> {
    std::fs::create_dir_all(&plan.backup_dir).map_err(|e| {
        miette::miette!(
            "failed to create backup dir {}: {e:#}",
            plan.backup_dir.display()
        )
    })?;

    let host_backed_up = backup_host_files(&plan.agent_dir, &plan.backup_dir)?;

    let mut session = open_sandbox_session(&plan.sandbox_name).await?;
    let sandbox_backed_up = backup_sandbox_files(&mut session, &plan.backup_dir).await?;

    // The sandbox is authoritative for sandboxed agents. Host identity,
    // sessions, and answers remain untouched unless its deletion completed.
    delete_identity_from_sandbox(&mut session).await?;
    delete_identity_from_host(&plan.agent_dir)?;

    write_bootstrap_md(&plan.agent_dir)?;
    let sessions_deactivated = deactivate_active_sessions(&plan.agent_dir).await?;
    clear_bootstrap_answers(&plan.agent_dir).await?;
    clear_bootstrap_finalization_intent(&plan.agent_dir)?;

    Ok(RebootstrapReport {
        backup_dir: plan.backup_dir.clone(),
        host_backed_up,
        sandbox_backed_up,
        sessions_deactivated,
    })
}

/// Copy any present identity files from `agent_dir` into `backup_dir`.
/// Returns the list of files that were actually copied.
///
/// `backup_dir` must already exist. Missing source files are skipped at
/// DEBUG level (not errors).
fn backup_host_files(agent_dir: &Path, backup_dir: &Path) -> miette::Result<Vec<&'static str>> {
    let mut copied = Vec::new();
    for &name in IDENTITY_FILES {
        let src = agent_dir.join(name);
        if !src.exists() {
            tracing::debug!(
                file = name,
                "rebootstrap: host file absent, skipping backup"
            );
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

/// Live gRPC handle to a sandbox we've already verified exists.
///
/// Reused across `backup_sandbox_files` and `delete_identity_from_sandbox` so
/// `execute()` only pays for one preflight + connect + existence probe.
struct SandboxSession {
    name: String,
    id: String,
    client: OpenShellClient<Channel>,
}

/// Resolve `sandbox_name` to a connected gRPC client + sandbox id.
///
/// Every failure to reach or resolve the configured sandbox is an error:
/// continuing would reset host state while leaving the authoritative sandbox
/// identity intact.
async fn open_sandbox_session(sandbox: &str) -> miette::Result<SandboxSession> {
    let mtls_dir = match right_openshell::openshell::preflight_check() {
        right_openshell::openshell::OpenShellStatus::Ready(d) => d,
        other => {
            return Err(miette::miette!(
                "OpenShell is not ready for configured sandbox '{sandbox}': {other:?}; refusing to reset host state"
            ));
        }
    };

    let mut client = right_openshell::openshell::connect_grpc(&mtls_dir).await?;
    if !right_openshell::openshell::sandbox_exists(&mut client, sandbox).await? {
        return Err(miette::miette!(
            "configured sandbox '{sandbox}' does not exist; refusing to reset host state"
        ));
    }
    let id = right_openshell::openshell::resolve_sandbox_id(&mut client, sandbox).await?;
    Ok(SandboxSession {
        name: sandbox.to_string(),
        id,
        client,
    })
}

/// Download identity files from sandbox into `<backup_dir>/sandbox/`.
///
/// Returns the list of files that were actually downloaded. A missing
/// sandbox file is not an error; a download failure on a present file is.
async fn backup_sandbox_files(
    session: &mut SandboxSession,
    backup_dir: &Path,
) -> miette::Result<Vec<&'static str>> {
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
        let (_stdout, exit) = right_openshell::openshell::exec_in_sandbox(
            &mut session.client,
            &session.id,
            &["test", "-f", &sandbox_path],
            right_openshell::openshell::DEFAULT_EXEC_TIMEOUT_SECS,
        )
        .await?;
        if exit != 0 {
            tracing::debug!(
                file = name,
                "rebootstrap: sandbox file absent, skipping backup"
            );
            continue;
        }
        let dst = sandbox_backup_dir.join(name);
        right_openshell::openshell::download_file(&session.name, &sandbox_path, &dst).await?;
        copied.push(name);
    }
    Ok(copied)
}

/// Recreate `BOOTSTRAP.md` on host with the canonical bootstrap instructions.
/// Overwrites any existing file.
fn write_bootstrap_md(agent_dir: &Path) -> miette::Result<()> {
    let path = agent_dir.join("BOOTSTRAP.md");
    std::fs::write(&path, right_codegen::BOOTSTRAP_INSTRUCTIONS)
        .map_err(|e| miette::miette!("failed to write BOOTSTRAP.md at {}: {e:#}", path.display()))
}

/// Remove any pending bootstrap-finalization intent after an explicit
/// rebootstrap has reset identity and session state. Missing state is already
/// clear; other filesystem errors fail the reset.
pub fn clear_bootstrap_finalization_intent(agent_dir: &Path) -> miette::Result<()> {
    let path = agent_dir.join(BOOTSTRAP_FINALIZATION_INTENT_FILE);
    match std::fs::remove_file(&path) {
        Ok(()) => {
            let directory = std::fs::File::open(agent_dir).map_err(|error| {
                miette::miette!(
                    "failed to open agent directory {} after clearing bootstrap finalization intent: {error:#}",
                    agent_dir.display()
                )
            })?;
            directory.sync_all().map_err(|error| {
                miette::miette!(
                    "failed to sync agent directory {} after clearing bootstrap finalization intent: {error:#}",
                    agent_dir.display()
                )
            })
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(miette::miette!(
            "failed to clear bootstrap finalization intent {}: {error:#}",
            path.display()
        )),
    }
}

/// Mark all active `sessions` rows in the agent's `data.db` as inactive.
/// Returns the number of rows updated. Skipped (returns 0) if `data.db`
/// is missing.
///
/// Opens the connection with `migrate: false`. This is safe in production
/// because the bot and MCP aggregator processes own schema migrations on
/// per-agent `data.db` (see ARCHITECTURE.md "SQLite Rules > Migration
/// Ownership") — by the time any rebootstrap call lands, those processes
/// have already created the `sessions` table. The assumption is invisible
/// at the call site: if this function were ever invoked against a `data.db`
/// whose schema predates the `sessions` table, the `UPDATE` would surface
/// an opaque "no such table: sessions" error. That state is unreachable in
/// production, so we accept the opacity rather than migrate defensively
/// here.
async fn deactivate_active_sessions(agent_dir: &Path) -> miette::Result<usize> {
    if !agent_dir.join("data.db").exists() {
        tracing::debug!("rebootstrap: data.db absent, skipping session deactivation");
        return Ok(0);
    }
    let conn = right_db::open_connection(agent_dir, false)
        .await
        .map_err(|e| miette::miette!("open data.db failed: {e:#}"))?;
    let n = conn
        .execute("UPDATE sessions SET is_active = 0 WHERE is_active = 1", [])
        .await
        .map_err(|e| miette::miette!("UPDATE sessions failed: {e:#}"))?;
    Ok(n)
}

async fn clear_bootstrap_answers(agent_dir: &Path) -> miette::Result<usize> {
    if !agent_dir.join("data.db").exists() {
        tracing::debug!("rebootstrap: data.db absent, skipping bootstrap answer cleanup");
        return Ok(0);
    }
    let conn = right_db::open_connection(agent_dir, false)
        .await
        .map_err(|error| miette::miette!("open data.db failed: {error:#}"))?;
    right_db::bootstrap_answers::clear(&conn)
        .await
        .map_err(|error| miette::miette!("clear bootstrap answers failed: {error:#}"))
}

/// Remove identity files from `agent_dir`. NotFound is silenced (idempotent);
/// any other I/O error propagates — leaving stale identity files on disk
/// while `BOOTSTRAP.md` is rewritten would defeat the whole command.
fn delete_identity_from_host(agent_dir: &Path) -> miette::Result<()> {
    for &name in IDENTITY_FILES {
        let p = agent_dir.join(name);
        match std::fs::remove_file(&p) {
            Ok(()) => tracing::debug!(file = name, "rebootstrap: removed host file"),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                return Err(miette::miette!(
                    "failed to remove host {}: {e:#}",
                    p.display()
                ));
            }
        }
    }
    Ok(())
}

/// Delete identity files from the sandbox via gRPC `exec_in_sandbox`.
///
/// `rm -f` makes missing files non-fatal, so a reachable sandbox is naturally
/// idempotent.
async fn delete_identity_from_sandbox(session: &mut SandboxSession) -> miette::Result<()> {
    let paths: Vec<String> = IDENTITY_FILES
        .iter()
        .map(|n| format!("/sandbox/{n}"))
        .collect();
    let mut cmd: Vec<&str> = vec!["rm", "-f"];
    cmd.extend(paths.iter().map(|s| s.as_str()));

    let (stdout, exit) = right_openshell::openshell::exec_in_sandbox(
        &mut session.client,
        &session.id,
        &cmd,
        right_openshell::openshell::DEFAULT_EXEC_TIMEOUT_SECS,
    )
    .await?;
    if exit != 0 {
        return Err(miette::miette!(
            "rm in sandbox returned exit {exit}: {stdout}"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    async fn record_user_name_answer(
        conn: &right_db::Connection,
        chat_id: i64,
        thread_id: i64,
        assistant_message_id: i32,
        user_message_id: i32,
    ) {
        assert_eq!(
            right_db::bootstrap_answers::claim_owner(conn, chat_id, thread_id)
                .await
                .unwrap(),
            right_db::bootstrap_answers::ClaimOwnerOutcome::Claimed
        );
        assert_eq!(
            right_db::bootstrap_answers::record_question_issue(
                conn,
                "user_name",
                chat_id,
                thread_id,
                assistant_message_id,
            )
            .await
            .unwrap(),
            right_db::bootstrap_answers::RecordQuestionIssueOutcome::Recorded
        );
        assert_eq!(
            right_db::bootstrap_answers::record_current_answer(
                conn,
                "Ada",
                chat_id,
                thread_id,
                user_message_id,
            )
            .await
            .unwrap(),
            right_db::bootstrap_answers::RecordCurrentAnswerOutcome::Recorded {
                stage: "user_name",
                next_stage: Some("agent_name")
            }
        );
    }

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
    fn plan_derives_sandbox_name_when_no_agent_yaml() {
        let home = make_home_with_agent("alice", None);
        let p = plan(home.path(), "alice").unwrap();
        assert_eq!(p.agent_name, "alice");
        assert_eq!(p.sandbox_name, "right-alice");
        assert!(
            p.backup_dir
                .starts_with(home.path().join("backups").join("alice")),
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
    fn plan_honours_explicit_sandbox_name() {
        let yaml = "sandbox:\n  name: bobs-box\n";
        let home = make_home_with_agent("bob", Some(yaml));
        let p = plan(home.path(), "bob").unwrap();
        assert_eq!(p.sandbox_name, "bobs-box");
    }

    #[test]
    fn delete_identity_from_host_removes_present_files() {
        let home = tempfile::tempdir().unwrap();
        let agent_dir = home.path().join("agents").join("e");
        std::fs::create_dir_all(&agent_dir).unwrap();
        std::fs::write(agent_dir.join("IDENTITY.md"), "x").unwrap();
        std::fs::write(agent_dir.join("SOUL.md"), "x").unwrap();
        // USER.md absent on purpose

        delete_identity_from_host(&agent_dir).unwrap();

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

        delete_identity_from_host(&agent_dir).unwrap();
        delete_identity_from_host(&agent_dir).unwrap();
    }
    #[test]
    fn clear_finalization_intent_removes_runtime_state_idempotently() {
        let dir = tempfile::tempdir().unwrap();
        let intent = dir.path().join(BOOTSTRAP_FINALIZATION_INTENT_FILE);
        std::fs::write(&intent, "pending").unwrap();

        clear_bootstrap_finalization_intent(dir.path()).unwrap();
        clear_bootstrap_finalization_intent(dir.path()).unwrap();

        assert!(!intent.exists());
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
        assert_eq!(content, right_codegen::BOOTSTRAP_INSTRUCTIONS);
    }

    #[test]
    fn write_bootstrap_md_overwrites_existing() {
        let home = tempfile::tempdir().unwrap();
        let agent_dir = home.path().join("agents").join("h");
        std::fs::create_dir_all(&agent_dir).unwrap();
        std::fs::write(agent_dir.join("BOOTSTRAP.md"), "stale").unwrap();

        write_bootstrap_md(&agent_dir).unwrap();

        let content = std::fs::read_to_string(agent_dir.join("BOOTSTRAP.md")).unwrap();
        assert_eq!(content, right_codegen::BOOTSTRAP_INSTRUCTIONS);
        assert_ne!(content, "stale");
    }

    #[tokio::test]
    async fn deactivate_active_sessions_flips_all_active_rows() {
        let dir = tempfile::tempdir().unwrap();
        let conn = right_db::open_connection(dir.path(), true).await.unwrap();
        // Two active sessions for two distinct (chat_id, thread_id),
        // and one already-inactive session that must stay untouched.
        conn.execute(
            "INSERT INTO sessions (chat_id, thread_id, root_session_id, is_active) \
             VALUES (1, 0, 'uuid-a', 1)",
            [],
        )
        .await
        .unwrap();
        conn.execute(
            "INSERT INTO sessions (chat_id, thread_id, root_session_id, is_active) \
             VALUES (2, 0, 'uuid-b', 1)",
            [],
        )
        .await
        .unwrap();
        conn.execute(
            "INSERT INTO sessions (chat_id, thread_id, root_session_id, is_active) \
             VALUES (3, 0, 'uuid-c', 0)",
            [],
        )
        .await
        .unwrap();
        drop(conn);

        let n = deactivate_active_sessions(dir.path()).await.unwrap();
        assert_eq!(n, 2);

        let conn = right_db::open_connection(dir.path(), true).await.unwrap();
        let active_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sessions WHERE is_active = 1",
                [],
                |r| r.get(0),
            )
            .await
            .unwrap();
        assert_eq!(active_count, 0, "no rows should remain active");
        let total: i64 = conn
            .query_row("SELECT COUNT(*) FROM sessions", [], |r| r.get(0))
            .await
            .unwrap();
        assert_eq!(total, 3, "no rows should be deleted");
    }

    #[tokio::test]
    async fn deactivate_active_sessions_skips_when_db_missing() {
        let dir = tempfile::tempdir().unwrap();
        // No data.db
        let n = deactivate_active_sessions(dir.path()).await.unwrap();
        assert_eq!(n, 0);
    }

    #[tokio::test]
    async fn deactivate_active_sessions_no_active_returns_zero() {
        let dir = tempfile::tempdir().unwrap();
        let _ = right_db::open_connection(dir.path(), true).await.unwrap();
        let n = deactivate_active_sessions(dir.path()).await.unwrap();
        assert_eq!(n, 0);
    }

    #[tokio::test]
    async fn clear_bootstrap_answers_removes_recorded_interview() {
        let dir = tempfile::tempdir().unwrap();
        let conn = right_db::open_connection(dir.path(), true).await.unwrap();
        record_user_name_answer(&conn, 1, 0, 9, 10).await;
        drop(conn);

        assert_eq!(clear_bootstrap_answers(dir.path()).await.unwrap(), 1);
        let conn = right_db::open_connection(dir.path(), false).await.unwrap();
        assert_eq!(
            right_db::bootstrap_answers::missing_stages(&conn, 1, 0)
                .await
                .unwrap(),
            vec!["user_name", "agent_name", "nature", "vibe", "emoji"]
        );
        assert_eq!(
            right_db::bootstrap_answers::owner(&conn).await.unwrap(),
            None
        );
    }

    #[tokio::test]
    async fn execute_configured_sandbox_unavailable_preserves_host_state() {
        let home = tempfile::tempdir().unwrap();
        let agent_dir = home.path().join("agents").join("unavailable");
        std::fs::create_dir_all(&agent_dir).unwrap();
        for &name in IDENTITY_FILES {
            std::fs::write(agent_dir.join(name), format!("original {name}\n")).unwrap();
        }
        let conn = right_db::open_connection(&agent_dir, true).await.unwrap();
        conn.execute(
            "INSERT INTO sessions (chat_id, thread_id, root_session_id, is_active) \
             VALUES (7, 3, 'preserved-session', 1)",
            [],
        )
        .await
        .unwrap();
        record_user_name_answer(&conn, 7, 3, 98, 99).await;
        drop(conn);

        let plan = RebootstrapPlan {
            agent_name: "unavailable".into(),
            agent_dir: agent_dir.clone(),
            backup_dir: home.path().join("backup"),
            sandbox_name: "right-unavailable".into(),
        };
        let error = execute(&plan).await.unwrap_err();
        assert!(
            format!("{error:#}").contains("refusing to reset host state"),
            "unexpected error: {error:#}"
        );

        for &name in IDENTITY_FILES {
            assert_eq!(
                std::fs::read_to_string(agent_dir.join(name)).unwrap(),
                format!("original {name}\n")
            );
        }
        assert!(!agent_dir.join("BOOTSTRAP.md").exists());
        let conn = right_db::open_connection(&agent_dir, false).await.unwrap();
        let active_sessions: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sessions WHERE is_active = 1",
                [],
                |row| row.get(0),
            )
            .await
            .unwrap();
        assert_eq!(active_sessions, 1);
        assert_eq!(
            right_db::bootstrap_answers::missing_stages(&conn, 7, 3)
                .await
                .unwrap(),
            vec!["agent_name", "nature", "vibe", "emoji"]
        );
    }

    // The former `execute_none_mode_full_path` test covered `execute()` end to
    // end without a sandbox. Sandboxless agents are gone: `execute()` now always
    // requires a reachable sandbox, and the full path is covered by the
    // sandbox-backed integration test in `tests/rebootstrap_sandbox.rs`.
}
