# Restore Implicit Bindings Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make backup restore preserve or intentionally rebind clone-sensitive implicit Hindsight memory bindings, while recording non-secret backup metadata and warning about explicit copied integrations.

**Architecture:** Add a focused `restore` helper module for manifest, legacy inference, binding resolution, YAML normalization decisions, and explicit-state warnings. Keep runtime Hindsight fallback unchanged; restore mutates copied `agent.yaml` before sandbox creation/codegen. Wire the helper into backup and restore paths in `crates/right/src/main.rs`.

**Tech Stack:** Rust 2024, `clap`, `serde`, `serde_json`, `serde_saphyr`, `rusqlite`, `chrono`, `inquire`, existing Right Agent config and wizard helpers.

---

## File Structure

- Create `crates/right/src/restore.rs`
  - Owns `backup.json` structs.
  - Resolves source agent from manifest or legacy backup path.
  - Resolves restore binding mode into a concrete action.
  - Detects explicit copied state from `agent.yaml` and `data.db`.
  - Builds warning strings and has unit tests.
- Modify `crates/right/src/main.rs`
  - Add `mod restore;`.
  - Add restore CLI flags to `AgentCommands::Init`.
  - Pass restore mode into direct and wizard restore calls.
  - Write `backup.json` during full backups.
  - Normalize restored `agent.yaml` before sandbox creation/codegen.
  - Print explicit copied-state warnings.
  - Add CLI parser tests.
- Modify `crates/right/src/wizard.rs`
  - Expose `update_agent_yaml_memory` as `pub(crate)` so restore can reuse the existing YAML writer instead of duplicating memory-block serialization.
- No architecture doc change is required unless implementation changes data flow beyond this plan. If it does, update `docs/architecture/lifecycle.md` in the same task that changes the flow.

Before implementation, run:

```bash
git status --short
```

Expected: existing unrelated modified files may be present. Do not stage or revert unrelated files.

---

### Task 1: Add Restore Helper Module Skeleton And Failing Unit Tests

**Files:**
- Create: `crates/right/src/restore.rs`
- Modify: `crates/right/src/main.rs`

- [ ] **Step 1: Add the module declaration**

In `crates/right/src/main.rs`, add this beside the other local modules:

```rust
mod restore;
```

- [ ] **Step 2: Create the restore module with data types and tests first**

Create `crates/right/src/restore.rs` with the following initial content:

```rust
use std::path::Path;

use chrono::{SecondsFormat, Utc};
use miette::IntoDiagnostic;
use right_agent::agent::types::{AgentConfig, MemoryProvider};
use serde::{Deserialize, Serialize};

pub(crate) const BACKUP_MANIFEST_FILENAME: &str = "backup.json";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RestoreBindingMode {
    DirectUnspecified,
    Interactive,
    PreserveSource,
    RebindToTarget,
    MemoryBankId(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RestoreMemoryAction {
    Noop,
    WriteBankId(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RestoreDecision {
    pub source_agent: Option<String>,
    pub memory_action: RestoreMemoryAction,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct BackupManifest {
    pub schema_version: u32,
    pub source_agent: String,
    pub created_at: String,
    pub sandbox_archive_root: String,
    pub memory: BackupMemoryManifest,
    pub explicit_state: ExplicitStateManifest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct BackupMemoryManifest {
    pub provider: String,
    pub bank_id_explicit: bool,
    pub resolved_bank_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub(crate) struct ExplicitStateManifest {
    pub has_telegram_token: bool,
    pub has_mcp_servers: bool,
    pub has_mcp_auth_tokens: bool,
    pub has_cron_specs: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RestoreSource {
    source_agent: Option<String>,
    resolved_bank_id: Option<String>,
}

pub(crate) fn build_backup_manifest(
    source_agent: &str,
    config: Option<&AgentConfig>,
    db_path: Option<&Path>,
) -> miette::Result<BackupManifest> {
    let memory = backup_memory_manifest(source_agent, config);
    let explicit_state = explicit_state_manifest(config, db_path)?;
    Ok(BackupManifest {
        schema_version: 1,
        source_agent: source_agent.to_string(),
        created_at: Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true),
        sandbox_archive_root: "sandbox".to_string(),
        memory,
        explicit_state,
    })
}

fn backup_memory_manifest(
    source_agent: &str,
    config: Option<&AgentConfig>,
) -> BackupMemoryManifest {
    let Some(memory) = config.and_then(|cfg| cfg.memory.as_ref()) else {
        return BackupMemoryManifest {
            provider: "file".to_string(),
            bank_id_explicit: false,
            resolved_bank_id: None,
        };
    };

    if memory.provider == MemoryProvider::Hindsight {
        BackupMemoryManifest {
            provider: "hindsight".to_string(),
            bank_id_explicit: memory.bank_id.is_some(),
            resolved_bank_id: Some(
                memory
                    .bank_id
                    .clone()
                    .unwrap_or_else(|| source_agent.to_string()),
            ),
        }
    } else {
        BackupMemoryManifest {
            provider: "file".to_string(),
            bank_id_explicit: memory.bank_id.is_some(),
            resolved_bank_id: None,
        }
    }
}

pub(crate) fn write_backup_manifest(
    backup_dir: &Path,
    manifest: &BackupManifest,
) -> miette::Result<()> {
    let path = backup_dir.join(BACKUP_MANIFEST_FILENAME);
    let json = serde_json::to_string_pretty(manifest)
        .into_diagnostic()
        .map_err(|e| miette::miette!("serialize backup manifest: {e:#}"))?;
    std::fs::write(&path, format!("{json}\n"))
        .into_diagnostic()
        .map_err(|e| miette::miette!("write {}: {e:#}", path.display()))
}

pub(crate) fn read_backup_manifest(
    backup_dir: &Path,
) -> miette::Result<Option<BackupManifest>> {
    let path = backup_dir.join(BACKUP_MANIFEST_FILENAME);
    if !path.exists() {
        return Ok(None);
    }
    let content = std::fs::read_to_string(&path)
        .into_diagnostic()
        .map_err(|e| miette::miette!("read {}: {e:#}", path.display()))?;
    let manifest = serde_json::from_str(&content)
        .into_diagnostic()
        .map_err(|e| miette::miette!("parse {}: {e:#}", path.display()))?;
    Ok(Some(manifest))
}

pub(crate) fn infer_legacy_source_agent(home: &Path, backup_dir: &Path) -> Option<String> {
    let root = home.join("backups").canonicalize().ok()?;
    let backup = backup_dir.canonicalize().ok()?;
    let rel = backup.strip_prefix(root).ok()?;
    let mut parts = rel.components();
    let source = parts.next()?.as_os_str().to_str()?.to_string();
    parts.next()?;
    if parts.next().is_some() {
        return None;
    }
    Some(source)
}

pub(crate) fn decide_restore(
    home: &Path,
    target_agent: &str,
    backup_dir: &Path,
    config: &AgentConfig,
    mode: RestoreBindingMode,
) -> miette::Result<RestoreDecision> {
    let manifest = read_backup_manifest(backup_dir)?;
    let source = resolve_source(home, backup_dir, manifest.as_ref());
    let memory_action = decide_memory_action(target_agent, config, &mode, &source)?;
    let explicit_state = if let Some(manifest) = manifest.as_ref() {
        manifest.explicit_state.clone()
    } else {
        explicit_state_manifest(Some(config), Some(&backup_dir.join("data.db")))?
    };
    let warnings = explicit_state_warnings(source.source_agent.as_deref(), target_agent, &explicit_state);
    Ok(RestoreDecision {
        source_agent: source.source_agent,
        memory_action,
        warnings,
    })
}

fn resolve_source(
    home: &Path,
    backup_dir: &Path,
    manifest: Option<&BackupManifest>,
) -> RestoreSource {
    if let Some(manifest) = manifest {
        return RestoreSource {
            source_agent: Some(manifest.source_agent.clone()),
            resolved_bank_id: manifest.memory.resolved_bank_id.clone(),
        };
    }

    let source_agent = infer_legacy_source_agent(home, backup_dir);
    RestoreSource {
        resolved_bank_id: source_agent.clone(),
        source_agent,
    }
}

fn decide_memory_action(
    target_agent: &str,
    config: &AgentConfig,
    mode: &RestoreBindingMode,
    source: &RestoreSource,
) -> miette::Result<RestoreMemoryAction> {
    let Some(memory) = config.memory.as_ref() else {
        return Ok(RestoreMemoryAction::Noop);
    };
    if memory.provider != MemoryProvider::Hindsight {
        return Ok(RestoreMemoryAction::Noop);
    }

    if let RestoreBindingMode::MemoryBankId(bank) = mode {
        return Ok(RestoreMemoryAction::WriteBankId(bank.clone()));
    }

    if memory.bank_id.is_some() {
        return Ok(RestoreMemoryAction::Noop);
    }

    if source.source_agent.as_deref() == Some(target_agent)
        && matches!(mode, RestoreBindingMode::DirectUnspecified | RestoreBindingMode::Interactive)
    {
        return Ok(RestoreMemoryAction::Noop);
    }

    match mode {
        RestoreBindingMode::PreserveSource => {
            let bank_id = source.resolved_bank_id.clone().ok_or_else(|| {
                miette::miette!(
                    help = "Use --memory-bank-id <id> or --rebind-to-target",
                    "cannot preserve source Hindsight bank because the backup source is unknown"
                )
            })?;
            Ok(RestoreMemoryAction::WriteBankId(bank_id))
        }
        RestoreBindingMode::RebindToTarget => Ok(RestoreMemoryAction::Noop),
        RestoreBindingMode::DirectUnspecified => Err(miette::miette!(
            help = "Use --preserve-source-bindings, --rebind-to-target, or --memory-bank-id <id>",
            "restoring Hindsight memory under a different or unknown agent name requires an explicit binding mode"
        )),
        RestoreBindingMode::Interactive => Err(miette::miette!(
            "interactive restore binding prompt was not resolved before restore decision"
        )),
        RestoreBindingMode::MemoryBankId(_) => unreachable!("handled above"),
    }
}

pub(crate) fn explicit_state_manifest(
    config: Option<&AgentConfig>,
    db_path: Option<&Path>,
) -> miette::Result<ExplicitStateManifest> {
    let has_telegram_token = config.and_then(|cfg| cfg.telegram_token.as_ref()).is_some();
    let Some(db_path) = db_path.filter(|path| path.exists()) else {
        return Ok(ExplicitStateManifest {
            has_telegram_token,
            ..ExplicitStateManifest::default()
        });
    };

    let conn = rusqlite::Connection::open(db_path)
        .into_diagnostic()
        .map_err(|e| miette::miette!("open {}: {e:#}", db_path.display()))?;

    Ok(ExplicitStateManifest {
        has_telegram_token,
        has_mcp_servers: table_has_rows(&conn, "mcp_servers")?,
        has_mcp_auth_tokens: table_has_rows(&conn, "auth_tokens")?,
        has_cron_specs: table_has_rows(&conn, "cron_specs")?,
    })
}

fn table_has_rows(conn: &rusqlite::Connection, table: &str) -> miette::Result<bool> {
    let exists: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
            [table],
            |row| row.get(0),
        )
        .into_diagnostic()
        .map_err(|e| miette::miette!("inspect sqlite schema: {e:#}"))?;
    if exists == 0 {
        return Ok(false);
    }

    let sql = format!("SELECT EXISTS(SELECT 1 FROM {table} LIMIT 1)");
    conn.query_row(&sql, [], |row| row.get::<_, bool>(0))
        .into_diagnostic()
        .map_err(|e| miette::miette!("inspect sqlite table {table}: {e:#}"))
}

pub(crate) fn explicit_state_warnings(
    source_agent: Option<&str>,
    target_agent: &str,
    state: &ExplicitStateManifest,
) -> Vec<String> {
    if source_agent == Some(target_agent) {
        return Vec::new();
    }

    let mut warnings = Vec::new();
    if state.has_telegram_token {
        warnings.push("restored clone contains the source Telegram bot token; do not run source and clone together with the same token".to_string());
    }
    if state.has_cron_specs {
        warnings.push("restored clone contains cron specs; running source and clone can duplicate scheduled delivery".to_string());
    }
    if state.has_mcp_servers || state.has_mcp_auth_tokens {
        warnings.push("restored clone contains MCP server configuration or auth tokens; it may use the same third-party accounts as the source".to_string());
    }
    warnings
}

pub(crate) fn apply_memory_action(
    agent_yaml_path: &Path,
    mut config: AgentConfig,
    action: RestoreMemoryAction,
) -> miette::Result<()> {
    let RestoreMemoryAction::WriteBankId(bank_id) = action else {
        return Ok(());
    };
    let Some(memory) = config.memory.as_mut() else {
        return Ok(());
    };
    memory.bank_id = Some(bank_id);
    crate::wizard::update_agent_yaml_memory(agent_yaml_path, memory)
}

#[cfg(test)]
mod tests {
    use super::*;
    use right_agent::agent::types::{MemoryConfig, RecallBudget};
    use tempfile::tempdir;

    fn hindsight_config(bank_id: Option<&str>) -> AgentConfig {
        AgentConfig {
            memory: Some(MemoryConfig {
                provider: MemoryProvider::Hindsight,
                api_key: Some("hs_test".to_string()),
                bank_id: bank_id.map(ToString::to_string),
                recall_budget: RecallBudget::Mid,
                recall_max_tokens: 4096,
            }),
            ..AgentConfig::default()
        }
    }

    #[test]
    fn manifest_records_implicit_hindsight_bank_without_secret_key() {
        let config = hindsight_config(None);
        let manifest = build_backup_manifest("right", Some(&config), None).unwrap();

        assert_eq!(manifest.source_agent, "right");
        assert_eq!(manifest.memory.provider, "hindsight");
        assert!(!manifest.memory.bank_id_explicit);
        assert_eq!(manifest.memory.resolved_bank_id.as_deref(), Some("right"));

        let json = serde_json::to_string(&manifest).unwrap();
        assert!(!json.contains("hs_test"), "manifest must not contain api keys");
    }

    #[test]
    fn infers_legacy_source_agent_from_right_home_backup_layout() {
        let dir = tempdir().unwrap();
        let backup = dir.path().join("backups").join("right").join("20260516-0117");
        std::fs::create_dir_all(&backup).unwrap();

        assert_eq!(
            infer_legacy_source_agent(dir.path(), &backup).as_deref(),
            Some("right")
        );
    }

    #[test]
    fn direct_restore_requires_mode_for_cross_name_implicit_hindsight() {
        let dir = tempdir().unwrap();
        let backup = dir.path().join("elsewhere");
        std::fs::create_dir_all(&backup).unwrap();
        let config = hindsight_config(None);

        let err = decide_restore(
            dir.path(),
            "right-drill",
            &backup,
            &config,
            RestoreBindingMode::DirectUnspecified,
        )
        .expect_err("cross-name implicit Hindsight restore must require a mode");

        assert!(
            format!("{err:?}").contains("requires an explicit binding mode"),
            "unexpected error: {err:?}"
        );
    }

    #[test]
    fn preserve_mode_materializes_source_bank_from_legacy_path() {
        let dir = tempdir().unwrap();
        let backup = dir.path().join("backups").join("right").join("20260516-0117");
        std::fs::create_dir_all(&backup).unwrap();
        let config = hindsight_config(None);

        let decision = decide_restore(
            dir.path(),
            "right-drill",
            &backup,
            &config,
            RestoreBindingMode::PreserveSource,
        )
        .unwrap();

        assert_eq!(
            decision.memory_action,
            RestoreMemoryAction::WriteBankId("right".to_string())
        );
    }

    #[test]
    fn rebind_mode_leaves_implicit_bank_omitted() {
        let dir = tempdir().unwrap();
        let backup = dir.path().join("backups").join("right").join("20260516-0117");
        std::fs::create_dir_all(&backup).unwrap();
        let config = hindsight_config(None);

        let decision = decide_restore(
            dir.path(),
            "right-drill",
            &backup,
            &config,
            RestoreBindingMode::RebindToTarget,
        )
        .unwrap();

        assert_eq!(decision.memory_action, RestoreMemoryAction::Noop);
    }

    #[test]
    fn explicit_override_writes_supplied_bank() {
        let dir = tempdir().unwrap();
        let backup = dir.path().join("backup");
        std::fs::create_dir_all(&backup).unwrap();
        let config = hindsight_config(None);

        let decision = decide_restore(
            dir.path(),
            "right-drill",
            &backup,
            &config,
            RestoreBindingMode::MemoryBankId("manual-bank".to_string()),
        )
        .unwrap();

        assert_eq!(
            decision.memory_action,
            RestoreMemoryAction::WriteBankId("manual-bank".to_string())
        );
    }

    #[test]
    fn explicit_bank_is_preserved_without_override() {
        let dir = tempdir().unwrap();
        let backup = dir.path().join("backup");
        std::fs::create_dir_all(&backup).unwrap();
        let config = hindsight_config(Some("already-explicit"));

        let decision = decide_restore(
            dir.path(),
            "right-drill",
            &backup,
            &config,
            RestoreBindingMode::DirectUnspecified,
        )
        .unwrap();

        assert_eq!(decision.memory_action, RestoreMemoryAction::Noop);
    }

    #[test]
    fn explicit_state_warnings_only_for_clone_restore() {
        let state = ExplicitStateManifest {
            has_telegram_token: true,
            has_mcp_servers: true,
            has_mcp_auth_tokens: false,
            has_cron_specs: true,
        };

        assert!(explicit_state_warnings(Some("right"), "right", &state).is_empty());

        let warnings = explicit_state_warnings(Some("right"), "right-drill", &state);
        assert_eq!(warnings.len(), 3);
        assert!(warnings.iter().any(|w| w.contains("Telegram bot token")));
        assert!(warnings.iter().any(|w| w.contains("cron specs")));
        assert!(warnings.iter().any(|w| w.contains("MCP server")));
    }
}
```

