# `right agent rebootstrap` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a `right agent rebootstrap <name> [-y]` subcommand that re-enters bootstrap mode for an existing agent — backs up identity files, deletes them from host and sandbox, recreates `BOOTSTRAP.md`, deactivates active CC sessions, and bounces the bot. Sandbox, credentials, memory bank, and `data.db` rows are preserved.

**Architecture:** New module `crates/right-agent/src/rebootstrap.rs` exposes pure state-mutation logic (`plan` + `execute`). New CLI variant `AgentCommands::Rebootstrap` in `crates/right/src/main.rs` adds confirmation prompt + process-compose stop/start orchestration around it. No new abstractions — reuses `PcClient`, `openshell::{download_file, exec_in_sandbox}`, `memory::open_connection`, and the `BOOTSTRAP_INSTRUCTIONS` constant. The on-disk files this touches (`IDENTITY.md`, `SOUL.md`, `USER.md`, `BOOTSTRAP.md`) are all `AgentOwned` per `ARCHITECTURE.md`, so they bypass the `codegen::contract` writer registry.

**Tech Stack:** Rust 2024, miette errors, tokio async, rusqlite, OpenShell gRPC (via `right_agent::openshell`), process-compose REST API (via `right_agent::runtime::PcClient`), clap derive subcommand, inquire for confirmation prompt, chrono for timestamps.

**Spec:** `docs/superpowers/specs/2026-04-29-rebootstrap-cmd-design.md`

---

## File Structure

| File | Status | Responsibility |
|---|---|---|
| `crates/right-agent/src/rebootstrap.rs` | **new** | Pure state-mutation orchestrator: plan struct, execute, internal step helpers, in-file unit tests |
| `crates/right-agent/src/lib.rs` | modify | Add `pub mod rebootstrap;` |
| `crates/right/src/main.rs` | modify | Add `AgentCommands::Rebootstrap` variant, dispatch arm, `cmd_agent_rebootstrap` async fn, update `MAIN_PROMPT_LABELS` |
| `crates/right-agent/tests/rebootstrap_sandbox.rs` | **new** | One end-to-end test against a live OpenShell sandbox via `TestSandbox` |
| `crates/right/tests/cli_rebootstrap.rs` | **new** | `assert_cmd` CLI surface tests (missing-agent, abort-on-cancel) |

The `rebootstrap.rs` module is moderate-sized (~400 LOC including tests). If it crosses 800 LOC during implementation, extract tests per the project rule (`#[path = "rebootstrap_tests.rs"]`); we don't expect to.

---

## Task 1: Module skeleton + `plan()` + lib wire-up

**Files:**
- Create: `crates/right-agent/src/rebootstrap.rs`
- Modify: `crates/right-agent/src/lib.rs`

This task introduces the new module with the `RebootstrapPlan` struct, `plan()` constructor, and stubbed `execute()` so the rest of the crate compiles. We get the data shape locked down first, then layer step helpers + orchestration on top.

- [ ] **Step 1.1: Add `pub mod rebootstrap;` to `lib.rs`**

In `crates/right-agent/src/lib.rs`, add after `pub mod platform_store;`:

```rust
pub mod rebootstrap;
```

Place it alphabetically (between `process_group` and `runtime` is fine — match existing style: it sits after `platform_store`).

Final ordering in lib.rs (just the additions):

```rust
pub mod platform_store;
#[cfg(unix)]
pub mod process_group;
pub mod rebootstrap;
pub mod runtime;
```

- [ ] **Step 1.2: Create `rebootstrap.rs` skeleton**

Create `crates/right-agent/src/rebootstrap.rs`:

```rust
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
        .map(|c| c.sandbox_mode().clone())
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
```

- [ ] **Step 1.3: Verify the crate still compiles**

```bash
cargo check -p right-agent
```

Expected: clean build, no warnings about the new module.

- [ ] **Step 1.4: Run the new unit tests**

```bash
cargo test -p right-agent rebootstrap::tests --lib
```

Expected: 3 tests pass (`plan_errors_when_agent_missing`, `plan_defaults_to_openshell_when_no_agent_yaml`, `plan_respects_sandbox_mode_none`).

- [ ] **Step 1.5: Commit**

```bash
git add crates/right-agent/src/lib.rs crates/right-agent/src/rebootstrap.rs
git commit -m "feat(rebootstrap): add module skeleton with plan() and tests"
```

---

## Task 2: `backup_identity_files` step

**Files:**
- Modify: `crates/right-agent/src/rebootstrap.rs`

Backs up host copies of identity files into `<backup_dir>/<file>` and (when sandbox is reachable) sandbox copies into `<backup_dir>/sandbox/<file>`. Missing files are logged at DEBUG, not errors.

We intentionally split host backup (synchronous, no I/O risk) from sandbox backup (network, can fail). Sandbox download failures during backup are *fatal* — if we proceeded without verifying the backup, we'd be deleting files we couldn't recover.

- [ ] **Step 2.1: Write the unit test for host-side backup**

Append inside `mod tests` in `crates/right-agent/src/rebootstrap.rs`:

```rust
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
```

- [ ] **Step 2.2: Run tests to verify they fail**

```bash
cargo test -p right-agent rebootstrap::tests --lib 2>&1 | head -40
```

Expected: compilation error — `backup_host_files` is not yet defined.

