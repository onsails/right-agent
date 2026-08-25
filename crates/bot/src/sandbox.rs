//! Agent Sandbox access for the bot.
//!
//! Every sandbox interaction — lifecycle, exec, file transfer — goes through
//! [`right_sandbox`]. There is no host execution path and no SSH: one
//! [`Sandbox`] handle replaces the OpenShell-era `resolved_sandbox` +
//! `ssh_config_path` pair, and it is either present (backend Ready) or absent
//! (backend degraded, in which case nothing runs — see
//! [`crate::cc::invocation::guard_no_sandboxed_host_exec`]).

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use right_sandbox::{ExecRequest, SandboxHandle};

/// A live Agent Sandbox handle.
///
/// `Arc` because one handle is shared by everything working on the agent's
/// behalf at a given moment; `SandboxHandle` itself is not `Clone`, so the
/// `Arc` is the sharing seam.
///
/// A handle is **not** stable for the bot's lifetime: it wraps an SDK
/// connection to one VM, and the supervisor publishes a replacement whenever
/// it recreates the sandbox. The live one lives in
/// [`crate::sandbox_runtime::SandboxRuntimeHandle`]; long-lived consumers
/// hold that runtime handle and call `current_sandbox()` per unit of work
/// (turn, cron job, delivery, dashboard request), keeping the result only for
/// that unit.
///
/// Re-exported as `right_bot::Sandbox` for the CLI, which attaches to the
/// sandbox itself before handing it to [`crate::keepalive::InitAuthProbe`].
pub type Sandbox = Arc<SandboxHandle>;

/// The agent's home inside the guest. Everything the agent owns lives here.
///
/// Defined by [`right_sandbox::GUEST_HOME`], which is also the workdir every
/// spec is created with — the two cannot drift.
pub(crate) const SANDBOX_HOME: &str = right_sandbox::GUEST_HOME;

/// Path to the aggregator `mcp.json` inside the guest.
pub(crate) const SANDBOX_MCP_JSON_PATH: &str = "/sandbox/mcp.json";

/// Guest profile script that puts `/sandbox/.local/bin` on PATH.
///
/// Provisioning writes it and `.bashrc` sources it, but a direct guest exec
/// gets no login shell — so anything needing the agent's own toolchain
/// (`claude` above all) has to source this first.
pub(crate) const GUEST_ENV_SCRIPT: &str = "/sandbox/.right/env.sh";

/// Guest directory attachments are uploaded into.
pub(crate) const SANDBOX_INBOX: &str = "/sandbox/inbox";

/// Guest directory the agent writes outgoing attachments into.
pub(crate) const SANDBOX_OUTBOX: &str = "/sandbox/outbox";

/// Timeout for cheap shell probes (`test -f`, `mkdir`, `getent`). Callers that
/// fetch over the network pass their own.
pub(crate) const DEFAULT_EXEC_TIMEOUT: Duration = Duration::from_secs(10);

/// Run `argv` in the guest with the default probe timeout and return
/// `(stdout, exit_code)`.
///
/// A non-zero exit code is data, not an error — callers decide. Only a
/// transport/spawn failure is an `Err`.
pub(crate) async fn exec_argv(
    sandbox: &SandboxHandle,
    argv: &[&str],
) -> miette::Result<(String, i32)> {
    exec_argv_with_timeout(sandbox, argv, DEFAULT_EXEC_TIMEOUT).await
}
/// Run a short provisioning command as the unprivileged guest user.
pub(crate) async fn exec_argv_as_guest(
    sandbox: &SandboxHandle,
    argv: &[&str],
) -> miette::Result<(String, i32)> {
    let request = guest_exec_request(argv, DEFAULT_EXEC_TIMEOUT);
    let outcome = sandbox
        .exec(&request)
        .await
        .map_err(|e| miette::miette!("sandbox guest exec {argv:?} failed: {e:#}"))?;
    Ok((
        String::from_utf8_lossy(&outcome.stdout).into_owned(),
        outcome.code,
    ))
}

/// Run `argv` in the guest with an explicit timeout. The guest process is
/// SIGKILLed on expiry.
pub(crate) async fn exec_argv_with_timeout(
    sandbox: &SandboxHandle,
    argv: &[&str],
    timeout: Duration,
) -> miette::Result<(String, i32)> {
    let outcome = sandbox
        .exec(&exec_request(argv, timeout))
        .await
        .map_err(|e| miette::miette!("sandbox exec {argv:?} failed: {e:#}"))?;
    Ok((
        String::from_utf8_lossy(&outcome.stdout).into_owned(),
        outcome.code,
    ))
}

/// Build an [`ExecRequest`] from an argv slice.
///
/// Split out so the request shape (guest cwd, no shell, hard timeout) is
/// stated once.
fn exec_request(argv: &[&str], timeout: Duration) -> ExecRequest {
    let (cmd, args) = argv.split_first().expect("exec argv must name a program");
    ExecRequest {
        cmd: (*cmd).to_owned(),
        args: args.iter().map(|arg| (*arg).to_owned()).collect(),
        cwd: Some(SANDBOX_HOME.to_owned()),
        timeout: Some(timeout),
        ..ExecRequest::default()
    }
}

fn guest_exec_request(argv: &[&str], timeout: Duration) -> ExecRequest {
    let mut request = exec_request(argv, timeout);
    request.user = Some(right_sandbox::GUEST_USER.to_owned());
    request
}

/// Upload a host file into a guest directory, mirroring `scp host guest_dir/`.
///
/// The SDK's copy takes a destination *file* path, so the file name is joined
/// here — the OpenShell-era call sites all passed a trailing-slash directory.
pub(crate) async fn upload_into_dir(
    sandbox: &SandboxHandle,
    host_path: &Path,
    guest_dir: &str,
) -> miette::Result<()> {
    let name = host_path
        .file_name()
        .ok_or_else(|| miette::miette!("upload source {} has no file name", host_path.display()))?
        .to_string_lossy()
        .into_owned();
    let guest_path = format!("{}/{name}", guest_dir.trim_end_matches('/'));
    sandbox
        .fs_copy_from_host(host_path, &guest_path)
        .await
        .map_err(|e| {
            miette::miette!(
                "upload {} -> {guest_path} failed: {e:#}",
                host_path.display()
            )
        })
}

#[cfg(test)]
#[path = "sandbox_tests.rs"]
mod tests;