- [ ] **Step 3: Run the targeted test and verify it fails for missing visibility**

Run:

```bash
devenv shell -- cargo test -p right restore::
```

Expected: FAIL because `crate::wizard::update_agent_yaml_memory` is private, and possibly because `mod restore;` was just added.

- [ ] **Step 4: Commit the failing-test slice only if the project convention allows red commits; otherwise keep it uncommitted and continue**

Preferred in this repo: do not commit a known failing slice. Continue to Task 2, then commit the green helper slice.

---

### Task 2: Expose Existing Memory YAML Helper And Make Restore Helper Tests Pass

**Files:**
- Modify: `crates/right/src/wizard.rs`
- Modify: `crates/right/src/restore.rs`

- [ ] **Step 1: Reuse the existing YAML writer**

In `crates/right/src/wizard.rs`, change only the memory writer visibility:

```rust
pub(crate) fn update_agent_yaml_memory(
    path: &Path,
    cfg: &right_agent::agent::types::MemoryConfig,
) -> miette::Result<()> {
```

Do not expose `remove_agent_yaml_memory`; it remains an implementation detail of the writer.

- [ ] **Step 2: Add the YAML mutation regression test**

Append this test to `crates/right/src/restore.rs` inside `mod tests`:

```rust
#[test]
fn apply_memory_action_writes_bank_id_and_preserves_recall_defaults() {
    let dir = tempdir().unwrap();
    let agent_yaml = dir.path().join("agent.yaml");
    std::fs::write(
        &agent_yaml,
        "model: \"sonnet\"\n\nmemory:\n  provider: hindsight\n  api_key: \"hs_test\"\n",
    )
    .unwrap();
    let config = hindsight_config(None);

    apply_memory_action(
        &agent_yaml,
        config,
        RestoreMemoryAction::WriteBankId("right".to_string()),
    )
    .unwrap();

    let content = std::fs::read_to_string(&agent_yaml).unwrap();
    assert!(content.contains("bank_id: \"right\""), "got:\n{content}");
    assert!(content.contains("api_key: \"hs_test\""), "got:\n{content}");
    assert!(
        !content.contains("recall_max_tokens"),
        "default recall_max_tokens should stay omitted, got:\n{content}"
    );
}
```

