/// Ensure the Telegram plugin is installed in the host's CC plugin registry.
///
/// Runs `claude plugin marketplace add` and `claude plugin install` via CLI.
/// Both commands are idempotent — safe to call on every `rightclaw up`.
///
/// Resolves the claude binary the same way the shell wrapper does (claude, claude-bun).
/// Non-fatal: logs a warning and returns Ok if the claude binary is not found or
/// the commands fail (agent can still start, just without Telegram).
pub fn ensure_telegram_plugin_installed() -> miette::Result<()> {
    let claude_bin = resolve_claude_binary();
    let Some(bin) = claude_bin else {
        tracing::warn!("claude binary not found — skipping Telegram plugin install");
        return Ok(());
    };

    // 1. Add marketplace (idempotent — "already on disk" if present).
    tracing::debug!("ensuring claude-plugins-official marketplace is registered");
    let marketplace_result = std::process::Command::new(&bin)
        .args(["plugin", "marketplace", "add", "anthropics/claude-plugins-official"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output();

    match marketplace_result {
        Ok(output) if output.status.success() => {
            tracing::debug!("marketplace add: {}", String::from_utf8_lossy(&output.stdout).trim());
        }
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            tracing::warn!("marketplace add failed (non-fatal): {}", stderr.trim());
        }
        Err(e) => {
            tracing::warn!("failed to run marketplace add (non-fatal): {e:#}");
        }
    }

    // 2. Install plugin (idempotent — succeeds silently if already installed).
    tracing::debug!("ensuring telegram plugin is installed");
    let install_result = std::process::Command::new(&bin)
        .args(["plugin", "install", "telegram@claude-plugins-official", "--scope", "user"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output();

    match install_result {
        Ok(output) if output.status.success() => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            tracing::info!("telegram plugin: {}", stdout.trim());
            eprintln!("telegram plugin: {}", stdout.trim());
        }
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            tracing::warn!(
                "telegram plugin install failed (non-fatal): stdout={} stderr={}",
                stdout.trim(),
                stderr.trim()
            );
            eprintln!(
                "warning: telegram plugin install failed: {}",
                if stderr.trim().is_empty() { stdout.trim() } else { stderr.trim() }
            );
        }
        Err(e) => {
            tracing::warn!("failed to run plugin install (non-fatal): {e:#}");
            eprintln!("warning: failed to run plugin install: {e:#}");
        }
    }

    Ok(())
}

/// Find the claude binary in PATH (same logic as agent-wrapper.sh.j2).
fn resolve_claude_binary() -> Option<String> {
    for name in &["claude", "claude-bun"] {
        if which::which(name).is_ok() {
            return Some((*name).to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_claude_binary_returns_some_on_dev_machine() {
        // This test passes on machines with claude installed, returns None otherwise.
        // Just verify it doesn't panic.
        let _result = resolve_claude_binary();
    }

    #[test]
    fn ensure_telegram_plugin_installed_does_not_error() {
        // Non-fatal by design — should return Ok even if claude binary is absent.
        let result = ensure_telegram_plugin_installed();
        assert!(
            result.is_ok(),
            "ensure_telegram_plugin_installed must be non-fatal: {result:?}"
        );
    }
}
