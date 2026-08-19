//! Frozen legacy read path into an OpenShell sandbox.
//!
//! This module exists for exactly one reason: `right agent migrate-sandbox`
//! has to *read* the sandbox an unmigrated agent still lives in, and the
//! `right-openshell` crate that used to do that is gone. Nothing else in Right
//! talks to OpenShell any more, and nothing else may call into here.
//!
//! Three rules keep it small and keep it dead-endable:
//!
//! - **Read-only, plus the one delete the migration owns.** No create, no
//!   policy, no provider mutation — the migration never puts anything *into*
//!   an OpenShell sandbox.
//! - **CLI only, never gRPC.** Every operation goes through the `openshell`
//!   binary, so this module carries no vendored protos, no `tonic` codegen,
//!   and no mTLS handling. That is what makes it a few hundred lines instead
//!   of a crate.
//! - **No ControlMaster.** The migration opens exactly two SSH connections
//!   (one listing, one archive stream) against a sandbox it is about to
//!   delete, so multiplexing would buy one TCP handshake in exchange for a
//!   socket that has to be torn down on every failure path.
//!
//! Delete this module — and the `migrate-sandbox` command with it — once the
//! migration window closes and no agent.yaml carries `sandbox.mode: openshell`.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use sha2::{Digest, Sha256};
use tokio::process::Command;

/// Maximum length of an OpenShell sandbox name.
///
/// Upstream `MAX_ROUTABLE_NAME_LEN`: DNS-routable names are composed as
/// `workspace--sandbox--service` and must fit a 63-char DNS label, so
/// 19+2+19+2+19 = 61.
const MAX_SANDBOX_NAME_LEN: usize = 19;

/// Bytes of `raw` kept as the prefix when [`fit_sandbox_name`] must shorten:
/// 14 prefix bytes + `-` + 4 hash chars = [`MAX_SANDBOX_NAME_LEN`].
const FIT_PREFIX_CHARS: usize = MAX_SANDBOX_NAME_LEN - 1 - 4;

/// How long any single `openshell` CLI call may take. Every call in this
/// module is a small control-plane request against a local gateway.
const CLI_TIMEOUT: Duration = Duration::from_secs(60);

