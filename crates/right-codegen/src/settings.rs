/// Generate a `.claude/settings.json` value for an agent.
///
/// Produces behavioral flags only — no sandbox configuration.
/// OpenShell is the security layer; CC native sandbox is not used.
pub fn generate_settings() -> miette::Result<serde_json::Value> {
    let settings = serde_json::json!({
        "skipDangerousModePermissionPrompt": true,
        "spinnerTipsEnabled": false,
        "prefersReducedMotion": true,
        "autoMemoryEnabled": true,
    });

    Ok(settings)
}

#[cfg(test)]
#[path = "settings_tests.rs"]
mod tests;
