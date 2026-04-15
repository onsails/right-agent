//! OpenShell gRPC client — mTLS connection, sandbox readiness polling.
//! Also provides CLI wrappers for sandbox lifecycle, SSH config, and remote exec.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use tokio::process::{Child, Command};
use tonic::transport::{Certificate, Channel, ClientTlsConfig, Identity};

use crate::openshell_proto::openshell::v1::open_shell_client::OpenShellClient;
use crate::openshell_proto::openshell::v1::{ExecSandboxRequest, GetSandboxRequest};

/// SANDBOX_PHASE_READY value from openshell.datamodel.v1.SandboxPhase.
const SANDBOX_PHASE_READY: i32 = 2;

/// Path to `mcp.json` inside an OpenShell sandbox.
pub const SANDBOX_MCP_JSON_PATH: &str = "/sandbox/mcp.json";

/// Generate deterministic sandbox name from agent name.
pub fn sandbox_name(agent_name: &str) -> String {
    format!("rightclaw-{agent_name}")
}

/// SSH host alias for an agent's sandbox (used in SSH config).
pub fn ssh_host(agent_name: &str) -> String {
    format!("openshell-rightclaw-{agent_name}")
}

/// Resolve the default mTLS directory for the OpenShell gateway.
///
/// Checks `OPENSHELL_MTLS_DIR` env var first, then falls back to the
/// platform config directory: `<config_dir>/openshell/gateways/openshell/mtls`.
pub fn default_mtls_dir() -> PathBuf {
    std::env::var("OPENSHELL_MTLS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            // OpenShell uses XDG_CONFIG_HOME (~/.config) on all platforms,
            // not the macOS-native ~/Library/Application Support.
            let config_base = std::env::var("XDG_CONFIG_HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|_| {
                    dirs::home_dir()
                        .unwrap_or_else(|| PathBuf::from("/"))
                        .join(".config")
                });
            config_base.join("openshell/gateways/openshell/mtls")
        })
}

/// Result of the OpenShell pre-flight check.
#[derive(Debug)]
pub enum OpenShellStatus {
    /// mTLS certs are present — sandbox mode can proceed.
    Ready(PathBuf),
    /// `openshell` binary is not in PATH.
    NotInstalled,
    /// Binary is installed but no gateway has been started (mTLS dir missing).
    NoGateway(PathBuf),
    /// Gateway metadata exists but mTLS certs are missing/corrupt.
    BrokenGateway(PathBuf),
}

