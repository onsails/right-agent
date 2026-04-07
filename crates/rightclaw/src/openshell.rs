//! OpenShell gRPC client — mTLS connection, sandbox readiness polling.
//! Also provides CLI wrappers for sandbox lifecycle, SSH config, and remote exec.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use tokio::process::{Child, Command};
use tonic::transport::{Certificate, Channel, ClientTlsConfig, Identity};

use crate::openshell_proto::openshell::v1::open_shell_client::OpenShellClient;
use crate::openshell_proto::openshell::v1::GetSandboxRequest;

/// SANDBOX_PHASE_READY value from openshell.datamodel.v1.SandboxPhase.
const SANDBOX_PHASE_READY: i32 = 2;

/// Generate deterministic sandbox name from agent name.
pub fn sandbox_name(agent_name: &str) -> String {
    format!("rightclaw-{agent_name}")
}

/// SSH host alias for an agent's sandbox (used in SSH config).
pub fn ssh_host(agent_name: &str) -> String {
    format!("openshell-rightclaw-{agent_name}")
}

/// Connect to the OpenShell gRPC server with mTLS.
///
/// Reads CA cert, client cert, and client key from `mtls_dir`:
/// - `ca.crt`  — CA certificate
/// - `tls.crt` — client certificate
/// - `tls.key` — client private key
pub async fn connect_grpc(
    mtls_dir: &Path,
) -> miette::Result<OpenShellClient<Channel>> {
    let ca_pem = tokio::fs::read(mtls_dir.join("ca.crt"))
        .await
        .map_err(|e| miette::miette!("failed to read ca.crt from {}: {e:#}", mtls_dir.display()))?;

    let client_cert = tokio::fs::read(mtls_dir.join("tls.crt"))
        .await
        .map_err(|e| miette::miette!("failed to read tls.crt from {}: {e:#}", mtls_dir.display()))?;

    let client_key = tokio::fs::read(mtls_dir.join("tls.key"))
        .await
        .map_err(|e| miette::miette!("failed to read tls.key from {}: {e:#}", mtls_dir.display()))?;

    let tls = ClientTlsConfig::new()
        .ca_certificate(Certificate::from_pem(ca_pem))
        .identity(Identity::from_pem(client_cert, client_key))
        .domain_name("localhost");

    let channel = Channel::from_static("https://127.0.0.1:8080")
        .tls_config(tls)
        .map_err(|e| miette::miette!("TLS config error: {e:#}"))?
        .connect()
        .await
        .map_err(|e| miette::miette!("gRPC connect to 127.0.0.1:8080 failed: {e:#}"))?;

    Ok(OpenShellClient::new(channel))
}

/// Check whether a sandbox has reached READY phase.
///
/// Returns `Ok(false)` when the sandbox does not exist yet (gRPC `NotFound`),
/// which is the normal state right after `spawn_sandbox` — the create process
/// hasn't registered the sandbox in OpenShell's registry yet.
pub async fn is_sandbox_ready(
    client: &mut OpenShellClient<Channel>,
    name: &str,
) -> miette::Result<bool> {
    let resp = match client
        .get_sandbox(GetSandboxRequest {
            name: name.to_owned(),
        })
        .await
    {
        Ok(r) => r,
        Err(status) if status.code() == tonic::Code::NotFound => {
            tracing::debug!(sandbox = name, "sandbox not found yet (expected during creation)");
            return Ok(false);
        }
        Err(e) => {
            return Err(miette::miette!("GetSandbox RPC failed for '{name}': {e:#}"));
        }
    };

    let sandbox = resp
        .into_inner()
        .sandbox
        .ok_or_else(|| miette::miette!("GetSandbox returned empty response for '{name}'"))?;

    Ok(sandbox.phase == SANDBOX_PHASE_READY)
}

/// Poll until a sandbox reaches READY phase, or timeout.
pub async fn wait_for_ready(
    client: &mut OpenShellClient<Channel>,
    name: &str,
    timeout_secs: u64,
    poll_interval_secs: u64,
) -> miette::Result<()> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(timeout_secs);
    let interval = Duration::from_secs(poll_interval_secs);

    loop {
        if is_sandbox_ready(client, name).await? {
            tracing::info!(sandbox = name, "sandbox is READY");
            return Ok(());
        }

        if tokio::time::Instant::now() + interval > deadline {
            return Err(miette::miette!(
                "sandbox '{name}' did not become READY within {timeout_secs}s"
            ));
        }

        tracing::debug!(sandbox = name, "sandbox not ready, polling again in {poll_interval_secs}s");
        tokio::time::sleep(interval).await;
    }
}