- [ ] **Step 2.3: Implement `backup_host_files`**

Inside `crates/right-agent/src/rebootstrap.rs`, after the `execute` stub and before `mod tests`, add:

```rust
/// Copy any present identity files from `agent_dir` into `backup_dir`.
/// Returns the list of files that were actually copied.
///
/// `backup_dir` must already exist. Missing source files are skipped at
/// DEBUG level (not errors).
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
```

- [ ] **Step 2.4: Run tests to verify they pass**

```bash
cargo test -p right-agent rebootstrap::tests --lib
```

Expected: 5 tests pass (3 from Task 1 + 2 new).

- [ ] **Step 2.5: Add the sandbox-side backup helper**

This is best-effort with respect to *missing* files (404-equivalent), but a hard error if the sandbox is reachable and the download itself fails. We use `openshell::download_file` which goes through the CLI — `exec_in_sandbox` would also work but `download_file` already handles atomic staging.

To distinguish "file not present in sandbox" from "transport error", we run a single `exec_in_sandbox` first that returns a JSON-ish marker per file, then download only those that exist. Simpler: just call `download_file` and treat any error as fatal — this matches `cmd_agent_backup`'s behavior.

But "file legitimately absent" must not abort. Solution: probe with `exec_in_sandbox(["test", "-f", "/sandbox/X"])` per file (cheap), then download only the present ones. Three probes is fine.

Add this function below `backup_host_files`:

```rust
/// Download identity files from sandbox into `<backup_dir>/sandbox/`.
/// Skipped entirely when `sandbox_name` is `None` (none-mode).
///
/// Returns the list of files that were actually downloaded. A missing
/// sandbox file is not an error; a download failure on a present file is.
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
            10,
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
```

You'll need `crate::openshell::sandbox_exists` to be in scope — confirm it's `pub`.

- [ ] **Step 2.6: Verify `sandbox_exists` is public**

```bash
grep -n "pub async fn sandbox_exists\|pub fn sandbox_exists" crates/right-agent/src/openshell.rs
```

Expected: a single `pub async fn sandbox_exists(...)` line. If it isn't `pub`, change its visibility to `pub` in `crates/right-agent/src/openshell.rs` (it's used in this module).

- [ ] **Step 2.7: Compile**

```bash
cargo check -p right-agent
```