/// Check whether the OpenShell environment is ready for sandbox mode.
///
/// Returns a diagnostic enum that callers can use to give targeted
/// guidance (install, gateway start, or destroy+recreate).
pub fn preflight_check() -> OpenShellStatus {
    let mtls_dir = default_mtls_dir();

    // Check if all three mTLS files are present.
    let has_certs = mtls_dir.join("ca.crt").exists()
        && mtls_dir.join("tls.crt").exists()
        && mtls_dir.join("tls.key").exists();

    if has_certs {
        return OpenShellStatus::Ready(mtls_dir);
    }

    // No certs — figure out why.
    if which::which("openshell").is_err() {
        return OpenShellStatus::NotInstalled;
    }

    // Binary exists — check if a gateway has been configured.
    // `openshell gateway info` exits non-zero when no gateway metadata exists.
    let gateway_exists = std::process::Command::new("openshell")
        .args(["gateway", "info"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);

    if gateway_exists {
        OpenShellStatus::BrokenGateway(mtls_dir)
    } else {
        OpenShellStatus::NoGateway(mtls_dir)
    }
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

/// Check if a sandbox exists (any phase). Returns `false` only on `NotFound`.
pub async fn sandbox_exists(
    client: &mut OpenShellClient<Channel>,
    name: &str,
) -> miette::Result<bool> {
    match client
        .get_sandbox(GetSandboxRequest {
            name: name.to_owned(),
        })
        .await
    {
        Ok(_) => Ok(true),
        Err(status) if status.code() == tonic::Code::NotFound => Ok(false),
        Err(e) => Err(miette::miette!("GetSandbox RPC failed for '{name}': {e:#}")),
    }
}

/// Poll until a sandbox is fully deleted (gRPC returns `NotFound`).
pub async fn wait_for_deleted(
    client: &mut OpenShellClient<Channel>,
    name: &str,
    timeout_secs: u64,
    poll_interval_secs: u64,
) -> miette::Result<()> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(timeout_secs);
    let interval = Duration::from_secs(poll_interval_secs);

    loop {
        if !sandbox_exists(client, name).await? {
            tracing::info!(sandbox = name, "sandbox fully deleted");
            return Ok(());
        }

        if tokio::time::Instant::now() + interval > deadline {
            return Err(miette::miette!(
                "sandbox '{name}' was not deleted within {timeout_secs}s"
            ));
        }

        tracing::debug!(sandbox = name, "sandbox still exists, polling again in {poll_interval_secs}s");
        tokio::time::sleep(interval).await;
    }
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
///
/// `sandbox_dir` must be a directory path ending with `/`.
/// The file lands in `sandbox_dir` with its original name from `host_path`.
pub async fn upload_file(sandbox: &str, host_path: &Path, sandbox_dir: &str) -> miette::Result<()> {
    if !sandbox_dir.ends_with('/') {
        miette::bail!(
            "upload destination must be a directory path ending with '/', got: {sandbox_dir}"
        );
    }

    if host_path.is_dir() {
        return upload_directory(sandbox, host_path, sandbox_dir).await;
    }

    upload_single_file(sandbox, host_path, sandbox_dir).await
}

/// Upload a single file to a sandbox directory.
async fn upload_single_file(sandbox: &str, host_path: &Path, sandbox_dir: &str) -> miette::Result<()> {
    let output = Command::new("openshell")
        .args(["sandbox", "upload", sandbox])
        .arg(host_path)
        .arg(sandbox_dir)
        .output()
        .await
        .map_err(|e| miette::miette!("openshell upload failed: {e:#}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(miette::miette!("openshell upload failed: {stderr}"));
    }
    Ok(())
}

/// Upload a directory by individually uploading each file in parallel.
///
/// OpenShell CLI has a known bug where directory uploads silently drop files.
/// Workaround: walk the directory tree and upload each file individually.
async fn upload_directory(sandbox: &str, host_dir: &Path, sandbox_dir: &str) -> miette::Result<()> {
    use futures::stream::{self, StreamExt};

    let dir_name = host_dir
        .file_name()
        .ok_or_else(|| miette::miette!("directory has no name: {}", host_dir.display()))?
        .to_string_lossy();

    let mut uploads = Vec::new();
    for entry in walkdir::WalkDir::new(host_dir) {
        let entry = entry.map_err(|e| miette::miette!("walkdir error: {e:#}"))?;
        if !entry.file_type().is_file() {
            continue;
        }
        let rel = entry
            .path()
            .strip_prefix(host_dir)
            .map_err(|e| miette::miette!("strip_prefix failed: {e:#}"))?;
        // Destination: sandbox_dir + dir_name + relative path's parent
        let dest_dir = std::path::Path::new(sandbox_dir)
            .join(&*dir_name)
            .join(rel.parent().unwrap_or(std::path::Path::new("")));
        let dest = format!("{}/", dest_dir.display());
        uploads.push((entry.path().to_path_buf(), dest));
    }

    if uploads.is_empty() {
        return Ok(());
    }

    let results: Vec<miette::Result<()>> = stream::iter(uploads.into_iter().map(|(path, dest)| {
        let sandbox = sandbox.to_owned();
        async move { upload_single_file(&sandbox, &path, &dest).await }
    }))
    .buffer_unordered(10)
    .collect()
    .await;

    for result in results {
        result?;
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

/// Prepare the staging directory for sandbox upload.
///
/// Copies curated files from the agent directory into staging/,
/// excluding credentials (sandbox gets its own via login flow) and plugins (not used).
pub fn prepare_staging_dir(agent_dir: &Path, upload_dir: &Path) -> miette::Result<()> {
    let staging_claude = upload_dir.join(".claude");
    if staging_claude.exists() {
        std::fs::remove_dir_all(&staging_claude)
            .map_err(|e| miette::miette!("failed to clean staging/.claude: {e:#}"))?;
    }
    std::fs::create_dir_all(&staging_claude)
        .map_err(|e| miette::miette!("failed to create staging/.claude: {e:#}"))?;

    let src_claude = agent_dir.join(".claude");

    // Minimal CC bootstrap files only — agent defs, skills, and schemas
    // are deployed via /platform/ store during initial_sync.
    let copy_items: &[&str] = &["settings.json", "reply-schema.json"];

    for item in copy_items {
        let src = src_claude.join(item);
        let dst = staging_claude.join(item);
        if !src.exists() {
            continue;
        }
        std::fs::copy(&src, &dst)
            .map_err(|e| miette::miette!("failed to copy {} to staging: {e:#}", item))?;
    }

    // Copy .claude.json (trust/onboarding — at agent root, not inside .claude/)
    let claude_json_src = agent_dir.join(".claude.json");
    let claude_json_dst = upload_dir.join(".claude.json");
    if claude_json_src.exists() {
        std::fs::copy(&claude_json_src, &claude_json_dst)
            .map_err(|e| miette::miette!("failed to copy .claude.json to staging: {e:#}"))?;
    }

    // Copy mcp.json
    let mcp_json_src = agent_dir.join("mcp.json");
    let mcp_json_dst = upload_dir.join("mcp.json");
    if mcp_json_src.exists() {
        std::fs::copy(&mcp_json_src, &mcp_json_dst)
            .map_err(|e| miette::miette!("failed to copy mcp.json to staging: {e:#}"))?;
    }

    tracing::info!("prepared staging dir for sandbox upload");
    Ok(())
}

/// Recursively copy a directory, resolving symlinks to regular files.
/// Skips entries that fail to read (e.g. broken symlinks).
pub fn copy_dir_resolve_symlinks(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());

        // Resolve symlinks via entry.metadata() (follows symlinks, avoids extra syscall).
        let meta = match entry.metadata() {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!(path = %src_path.display(), "skipping unresolvable entry: {e}");
                continue;
            }
        };

        if meta.is_dir() {
            copy_dir_resolve_symlinks(&src_path, &dst_path)?;
        } else if meta.is_file() {
            std::fs::copy(&src_path, &dst_path)?;
        }
    }
    Ok(())
}

/// Outcome of `ensure_sandbox` — tells the caller what happened.
#[derive(Debug, PartialEq)]
pub enum SandboxOutcome {
    Created,
    Recreated,
}

/// Create a sandbox, handling the case where one already exists.
///
/// - If no sandbox exists: create it, wait for READY, return `Created`.
/// - If sandbox exists and `force_recreate`: delete + create, return `Recreated`.
/// - If sandbox exists and not force: return error.
pub async fn ensure_sandbox(
    agent_name: &str,
    policy_path: &Path,
    staging_dir: Option<&Path>,
    force_recreate: bool,
) -> miette::Result<SandboxOutcome> {
    let sandbox = sandbox_name(agent_name);

    let mtls_dir = match preflight_check() {
        OpenShellStatus::Ready(dir) => dir,
        OpenShellStatus::NotInstalled => {
            return Err(miette::miette!(
                help = "Install OpenShell and run `openshell auth login`,\n  \
                        or use `--sandbox-mode none` to run without a sandbox",
                "OpenShell is required for sandbox mode 'openshell'"
            ));
        }
        OpenShellStatus::NoGateway(_) => {
            return Err(miette::miette!(
                help = "Run `openshell gateway start`,\n  \
                        or use `--sandbox-mode none`",
                "OpenShell gateway is not running"
            ));
        }
        OpenShellStatus::BrokenGateway(dir) => {
            return Err(miette::miette!(
                help = "Try `openshell gateway destroy && openshell gateway start`,\n  \
                        or use `--sandbox-mode none`",
                "OpenShell gateway exists but mTLS certificates are missing at {}",
                dir.display()
            ));
        }
    };

    let mut grpc_client = connect_grpc(&mtls_dir).await?;
    // Use sandbox_exists (not is_sandbox_ready) to detect sandboxes in ANY phase,
    // including DELETING — otherwise we'd skip wait_for_deleted and create would
    // conflict with a sandbox still being torn down.
    let exists = sandbox_exists(&mut grpc_client, &sandbox).await?;

    if exists && !force_recreate {
        return Err(miette::miette!(
            help = "Use --force to recreate the sandbox,\n  \
                    or `rightclaw agent config` to update an existing agent",
            "Sandbox '{sandbox}' already exists"
        ));
    }

    if exists {
        tracing::info!(sandbox = %sandbox, "deleting existing sandbox for recreate");
        delete_sandbox(&sandbox).await;
        wait_for_deleted(&mut grpc_client, &sandbox, 60, 2).await?;
    }

    tracing::info!(sandbox = %sandbox, "creating sandbox");
    let mut child = spawn_sandbox(&sandbox, policy_path, staging_dir)?;

    tokio::select! {
        result = wait_for_ready(&mut grpc_client, &sandbox, 120, 2) => {
            result?;
            drop(child);
        }
        status = child.wait() => {
            let status = status.map_err(|e| miette::miette!("sandbox create child wait failed: {e:#}"))?;
            if !status.success() {
                return Err(miette::miette!(
                    "openshell sandbox create for '{}' exited with {status} before reaching READY",
                    agent_name
                ));
            }
        }
    }

    // SSH readiness probe — gRPC READY doesn't guarantee SSH transport is up.
    // 60s timeout: sandboxes with TLS termination + proxy can take 30-50s after READY.
    let sandbox_id = resolve_sandbox_id(&mut grpc_client, &sandbox).await?;
    wait_for_ssh(&mut grpc_client, &sandbox_id, 60, 2).await?;

    let outcome = if exists { SandboxOutcome::Recreated } else { SandboxOutcome::Created };
    tracing::info!(sandbox = %sandbox, ?outcome, "sandbox ready");
    Ok(outcome)
}

/// Resolve sandbox ID from sandbox name via gRPC `GetSandbox`.
pub async fn resolve_sandbox_id(
    client: &mut OpenShellClient<Channel>,
    name: &str,
) -> miette::Result<String> {
    let resp = client
        .get_sandbox(GetSandboxRequest {
            name: name.to_owned(),
        })
        .await
        .map_err(|e| miette::miette!("GetSandbox failed for '{name}': {e:#}"))?;

    let sandbox = resp
        .into_inner()
        .sandbox
        .ok_or_else(|| miette::miette!("GetSandbox returned empty response for '{name}'"))?;

    Ok(sandbox.id)
}

/// Execute a command inside a sandbox via gRPC `ExecSandbox` (single attempt).
///
/// Returns `Ok((stdout, exit_code))` on success, or an error if the RPC or
/// stream fails (e.g. SSH transport not ready yet).
async fn exec_in_sandbox_once(
    client: &mut OpenShellClient<Channel>,
    sandbox_id: &str,
    command: &[&str],
) -> miette::Result<(String, i32)> {
    use crate::openshell_proto::openshell::v1::exec_sandbox_event::Payload;

    let req = ExecSandboxRequest {
        sandbox_id: sandbox_id.to_owned(),
        command: command.iter().map(|s| s.to_string()).collect(),
        timeout_seconds: 10,
        ..Default::default()
    };

    let mut stream = client
        .exec_sandbox(req)
        .await
        .map_err(|e| miette::miette!("ExecSandbox RPC failed: {e:#}"))?
        .into_inner();

    let mut stdout = Vec::new();
    let mut exit_code = -1;

    loop {
        match stream.message().await {
            Ok(Some(event)) => match event.payload {
                Some(Payload::Stdout(chunk)) => stdout.extend_from_slice(&chunk.data),
                Some(Payload::Exit(exit)) => exit_code = exit.exit_code,
                _ => {}
            },
            Ok(None) => break,
            Err(e) => {
                return Err(miette::miette!("ExecSandbox stream error: {e:#}"));
            }
        }
    }

    let stdout_str = String::from_utf8_lossy(&stdout).to_string();
    Ok((stdout_str, exit_code))
}

/// Execute a command inside a sandbox via gRPC `ExecSandbox` and return (stdout, exit_code).
///
/// Retries up to 5 times with 1s backoff if the SSH transport isn't ready yet
/// (common immediately after sandbox creation — gRPC reports READY before SSH is up).
/// Retries cover both RPC-level and stream-level errors.
pub async fn exec_in_sandbox(
    client: &mut OpenShellClient<Channel>,
    sandbox_id: &str,
    command: &[&str],
) -> miette::Result<(String, i32)> {
    let mut last_err = String::new();

    for attempt in 0..5u32 {
        if attempt > 0 {
            tracing::debug!(sandbox_id, attempt, "exec: retrying after SSH transport delay");
            tokio::time::sleep(Duration::from_secs(1)).await;
        }

        match exec_in_sandbox_once(client, sandbox_id, command).await {
            Ok(result) => return Ok(result),
            Err(e) => {
                last_err = format!("{e:#}");
                tracing::debug!(sandbox_id, attempt, error = %last_err, "exec: attempt failed");
            }
        }
    }

    Err(miette::miette!(
        "ExecSandbox failed after 5 attempts: {last_err}"
    ))
}

/// Poll until SSH transport inside the sandbox is ready.
///
/// gRPC READY doesn't guarantee SSH is accepting connections — there's a gap
/// where `ExecSandbox` fails with "Connection reset by peer". This probe
/// runs a trivial `echo` command until it succeeds.
async fn wait_for_ssh(
    client: &mut OpenShellClient<Channel>,
    sandbox_id: &str,
    timeout_secs: u64,
    poll_interval_secs: u64,
) -> miette::Result<()> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(timeout_secs);
    let interval = Duration::from_secs(poll_interval_secs);

    loop {
        match exec_in_sandbox_once(client, sandbox_id, &["echo", "ready"]).await {
            Ok(_) => {
                tracing::info!(sandbox_id, "SSH transport is ready");
                return Ok(());
            }
            Err(e) => {
                if tokio::time::Instant::now() + interval > deadline {
                    return Err(miette::miette!(
                        "SSH transport not ready after {timeout_secs}s: {e:#}"
                    ));
                }
                tracing::debug!(sandbox_id, error = %e, "SSH not ready, retrying");
                tokio::time::sleep(interval).await;
            }
        }
    }
}

/// Resolve the host IP as seen from inside a sandbox.
///
/// Runs `getent ahostsv4 host.openshell.internal` inside the sandbox via gRPC exec
/// and parses the first IPv4 address from the output.
///
/// This IP varies by platform:
/// - macOS Docker Desktop: `192.168.65.254`
/// - Linux Docker bridge: `172.17.0.1`
/// - Custom networks: varies
///
/// Returns `None` if `host.openshell.internal` doesn't resolve (e.g. Linux without
/// `--add-host` flag), or if the sandbox exec fails.
pub async fn resolve_host_ip(
    client: &mut OpenShellClient<Channel>,
    sandbox_id: &str,
) -> miette::Result<Option<std::net::IpAddr>> {
    let (stdout, exit_code) = exec_in_sandbox(
        client,
        sandbox_id,
        &["getent", "ahostsv4", "host.openshell.internal"],
    )
    .await?;

    if exit_code != 0 || stdout.trim().is_empty() {
        tracing::warn!(sandbox_id, exit_code, "host.openshell.internal not resolvable in sandbox");
        return Ok(None);
    }

    // Output format: "192.168.65.254  STREAM host.openshell.internal\n192.168.65.254  DGRAM\n..."
    // Take the first token of the first line.
    let ip_str = stdout
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().next())
        .ok_or_else(|| miette::miette!("unexpected getent output: {stdout}"))?;

    let ip: std::net::IpAddr = ip_str
        .parse()
        .map_err(|e| miette::miette!("failed to parse host IP '{ip_str}': {e}"))?;

    tracing::info!(sandbox_id, %ip, "resolved host.openshell.internal");
    Ok(Some(ip))
}

