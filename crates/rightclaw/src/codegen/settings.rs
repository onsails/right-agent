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

/// Sensitive paths denied for reading by default (security-first).
const DEFAULT_DENY_READ: &[&str] = &["~/.ssh", "~/.aws", "~/.gnupg"];

/// Generate a `.claude/settings.json` value for an agent.
///
/// Produces sandbox configuration with filesystem and network restrictions.
/// When `no_sandbox` is true, `sandbox.enabled` is `false` but all other
/// settings remain (agents still need skipDangerousModePermissionPrompt, etc.).
///
/// User overrides from `agent.yaml` `sandbox:` section are merged with
/// generated defaults (arrays are extended, not replaced).
pub fn generate_settings(agent: &AgentDef, no_sandbox: bool) -> miette::Result<serde_json::Value> {
    // Base filesystem allowWrite: agent's own directory (absolute path, D-02).
    let mut allow_write = vec![agent.path.display().to_string()];

    // Base allowed domains (D-03).
    let mut allowed_domains: Vec<String> = DEFAULT_ALLOWED_DOMAINS
        .iter()
        .map(|s| (*s).to_string())
        .collect();

    let mut excluded_commands: Vec<String> = vec![];

    // Merge user overrides from agent.yaml sandbox section (D-08).
    if let Some(ref config) = agent.config {
        if let Some(ref overrides) = config.sandbox {
            allow_write.extend(overrides.allow_write.iter().cloned());
            allowed_domains.extend(overrides.allowed_domains.iter().cloned());
            excluded_commands.extend(overrides.excluded_commands.iter().cloned());
        }
    }

    let deny_read: Vec<String> = DEFAULT_DENY_READ.iter().map(|s| (*s).to_string()).collect();

    let mut settings = serde_json::json!({
        // Non-sandbox settings (D-04).
        "skipDangerousModePermissionPrompt": true,
        "spinnerTipsEnabled": false,
        "prefersReducedMotion": true,

        // Sandbox configuration (D-01, D-12).
        "sandbox": {
            "enabled": !no_sandbox,
            "autoAllowBashIfSandboxed": true,
            "allowUnsandboxedCommands": false,
            "filesystem": {
                "allowWrite": allow_write,
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

    // Telegram plugin (D-05) -- conditional on .mcp.json presence.
    if agent.mcp_config_path.is_some() {
        settings["enabledPlugins"] = serde_json::json!({
            "telegram@claude-plugins-official": true
        });
    }

    Ok(settings)
}

#[cfg(test)]
#[path = "settings_tests.rs"]
mod tests;