/// Sanitize to the upstream DNS-1123 label charset (lowercase alnum + `-`, no
/// leading/trailing `-`, no `--`), which OpenShell enforces at sandbox create.
fn sanitize_dns1123(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut last_dash = false;
    for ch in raw.chars().flat_map(char::to_lowercase) {
        if ch.is_ascii_lowercase() || ch.is_ascii_digit() {
            out.push(ch);
            last_dash = false;
        } else if !last_dash && !out.is_empty() {
            // Any invalid char (incl. non-ASCII, '_', '.') collapses to one '-'.
            out.push('-');
            last_dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    out
}

/// Fit `raw` into OpenShell's name space, exactly as the retired crate did.
///
/// This is a *lookup* key: it has to reproduce the name the sandbox was
/// created under, byte for byte, or the migration reads the wrong sandbox.
fn fit_sandbox_name(raw: &str) -> String {
    if raw.len() <= MAX_SANDBOX_NAME_LEN && sanitize_dns1123(raw) == raw {
        return raw.to_owned();
    }
    let sanitized = sanitize_dns1123(raw);
    if sanitized.len() <= MAX_SANDBOX_NAME_LEN {
        return sanitized;
    }
    let boundary = sanitized
        .char_indices()
        .map(|(i, _)| i)
        .take_while(|&i| i <= FIT_PREFIX_CHARS)
        .last()
        .unwrap_or(0);
    let prefix = sanitized[..boundary].trim_end_matches('-');
    let digest = Sha256::digest(raw.as_bytes());
    format!("{prefix}-{:02x}{:02x}", digest[0], digest[1])
}

/// Resolve the OpenShell sandbox an agent's files live in: the explicit
/// `sandbox.name` from `agent.yaml` when set, else the deterministic
/// `right-{agent}` the old bot created.
pub(super) fn resolve_sandbox_name(agent_name: &str, explicit_name: Option<&str>) -> String {
    explicit_name
        .map(str::to_owned)
        .unwrap_or_else(|| fit_sandbox_name(&format!("right-{agent_name}")))
}

/// SSH host alias emitted by `openshell sandbox ssh-config`. Right is
/// single-tenant, so the workspace is always `default`.
pub(super) fn ssh_host_for_sandbox(sandbox_name: &str) -> String {
    format!("openshell-{sandbox_name}.default")
}

/// Run an `openshell` CLI subcommand and return its stdout.
///
/// A missing binary is reported as such rather than as a generic spawn
/// failure: it is the single most likely reason this whole path is
/// unavailable, and the operator's fix is different from every other error.
async fn openshell_cli(args: &[&str]) -> miette::Result<String> {
    let mut command = Command::new("openshell");
    command.args(args);
    command.env("NO_COLOR", "1");
    command.stdin(Stdio::null());
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());
    command.kill_on_drop(true);

    let output = match command.output().await {
        Ok(output) => output,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(miette::miette!(
                help = "Install OpenShell and start its gateway, then re-run the migration.",
                "the `openshell` CLI is not installed, so this agent's old sandbox cannot be read"
            ));
        }
        Err(e) => {
            return Err(miette::miette!(
                "spawn `openshell {}`: {e:#}",
                args.join(" ")
            ));
        }
    };
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(miette::miette!(
            "`openshell {}` failed ({}): {}",
            args.join(" "),
            output.status,
            stderr.trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Names of every sandbox the gateway knows about.
///
/// Doubles as the reachability probe: it needs the CLI *and* a working
/// gateway, so a migration that gets a list back can stop worrying about both.
async fn list_sandbox_names() -> miette::Result<Vec<String>> {
    let stdout = tokio::time::timeout(CLI_TIMEOUT, openshell_cli(&["sandbox", "list", "--names"]))
        .await
        .map_err(|_| miette::miette!("`openshell sandbox list` timed out"))?
        .map_err(|error| {
            miette::miette!(
                help = "Start the OpenShell gateway, then re-run the migration.",
                "no usable OpenShell gateway — this agent's old sandbox cannot be read: {error:#}"
            )
        })?;
    Ok(stdout
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect())
}

/// Whether a sandbox exists, in any phase.
///
/// Asks about the one name rather than scanning `sandbox list`, which
/// paginates at 100 by default: on a busy gateway a listed check would report
/// a sandbox outside the first page as absent, aborting a migration that
/// should run — and, in the delete-confirmation loop, claiming a deletion
/// that never happened.
pub(super) async fn sandbox_exists(name: &str) -> miette::Result<bool> {
    let probe = tokio::time::timeout(
        CLI_TIMEOUT,
        openshell_cli(&["sandbox", "get", name, "-o", "json"]),
    )
    .await
    .map_err(|_| miette::miette!("`openshell sandbox get {name}` timed out"))?;
    match probe {
        // The CLI reports a missing sandbox on stderr with a zero exit, so the
        // absence has to be read out of the payload rather than the status.
        Ok(json) => Ok(sandbox_phase(&json).is_ok()),
        Err(error) if is_sandbox_not_found(&format!("{error:#}")) => Ok(false),
        Err(error) => Err(error),
    }
}

/// Whether a CLI failure means "no such sandbox" rather than a real fault.
fn is_sandbox_not_found(message: &str) -> bool {
    message.contains("sandbox not found") || message.contains("entity was not found")
}

/// Prove the CLI and gateway are usable before the migration copies anything.
pub(super) async fn preflight() -> miette::Result<()> {
    list_sandbox_names().await.map(|_| ())
}

/// The lifecycle phase `openshell sandbox get -o json` reports.
///
/// Only two values steer the migration; everything else is "still moving".
/// The raw string is kept so a timeout can say what the sandbox was actually
/// doing (`Stopped` and `Provisioning` are very different problems).
fn sandbox_phase(json: &str) -> miette::Result<String> {
    let value: serde_json::Value = serde_json::from_str(json)
        .map_err(|e| miette::miette!("`openshell sandbox get -o json` is not JSON: {e:#}"))?;
    value
        .get("phase")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| miette::miette!("`openshell sandbox get -o json` reported no phase"))
}

/// Poll until a sandbox reports `Ready`.
///
/// `Error` fails immediately: it is terminal, and waiting out the timeout
/// would only delay the same failure. Every other phase is treated as
/// in-progress, which is why the timeout message carries the last one seen.
pub(super) async fn wait_for_ready(
    name: &str,
    timeout_secs: u64,
    poll_interval_secs: u64,
) -> miette::Result<()> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(timeout_secs);
    let interval = Duration::from_secs(poll_interval_secs);

    loop {
        let json = tokio::time::timeout(
            CLI_TIMEOUT,
            openshell_cli(&["sandbox", "get", name, "-o", "json"]),
        )
        .await
        .map_err(|_| miette::miette!("`openshell sandbox get {name}` timed out"))??;
        let last_phase = sandbox_phase(&json)?;
        match last_phase.as_str() {
            "Ready" => {
                tracing::info!(sandbox = name, "openshell sandbox is READY");
                return Ok(());
            }
            "Error" => {
                return Err(miette::miette!(
                    "OpenShell sandbox '{name}' is in phase Error and cannot be read"
                ));
            }
            phase => tracing::debug!(sandbox = name, phase, "openshell sandbox not ready"),
        }

        if tokio::time::Instant::now() + interval > deadline {
            return Err(miette::miette!(
                "OpenShell sandbox '{name}' did not become READY within {timeout_secs}s (last phase: {last_phase})"
            ));
        }
        tokio::time::sleep(interval).await;
    }
}

/// Strip SGR escape sequences (`ESC [ … m`) from a line.
///
/// The CLI renders its table header bold even when stdout is a pipe and
/// `NO_COLOR` is set, so the header arrives as `ESC[1mNAME ESC[0m`. Matching
/// the raw bytes silently found no header and reported every sandbox as
/// having no attached providers.
fn strip_sgr(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        // ESC [ … m — consume through the terminating byte.
        if chars.next() != Some('[') {
            continue;
        }
        for tail in chars.by_ref() {
            if tail.is_ascii_alphabetic() {
                break;
            }
        }
    }
    out
}