/// Spawn an OpenShell sandbox. Returns the child process handle.
///
/// The child has `kill_on_drop(false)` so the sandbox survives if the
/// parent process exits.
pub fn spawn_sandbox(
    name: &str,
    policy_path: &Path,
    upload_dir: Option<&Path>,
) -> miette::Result<Child> {
    let mut cmd = Command::new("openshell");
    cmd.args(["sandbox", "create", "--name", name, "--policy"]);
    cmd.arg(policy_path);
    cmd.arg("--no-tty");

    if let Some(dir) = upload_dir {
        cmd.arg("--upload");
        cmd.arg(dir);
    }

    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    cmd.kill_on_drop(false);

    let child = cmd
        .spawn()
        .map_err(|e| miette::miette!("failed to spawn openshell sandbox create: {e:#}"))?;

    tracing::info!(sandbox = name, "spawned sandbox create process");
    Ok(child)
}

/// Run `openshell sandbox ssh-config NAME` and write the output to
/// `config_dir/{name}.ssh-config`. Returns the path of the written file.
pub async fn generate_ssh_config(
    name: &str,
    config_dir: &Path,
) -> miette::Result<PathBuf> {
    let output = Command::new("openshell")
        .args(["sandbox", "ssh-config", name])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(|e| miette::miette!("failed to run openshell sandbox ssh-config: {e:#}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(miette::miette!(
            "openshell sandbox ssh-config failed (exit {}): {stderr}",
            output.status
        ));
    }

    let dest = config_dir.join(format!("{name}.ssh-config"));
    tokio::fs::write(&dest, &output.stdout)
        .await
        .map_err(|e| miette::miette!("failed to write ssh-config to {}: {e:#}", dest.display()))?;

    tracing::info!(sandbox = name, path = %dest.display(), "wrote ssh-config");
    Ok(dest)
}

/// Apply a policy to a running sandbox via `openshell policy set`.
///
/// Uses `--wait` to block until the sandbox confirms it loaded the new policy.
pub async fn apply_policy(name: &str, policy_path: &Path) -> miette::Result<()> {
    let output = Command::new("openshell")
        .args(["policy", "set", name, "--policy"])
        .arg(policy_path)
        .args(["--wait", "--timeout", "30"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(|e| miette::miette!("failed to run openshell policy set: {e:#}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(miette::miette!(
            "openshell policy set failed (exit {}): {stderr}",
            output.status
        ));
    }

    tracing::info!(sandbox = name, "policy applied");
    Ok(())
}

/// Execute a command inside a sandbox over SSH.
///
/// Uses `-F config_path` to pick up the sandbox SSH config, and
/// `host` as the SSH host alias (see [`ssh_host`]).
/// Returns stdout on success.
pub async fn ssh_exec(
    config_path: &Path,
    host: &str,
    cmd: &[&str],
    timeout_secs: u64,
) -> miette::Result<String> {
    let mut command = Command::new("ssh");
    command.arg("-F").arg(config_path);
    command.arg(host);
    command.arg("--");
    command.args(cmd);
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());

    let child = command
        .spawn()
        .map_err(|e| miette::miette!("failed to spawn ssh: {e:#}"))?;

    let timeout_dur = Duration::from_secs(timeout_secs);
    let output = tokio::time::timeout(timeout_dur, child.wait_with_output())
        .await
        .map_err(|_| miette::miette!("ssh exec timed out after {timeout_secs}s"))?
        .map_err(|e| miette::miette!("ssh exec failed: {e:#}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        tracing::error!(host, ?cmd, %stderr, "ssh exec failed");
        return Err(miette::miette!(
            "ssh exec on '{host}' failed (exit {}): {stderr}",
            output.status
        ));
    }

    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    Ok(stdout)
}

/// Upload a file from host into a running sandbox.
pub async fn upload_file(sandbox: &str, host_path: &Path, sandbox_path: &str) -> miette::Result<()> {
    let output = Command::new("openshell")
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

/// Download a file or directory from a sandbox to the host.
pub async fn download_file(sandbox: &str, sandbox_path: &str, host_dest: &Path) -> miette::Result<()> {
    let output = Command::new("openshell")
        .args(["sandbox", "download", sandbox, sandbox_path])
        .arg(host_dest)
        .output()
        .await
        .map_err(|e| miette::miette!("openshell download failed: {e:#}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(miette::miette!("openshell download failed: {stderr}"));
    }
    Ok(())
}

/// Delete a sandbox. Best-effort — logs a warning on failure but does not
/// propagate the error (stale sandboxes that don't exist shouldn't block callers).
pub async fn delete_sandbox(name: &str) {
    let result = Command::new("openshell")
        .args(["sandbox", "delete", name])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await;

    match result {
        Ok(output) if output.status.success() => {
            tracing::info!(sandbox = name, "deleted sandbox");
        }
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            tracing::warn!(
                sandbox = name,
                exit = %output.status,
                %stderr,
                "failed to delete sandbox (best-effort)"
            );
        }
        Err(e) => {
            tracing::warn!(
                sandbox = name,
                error = %e,
                "failed to run openshell sandbox delete (best-effort)"
            );
        }
    }
}

#[cfg(test)]
#[path = "openshell_tests.rs"]
mod tests;
