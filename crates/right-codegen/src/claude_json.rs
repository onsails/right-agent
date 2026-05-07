use std::path::Path;

use right_core::agent_types::AgentDef;

/// Generate a per-agent `.claude.json` file with workspace trust and onboarding state.
///
/// Creates or updates `$AGENT_DIR/.claude.json` with:
/// - `projects[agent_abs_path].hasTrustDialogAccepted: true`
/// - `hasCompletedOnboarding: true` (suppresses theme picker / first-run flow)
///
/// Uses read-modify-write to preserve any existing fields CC has written.
/// Pattern follows `pre_trust_directory()` in `init.rs`.
pub fn generate_agent_claude_json(agent: &AgentDef) -> miette::Result<()> {
    let claude_json_path = agent.path.join(".claude.json");
    let path_key = agent
        .path
        .canonicalize()
        .unwrap_or_else(|_| agent.path.clone())
        .display()
        .to_string();

    crate::contract::write_merged_rmw(&claude_json_path, |existing| {
        let mut config: serde_json::Value = match existing {
            Some(content) => serde_json::from_str(content).map_err(|e| {
                miette::miette!("failed to parse {}: {e:#}", claude_json_path.display())
            })?,
            None => serde_json::json!({}),
        };

        let root = config
            .as_object_mut()
            .ok_or_else(|| miette::miette!(".claude.json is not a JSON object"))?;

        root.entry("hasCompletedOnboarding")
            .or_insert(serde_json::Value::Bool(true));

        let projects = root
            .entry("projects")
            .or_insert_with(|| serde_json::json!({}));

        let project = projects
            .as_object_mut()
            .ok_or_else(|| miette::miette!("projects is not a JSON object"))?
            .entry(&path_key)
            .or_insert_with(|| serde_json::json!({}));

        project
            .as_object_mut()
            .ok_or_else(|| miette::miette!("project entry is not a JSON object"))?
            .insert(
                "hasTrustDialogAccepted".to_owned(),
                serde_json::Value::Bool(true),
            );

        projects
            .as_object_mut()
            .ok_or_else(|| miette::miette!("projects is not a JSON object"))?
            .entry("/sandbox")
            .or_insert_with(|| serde_json::json!({"hasTrustDialogAccepted": true}));

        serde_json::to_string_pretty(&config)
            .map_err(|e| miette::miette!("failed to serialize .claude.json: {e:#}"))
    })?;

    tracing::debug!(agent = %agent.name, "wrote .claude.json");
    Ok(())
}