/// Provider names out of `openshell sandbox provider list`'s table.
///
/// The CLI has no JSON mode for this subcommand, so the table is the wire
/// format. It is either a `NAME …` header followed by one row per provider,
/// or a single "No providers attached…" sentence. Both the header and the
/// rows are stripped of SGR escapes before matching.
fn parse_attached_provider_names(table: &str) -> Vec<String> {
    table
        .lines()
        .map(strip_sgr)
        .skip_while(|line| !line.trim_start().starts_with("NAME"))
        .skip(1)
        .filter_map(|line| line.split_whitespace().next().map(str::to_owned))
        .collect()
}

/// Provider names attached to a sandbox.
///
/// Names only — OpenShell redacts credential values on every read path, which
/// is why the migration can report what an agent had but never carry it.
pub(super) async fn list_attached_providers(name: &str) -> miette::Result<Vec<String>> {
    let table = tokio::time::timeout(
        CLI_TIMEOUT,
        openshell_cli(&["sandbox", "provider", "list", name]),
    )
    .await
    .map_err(|_| miette::miette!("`openshell sandbox provider list {name}` timed out"))??;
    Ok(parse_attached_provider_names(&table))
}

/// Delete a sandbox and return only once the gateway stops listing it.
///
/// Confirmation is the point: the migration reports this deletion to the
/// operator, and "we asked" is not the same claim as "it is gone".
pub(super) async fn delete_sandbox_confirmed(
    name: &str,
    timeout_secs: u64,
    poll_interval_secs: u64,
) -> miette::Result<()> {
    tokio::time::timeout(CLI_TIMEOUT, openshell_cli(&["sandbox", "delete", name]))
        .await
        .map_err(|_| miette::miette!("`openshell sandbox delete {name}` timed out"))??;

    let deadline = tokio::time::Instant::now() + Duration::from_secs(timeout_secs);
    let interval = Duration::from_secs(poll_interval_secs);
    loop {
        if !sandbox_exists(name).await? {
            return Ok(());
        }
        if tokio::time::Instant::now() + interval > deadline {
            return Err(miette::miette!(
                "sandbox '{name}' was still listed {timeout_secs}s after the delete was accepted"
            ));
        }
        tokio::time::sleep(interval).await;
    }
}