/// Verify that critical files exist in the sandbox after creation/upload.
///
/// Uses gRPC `ExecSandbox` to check file existence (fast, no download).
/// If any are missing, attempts individual re-upload from `host_source_dir`
/// and re-checks. Fails with a detailed error listing all missing files.
pub async fn verify_sandbox_files(
    sandbox_name: &str,
    host_source_dir: &Path,
    sandbox_base_path: &str,
    expected_files: &[&str],
) -> miette::Result<()> {
    let mtls_dir = match preflight_check() {
        OpenShellStatus::Ready(dir) => dir,
        _ => return Ok(()), // Can't verify without OpenShell
    };
    let mut client = connect_grpc(&mtls_dir).await?;
    let sandbox_id = resolve_sandbox_id(&mut client, sandbox_name).await?;

    tracing::info!(sandbox = sandbox_name, count = expected_files.len(), "verify: checking files");

    // Build a single shell command to check all files at once.
    let checks: Vec<String> = expected_files
        .iter()
        .map(|f| format!("test -f {sandbox_base_path}{f} && echo 'OK:{f}' || echo 'MISSING:{f}'"))
        .collect();
    let check_cmd = checks.join("; ");

    let (output, exit_code) = exec_in_sandbox(
        &mut client,
        &sandbox_id,
        &["sh", "-c", &check_cmd],
    )
    .await?;

    tracing::debug!(sandbox = sandbox_name, exit_code, output = %output.trim(), "verify: check result");

    // Parse which files are missing.
    let mut missing_files: Vec<&str> = Vec::new();
    for &filename in expected_files {
        let ok_marker = format!("OK:{filename}");
        if !output.contains(&ok_marker) {
            missing_files.push(filename);
        }
    }

    if missing_files.is_empty() {
        tracing::info!(sandbox = sandbox_name, "verify: all expected files present");
        return Ok(());
    }

    // Re-upload missing files individually.
    tracing::warn!(
        sandbox = sandbox_name,
        missing = ?missing_files,
        "verify: some files missing, attempting individual re-upload"
    );

    for &filename in &missing_files {
        let host_path = host_source_dir.join(filename);
        if host_path.exists() {
            upload_file(sandbox_name, &host_path, sandbox_base_path)
                .await
                .map_err(|e| miette::miette!("verify: re-upload {filename} failed: {e:#}"))?;
            tracing::info!(file = filename, "verify: re-uploaded");
        } else {
            tracing::debug!(file = filename, "verify: not on host, skipping");
        }
    }

    // Re-check after re-upload.
    let (output2, _) = exec_in_sandbox(
        &mut client,
        &sandbox_id,
        &["sh", "-c", &check_cmd],
    )
    .await?;

    let mut still_missing: Vec<String> = Vec::new();
    for &filename in &missing_files {
        let ok_marker = format!("OK:{filename}");
        if !output2.contains(&ok_marker) {
            still_missing.push(filename.to_string());
        }
    }

    if still_missing.is_empty() {
        tracing::info!(sandbox = sandbox_name, "verify: all files present after re-upload");
        Ok(())
    } else {
        Err(miette::miette!(
            help = "This is likely an OpenShell upload bug.\n  \
                    Try running `rightclaw init` again.",
            "Sandbox '{}' is missing critical files after re-upload: {}",
            sandbox_name,
            still_missing.join(", ")
        ))
    }
}

#[cfg(test)]
#[path = "openshell_tests.rs"]
mod tests;