/// Create a credential symlink from agent `.claude/.credentials.json` to host credentials.
///
/// `host_home` MUST be resolved via `dirs::home_dir()` BEFORE any HOME env var manipulation.
/// The symlink enables OAuth authentication under HOME override.
///
/// If the host credentials file does not exist: logs a warning and returns Ok (per D-08).
/// Idempotent: removes existing symlink/file before creating (per Pitfall 3).
pub fn create_credential_symlink(agent: &AgentDef, host_home: &Path) -> miette::Result<()> {
    use std::os::unix::fs as unix_fs;

    let host_creds = host_home.join(".claude").join(".credentials.json");
    let agent_claude_dir = agent.path.join(".claude");
    std::fs::create_dir_all(&agent_claude_dir)
        .map_err(|e| miette::miette!("failed to create .claude dir for '{}': {e:#}", agent.name))?;
    let agent_creds = agent_claude_dir.join(".credentials.json");

    if host_creds.exists() {
        // Remove stale symlink/file if present (idempotent on re-runs).
        let _ = std::fs::remove_file(&agent_creds);
        unix_fs::symlink(&host_creds, &agent_creds).map_err(|e| {
            miette::miette!(
                "failed to create credentials symlink for '{}': {e:#}",
                agent.name
            )
        })?;
        tracing::debug!(agent = %agent.name, "credentials symlink created");
    } else {
        tracing::warn!(
            "no OAuth credentials found at {} -- agents will need ANTHROPIC_API_KEY to authenticate",
            host_creds.display()
        );
        eprintln!(
            "warning: no OAuth credentials at {} -- agent '{}' needs ANTHROPIC_API_KEY",
            host_creds.display(),
            agent.name
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use tempfile::tempdir;

    use super::*;
    use right_core::agent_types::{AgentConfig, AgentDef, RestartPolicy};

    fn make_test_agent(dir: &std::path::Path, name: &str) -> AgentDef {
        AgentDef {
            name: name.to_owned(),
            path: dir.to_path_buf(),
            identity_path: dir.join("IDENTITY.md"),
            config: Some(AgentConfig {
                restart: RestartPolicy::OnFailure,
                max_restarts: 3,
                backoff_seconds: 5,
                network_policy: Default::default(),
                model: None,
                sandbox: None,
                telegram_token: None,
                allowed_chat_ids: vec![],
                env: std::collections::HashMap::new(),
                secret: None,
                attachments: Default::default(),
                show_thinking: true,
                memory: None,
                stt: Default::default(),
            }),
            soul_path: None,
            user_path: None,
            tools_path: None,
            bootstrap_path: None,
            heartbeat_path: None,
        }
    }

    #[test]
    fn test_generates_claude_json_with_trust() {
        let dir = tempdir().unwrap();
        let agent = make_test_agent(dir.path(), "testbot");

        generate_agent_claude_json(&agent).unwrap();

        let claude_json = dir.path().join(".claude.json");
        assert!(claude_json.exists(), ".claude.json should be created");

        let content = std::fs::read_to_string(&claude_json).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();

        // Find the project entry (keyed by canonicalized path).
        let projects = parsed["projects"]
            .as_object()
            .expect("projects should exist");
        let (_key, project) = projects
            .iter()
            .next()
            .expect("at least one project entry expected");

        assert_eq!(
            project["hasTrustDialogAccepted"],
            serde_json::Value::Bool(true),
            "hasTrustDialogAccepted should be true"
        );
    }

    #[test]
    fn test_sets_onboarding_completed() {
        let dir = tempdir().unwrap();
        let agent = make_test_agent(dir.path(), "testbot");

        generate_agent_claude_json(&agent).unwrap();

        let content = std::fs::read_to_string(dir.path().join(".claude.json")).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();

        assert_eq!(
            parsed["hasCompletedOnboarding"],
            serde_json::Value::Bool(true),
            "hasCompletedOnboarding should be true to suppress theme picker"
        );
    }

    #[test]
    fn test_does_not_overwrite_existing_onboarding() {
        let dir = tempdir().unwrap();
        let agent = make_test_agent(dir.path(), "testbot");

        // Pre-write .claude.json with hasCompletedOnboarding already set.
        let existing = serde_json::json!({"hasCompletedOnboarding": true, "numStartups": 42});
        std::fs::write(
            dir.path().join(".claude.json"),
            serde_json::to_string_pretty(&existing).unwrap(),
        )
        .unwrap();

        generate_agent_claude_json(&agent).unwrap();

        let content = std::fs::read_to_string(dir.path().join(".claude.json")).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();

        assert_eq!(
            parsed["hasCompletedOnboarding"],
            serde_json::Value::Bool(true),
            "existing hasCompletedOnboarding should be preserved"
        );
        assert_eq!(
            parsed["numStartups"], 42,
            "other fields should be preserved"
        );
    }

    #[test]
    fn test_preserves_existing_fields() {
        let dir = tempdir().unwrap();
        let agent = make_test_agent(dir.path(), "testbot");

        // Pre-write a .claude.json with an existing field.
        let existing = serde_json::json!({"foo": "bar"});
        std::fs::write(
            dir.path().join(".claude.json"),
            serde_json::to_string_pretty(&existing).unwrap(),
        )
        .unwrap();

        generate_agent_claude_json(&agent).unwrap();

        let content = std::fs::read_to_string(dir.path().join(".claude.json")).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();

        assert_eq!(
            parsed["foo"],
            serde_json::Value::String("bar".to_owned()),
            "existing 'foo' field should be preserved"
        );
        assert!(
            parsed["projects"].is_object(),
            "projects key should exist after merge"
        );
    }

    #[test]
    fn test_creates_file_when_absent() {
        let dir = tempdir().unwrap();
        let agent = make_test_agent(dir.path(), "testbot");

        assert!(!dir.path().join(".claude.json").exists());

        generate_agent_claude_json(&agent).unwrap();

        assert!(
            dir.path().join(".claude.json").exists(),
            ".claude.json should be created"
        );
    }

    #[test]
    fn test_credential_symlink_created() {
        let agent_dir = tempdir().unwrap();
        let host_home = tempdir().unwrap();
        let agent = make_test_agent(agent_dir.path(), "testbot");

        // Create host credentials file.
        let host_claude_dir = host_home.path().join(".claude");
        std::fs::create_dir_all(&host_claude_dir).unwrap();
        let host_creds = host_claude_dir.join(".credentials.json");
        std::fs::write(&host_creds, r#"{"token":"test"}"#).unwrap();

        create_credential_symlink(&agent, host_home.path()).unwrap();

        let symlink_path = agent_dir.path().join(".claude").join(".credentials.json");
        assert!(
            symlink_path.exists(),
            "symlink should exist at agent .claude/.credentials.json"
        );
        // Verify it's a symlink pointing to host creds.
        let target = std::fs::read_link(&symlink_path).unwrap();
        assert_eq!(
            target, host_creds,
            "symlink should point to host credentials"
        );
    }

    #[test]
    fn test_credential_symlink_idempotent() {
        let agent_dir = tempdir().unwrap();
        let host_home = tempdir().unwrap();
        let agent = make_test_agent(agent_dir.path(), "testbot");

        let host_claude_dir = host_home.path().join(".claude");
        std::fs::create_dir_all(&host_claude_dir).unwrap();
        std::fs::write(
            host_claude_dir.join(".credentials.json"),
            r#"{"token":"test"}"#,
        )
        .unwrap();

        // Call twice — second call should not error.
        create_credential_symlink(&agent, host_home.path()).unwrap();
        create_credential_symlink(&agent, host_home.path()).unwrap();

        assert!(
            agent_dir
                .path()
                .join(".claude")
                .join(".credentials.json")
                .exists(),
            "symlink should still exist after second call"
        );
    }

    #[test]
    fn test_credential_symlink_warns_when_no_host_creds() {
        let agent_dir = tempdir().unwrap();
        let host_home = tempdir().unwrap(); // empty — no .claude/.credentials.json
        let agent = make_test_agent(agent_dir.path(), "testbot");

        // Should return Ok (warn only, no error).
        let result = create_credential_symlink(&agent, host_home.path());
        assert!(
            result.is_ok(),
            "should not error when host credentials are missing"
        );
    }

    #[test]
    fn test_uses_canonicalized_path_as_key() {
        let dir = tempdir().unwrap();
        let agent = make_test_agent(dir.path(), "testbot");

        generate_agent_claude_json(&agent).unwrap();

        let content = std::fs::read_to_string(dir.path().join(".claude.json")).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();

        let projects = parsed["projects"].as_object().unwrap();
        // The key should not contain symlinks (canonicalized). At minimum it must be an absolute path.
        for key in projects.keys() {
            assert!(
                key.starts_with('/'),
                "project key should be an absolute path, got: {key}"
            );
        }
    }

    #[test]
    fn test_agent_home_is_not_host_home() {
        // Verify that generate_agent_claude_json writes to agent dir, not host home.
        let dir = tempdir().unwrap();
        let agent_path = dir.path().join("agents").join("testbot");
        std::fs::create_dir_all(&agent_path).unwrap();

        let agent = AgentDef {
            name: "testbot".to_owned(),
            path: agent_path.clone(),
            identity_path: agent_path.join("IDENTITY.md"),
            config: None,
            soul_path: None,
            user_path: None,
            tools_path: None,
            bootstrap_path: None,
            heartbeat_path: None,
        };

        generate_agent_claude_json(&agent).unwrap();

        // Should write to agent dir, NOT to host home.
        assert!(
            agent_path.join(".claude.json").exists(),
            ".claude.json should be in agent dir"
        );
        // host home should NOT be affected (we can't check the real host home,
        // but we verify the agent path is distinct from host home).
        let host_home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/root"));
        assert_ne!(
            agent_path, host_home,
            "agent path should not equal host home"
        );
    }
}
