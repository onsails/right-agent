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

pub(crate) fn read_backup_manifest(backup_dir: &Path) -> miette::Result<Option<BackupManifest>> {
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
    let warnings = explicit_state_warnings(
        source.source_agent.as_deref(),
        target_agent,
        &explicit_state,
    );
    Ok(RestoreDecision {
        source_agent: source.source_agent,
        memory_action,
        warnings,
    })
}

pub(crate) fn restore_binding_choice_required(
    home: &Path,
    target_agent: &str,
    backup_dir: &Path,
    config: &AgentConfig,
) -> miette::Result<bool> {
    let Some(memory) = config.memory.as_ref() else {
        return Ok(false);
    };
    if memory.provider != MemoryProvider::Hindsight || memory.bank_id.is_some() {
        return Ok(false);
    }

    let manifest = read_backup_manifest(backup_dir)?;
    let source = resolve_source(home, backup_dir, manifest.as_ref());
    Ok(source.source_agent.as_deref() != Some(target_agent))
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
        && matches!(
            mode,
            RestoreBindingMode::DirectUnspecified | RestoreBindingMode::Interactive
        )
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

    let conn = right_db::open_database_path_readonly(db_path)
        .into_diagnostic()
        .map_err(|e| miette::miette!("open {}: {e:#}", db_path.display()))?;

    Ok(ExplicitStateManifest {
        has_telegram_token,
        has_mcp_servers: table_has_rows(&conn, "mcp_servers")?,
        has_mcp_auth_tokens: table_has_rows(&conn, "auth_tokens")?,
        has_cron_specs: table_has_rows(&conn, "cron_specs")?,
    })
}

fn table_has_rows(conn: &right_db::Connection, table: &str) -> miette::Result<bool> {
    let exists: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
            right_db::params![table],
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
        warnings.push(
            "restored clone contains cron specs; running source and clone can duplicate scheduled delivery"
                .to_string(),
        );
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
        assert!(
            !json.contains("hs_test"),
            "manifest must not contain api keys"
        );
    }

    #[test]
    fn write_and_read_backup_manifest_roundtrips() {
        let dir = tempdir().unwrap();
        let config = hindsight_config(Some("shared-bank"));
        let manifest = build_backup_manifest("right", Some(&config), None).unwrap();

        write_backup_manifest(dir.path(), &manifest).unwrap();

        let content = std::fs::read_to_string(dir.path().join(BACKUP_MANIFEST_FILENAME)).unwrap();
        assert!(
            content.ends_with('\n'),
            "backup manifest should be newline-terminated"
        );
        let restored = read_backup_manifest(dir.path()).unwrap();
        assert_eq!(restored.as_ref(), Some(&manifest));
    }

    #[test]
    fn explicit_state_manifest_detects_db_tables_without_reading_secrets() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("data.db");
        let conn = right_db::open_connection(dir.path(), true).unwrap();
        conn.execute_batch(
            r#"
            INSERT INTO mcp_servers (name, url) VALUES ('linear', 'https://mcp.example.test');
            INSERT INTO auth_tokens (token) VALUES ('claude_secret');
            INSERT INTO cron_specs (job_name, schedule, prompt, created_at, updated_at)
            VALUES ('daily', '0 9 * * *', 'secret prompt', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z');
            "#,
        )
        .unwrap();
        let config = AgentConfig {
            telegram_token: Some("telegram_secret".to_string()),
            ..AgentConfig::default()
        };
        drop(conn);

        let manifest = build_backup_manifest("right", Some(&config), Some(&db_path)).unwrap();

        assert!(manifest.explicit_state.has_telegram_token);
        assert!(manifest.explicit_state.has_mcp_servers);
        assert!(manifest.explicit_state.has_mcp_auth_tokens);
        assert!(manifest.explicit_state.has_cron_specs);
        let json = serde_json::to_string(&manifest).unwrap();
        assert!(!json.contains("telegram_secret"));
        assert!(!json.contains("mcp_secret"));
        assert!(!json.contains("claude_secret"));
        assert!(!json.contains("secret prompt"));
    }

    #[test]
    fn infers_legacy_source_agent_from_right_home_backup_layout() {
        let dir = tempdir().unwrap();
        let backup = dir
            .path()
            .join("backups")
            .join("right")
            .join("20260516-0117");
        std::fs::create_dir_all(&backup).unwrap();

        assert_eq!(
            infer_legacy_source_agent(dir.path(), &backup).as_deref(),
            Some("right")
        );
    }

    #[test]
    fn legacy_source_inference_returns_none_for_path_outside_right_home_backups() {
        let dir = tempdir().unwrap();
        let backup_root = dir.path().join("backups");
        std::fs::create_dir_all(&backup_root).unwrap();
        let outside = dir
            .path()
            .join("elsewhere")
            .join("right")
            .join("20260516-0117");
        std::fs::create_dir_all(&outside).unwrap();

        assert_eq!(infer_legacy_source_agent(dir.path(), &outside), None);
    }

    #[test]
    fn legacy_source_inference_returns_none_for_extra_nested_components() {
        let dir = tempdir().unwrap();
        let backup = dir
            .path()
            .join("backups")
            .join("right")
            .join("20260516-0117")
            .join("extra");
        std::fs::create_dir_all(&backup).unwrap();

        assert_eq!(infer_legacy_source_agent(dir.path(), &backup), None);
    }

    #[test]
    fn legacy_source_inference_returns_none_when_backup_path_does_not_exist() {
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("backups")).unwrap();
        let missing = dir
            .path()
            .join("backups")
            .join("right")
            .join("20260516-0117");

        assert_eq!(infer_legacy_source_agent(dir.path(), &missing), None);
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
        let backup = dir
            .path()
            .join("backups")
            .join("right")
            .join("20260516-0117");
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
        let backup = dir
            .path()
            .join("backups")
            .join("right")
            .join("20260516-0117");
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

    #[test]
    fn preserve_mode_normalizes_yaml_to_source_bank() {
        let dir = tempdir().unwrap();
        let backup = dir
            .path()
            .join("backups")
            .join("right")
            .join("20260516-0117");
        std::fs::create_dir_all(&backup).unwrap();
        let agent_yaml = dir.path().join("agent.yaml");
        std::fs::write(
            &agent_yaml,
            "model: \"sonnet\"\n\nmemory:\n  provider: hindsight\n  api_key: \"hs_test\"\n",
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

        let content = std::fs::read_to_string(&agent_yaml).unwrap();
        assert!(content.contains("bank_id: \"right\""), "got:\n{content}");
    }
}
