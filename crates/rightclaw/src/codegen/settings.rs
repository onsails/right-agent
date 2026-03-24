use crate::agent::AgentDef;

/// Generate a `.claude/settings.json` value for an agent.
///
/// Produces sandbox configuration with filesystem and network restrictions.
/// When `no_sandbox` is true, `sandbox.enabled` is `false` but all other
/// settings remain (agents still need skipDangerousModePermissionPrompt, etc.).
///
/// User overrides from `agent.yaml` `sandbox:` section are merged with
/// generated defaults (arrays are extended, not replaced).
pub fn generate_settings(_agent: &AgentDef, _no_sandbox: bool) -> miette::Result<serde_json::Value> {
    todo!("implement generate_settings")
}

#[cfg(test)]
#[path = "settings_tests.rs"]
mod tests;