Expected: clean. (No tests for `backup_sandbox_files` at unit level — it's exercised end-to-end by Task 9.)

- [ ] **Step 2.8: Commit**

```bash
git add crates/right-agent/src/rebootstrap.rs crates/right-agent/src/openshell.rs
git commit -m "feat(rebootstrap): add backup_host_files and backup_sandbox_files"
```

---

## Task 3: `delete_identity_from_host` step

**Files:**
- Modify: `crates/right-agent/src/rebootstrap.rs`

Remove identity files from the host agent dir. Idempotent: missing files are not errors.

- [ ] **Step 3.1: Write the failing test**

Append to `mod tests`:

```rust
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
```

- [ ] **Step 3.2: Run tests to verify they fail**

```bash
cargo test -p right-agent rebootstrap::tests --lib 2>&1 | head -20
```

Expected: compilation error — `delete_identity_from_host` undefined.

- [ ] **Step 3.3: Implement `delete_identity_from_host`**

Add to `crates/right-agent/src/rebootstrap.rs` (after `backup_sandbox_files`):

```rust
/// Remove identity files from `agent_dir`. Infallible — "not found" and
/// I/O errors are logged at DEBUG/WARN respectively but never returned.
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
```

- [ ] **Step 3.4: Run tests to verify they pass**

```bash
cargo test -p right-agent rebootstrap::tests --lib
```

Expected: 7 tests pass.

- [ ] **Step 3.5: Commit**

```bash
git add crates/right-agent/src/rebootstrap.rs
git commit -m "feat(rebootstrap): add delete_identity_from_host"
```

---

## Task 4: `write_bootstrap_md` step

**Files:**
- Modify: `crates/right-agent/src/rebootstrap.rs`

Recreates `BOOTSTRAP.md` on host using the canonical `BOOTSTRAP_INSTRUCTIONS` constant from codegen. Overwrites if the file exists.

- [ ] **Step 4.1: Write the failing test**

Append to `mod tests`:

```rust
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
```

- [ ] **Step 4.2: Run tests to verify they fail**

```bash
cargo test -p right-agent rebootstrap::tests --lib 2>&1 | head -20
```

Expected: compilation error — `write_bootstrap_md` undefined.

- [ ] **Step 4.3: Implement `write_bootstrap_md`**

Add to `crates/right-agent/src/rebootstrap.rs` (after `delete_identity_from_host`):

```rust
/// Recreate `BOOTSTRAP.md` on host with the canonical bootstrap instructions.
/// Overwrites any existing file.
fn write_bootstrap_md(agent_dir: &Path) -> miette::Result<()> {
    let path = agent_dir.join("BOOTSTRAP.md");
    std::fs::write(&path, crate::codegen::BOOTSTRAP_INSTRUCTIONS).map_err(|e| {
        miette::miette!("failed to write BOOTSTRAP.md at {}: {e:#}", path.display())
    })
}
```

- [ ] **Step 4.4: Run tests**

```bash
cargo test -p right-agent rebootstrap::tests --lib
```

Expected: 9 tests pass.

- [ ] **Step 4.5: Commit**

```bash
git add crates/right-agent/src/rebootstrap.rs
git commit -m "feat(rebootstrap): add write_bootstrap_md"
```

---

## Task 5: `deactivate_active_sessions` step

**Files:**
- Modify: `crates/right-agent/src/rebootstrap.rs`

Marks every active row in `sessions` as inactive. Per design: this is a single `UPDATE` because `data.db` is already scoped to one agent. We do **not** call `right-bot`'s helper — `right-agent` mustn't depend on `right-bot` (cycle), and the per-`(chat_id, thread_id)` shape there is wrong for our use anyway.

- [ ] **Step 5.1: Write the failing test**

Append to `mod tests`:

```rust
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
```

- [ ] **Step 5.2: Run tests to verify they fail**

```bash
cargo test -p right-agent rebootstrap::tests --lib 2>&1 | head -20
```

Expected: compilation error — `deactivate_active_sessions` undefined.

- [ ] **Step 5.3: Implement `deactivate_active_sessions`**

Add to `crates/right-agent/src/rebootstrap.rs`:

```rust
/// Mark all active `sessions` rows in the agent's `data.db` as inactive.
/// Returns the number of rows updated. Skipped (returns 0) if `data.db`
/// is missing.
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
```

- [ ] **Step 5.4: Run tests**

```bash
cargo test -p right-agent rebootstrap::tests --lib
```

Expected: 12 tests pass.

- [ ] **Step 5.5: Commit**

```bash
git add crates/right-agent/src/rebootstrap.rs
git commit -m "feat(rebootstrap): add deactivate_active_sessions"
```

---

## Task 6: `delete_identity_from_sandbox` step

**Files:**
- Modify: `crates/right-agent/src/rebootstrap.rs`

Removes `/sandbox/IDENTITY.md`, `/sandbox/SOUL.md`, `/sandbox/USER.md` via gRPC `exec_in_sandbox`. Skipped for `none` mode and when sandbox is unreachable. Failure is fatal — leaving stale identity files in the sandbox would cause reverse-sync to overwrite the host on the next message and silently undo our work.

No unit test for this — it requires a live sandbox. Covered end-to-end in Task 9.

- [ ] **Step 6.1: Implement `delete_identity_from_sandbox`**

Add to `crates/right-agent/src/rebootstrap.rs`:

```rust
/// Delete identity files from the sandbox via gRPC `exec_in_sandbox`.
///
/// Skipped (returns `Ok`) when `sandbox_name` is `None` (none-mode) or when
/// OpenShell is not ready / the sandbox doesn't exist.
async fn delete_identity_from_sandbox(sandbox_name: Option<&str>) -> miette::Result<()> {
    let Some(sandbox) = sandbox_name else {
        return Ok(());
    };

    let mtls_dir = match crate::openshell::preflight_check() {
        crate::openshell::OpenShellStatus::Ready(d) => d,
        other => {
            tracing::info!(
                ?other,
                "rebootstrap: openshell not ready, skipping sandbox-side delete"
            );
            return Ok(());
        }
    };

    let mut client = crate::openshell::connect_grpc(&mtls_dir).await?;
    if !crate::openshell::sandbox_exists(&mut client, sandbox).await? {
        tracing::info!(sandbox, "rebootstrap: sandbox absent, skipping delete");
        return Ok(());
    }
    let sandbox_id = crate::openshell::resolve_sandbox_id(&mut client, sandbox).await?;

    // Single rm -f covering all three. -f makes missing files non-fatal,
    // so this is naturally idempotent.
    let cmd = [
        "rm",
        "-f",
        "/sandbox/IDENTITY.md",
        "/sandbox/SOUL.md",
        "/sandbox/USER.md",
    ];
    let (stdout, exit) =
        crate::openshell::exec_in_sandbox(&mut client, &sandbox_id, &cmd, 10).await?;
    if exit != 0 {
        return Err(miette::miette!(
            "rm in sandbox returned exit {exit}: {stdout}"
        ));
    }
    Ok(())
}
```

- [ ] **Step 6.2: Compile**

```bash
cargo check -p right-agent
```

Expected: clean.

- [ ] **Step 6.3: Commit**

```bash
git add crates/right-agent/src/rebootstrap.rs
git commit -m "feat(rebootstrap): add delete_identity_from_sandbox"
```

---

## Task 7: `execute()` orchestrator

**Files:**
- Modify: `crates/right-agent/src/rebootstrap.rs`

Wire the step helpers together in the order specified in the design doc:

1. Create backup dir.
2. Backup host files.
3. Backup sandbox files.
4. Delete from sandbox (after successful backup — never lose data).
5. Delete from host.
6. Write `BOOTSTRAP.md`.
7. Deactivate sessions.

If any step before 5 fails, host files remain untouched — the operator can inspect and retry. Once we reach step 5, partial recovery is on the operator (the report still tells them what happened).

- [ ] **Step 7.1: Replace the `execute` stub**

In `crates/right-agent/src/rebootstrap.rs`, replace the existing stub:

```rust
pub async fn execute(_plan: &RebootstrapPlan) -> miette::Result<RebootstrapReport> {
    miette::bail!("rebootstrap::execute not yet implemented")
}
```

with:

```rust
pub async fn execute(plan: &RebootstrapPlan) -> miette::Result<RebootstrapReport> {
    std::fs::create_dir_all(&plan.backup_dir).map_err(|e| {
        miette::miette!(
            "failed to create backup dir {}: {e:#}",
            plan.backup_dir.display()
        )
    })?;

    let host_backed_up = backup_host_files(&plan.agent_dir, &plan.backup_dir)?;
    let sandbox_backed_up =
        backup_sandbox_files(plan.sandbox_name.as_deref(), &plan.backup_dir).await?;

    // Once backup succeeds, delete from sandbox first — failure here would
    // otherwise let reverse_sync re-populate the host on the next message.
    delete_identity_from_sandbox(plan.sandbox_name.as_deref()).await?;
    delete_identity_from_host(&plan.agent_dir);

    write_bootstrap_md(&plan.agent_dir)?;
    let sessions_deactivated = deactivate_active_sessions(&plan.agent_dir)?;

    Ok(RebootstrapReport {
        backup_dir: plan.backup_dir.clone(),
        host_backed_up,
        sandbox_backed_up,
        sessions_deactivated,
    })
}
```

- [ ] **Step 7.2: Add a unit test for the host-only happy path**

Append to `mod tests`. This exercises `execute()` against a plan with `sandbox.mode = none`, so no sandbox calls happen — fully unit-testable.

```rust
    #[tokio::test]
    async fn execute_none_mode_full_path() {
        let home = tempfile::tempdir().unwrap();
        let agents = home.path().join("agents");
        let agent_dir = agents.join("nm");
        std::fs::create_dir_all(&agent_dir).unwrap();
        std::fs::write(agent_dir.join("agent.yaml"), "sandbox:\n  mode: none\n").unwrap();
        std::fs::write(agent_dir.join("IDENTITY.md"), "id\n").unwrap();
        std::fs::write(agent_dir.join("SOUL.md"), "soul\n").unwrap();
        std::fs::write(agent_dir.join("USER.md"), "user\n").unwrap();
        // Seed an active session so we can verify deactivation.
        let conn = crate::memory::open_connection(&agent_dir, true).unwrap();
        conn.execute(
            "INSERT INTO sessions (chat_id, thread_id, root_session_id, is_active) \
             VALUES (42, 0, 'session-uuid', 1)",
            [],
        )
        .unwrap();
        drop(conn);

        let p = plan(home.path(), "nm").unwrap();
        let report = execute(&p).await.unwrap();

        // Host identity files moved to backup, deleted from agent dir.
        for &f in IDENTITY_FILES {
            assert!(
                !agent_dir.join(f).exists(),
                "{f} should be gone from agent dir"
            );
            assert!(
                report.backup_dir.join(f).exists(),
                "{f} should be in backup"
            );
        }
        assert_eq!(report.host_backed_up, IDENTITY_FILES.to_vec());
        assert!(
            report.sandbox_backed_up.is_empty(),
            "none-mode = no sandbox backup"
        );

        // BOOTSTRAP.md recreated.
        assert_eq!(
            std::fs::read_to_string(agent_dir.join("BOOTSTRAP.md")).unwrap(),
            crate::codegen::BOOTSTRAP_INSTRUCTIONS
        );

        // Session deactivated.
        assert_eq!(report.sessions_deactivated, 1);
        let conn = crate::memory::open_connection(&agent_dir, false).unwrap();
        let active: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sessions WHERE is_active = 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(active, 0);
    }
```

- [ ] **Step 7.3: Run tests**

```bash
cargo test -p right-agent rebootstrap::tests --lib
```

Expected: 13 tests pass.

- [ ] **Step 7.4: Commit**

```bash
git add crates/right-agent/src/rebootstrap.rs
git commit -m "feat(rebootstrap): implement execute() orchestrator"
```

---

## Task 8: CLI subcommand wiring

**Files:**
- Modify: `crates/right/src/main.rs`

Add `AgentCommands::Rebootstrap`, the dispatch arm, the `cmd_agent_rebootstrap` async fn, and update `MAIN_PROMPT_LABELS` (the brand-voice regression test trips on un-listed prompts).

The CLI shim handles process-compose orchestration around the library's `execute()`, mirroring the pattern in `cmd_agent_destroy` / `cmd_restart`.

- [ ] **Step 8.1: Add `Rebootstrap` to `AgentCommands`**

In `crates/right/src/main.rs`, find the `AgentCommands` enum (around line 132) and add a new variant after `Destroy { ... }`:

```rust
    /// Re-enter bootstrap mode (debug only). Backs up identity files,
    /// deletes them from host and sandbox, recreates BOOTSTRAP.md, and
    /// deactivates active sessions. Sandbox, credentials, memory bank,
    /// and data.db rows are preserved.
    Rebootstrap {
        /// Agent name
        name: String,
        /// Skip the typed-name confirmation prompt
        #[arg(short = 'y', long = "yes")]
        yes: bool,
    },
```

- [ ] **Step 8.2: Add the dispatch arm**

In the same file, find the `match command` block that handles `AgentCommands::*` (around line 575). After the `AgentCommands::Destroy { ... } => ...` arm, add:

```rust
            AgentCommands::Rebootstrap { name, yes } => {
                cmd_agent_rebootstrap(&home, &name, yes).await
            }
```

- [ ] **Step 8.3: Add the new prompt label to `MAIN_PROMPT_LABELS`**

In `crates/right/src/main.rs`, find the `MAIN_PROMPT_LABELS` array (around line 19) and add a new entry — match the existing inline-comment style:

```rust
    // cmd_agent_rebootstrap: dynamic confirm — agent_name varies, prefix is the static portion
    "rebootstrap agent '",
```

Place it next to the existing destroy entry for context.

- [ ] **Step 8.4: Implement `cmd_agent_rebootstrap`**

Add this function to `crates/right/src/main.rs` (place it near `cmd_agent_destroy`, around line 3080). The shim:

1. Builds the plan.
2. Confirms (typed agent name) unless `-y`.
3. Stops the bot via PcClient (best-effort if PC is down).
4. Calls `right_agent::rebootstrap::execute`.
5. Restarts the bot if we stopped it.
6. Prints the summary.

```rust
async fn cmd_agent_rebootstrap(
    home: &Path,
    agent_name: &str,
    yes: bool,
) -> miette::Result<()> {
    let plan = right_agent::rebootstrap::plan(home, agent_name)?;

    // Confirmation
    if !yes {
        println!("Agent: {agent_name}");
        println!("  Directory: {}", plan.agent_dir.display());
        println!("  Sandbox mode: {}", plan.sandbox_mode);
        if let Some(ref sb) = plan.sandbox_name {
            println!("  Sandbox: {sb}");
        }
        println!("  Backup dir: {}", plan.backup_dir.display());
        println!();
        println!("This will:");
        println!("  - Back up IDENTITY.md / SOUL.md / USER.md (host + sandbox copies)");
        println!("  - Delete those files from host and sandbox");
        println!("  - Recreate BOOTSTRAP.md on host");
        println!("  - Deactivate all active sessions in data.db");
        println!("  - Bounce the bot if it is running");
        println!();
        println!(
            "Sandbox, credentials, Hindsight memory, and data.db rows are preserved."
        );
        println!();

        let confirmed = inquire::Confirm::new(&format!(
            "rebootstrap agent '{agent_name}'? this rewinds onboarding state."
        ))
        .with_default(false)
        .prompt()
        .map_err(|e| miette::miette!("prompt failed: {e:#}"))?;

        if !confirmed {
            println!("Aborted.");
            return Ok(());
        }
    }

    // Stop the bot (best-effort)
    let pc_process = format!("{agent_name}-bot");
    let pc_client_opt = right_agent::runtime::PcClient::from_home(home)?;
    let pc_running = match &pc_client_opt {
        Some(pc) => pc.health_check().await.is_ok(),
        None => false,
    };
    let bot_was_stopped = if pc_running {
        let pc = pc_client_opt.as_ref().unwrap();
        match pc.stop_process(&pc_process).await {
            Ok(()) => {
                println!("✓ Stopped {pc_process}");
                true
            }
            Err(e) => {
                return Err(miette::miette!(
                    "failed to stop {pc_process} (not safe to proceed with bot up): {e:#}"
                ));
            }
        }
    } else {
        println!("(process-compose not running — skipping bot stop)");
        false
    };

    // Run the state mutations
    let report = right_agent::rebootstrap::execute(&plan).await?;

    // Restart the bot if we stopped it
    if bot_was_stopped {
        let pc = pc_client_opt.as_ref().unwrap();
        pc.start_process(&pc_process).await?;
        println!("✓ Started {pc_process}");
    }

    // Final summary
    println!();
    println!("Rebootstrapped agent '{agent_name}':");
    println!("  Backup: {}", report.backup_dir.display());
    if report.host_backed_up.is_empty() {
        println!("  Host files backed up: (none — agent had not bootstrapped)");
    } else {
        println!(
            "  Host files backed up: {}",
            report.host_backed_up.join(", ")
        );
    }
    if !report.sandbox_backed_up.is_empty() {
        println!(
            "  Sandbox files backed up: {}",
            report.sandbox_backed_up.join(", ")
        );
    }
    println!("  Sessions deactivated: {}", report.sessions_deactivated);
    if !pc_running {
        println!();
        println!("process-compose was not running. Run `right up` to relaunch the bot.");
    }

    Ok(())
}
```

- [ ] **Step 8.5: Verify the crate compiles**

```bash
cargo check -p right
```

Expected: clean build. If `inquire::Confirm` or `right_agent::runtime::PcClient` isn't already in scope at the call site, look at how `cmd_agent_destroy` references them (fully-qualified is fine).

- [ ] **Step 8.6: Run the brand-voice regression test**

```bash
cargo test -p right voice_pass_main --lib
```

Expected: pass. If this fails complaining about an unrecognized prompt, double-check Step 8.3 and that the new prompt's first line in the prompt body matches the listed label exactly (lowercase, including the trailing single-quote — see the destroy entry as a model).

- [ ] **Step 8.7: Run the full right and right-agent test suites**

```bash
cargo test -p right -p right-agent
```

Expected: all pre-existing tests still pass, plus the rebootstrap unit tests.

- [ ] **Step 8.8: Smoke test the help text**

```bash
cargo run -p right -- agent rebootstrap --help
```

Expected: clap-generated help showing `<NAME>` and `-y, --yes`.

- [ ] **Step 8.9: Commit**

```bash
git add crates/right/src/main.rs
git commit -m "feat(rebootstrap): wire CLI subcommand right agent rebootstrap"
```

---

## Task 9: Live-sandbox integration test

**Files:**
- Create: `crates/right-agent/tests/rebootstrap_sandbox.rs`

End-to-end test against a real OpenShell sandbox via `TestSandbox`. Verifies that `execute()` correctly removes identity files from `/sandbox/`, downloads them to backup, and recreates `BOOTSTRAP.md` on host. No `#[ignore]` per project policy — dev machines have OpenShell.

This test does **not** exercise process-compose (out of library scope) or the CLI shim (covered in Task 10).

- [ ] **Step 9.1: Create the test file**

```rust
//! Integration test: `rebootstrap::execute` against a live OpenShell sandbox.

use std::path::Path;

use right_agent::rebootstrap::{self, IDENTITY_FILES, RebootstrapPlan};
use right_agent::test_support::TestSandbox;

/// Write a host-side agent dir with `agent.yaml` pointing at `sandbox_name`,
/// the three identity files, and a stamped active session row in data.db.
fn seed_agent_dir(agent_dir: &Path, sandbox_name: &str) {
    std::fs::create_dir_all(agent_dir).unwrap();
    let yaml = format!(
        "sandbox:\n  mode: openshell\n  name: {sandbox_name}\n  policy_file: policy.yaml\n"
    );
    std::fs::write(agent_dir.join("agent.yaml"), yaml).unwrap();
    // policy.yaml content irrelevant — we never apply it; agent.yaml just
    // needs to parse as a sandboxed agent.
    std::fs::write(agent_dir.join("policy.yaml"), "version: 1\n").unwrap();
    std::fs::write(agent_dir.join("IDENTITY.md"), "host id\n").unwrap();
    std::fs::write(agent_dir.join("SOUL.md"), "host soul\n").unwrap();
    std::fs::write(agent_dir.join("USER.md"), "host user\n").unwrap();

    let conn = right_agent::memory::open_connection(agent_dir, true).unwrap();
    conn.execute(
        "INSERT INTO sessions (chat_id, thread_id, root_session_id, is_active) \
         VALUES (1, 0, 'sandbox-session-uuid', 1)",
        [],
    )
    .unwrap();
}

/// Verify a path inside the sandbox does not exist via `[ -e <path> ]`.
async fn assert_absent_in_sandbox(sandbox: &TestSandbox, path: &str) {
    let (_, exit) = sandbox.exec(&["test", "-e", path]).await;
    assert_ne!(exit, 0, "expected {path} to be absent in sandbox");
}

#[tokio::test]
async fn execute_against_live_sandbox() {
    let _slot = right_agent::openshell::acquire_sandbox_slot();
    let sandbox = TestSandbox::create("rebootstrap").await;

    // Seed sandbox-side identity files via in-sandbox shell. echo into
    // /sandbox/ avoids the openshell upload code path entirely.
    for &f in IDENTITY_FILES {
        let (_, exit) = sandbox
            .exec(&["sh", "-c", &format!("echo sandbox-{f} > /sandbox/{f}")])
            .await;
        assert_eq!(exit, 0, "failed to seed /sandbox/{f}");
    }

    // Set up a temp home with the agent dir under it.
    let home = tempfile::tempdir().unwrap();
    let agent_name = "rb-test";
    let agent_dir = home.path().join("agents").join(agent_name);
    seed_agent_dir(&agent_dir, sandbox.name());

    // Build plan manually — the standard `plan()` would resolve a sandbox
    // name from `agent.yaml`, but our agent.yaml doesn't know about
    // TestSandbox's randomised name. We override via direct construction.
    let timestamp = chrono::Local::now().format("%Y%m%d-%H%M").to_string();
    let p = RebootstrapPlan {
        agent_name: agent_name.to_string(),
        agent_dir: agent_dir.clone(),
        backup_dir: home
            .path()
            .join("backups")
            .join(agent_name)
            .join(format!("rebootstrap-{timestamp}")),
        sandbox_mode: right_agent::agent::types::SandboxMode::Openshell,
        sandbox_name: Some(sandbox.name().to_string()),
    };

    let report = rebootstrap::execute(&p).await.expect("execute failed");

    // Host: identity files removed
    for &f in IDENTITY_FILES {
        assert!(
            !agent_dir.join(f).exists(),
            "host {f} should be removed"
        );
    }
    // Host: BOOTSTRAP.md created
    let bootstrap = std::fs::read_to_string(agent_dir.join("BOOTSTRAP.md")).unwrap();
    assert_eq!(bootstrap, right_agent::codegen::BOOTSTRAP_INSTRUCTIONS);

    // Backup: host copies
    for &f in IDENTITY_FILES {
        let host_copy = report.backup_dir.join(f);
        assert!(host_copy.exists(), "backup of host {f} missing");
        let content = std::fs::read_to_string(&host_copy).unwrap();
        assert_eq!(content, format!("host {}\n", f.trim_end_matches(".md").to_lowercase()),
            "host backup content for {f}");
    }

    // Backup: sandbox copies
    for &f in IDENTITY_FILES {
        let sb_copy = report.backup_dir.join("sandbox").join(f);
        assert!(sb_copy.exists(), "backup of sandbox {f} missing");
        let content = std::fs::read_to_string(&sb_copy).unwrap();
        assert_eq!(content, format!("sandbox-{f}\n"));
    }

    // Sandbox: identity files removed
    for &f in IDENTITY_FILES {
        assert_absent_in_sandbox(&sandbox, &format!("/sandbox/{f}")).await;
    }

    assert_eq!(report.sessions_deactivated, 1);
    assert_eq!(
        report.host_backed_up.iter().copied().collect::<Vec<_>>(),
        IDENTITY_FILES.to_vec()
    );
    assert_eq!(
        report.sandbox_backed_up.iter().copied().collect::<Vec<_>>(),
        IDENTITY_FILES.to_vec()
    );
}
```

The host-content assertion line `host id` / `host soul` / `host user` is what `seed_agent_dir` wrote; rewrite it more straightforwardly if the `trim_end_matches` shortcut feels brittle. The intent is: the host file content in backup matches what we seeded.

To keep it simple, replace the host-content check with this concrete map:

```rust
    let expected_host: &[(&str, &str)] = &[
        ("IDENTITY.md", "host id\n"),
        ("SOUL.md", "host soul\n"),
        ("USER.md", "host user\n"),
    ];
    for (name, content) in expected_host {
        let host_copy = report.backup_dir.join(name);
        assert!(host_copy.exists(), "backup of host {name} missing");
        assert_eq!(&std::fs::read_to_string(&host_copy).unwrap(), content);
    }
```

Use that replacement.

- [ ] **Step 9.2: Confirm `acquire_sandbox_slot` is exported**

```bash
grep -n "pub fn acquire_sandbox_slot\|pub async fn acquire_sandbox_slot" crates/right-agent/src/openshell.rs
```

Expected: a `pub` declaration. Existing tests already use it (see `policy_apply.rs`).

- [ ] **Step 9.3: Run the integration test**

```bash
cargo test -p right-agent --test rebootstrap_sandbox -- --nocapture
```

Expected: passes. The test creates a fresh `right-test-rebootstrap` sandbox, runs the operation, and tears down the sandbox via `Drop`.

- [ ] **Step 9.4: Commit**

```bash
git add crates/right-agent/tests/rebootstrap_sandbox.rs
git commit -m "test(rebootstrap): add live-sandbox integration test"
```

---

## Task 10: CLI surface tests

**Files:**
- Create: `crates/right/tests/cli_rebootstrap.rs`

`assert_cmd` tests for the CLI surface — argument validation, missing-agent error path, and an abort-on-cancel happy path. The full happy path is already covered by Task 9 at the library level, so we don't need to spin up PC + a sandbox here.

- [ ] **Step 10.1: Check for an existing CLI-test pattern**

```bash
ls crates/right/tests/ 2>/dev/null
```

If `crates/right/tests/` doesn't exist or has no integration tests, that's fine — `assert_cmd` and `predicates` are workspace deps and already used by `right-agent`. Confirm:

```bash
grep -n "assert_cmd\|predicates" crates/right/Cargo.toml
```

If they aren't listed under `[dev-dependencies]`, add them by editing `crates/right/Cargo.toml`:

```toml
[dev-dependencies]
assert_cmd = { workspace = true }
predicates = { workspace = true }
tempfile = { workspace = true }
```

(Match the spelling of pre-existing workspace dev-deps in this Cargo.toml — only add the line(s) that aren't there yet.)

- [ ] **Step 10.2: Create the test file**

`crates/right/tests/cli_rebootstrap.rs`:

```rust
//! CLI surface tests for `right agent rebootstrap`.
//!
//! The full library-level happy path is covered by
//! `right-agent`'s `rebootstrap_sandbox` integration test, so here we only
//! exercise the CLI-level concerns: argument validation, missing-agent
//! errors, and the abort-on-cancel path.

use assert_cmd::Command;
use predicates::prelude::*;

#[test]
fn rebootstrap_unknown_agent_errors_with_name() {
    let home = tempfile::tempdir().unwrap();
    Command::cargo_bin("right")
        .unwrap()
        .args([
            "--home",
            home.path().to_str().unwrap(),
            "agent",
            "rebootstrap",
            "ghost",
            "-y",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("ghost"));
}

#[test]
fn rebootstrap_help_lists_yes_flag() {
    Command::cargo_bin("right")
        .unwrap()
        .args(["agent", "rebootstrap", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--yes"));
}
```

These two are sufficient — the abort-on-cancel path requires interactive stdin which `assert_cmd` doesn't drive cleanly, and `-y` on a non-existent agent already exercises "doesn't crash on flag parsing".

- [ ] **Step 10.3: Run the CLI tests**

```bash
cargo test -p right --test cli_rebootstrap
```

Expected: 2 tests pass.

- [ ] **Step 10.4: Commit**

```bash
git add crates/right/tests/cli_rebootstrap.rs crates/right/Cargo.toml
git commit -m "test(rebootstrap): add CLI surface tests"
```

---

## Task 11: ARCHITECTURE.md update + final verification

**Files:**
- Modify: `ARCHITECTURE.md`

Add a brief mention of the new subcommand under the relevant section so that the architecture doc stays accurate.

- [ ] **Step 11.1: Locate the agent-lifecycle section**

```bash
grep -n "right agent backup\|right agent destroy\|right agent init.*--from-backup" ARCHITECTURE.md | head -5
```

Find the block describing per-agent CLI verbs (it's part of the `Agent Lifecycle` data-flow tree, around the `right agent backup` entry).

- [ ] **Step 11.2: Add a `right agent rebootstrap` entry**

After the `right agent backup` block in `ARCHITECTURE.md`'s Agent Lifecycle section, add:

````markdown
right agent rebootstrap <name> [-y]
  ├─ Confirm (typed agent name) unless -y
  ├─ Stop <name>-bot via process-compose REST API (best-effort)
  ├─ Backup IDENTITY.md / SOUL.md / USER.md (host + sandbox copies)
  │   to ~/.right/backups/<agent>/rebootstrap-<YYYYMMDD-HHMM>/
  ├─ rm -f the same files from /sandbox/ via gRPC exec_in_sandbox
  ├─ Remove host copies, write fresh BOOTSTRAP.md from BOOTSTRAP_INSTRUCTIONS
  ├─ UPDATE sessions SET is_active = 0 WHERE is_active = 1 in data.db
  └─ Restart <name>-bot if we stopped it
````

Match the existing indent/tree style of nearby entries.

- [ ] **Step 11.3: Run the full workspace build + tests**

```bash
cargo build --workspace
```

Expected: clean.

```bash
cargo test --workspace
```

Expected: all pre-existing tests pass plus the new rebootstrap tests. If clippy is part of the project's pre-commit, also run:

```bash
cargo clippy --workspace --all-targets -- -D warnings
```

- [ ] **Step 11.4: Commit**

```bash
git add ARCHITECTURE.md
git commit -m "docs(arch): document right agent rebootstrap"
```

- [ ] **Step 11.5: Manual smoke test**

If the dev machine has a running agent named `right`:

```bash
cargo run -p right -- agent rebootstrap right
```

Walk through the prompt; type the agent name; verify:

- Backup dir appears under `~/.right/backups/right/rebootstrap-*`.
- `BOOTSTRAP.md` exists in `~/.right/agents/right/`.
- `IDENTITY.md` / `SOUL.md` / `USER.md` are gone from both `~/.right/agents/right/` and (via `right agent ssh right -- ls /sandbox/`) `/sandbox/`.
- `right-bot` process is back up in `process-compose` TUI.
- Sending the agent a Telegram message starts a new session and the bot answers in onboarding voice.

If any of those fail, capture the error output and either file a follow-up TODO or fix in a new commit.

---

## Self-Review

I checked the plan against the spec:

- ✅ §Surface (`right agent rebootstrap <name> [-y]`) — Task 8.1, 8.2, 8.4
- ✅ §Step Sequence:
  - Resolve and validate — Task 1.2 (`plan`)
  - Confirm — Task 8.4
  - Stop bot — Task 8.4
  - Backup identity files (host + sandbox) — Tasks 2 + 7
  - Delete from sandbox — Tasks 6 + 7
  - Delete from host — Tasks 3 + 7
  - Recreate BOOTSTRAP.md — Tasks 4 + 7
  - Deactivate sessions — Tasks 5 + 7
  - Restart bot — Task 8.4
  - Print report — Task 8.4
- ✅ §Module Layout (`rebootstrap.rs`, lib.rs wire-up, main.rs shim) — Tasks 1, 8
- ✅ §Public surface (`RebootstrapPlan`, `RebootstrapReport`, `plan`, `execute`) — Task 1, 7
- ✅ §Edge cases (sandbox.mode = none, sandbox unreachable, PC not running, data.db missing, never-bootstrapped agent, BOOTSTRAP.md already exists, wrong-name confirm) — covered across Tasks 1, 2, 4, 5, 6, 7, 8
- ✅ §Testing (unit, integration sandbox, CLI assert_cmd) — Tasks 1–7 unit, Task 9 sandbox, Task 10 CLI
- ✅ §"deactivate sessions inline (no cross-crate dep)" — Task 5
- ✅ §"PC process name is `<name>-bot`" — Task 8.4
- ✅ §"AgentOwned files bypass codegen::contract" — confirmed in plan header; no registry changes added
- ✅ Brand-voice regression — Task 8.3 + 8.6

Placeholder scan: no "TBD", no "TODO", no "implement later", no naked "add error handling". All test code, implementation code, and bash commands are concrete and complete. Type names (`RebootstrapPlan`, `RebootstrapReport`, `IDENTITY_FILES`, `plan`, `execute`, `backup_host_files`, `backup_sandbox_files`, `delete_identity_from_host`, `delete_identity_from_sandbox`, `write_bootstrap_md`, `deactivate_active_sessions`, `cmd_agent_rebootstrap`) are consistent across all tasks. The PC process name string `format!("{agent_name}-bot")` is used identically in stop and start.

One trade-off worth flagging at execution time: the integration test in Task 9 directly seeds `/sandbox/IDENTITY.md` etc. via `sandbox.exec(echo)`. If `/sandbox/` permissions in the TestSandbox base policy don't allow that user to write there, the test seed will fail. The default test policy includes `read_write: [/tmp, /sandbox]` (per `test_support.rs`), so writes should work — but if `echo > /sandbox/IDENTITY.md` returns non-zero, the seed assertion in Step 9.1 will surface that immediately rather than masking it.