- [ ] **Step 3: Run targeted helper tests**

Run:

```bash
devenv shell -- cargo test -p right restore::
```

Expected: PASS for restore helper tests.

- [ ] **Step 4: Run existing wizard memory YAML tests**

Run:

```bash
devenv shell -- cargo test -p right memory_yaml_tests
```

Expected: PASS. This proves the visibility change did not alter behavior.

- [ ] **Step 5: Commit helper slice**

Run:

```bash
git add crates/right/src/main.rs crates/right/src/restore.rs crates/right/src/wizard.rs
git commit -m "test(restore): cover restore binding decisions"
```

Stage only these files.

---

### Task 3: Write Backup Manifest During Full Backups

**Files:**
- Modify: `crates/right/src/main.rs`
- Modify: `crates/right/src/restore.rs`

- [ ] **Step 1: Add a manifest write unit test**

Append this test to `crates/right/src/restore.rs` inside `mod tests`:

```rust
#[test]
fn write_and_read_backup_manifest_roundtrips() {
    let dir = tempdir().unwrap();
    let manifest = BackupManifest {
        schema_version: 1,
        source_agent: "right".to_string(),
        created_at: "2026-05-16T02:00:00Z".to_string(),
        sandbox_archive_root: "sandbox".to_string(),
        memory: BackupMemoryManifest {
            provider: "hindsight".to_string(),
            bank_id_explicit: false,
            resolved_bank_id: Some("right".to_string()),
        },
        explicit_state: ExplicitStateManifest {
            has_telegram_token: true,
            has_mcp_servers: true,
            has_mcp_auth_tokens: true,
            has_cron_specs: true,
        },
    };

    write_backup_manifest(dir.path(), &manifest).unwrap();
    let loaded = read_backup_manifest(dir.path()).unwrap().unwrap();

    assert_eq!(loaded, manifest);
    assert!(dir.path().join(BACKUP_MANIFEST_FILENAME).exists());
}
```

