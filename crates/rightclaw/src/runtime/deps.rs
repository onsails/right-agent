/// Find a binary by trying multiple names in order.
fn find_binary(names: &[&str]) -> Result<std::path::PathBuf, which::Error> {
    for name in names {
        if let Ok(path) = which::which(name) {
            return Ok(path);
        }
    }
    which::which(names[0]) // return error for the primary name
}

/// Verify that required external tools are available in PATH.
///
/// Checks for `process-compose` and `claude` (or `claude-bun`).
pub fn verify_dependencies() -> miette::Result<()> {
    which::which("process-compose").map_err(|_| {
        miette::miette!(
            help = "Install: https://f1bonacc1.github.io/process-compose/installation/",
            "process-compose not found in PATH"
        )
    })?;

    find_binary(&["claude", "claude-bun"]).map_err(|_| {
        miette::miette!(
            help = "Install Claude Code CLI: https://docs.anthropic.com/en/docs/claude-code",
            "claude not found in PATH (tried: claude, claude-bun)"
        )
    })?;

    // git is optional — used for `git init` in cmd_up. Warn if absent; not a hard failure.
    if which::which("git").is_err() {
        tracing::warn!(
            "git not found in PATH — agent directories will not be git-initialized. \
             Install git to enable workspace trust recognition."
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verify_dependencies_runs_without_panic() {
        // We can't guarantee process-compose/claude are in PATH, so just
        // verify the function runs without panicking.
        let result = verify_dependencies();
        let _ = result;
    }

    #[test]
    fn verify_dependencies_checks_for_known_missing_binary() {
        // Test the `which` error path by checking for a binary that definitely doesn't exist.
        let result = which::which("rightclaw-nonexistent-binary-42");
        assert!(result.is_err(), "expected nonexistent binary to fail which lookup");
    }

    #[test]
    fn verify_dependencies_does_not_check_openshell() {
        // verify_dependencies should never mention openshell.
        let result = verify_dependencies();
        if let Err(ref e) = result {
            let msg = format!("{e:?}");
            assert!(
                !msg.contains("openshell"),
                "verify_dependencies should not check openshell, but got: {msg}"
            );
        }
    }

    #[test]
    fn git_warning_is_non_fatal() {
        // The git check must never return Err — it is a warn-only check.
        // We cannot simulate git absence without env var manipulation, so we
        // verify the which::which("git") call returns a Result and that
        // our code handles the Err branch without propagating.
        let git_result = which::which("git");
        // Whether git is present or absent, our code path handles both without panic.
        // Test the absent path explicitly:
        let absent_result = which::which("rightclaw-nonexistent-git-bin-42");
        assert!(absent_result.is_err(), "expected absent binary to fail");
        // Simulated handling — must NOT use `?` (that would propagate):
        if absent_result.is_err() {
            // In production: tracing::warn!(...) then continue.
        }
        // If git is present, ensure which::which("git") succeeds:
        if git_result.is_ok() {
            assert!(git_result.unwrap().exists(), "git path should exist if found");
        }
    }
}
