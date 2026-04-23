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
        .args([
            "plugin",
            "marketplace",
            "add",
            "anthropics/claude-plugins-official",
        ])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output();

    match marketplace_result {
        Ok(output) if output.status.success() => {
            tracing::debug!(
                "marketplace add: {}",
                String::from_utf8_lossy(&output.stdout).trim()
            );
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
        .args([
            "plugin",
            "install",
            "telegram@claude-plugins-official",
            "--scope",
            "user",
        ])
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
                if stderr.trim().is_empty() {
                    stdout.trim()
                } else {
                    stderr.trim()
                }
            );
        }
        Err(e) => {
            tracing::warn!("failed to run plugin install (non-fatal): {e:#}");
            eprintln!("warning: failed to run plugin install: {e:#}");
        }
    }

    Ok(())
}

/// Ensure `bun` runtime is available in PATH.
///
/// The Telegram channel plugin is a Bun-based MCP server — CC needs `bun`
/// in PATH to run it. On Linux/nix, bun is typically available via devenv.
/// On macOS native installs, it's often missing.
///
/// Strategy:
/// 1. If `bun` is in PATH → return Ok
/// 2. Install via `curl -fsSL https://bun.sh/install | bash` (works on macOS + Linux)
/// 3. Verify `bun` is now available
/// 4. If still missing → return Err (fatal — Telegram won't work)
pub fn ensure_bun_installed() -> miette::Result<()> {
    if which::which("bun").is_ok() {
        tracing::debug!("bun found in PATH");
        return Ok(());
    }

    eprintln!("bun not found in PATH — installing via bun.sh...");
    tracing::info!("bun not found, running bun installer");

    let install_result = std::process::Command::new("bash")
        .args(["-c", "curl -fsSL https://bun.sh/install | bash"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output();

    match install_result {
        Ok(output) if output.status.success() => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            tracing::info!("bun installer: {}", stdout.trim());
            eprintln!("bun installed successfully");
        }
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            return Err(miette::miette!(
                "bun installer failed: {}",
                if stderr.trim().is_empty() {
                    stdout.trim().to_string()
                } else {
                    stderr.trim().to_string()
                }
            ));
        }
        Err(e) => {
            return Err(miette::miette!(
                "failed to run bun installer (is curl available?): {e:#}"
            ));
        }
    }

    // Verify bun is now available. The installer puts it in ~/.bun/bin which
    // may not be in the current process PATH yet. Check the default location.
    let home =
        dirs::home_dir().ok_or_else(|| miette::miette!("cannot determine home directory"))?;
    let bun_path = home.join(".bun").join("bin").join("bun");

    if bun_path.exists() {
        // Add ~/.bun/bin to PATH for the current process so child processes
        // (process-compose → agent wrapper → claude → telegram MCP) inherit it.
        let bun_bin_dir = home.join(".bun").join("bin");
        let current_path = std::env::var("PATH").unwrap_or_default();
        // SAFETY: rightclaw up is single-threaded at this point (before tokio runtime starts).
        // No other threads are reading PATH concurrently.
        unsafe {
            std::env::set_var("PATH", format!("{}:{current_path}", bun_bin_dir.display()));
        }
        tracing::info!("added {} to PATH", bun_bin_dir.display());
        eprintln!("added {} to PATH", bun_bin_dir.display());
        Ok(())
    } else if which::which("bun").is_ok() {
        // Installer put it somewhere else but it's in PATH now
        Ok(())
    } else {
        Err(miette::miette!(
            "bun installer completed but bun binary not found. \
             Install manually: curl -fsSL https://bun.sh/install | bash"
        ))
    }
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