- [ ] **Step 2: Add explicit-state DB detection test**

Append this test to `crates/right/src/restore.rs` inside `mod tests`:

```rust
#[test]
fn explicit_state_manifest_detects_db_tables_without_reading_secrets() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("data.db");
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    conn.execute_batch(
        "CREATE TABLE mcp_servers (name TEXT);
         CREATE TABLE auth_tokens (token TEXT);
         CREATE TABLE cron_specs (job_name TEXT);
         INSERT INTO mcp_servers (name) VALUES ('composio');
         INSERT INTO auth_tokens (token) VALUES ('secret-token');
         INSERT INTO cron_specs (job_name) VALUES ('daily');",
    )
    .unwrap();

    let config = AgentConfig {
        telegram_token: Some("123:ABC".to_string()),
        ..AgentConfig::default()
    };

    let state = explicit_state_manifest(Some(&config), Some(&db_path)).unwrap();
    assert!(state.has_telegram_token);
    assert!(state.has_mcp_servers);
    assert!(state.has_mcp_auth_tokens);
    assert!(state.has_cron_specs);
}
```

- [ ] **Step 3: Run tests and verify the new tests pass before wiring**

Run:

```bash
devenv shell -- cargo test -p right restore::
```

Expected: PASS.

- [ ] **Step 4: Wire manifest writing into full backup**

In `crates/right/src/main.rs`, inside `cmd_agent_backup`, after the `data.db` VACUUM block and still inside `if !sandbox_only { ... }`, add:

```rust
        let manifest = restore::build_backup_manifest(
            agent_name,
            config.as_ref(),
            Some(&backup_dir.join("data.db")),
        )?;
        restore::write_backup_manifest(&backup_dir, &manifest)?;
        println!("backup.json written");
```