/// Write `openshell sandbox ssh-config NAME` to `<config_dir>/<name>.ssh-config`.
///
/// The config is used verbatim: no ControlMaster block is appended, so no
/// multiplexing socket is created and none has to be torn down.
pub(super) async fn generate_ssh_config(name: &str, config_dir: &Path) -> miette::Result<PathBuf> {
    let config = tokio::time::timeout(CLI_TIMEOUT, openshell_cli(&["sandbox", "ssh-config", name]))
        .await
        .map_err(|_| miette::miette!("`openshell sandbox ssh-config {name}` timed out"))??;

    let dest = config_dir.join(format!("{name}.ssh-config"));
    tokio::fs::write(&dest, config.as_bytes())
        .await
        .map_err(|e| miette::miette!("write ssh-config to {}: {e:#}", dest.display()))?;
    Ok(dest)
}

/// Quote argv for OpenSSH remote execution.
///
/// OpenSSH does not preserve remote argv: it sends one command string to the
/// remote login shell, so the result must be passed as exactly one argument
/// after the host. Fails only on an interior NUL byte.
fn quote_ssh_remote_args<'a>(args: impl IntoIterator<Item = &'a str>) -> miette::Result<String> {
    shlex::try_join(args).map_err(|e| miette::miette!("quote SSH remote command args: {e}"))
}

