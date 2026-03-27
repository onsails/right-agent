use std::path::Path;

use crate::agent::AgentDef;

/// Default domains agents need to reach.
const DEFAULT_ALLOWED_DOMAINS: &[&str] = &[
    "api.anthropic.com",
    "github.com",
    "npmjs.org",
    "crates.io",
    "agentskills.io",
    "api.telegram.org",
];

/// Generate a `.claude/settings.json` value for an agent.
///
/// Produces sandbox configuration with filesystem and network restrictions.
/// When `no_sandbox` is true, `sandbox.enabled` is `false` but all other
/// settings remain (agents still need skipDangerousModePermissionPrompt, etc.).
///
/// `host_home` must be the real host home directory (resolved before any HOME
/// env override). Used to build absolute denyRead paths — tilde paths resolve
/// to the agent dir under HOME override, making denyRead ineffective (HOME-05).
///
/// User overrides from `agent.yaml` `sandbox:` section are merged with
/// generated defaults (arrays are extended, not replaced).
pub fn generate_settings(
    agent: &AgentDef,
    no_sandbox: bool,
    host_home: &Path,
) -> miette::Result<serde_json::Value> {
    // Base filesystem allowWrite: agent's own directory (absolute path, D-02).
    let mut allow_write = vec![agent.path.display().to_string()];

    // Base allowed domains (D-03).
    let mut allowed_domains: Vec<String> = DEFAULT_ALLOWED_DOMAINS
        .iter()
        .map(|s| (*s).to_string())
        .collect();

    let mut excluded_commands: Vec<String> = vec![];

    // Build allowRead: agent path as default, plus user overrides (D-09b).
    // Agent path must be in allowRead because it lives inside the denied host HOME.
    let mut allow_read = vec![agent.path.display().to_string()];

    // Merge user overrides from agent.yaml sandbox section (D-08, D-09b).
    if let Some(ref config) = agent.config
        && let Some(ref overrides) = config.sandbox
    {
        allow_write.extend(overrides.allow_write.iter().cloned());
        allowed_domains.extend(overrides.allowed_domains.iter().cloned());
        excluded_commands.extend(overrides.excluded_commands.iter().cloned());
        allow_read.extend(overrides.allow_read.iter().cloned());
    }

    // Build denyRead with absolute host HOME paths (HOME-05).
    // Tilde paths (e.g. ~/.ssh) resolve to agent HOME under override, defeating denyRead.
    let deny_read = vec![
        host_home.join(".ssh").display().to_string(),
        host_home.join(".aws").display().to_string(),
        host_home.join(".gnupg").display().to_string(),
        // Belt: deny entire host HOME. allowRead[agent_path] overrides this for agent dir.
        format!("{}/", host_home.display()),
    ];

    let mut settings = serde_json::json!({
        // Non-sandbox settings (D-04).
        "skipDangerousModePermissionPrompt": true,
        "spinnerTipsEnabled": false,
        "prefersReducedMotion": true,
        "autoMemoryEnabled": false,

        // Sandbox configuration (D-01, D-12).
        "sandbox": {
            "enabled": !no_sandbox,
            "autoAllowBashIfSandboxed": true,
            "allowUnsandboxedCommands": false,
            "filesystem": {
                "allowWrite": allow_write,
                "allowRead": allow_read,
                "denyRead": deny_read,
            },
            "network": {
                "allowedDomains": allowed_domains,
            },
        }
    });

    // Add excludedCommands only if non-empty (cleaner output).
    if !excluded_commands.is_empty() {
        settings["sandbox"]["excludedCommands"] = serde_json::json!(excluded_commands);
    }

    // Telegram plugin (D-05) -- conditional on telegram config presence (D-01).
    if agent
        .config
        .as_ref()
        .map(|c| c.telegram_token.is_some() || c.telegram_token_file.is_some())
        .unwrap_or(false)
    {
        settings["enabledPlugins"] = serde_json::json!({
            "telegram@claude-plugins-official": true
        });
    }

    Ok(settings)
}

#[cfg(test)]
#[path = "settings_tests.rs"]
mod tests;