This uses the backed-up SQLite file so manifest inventory matches the artifact being restored.

- [ ] **Step 5: Run targeted backup/restore helper tests**

Run:

```bash
devenv shell -- cargo test -p right restore::
```

Expected: PASS.

- [ ] **Step 6: Commit manifest slice**

Run:

```bash
git add crates/right/src/main.rs crates/right/src/restore.rs
git commit -m "feat(backup): write non-secret restore manifest"
```

---

### Task 4: Add CLI Restore Binding Flags And Parser Tests

**Files:**
- Modify: `crates/right/src/main.rs`
- Modify: `crates/right/src/restore.rs`

- [ ] **Step 1: Add a non-empty string parser**

In `crates/right/src/main.rs`, add this helper near the CLI enum definitions:

```rust
fn non_empty_arg(value: &str) -> Result<String, String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        Err("value must not be empty".to_string())
    } else {
        Ok(trimmed.to_string())
    }
}
```

- [ ] **Step 2: Extend `AgentCommands::Init`**

In `crates/right/src/main.rs`, add these fields to the `Init` variant after `from_backup`:

```rust
        /// Preserve clone-sensitive external bindings from the source backup
        #[arg(
            long,
            requires = "from_backup",
            conflicts_with_all = ["rebind_to_target", "memory_bank_id"]
        )]
        preserve_source_bindings: bool,
        /// Rebind clone-sensitive implicit defaults to the target agent name
        #[arg(
            long,
            requires = "from_backup",
            conflicts_with_all = ["preserve_source_bindings", "memory_bank_id"]
        )]
        rebind_to_target: bool,
        /// Explicit Hindsight memory bank ID to write during restore
        #[arg(
            long,
            requires = "from_backup",
            value_parser = non_empty_arg,
            conflicts_with_all = ["preserve_source_bindings", "rebind_to_target"]
        )]
        memory_bank_id: Option<String>,
```

- [ ] **Step 3: Add conversion helper**

In `crates/right/src/main.rs`, add:

```rust
fn restore_binding_mode_from_flags(
    preserve_source_bindings: bool,
    rebind_to_target: bool,
    memory_bank_id: Option<String>,
) -> restore::RestoreBindingMode {
    if preserve_source_bindings {
        restore::RestoreBindingMode::PreserveSource
    } else if rebind_to_target {
        restore::RestoreBindingMode::RebindToTarget
    } else if let Some(bank_id) = memory_bank_id {
        restore::RestoreBindingMode::MemoryBankId(bank_id)
    } else {
        restore::RestoreBindingMode::DirectUnspecified
    }
}
```

- [ ] **Step 4: Update dispatch destructuring**

In the `Commands::Agent` match in `crates/right/src/main.rs`, update the `Init` destructuring and restore call:

```rust
            AgentCommands::Init {
                name,
                yes,
                force_recreate,
                fresh,
                network_policy,
                sandbox_mode,
                from_backup,
                preserve_source_bindings,
                rebind_to_target,
                memory_bank_id,
            } => {
                if let Some(backup_path) = from_backup {
                    let restore_binding_mode = restore_binding_mode_from_flags(
                        preserve_source_bindings,
                        rebind_to_target,
                        memory_bank_id,
                    );
                    cmd_agent_restore(&home, &name, &backup_path, restore_binding_mode).await
                } else {
                    cmd_agent_init(
                        &home,
                        &name,
                        yes,
                        force_recreate,
                        fresh,
                        network_policy,
                        sandbox_mode,
                    )
                }
            }
```

- [ ] **Step 5: Update `cmd_agent_restore` signature**

In `crates/right/src/main.rs`, change:

```rust
async fn cmd_agent_restore(
    home: &Path,
    agent_name: &str,
    backup_path: &Path,
    restore_binding_mode: restore::RestoreBindingMode,
) -> miette::Result<()> {
```

Do not use the new parameter yet; Task 5 wires behavior.

- [ ] **Step 6: Update the wizard restore call**

In `cmd_agent_init`, update the interactive restore path call to:

```rust
                    tokio::runtime::Handle::current().block_on(cmd_agent_restore(
                        home,
                        name,
                        &backup_path,
                        restore::RestoreBindingMode::Interactive,
                    ))
```

- [ ] **Step 7: Add CLI parser tests**

In `crates/right/src/main.rs`, inside `#[cfg(test)] mod tests`, add:

