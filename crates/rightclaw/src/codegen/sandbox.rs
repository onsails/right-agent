//! OpenShell sandbox lifecycle: create, upload, exec, delete.

use std::path::Path;
use std::process::Stdio;

/// Generate deterministic sandbox name from agent name.
pub fn sandbox_name(agent_name: &str) -> String {
    format!("rightclaw-{agent_name}")
}

/// Build args for `openshell sandbox create`.
pub fn build_create_args<'a>(sandbox: &'a str, policy_path: &'a str) -> Vec<&'a str> {
    vec![
        "sandbox", "create",
        "--policy", policy_path,
        "--name", sandbox,
        "--", "sleep", "infinity",
    ]
}

/// Build args for `openshell sandbox upload`.
pub fn build_upload_args<'a>(sandbox: &'a str, host_path: &'a str, sandbox_path: &'a str) -> Vec<&'a str> {
    vec![
        "sandbox", "upload",
        sandbox,
        host_path,
        sandbox_path,
    ]
}

/// Build args for `openshell sandbox exec`.
pub fn build_exec_args<'a>(sandbox: &'a str, cmd: &[&'a str]) -> Vec<&'a str> {
    let mut args = vec![
        "sandbox", "exec",
        sandbox,
        "--",
    ];
    args.extend_from_slice(cmd);
    args
}

/// Build args for `openshell sandbox delete`.
pub fn build_delete_args(sandbox: &str) -> Vec<&str> {
    vec![
        "sandbox", "delete",
        sandbox,
    ]
}

/// Create an OpenShell sandbox with the given policy.
pub async fn create_sandbox(name: &str, policy_path: &Path) -> miette::Result<()> {
    let status = tokio::process::Command::new("openshell")
        .args(["sandbox", "create", "--policy"])
        .arg(policy_path)
        .args(["--name", name, "--", "sleep", "infinity"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .status()
        .await
        .map_err(|e| miette::miette!("openshell sandbox create failed: {e:#}"))?;

    if !status.success() {
        return Err(miette::miette!("openshell sandbox create exited with {status}"));
    }
    tracing::info!(sandbox = name, "sandbox created");
    Ok(())
}

/// Upload a file from host into a running sandbox.
pub async fn upload_file(sandbox: &str, host_path: &Path, sandbox_path: &str) -> miette::Result<()> {
    let output = tokio::process::Command::new("openshell")
        .args(["sandbox", "upload", sandbox])
        .arg(host_path)
        .arg(sandbox_path)
        .output()
        .await
        .map_err(|e| miette::miette!("openshell upload failed: {e:#}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(miette::miette!("openshell upload failed: {stderr}"));
    }
    Ok(())
}

/// Execute a command inside a running sandbox, capturing output.
pub async fn exec_in_sandbox(sandbox: &str, cmd: &[&str], timeout_secs: u64) -> miette::Result<std::process::Output> {
    let child = tokio::process::Command::new("openshell")
        .args(["sandbox", "exec", sandbox, "--"])
        .args(cmd)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| miette::miette!("openshell exec spawn failed: {e:#}"))?;

    let output = tokio::time::timeout(
        std::time::Duration::from_secs(timeout_secs),
        child.wait_with_output(),
    )
    .await
    .map_err(|_| miette::miette!("openshell exec timed out after {timeout_secs}s"))?
    .map_err(|e| miette::miette!("openshell exec failed: {e:#}"))?;

    Ok(output)
}

/// Delete a sandbox.
pub async fn delete_sandbox(name: &str) -> miette::Result<()> {
    let status = tokio::process::Command::new("openshell")
        .args(["sandbox", "delete", name])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .status()
        .await
        .map_err(|e| miette::miette!("openshell sandbox delete failed: {e:#}"))?;

    if !status.success() {
        tracing::warn!(sandbox = name, "sandbox delete returned non-zero (may already be gone)");
    }
    tracing::info!(sandbox = name, "sandbox deleted");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sandbox_name_is_deterministic() {
        assert_eq!(sandbox_name("brain"), "rightclaw-brain");
        assert_eq!(sandbox_name("worker"), "rightclaw-worker");
    }

    #[test]
    fn create_command_is_correct() {
        let cmd = build_create_args("rightclaw-brain", "/tmp/policy.yaml");
        assert_eq!(cmd, vec![
            "sandbox", "create",
            "--policy", "/tmp/policy.yaml",
            "--name", "rightclaw-brain",
            "--", "sleep", "infinity",
        ]);
    }

    #[test]
    fn upload_command_is_correct() {
        let cmd = build_upload_args("rightclaw-brain", "/host/file.txt", "/sandbox/file.txt");
        assert_eq!(cmd, vec![
            "sandbox", "upload",
            "rightclaw-brain",
            "/host/file.txt",
            "/sandbox/file.txt",
        ]);
    }

    #[test]
    fn exec_command_is_correct() {
        let cmd = build_exec_args("rightclaw-brain", &["claude", "-p", "--", "hello"]);
        assert_eq!(cmd, vec![
            "sandbox", "exec",
            "rightclaw-brain",
            "--",
            "claude", "-p", "--", "hello",
        ]);
    }

    #[test]
    fn delete_command_is_correct() {
        let cmd = build_delete_args("rightclaw-brain");
        assert_eq!(cmd, vec![
            "sandbox", "delete",
            "rightclaw-brain",
        ]);
    }
}
