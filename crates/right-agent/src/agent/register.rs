//! Register a newly-created agent with a running process-compose.
//!
//! Mirrors [`crate::agent::destroy`]: probes PC via
//! [`crate::runtime::PcClient::from_home`] (which enforces `--home` isolation),
//! regenerates cross-agent codegen, and reloads PC's configuration so the new
//! agent's bot process appears live.

use std::path::Path;

/// Inputs for [`register_with_running_pc`].
pub struct RegisterOptions {
    pub agent_name: String,
    /// True when init wiped a pre-existing agent dir (`--force-recreate` on an
    /// existing agent). Drives the post-reload `restart_process` call.
    pub recreated: bool,
}

/// Outcome of [`register_with_running_pc`].
#[derive(Debug, PartialEq, Eq)]
pub struct RegisterResult {
    /// True if PC was alive and the reload succeeded.
    /// False if PC was not running (no `state.json`, stale port, health-check fail).
    pub pc_running: bool,
}

/// Register a newly-init'd agent with a running PC instance.
///
/// Returns `Ok(RegisterResult { pc_running: false })` if PC isn't running —
/// caller should print `next: right up`. Returns `Err` only if PC was alive but
/// the config reload failed; caller renders a warn row.
pub async fn register_with_running_pc(
    home: &Path,
    options: RegisterOptions,
) -> miette::Result<RegisterResult> {
    // `from_home` enforces --home isolation by reading
    // `<home>/run/state.json` for the PC port + token. Absent or stale state
    // ⇒ no PC ⇒ skip everything. See ARCHITECTURE.md
    // "Runtime isolation — mandatory".
    let Some(client) = crate::runtime::PcClient::from_home(home)? else {
        tracing::debug!(
            home = %home.display(),
            agent = %options.agent_name,
            "no runtime state — PC not running, skipping reload"
        );
        return Ok(RegisterResult { pc_running: false });
    };

    if client.health_check().await.is_err() {
        tracing::debug!(
            agent = %options.agent_name,
            "state.json present but PC health-check failed — treating as not running"
        );
        return Ok(RegisterResult { pc_running: false });
    }

    // PC is alive. Mirrors `crate::agent::destroy::destroy_agent` after the
    // dir-removal step: rediscover all agents, regenerate cross-agent codegen
    // (process-compose.yaml, agent-tokens.json, cloudflared config), then ask
    // PC to diff its running config against the new file via POST
    // /project/configuration. PC adds the new agent's processes live.
    let agents_dir = right_core::config::agents_dir(home);
    let all_agents = crate::agent::discover_agents(&agents_dir)?;
    let self_exe = std::env::current_exe()
        .map_err(|e| miette::miette!("failed to resolve current executable path: {e:#}"))?;
    right_codegen::run_agent_codegen(home, &all_agents, &self_exe, false)?;

    client.reload_configuration().await?;
    tracing::info!(agent = %options.agent_name, "reloaded process-compose configuration");

    if options.recreated {
        let process_name = format!("{}-bot", options.agent_name);
        if let Err(e) = client.restart_process(&process_name).await {
            // Non-fatal: config is correct on disk and in PC; only the live
            // process didn't bounce. Surface to the log file, not the recap.
            tracing::warn!(
                process = %process_name,
                error = format!("{e:#}"),
                "failed to restart bot process after recreate (non-fatal)"
            );
        }
    }

    Ok(RegisterResult { pc_running: true })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn returns_pc_running_false_when_state_json_absent() {
        let dir = tempfile::TempDir::new().unwrap();
        let result = register_with_running_pc(
            dir.path(),
            RegisterOptions {
                agent_name: "test".to_string(),
                recreated: false,
            },
        )
        .await
        .unwrap();
        assert_eq!(result, RegisterResult { pc_running: false });
    }

    #[tokio::test]
    async fn returns_pc_running_false_when_state_json_points_at_closed_port() {
        let dir = tempfile::TempDir::new().unwrap();
        let run = dir.path().join("run");
        std::fs::create_dir_all(&run).unwrap();
        // Write a state.json that points at a port nothing is listening on.
        // Port 1 is reserved and unbound on developer machines; if it ever
        // is bound, the test will be flaky — pick another low port.
        std::fs::write(
            run.join("state.json"),
            r#"{"agents":[],"socket_path":"/tmp/test.sock","started_at":"2026-01-01T00:00:00Z","pc_port":1,"pc_api_token":"x"}"#,
        )
        .unwrap();

        let result = register_with_running_pc(
            dir.path(),
            RegisterOptions {
                agent_name: "test".to_string(),
                recreated: false,
            },
        )
        .await
        .unwrap();
        assert_eq!(result, RegisterResult { pc_running: false });
    }

    #[tokio::test]
    async fn propagates_error_when_state_json_is_malformed() {
        let dir = tempfile::TempDir::new().unwrap();
        let run = dir.path().join("run");
        std::fs::create_dir_all(&run).unwrap();
        std::fs::write(run.join("state.json"), "{not valid json").unwrap();

        let err = register_with_running_pc(
            dir.path(),
            RegisterOptions {
                agent_name: "test".to_string(),
                recreated: false,
            },
        )
        .await
        .expect_err("malformed state.json must be a parse error");
        // Just confirm the error chain reaches us — exact wording is from_home's.
        let msg = format!("{err:#}");
        assert!(
            msg.to_lowercase().contains("state.json")
                || msg.to_lowercase().contains("parse")
                || msg.to_lowercase().contains("json"),
            "expected error to mention state.json/parse/json, got: {msg}"
        );
    }
}