```rust
    use clap::Parser as _;

    #[test]
    fn restore_binding_flags_require_from_backup() {
        let err = Cli::try_parse_from([
            "right",
            "agent",
            "init",
            "right-drill",
            "--preserve-source-bindings",
        ])
        .expect_err("restore binding flags require --from-backup");

        assert_eq!(err.kind(), clap::error::ErrorKind::MissingRequiredArgument);
    }

    #[test]
    fn restore_binding_flags_conflict() {
        let err = Cli::try_parse_from([
            "right",
            "agent",
            "init",
            "right-drill",
            "--from-backup",
            "/tmp/backup",
            "--preserve-source-bindings",
            "--rebind-to-target",
        ])
        .expect_err("restore binding flags must conflict");

        assert_eq!(err.kind(), clap::error::ErrorKind::ArgumentConflict);
    }

    #[test]
    fn memory_bank_id_rejects_empty_value() {
        let err = Cli::try_parse_from([
            "right",
            "agent",
            "init",
            "right-drill",
            "--from-backup",
            "/tmp/backup",
            "--memory-bank-id",
            "   ",
        ])
        .expect_err("empty bank id must be rejected");

        assert_eq!(err.kind(), clap::error::ErrorKind::ValueValidation);
    }
```

- [ ] **Step 8: Run parser tests**

Run:

```bash
devenv shell -- cargo test -p right restore_binding_flags memory_bank_id_rejects_empty_value
```

Expected: PASS.

- [ ] **Step 9: Commit CLI slice**

Run:

```bash
git add crates/right/src/main.rs
git commit -m "feat(restore): add explicit binding mode flags"
```

---

### Task 5: Resolve Binding Mode, Normalize Agent YAML, And Print Warnings

**Files:**
- Modify: `crates/right/src/main.rs`
- Modify: `crates/right/src/restore.rs`

- [ ] **Step 1: Add interactive prompt resolver**

In `crates/right/src/main.rs`, add this helper near `restore_binding_mode_from_flags`:

```rust
fn prompt_restore_binding_mode() -> miette::Result<restore::RestoreBindingMode> {
    let options = vec![
        "preserve source bindings",
        "rebind to target",
        "set memory bank id",
    ];
    let choice = inquire::Select::new("restore binding mode:", options)
        .prompt()
        .map_err(|e| miette::miette!("prompt failed: {e:#}"))?;

    match choice {
        "preserve source bindings" => Ok(restore::RestoreBindingMode::PreserveSource),
        "rebind to target" => Ok(restore::RestoreBindingMode::RebindToTarget),
        "set memory bank id" => {
            let bank_id = inquire::Text::new("memory bank id:")
                .prompt()
                .map_err(|e| miette::miette!("prompt failed: {e:#}"))?;
            non_empty_arg(&bank_id)
                .map(restore::RestoreBindingMode::MemoryBankId)
                .map_err(|e| miette::miette!("{e}"))
        }
        other => Err(miette::miette!("unknown restore binding mode: {other}")),
    }
}
```

- [ ] **Step 2: Update prompt label list**

In `crates/right/src/main.rs`, add these strings to `MAIN_PROMPT_LABELS`:

```rust
    // cmd_agent_restore: implicit binding prompt
    "restore binding mode:",
    "preserve source bindings",
    "rebind to target",
    "set memory bank id",
    "memory bank id:",
```

- [ ] **Step 3: Add pre-normalization code in `cmd_agent_restore`**

In `crates/right/src/main.rs`, after parsing restored config and before `let is_sandboxed = ...`, replace the existing parse block with:

```rust
    // 3. Parse restored config, resolve restore binding semantics, then
    // normalize restored agent.yaml before codegen or sandbox creation can use it.
    let config = right_agent::agent::discovery::parse_agent_config(&agent_dir)?;
    let config = config.ok_or_else(|| {
        miette::miette!(
            "agent.yaml restored but parsed config is unavailable at {}",
            agent_dir.display()
        )
    })?;

    let effective_restore_binding_mode = if matches!(
        restore_binding_mode,
        restore::RestoreBindingMode::Interactive
    ) {
        prompt_restore_binding_mode()?
    } else {
        restore_binding_mode
    };

    let restore_decision = restore::decide_restore(
        home,
        agent_name,
        backup_path,
        &config,
        effective_restore_binding_mode,
    )?;

    restore::apply_memory_action(
        &agent_dir.join("agent.yaml"),
        config.clone(),
        restore_decision.memory_action,
    )?;

    for warning in restore_decision.warnings {
        eprintln!("warning: {warning}");
    }

    let config = right_agent::agent::discovery::parse_agent_config(&agent_dir)?;
    let is_sandboxed = config.as_ref().map(|c| c.is_sandboxed()).unwrap_or(true);
```

Remove the old duplicate `let config = ...` and `let is_sandboxed = ...` lines.

- [ ] **Step 4: Add an integration-style helper test for YAML normalization**

Append this test to `crates/right/src/restore.rs` inside `mod tests` if not already covered by Task 2:

```rust
#[test]
fn preserve_mode_normalizes_yaml_to_source_bank() {
    let dir = tempdir().unwrap();
    let backup = dir.path().join("backups").join("right").join("20260516-0117");
    std::fs::create_dir_all(&backup).unwrap();
    let agent_yaml = dir.path().join("agent.yaml");
    std::fs::write(
        &agent_yaml,
        "memory:\n  provider: hindsight\n  api_key: \"hs_test\"\n",
    )
    .unwrap();

    let config = hindsight_config(None);
    let decision = decide_restore(
        dir.path(),
        "right-drill",
        &backup,
        &config,
        RestoreBindingMode::PreserveSource,
    )
    .unwrap();
    apply_memory_action(&agent_yaml, config, decision.memory_action).unwrap();

    let content = std::fs::read_to_string(agent_yaml).unwrap();
    assert!(content.contains("bank_id: \"right\""), "got:\n{content}");
}
```

- [ ] **Step 5: Run targeted restore tests and prompt label tests**

Run:

```bash
devenv shell -- cargo test -p right restore:: main_prompt_labels
```

Expected: PASS.

- [ ] **Step 6: Commit restore behavior slice**

Run:

```bash
git add crates/right/src/main.rs crates/right/src/restore.rs
git commit -m "fix(restore): preserve implicit Hindsight bindings"
```

---

### Task 6: Update Lifecycle Docs If Flow Changed

**Files:**
- Modify if needed: `docs/architecture/lifecycle.md`

- [ ] **Step 1: Re-read restore section**

Run:

```bash
sed -n '88,118p' docs/architecture/lifecycle.md
```

Expected: shows current backup/restore flow.

- [ ] **Step 2: Patch the restore flow if it lacks manifest or binding normalization**

If the restore section still says only "restore config files" and "parse restored config", update it to include:

```markdown
  ├─ Read backup.json when present, or infer legacy source from ~/.right/backups/<agent>/<timestamp>
  ├─ Resolve restore binding mode for clone-sensitive implicit defaults
  ├─ Normalize restored agent.yaml before codegen/sandbox creation
  ├─ Warn when clone restore copies explicit external state (Telegram, MCP, cron)
```

- [ ] **Step 3: Commit docs slice only if changed**

Run:

```bash
git add docs/architecture/lifecycle.md
git commit -m "docs(restore): document implicit binding restore flow"
```

Expected: if no doc change was needed, skip this commit.

---

### Task 7: Local Restore Drill And Final Verification

**Files:**
- Runtime state only unless a bug is found.

- [ ] **Step 1: Run targeted package tests**

Run:

```bash
devenv shell -- cargo test -p right restore:: restore_binding_flags memory_bank_id_rejects_empty_value main_prompt_labels
```

Expected: PASS.

- [ ] **Step 2: Run full workspace tests**

Run:

```bash
devenv shell -- cargo test --workspace
```

Expected: PASS. If failures are unrelated and pre-existing, record exact failing tests and output. Do not claim completion without this command.

- [ ] **Step 3: Build debug workspace**

Run:

```bash
devenv shell -- cargo build --workspace
```

Expected: PASS. This is required by the Rust project instructions.

- [ ] **Step 4: UAT restore drill**

Use the existing known backup path for the `right` agent. Replace `<backup>` with the actual backup directory:

```bash
devenv shell -- right agent init right-drill --from-backup <backup> --preserve-source-bindings
```

Expected:

- restore completes
- restored `~/.right/agents/right-drill/agent.yaml` contains `memory.bank_id: "right"`
- warnings mention copied explicit state if the backup contains Telegram, MCP, or cron state

- [ ] **Step 5: Start restored agent and verify runtime**

Run the project-local start command already used for this worktree. Then inspect process logs for `right-drill-bot`.

Expected:

- no Hindsight error containing `Bank 'right-drill' not found`
- Hindsight startup uses the source bank
- Telegram response works
- MCP servers register
- restored sandbox is a new target sandbox, and durable backup files exist under `/sandbox`

- [ ] **Step 6: Stop if UAT reveals another backup or restore bug**

If UAT finds another restore correctness issue, stop and update the spec or write a follow-up spec before stacking more fixes. Do not hide the issue with manual sandbox or config edits.

- [ ] **Step 7: Final status and commit check**

Run:

```bash
git status --short
git log --oneline -5
```

Expected: only intentional implementation/docs changes remain, committed in the slices above. Unrelated pre-existing worktree changes may still be unstaged if they were present before this plan execution.

---

## Self-Review Result

- Spec coverage: manifest writing, legacy source inference, restore flags, preserve/rebind/override modes, YAML normalization, explicit copied-state warnings, UAT stop condition, targeted tests, full workspace tests, and debug build are covered.
- Scope: one subsystem, restore semantics, with backup manifest support required for cross-machine restore determinism.
- Type consistency: `RestoreBindingMode`, `RestoreMemoryAction`, `BackupManifest`, `BackupMemoryManifest`, and `ExplicitStateManifest` are defined before use.
- Placeholder scan: no pending implementation placeholders are intentionally left in this plan.