/// Run one command inside the sandbox over SSH and return its stdout.
pub(super) async fn ssh_exec(
    config_path: &Path,
    host: &str,
    cmd: &[&str],
    timeout_secs: u64,
) -> miette::Result<String> {
    let mut command = Command::new("ssh");
    command.arg("-F").arg(config_path);
    command.arg(host);
    command.arg("--");
    command.arg(quote_ssh_remote_args(cmd.iter().copied())?);
    command.stdin(Stdio::null());
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());

    let mut child = right_process::ProcessGroupChild::spawn(command)
        .map_err(|e| miette::miette!("spawn ssh for '{host}': {e:#}"))?;
    let output = tokio::time::timeout(Duration::from_secs(timeout_secs), child.wait_with_output())
        .await
        .map_err(|_| miette::miette!("ssh exec on '{host}' timed out after {timeout_secs}s"))?
        .map_err(|e| miette::miette!("ssh exec on '{host}' failed: {e:#}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(miette::miette!(
            "ssh exec on '{host}' failed ({}): {}",
            output.status,
            stderr.trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// The remote `tar` argv that produces the migration archive.
///
/// The two transforms are what make the archive interchangeable with a
/// microsandbox backup: members come out rooted at `sandbox/` rather than
/// `./`, so both restore through the same `--strip-components=1`.
/// `flags=rh` rewrites regular member names and hardlink targets while
/// leaving symlink targets (`./target`) alone.
fn sandbox_tar_download_args(sandbox_path: &str, excludes: &[&str]) -> miette::Result<Vec<String>> {
    let archive_root = sandbox_path.trim_matches('/');
    if archive_root.is_empty() {
        miette::bail!("sandbox path must not be empty");
    }

    let mut args = vec![
        "tar".to_owned(),
        "czpf".to_owned(),
        "-".to_owned(),
        "-C".to_owned(),
        format!("/{archive_root}"),
        format!("--transform=flags=rh;s,^\\.$,{archive_root},"),
        format!("--transform=flags=rh;s,^\\./,{archive_root}/,"),
    ];

    for path in excludes {
        // GNU tar evaluates excludes before transforms, so these match names
        // as seen under `-C /sandbox .`, not the final `sandbox/...` names.
        args.push(format!("--exclude=./{path}"));
        args.push(format!("--exclude=./{path}/*"));
    }

    args.push(".".to_owned());
    Ok(args)
}

/// Stream the sandbox home to `dest_path` as a gzipped tarball.
///
/// stdout goes straight to the file: an agent home is routinely gigabytes, so
/// it is never buffered in memory.
pub(super) async fn ssh_tar_download(
    config_path: &Path,
    ssh_host: &str,
    sandbox_path: &str,
    dest_path: &Path,
    excludes: &[&str],
    timeout_secs: u64,
) -> miette::Result<()> {
    let mut command = Command::new("ssh");
    command.arg("-F").arg(config_path);
    command.arg(ssh_host);
    command.arg("--");
    let tar_args = sandbox_tar_download_args(sandbox_path, excludes)?;
    command.arg(quote_ssh_remote_args(tar_args.iter().map(String::as_str))?);
    command.stdin(Stdio::null());
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());

    let mut child = right_process::ProcessGroupChild::spawn(command)
        .map_err(|e| miette::miette!("spawn ssh for tar download: {e:#}"))?;
    let mut stdout = child
        .stdout()
        .ok_or_else(|| miette::miette!("no stdout handle from ssh tar download"))?;
    let mut stderr = child
        .stderr()
        .ok_or_else(|| miette::miette!("no stderr handle from ssh tar download"))?;

    let mut file = tokio::fs::File::create(dest_path)
        .await
        .map_err(|e| miette::miette!("create archive {}: {e:#}", dest_path.display()))?;

    // Copy, drain and wait concurrently: a `tar` big enough to fill the pipe
    // buffer deadlocks against a sequential wait. `try_join!` keeps all three
    // in one future, so a timeout cancels the lot and Drop kills the group.
    let copy = async {
        tokio::io::copy(&mut stdout, &mut file)
            .await
            .map_err(|e| miette::miette!("I/O error during tar download: {e:#}"))?;
        Ok::<_, miette::Error>(())
    };
    let drain = async {
        let mut buf = Vec::new();
        tokio::io::AsyncReadExt::read_to_end(&mut stderr, &mut buf)
            .await
            .map_err(|e| miette::miette!("I/O error reading tar stderr: {e:#}"))?;
        Ok::<_, miette::Error>(String::from_utf8_lossy(&buf).into_owned())
    };
    let wait = async {
        child
            .wait()
            .await
            .map_err(|e| miette::miette!("ssh wait failed: {e:#}"))
    };

    let ((), stderr, status) = tokio::time::timeout(Duration::from_secs(timeout_secs), async {
        tokio::try_join!(copy, drain, wait)
    })
    .await
    .map_err(|_| miette::miette!("ssh tar download timed out after {timeout_secs}s"))??;

    if !status.success() {
        let stderr = stderr.trim();
        if stderr.is_empty() {
            return Err(miette::miette!("ssh tar download failed ({status})"));
        }
        return Err(miette::miette!(
            "ssh tar download failed ({status}): {stderr}"
        ));
    }
    Ok(())
}

#[cfg(test)]
#[path = "legacy_openshell_tests.rs"]
mod tests;
