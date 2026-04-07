use crate::agent::AgentDef;
use crate::config::ChromeConfig;

/// Generate a `.claude/settings.json` value for an agent.
///
/// Produces behavioral flags only — no sandbox configuration.
/// OpenShell is the security layer; CC native sandbox is not used.
///
/// `_chrome_config` is kept for API compatibility — Chrome MCP injection
/// happens in `.mcp.json`, not settings.json. Will be removed in a future cleanup.
pub fn generate_settings(
    _agent: &AgentDef,
    _chrome_config: Option<&ChromeConfig>,
) -> miette::Result<serde_json::Value> {
    let settings = serde_json::json!({
        "skipDangerousModePermissionPrompt": true,
        "spinnerTipsEnabled": false,
        "prefersReducedMotion": true,
        "autoMemoryEnabled": false,
    });

    Ok(settings)
}

#[cfg(test)]
#[path = "settings_tests.rs"]
mod tests;
