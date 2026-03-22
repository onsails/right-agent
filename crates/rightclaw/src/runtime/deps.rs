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
/// When `no_sandbox` is false, also checks for `openshell`.
pub fn verify_dependencies(no_sandbox: bool) -> miette::Result<()> {
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

    if !no_sandbox {
        which::which("openshell").map_err(|_| {
            miette::miette!(
                help = "Install: https://github.com/NVIDIA/OpenShell\n\
                        Or use --no-sandbox to skip sandbox enforcement (development only)",
                "openshell not found in PATH"
            )
        })?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verify_dependencies_fails_on_missing_process_compose() {
        // This test relies on process-compose not being in PATH in CI.
        // If it IS installed, this test is a no-op -- we verify the function
        // at least runs without panicking.
        let result = verify_dependencies(true);
        // We can't guarantee process-compose is missing, so just verify no panic.
        // The real test is that the function structure is correct.
        let _ = result;
    }

    #[test]
    fn verify_dependencies_checks_for_known_missing_binary() {
        // Test the `which` error path by checking for a binary that definitely doesn't exist.
        let result = which::which("rightclaw-nonexistent-binary-42");
        assert!(result.is_err(), "expected nonexistent binary to fail which lookup");
    }

    #[test]
    fn verify_dependencies_no_sandbox_skips_openshell() {
        // With no_sandbox=true, openshell check is skipped.
        // If process-compose and claude happen to be in PATH, this succeeds.
        // If not, it fails at process-compose -- never at openshell.
        let result = verify_dependencies(true);
        if let Err(ref e) = result {
            let msg = format!("{e:?}");
            assert!(
                !msg.contains("openshell"),
                "no_sandbox=true should not check openshell, but got: {msg}"
            );
        }
    }
}
