//! Live-microVM probes for issue #172's execution, resource and guest-user
//! assumptions, plus the runtime-install and guest-filesystem behaviour the
//! design depends on.
//!
//! Every probe boots a real microVM, so all of them are `#[ignore]`d behind the
//! `ci-msb` marker and are run with
//! `cargo nextest run -p right-sandbox --run-ignored all`.

mod common;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use common::{
    ALPINE_IMAGE, NODE_IMAGE, PROBE_CPUS, PROBE_MEMORY_MIB, SandboxGuard, acquire_vm_slot,
    ensure_runtime_installed,
};
use microsandbox::sandbox::SandboxStatus;
use microsandbox::{ExecEvent, Sandbox, setup};

/// The runtime install root the SDK resolves (`MSB_HOME`, else `~/.microsandbox`).
fn install_root() -> PathBuf {
    match std::env::var("MSB_HOME") {
        Ok(home) if !home.is_empty() => PathBuf::from(home),
        _ => PathBuf::from(std::env::var("HOME").expect("HOME is set")).join(".microsandbox"),
    }
}

/// A sorted `name (size bytes)` listing of one directory, for evidence.
fn describe_dir(dir: &PathBuf) -> Result<Vec<String>> {
    let mut entries = Vec::new();
    for entry in std::fs::read_dir(dir).with_context(|| format!("read {}", dir.display()))? {
        let entry = entry?;
        let meta = entry.metadata()?;
        entries.push(format!(
            "{} ({} bytes{})",
            entry.file_name().to_string_lossy(),
            meta.len(),
            if meta.file_type().is_symlink() {
                ", symlink"
            } else {
                ""
            }
        ));
    }
    entries.sort();
    Ok(entries)
}

#[tokio::test]
#[ignore = "ci-msb: downloads and verifies the pinned msb runtime"]
async fn ci_msb_runtime_install_is_idempotent() -> Result<()> {
    // Must share the key with common::ensure_runtime_installed so a concurrent
    // probe process cannot install between the two calls and flake the mtime
    // no-op assertions below.
    let _lock = common::acquire_file_lock("rt-msb-runtime-install");

    let installed_before = setup::is_installed();

    let first_start = Instant::now();
    setup::install()
        .await
        .context("first setup::install() call")?;
    let first_elapsed = first_start.elapsed();

    assert!(
        setup::is_installed(),
        "is_installed() must be true after install()"
    );

    let root = install_root();
    let msb = root.join("bin").join("msb");
    let before_meta = std::fs::metadata(&msb).with_context(|| format!("stat {}", msb.display()))?;
    let before_mtime = before_meta.modified()?;

    let second_start = Instant::now();
    setup::install()
        .await
        .context("second setup::install() call")?;
    let second_elapsed = second_start.elapsed();

    let after_meta = std::fs::metadata(&msb)?;
    assert_eq!(
        before_meta.len(),
        after_meta.len(),
        "second install() rewrote bin/msb"
    );
    assert_eq!(
        before_mtime,
        after_meta.modified()?,
        "second install() replaced bin/msb (mtime changed), so it is not a no-op"
    );
    assert!(
        second_elapsed < Duration::from_secs(5),
        "second install() took {second_elapsed:?}; expected a no-op"
    );
    assert!(setup::is_installed());

    println!(
        "install: is_installed() before = {installed_before}, first install = {first_elapsed:?}, \
         second install = {second_elapsed:?}"
    );
    println!("install root = {}", root.display());
    println!("  bin/: {:?}", describe_dir(&root.join("bin"))?);
    println!("  lib/: {:?}", describe_dir(&root.join("lib"))?);

    // The default root above is already warm on this host, so measure the cold
    // path — the operator's very first start — in a throwaway root.
    let cold_root = tempfile::tempdir().context("temp install root")?;
    let cold = setup::Setup::builder()
        .base_dir(cold_root.path().to_path_buf())
        .build();
    let cold_start = Instant::now();
    cold.install().await.context("cold setup::install()")?;
    let cold_elapsed = cold_start.elapsed();
    let cold_msb = cold_root.path().join("bin").join("msb");
    let cold_meta = std::fs::metadata(&cold_msb)
        .with_context(|| format!("cold install did not produce {}", cold_msb.display()))?;
    let cold_mtime = cold_meta.modified()?;

    let cold_again_start = Instant::now();
    cold.install().await.context("cold root re-install")?;
    let cold_again_elapsed = cold_again_start.elapsed();
    assert_eq!(
        cold_mtime,
        std::fs::metadata(&cold_msb)?.modified()?,
        "re-install into a populated root replaced bin/msb"
    );

    println!(
        "cold install into {} = {cold_elapsed:?}, re-install = {cold_again_elapsed:?}",
        cold_root.path().display()
    );
    println!(
        "  cold bin/: {:?}",
        describe_dir(&cold_root.path().join("bin"))?
    );
    println!(
        "  cold lib/: {:?}",
        describe_dir(&cold_root.path().join("lib"))?
    );
    assert_eq!(
        before_meta.len(),
        std::fs::metadata(&msb)?.len(),
        "installing into a custom base_dir disturbed the default root"
    );
    Ok(())
}

/// Boot a probe-sized alpine microVM (2 vCPU / 2 GiB, never the 8 GiB default).
async fn boot_alpine(name: &str) -> Result<Sandbox> {
    Sandbox::builder(name)
        .image(ALPINE_IMAGE)
        .cpus(PROBE_CPUS)
        .memory(PROBE_MEMORY_MIB)
        .create()
        .await
        .with_context(|| format!("boot alpine sandbox {name}"))
}

/// Reads stdin line by line and frames every line onto BOTH stdout and stderr.
const FRAMING_SCRIPT: &str = r#"
n=0
while IFS= read -r line; do
  n=$((n + 1))
  echo "OUT $n ${#line}"
  echo "ERR $n ${#line}" >&2
done
echo "OUT done $n"
echo "ERR done $n" >&2
"#;

#[tokio::test]
#[ignore = "ci-msb: boots a live microVM and streams >1 MiB through exec stdin"]
async fn ci_msb_exec_stream_pipes_stdin_and_streams_output() -> Result<()> {
    ensure_runtime_installed().await?;
    let _slot = acquire_vm_slot();

    let guard = SandboxGuard::new("exec");
    let sandbox = boot_alpine(guard.name()).await?;

    const LINES: usize = 1200;
    const LINE_LEN: usize = 1000;
    let mut payload = Vec::with_capacity(LINES * (LINE_LEN + 1));
    for _ in 0..LINES {
        payload.extend(std::iter::repeat_n(b'a', LINE_LEN));
        payload.push(b'\n');
    }
    assert!(
        payload.len() > 1024 * 1024,
        "probe must push more than 1 MiB through stdin, got {}",
        payload.len()
    );

    let started = Instant::now();
    let mut handle = sandbox
        .exec_stream_with("/bin/sh", |opts| {
            opts.args(["-c", FRAMING_SCRIPT]).stdin_pipe()
        })
        .await
        .context("start framing exec session")?;

    let sink = handle
        .take_stdin()
        .context("stdin_pipe() must expose an ExecSink")?;

    // Write concurrently with the read loop: the guest blocks on stdout once
    // its pipe buffer fills, so a write-everything-then-read shape deadlocks.
    let writer_payload = payload.clone();
    let writer = tokio::spawn(async move {
        // ExecSink::write() maps one call to one protocol frame and the frame
        // limit is 4 MiB, so the caller owns chunking.
        for chunk in writer_payload.chunks(64 * 1024) {
            sink.write(chunk).await?;
        }
        sink.close().await?;
        anyhow::Ok(())
    });

    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let mut stdout_events = 0usize;
    let mut stderr_events = 0usize;
    let mut first_stdout_at = None;
    let mut exit_code = None;
    let mut started_pid = None;
    while let Some(event) = handle.recv().await {
        match event {
            ExecEvent::Started { pid } => started_pid = Some(pid),
            ExecEvent::Stdout(bytes) => {
                stdout_events += 1;
                first_stdout_at.get_or_insert_with(|| started.elapsed());
                stdout.extend_from_slice(&bytes);
            }
            ExecEvent::Stderr(bytes) => {
                stderr_events += 1;
                stderr.extend_from_slice(&bytes);
            }
            ExecEvent::Exited { code } => exit_code = Some(code),
            ExecEvent::Failed(failed) => bail!("exec failed: {failed:?}"),
            ExecEvent::StdinError(err) => bail!("stdin error: {err:?}"),
        }
    }
    writer.await.context("stdin writer task")??;

    let stdout = String::from_utf8(stdout).context("stdout is utf-8")?;
    let stderr = String::from_utf8(stderr).context("stderr is utf-8")?;
    let stdout_lines: Vec<&str> = stdout.lines().collect();
    let stderr_lines: Vec<&str> = stderr.lines().collect();

    let expected_stdout: Vec<String> = (1..=LINES)
        .map(|n| format!("OUT {n} {LINE_LEN}"))
        .chain(std::iter::once(format!("OUT done {LINES}")))
        .collect();
    let expected_stderr: Vec<String> = (1..=LINES)
        .map(|n| format!("ERR {n} {LINE_LEN}"))
        .chain(std::iter::once(format!("ERR done {LINES}")))
        .collect();

    assert_eq!(
        stdout_lines, expected_stdout,
        "stdout must arrive complete and in order"
    );
    assert_eq!(
        stderr_lines, expected_stderr,
        "stderr must arrive complete and in order"
    );
    assert!(
        !stdout.contains("ERR "),
        "stderr content leaked into the stdout stream"
    );
    assert!(
        !stderr.contains("OUT "),
        "stdout content leaked into the stderr stream"
    );
    assert!(
        stdout_events > 1 && stderr_events > 1,
        "output arrived as {stdout_events} stdout / {stderr_events} stderr events; \
         that is buffering, not streaming"
    );
    assert!(started_pid.is_some(), "no Started{{pid}} event");

    let code = exit_code.context("no Exited event")?;
    assert_eq!(code, 0, "framing script exited {code}");

    // ExitStatus mapping, observed through wait() in both directions.
    let mut ok_exec = sandbox
        .exec_stream("/bin/sh", ["-c", "exit 0"])
        .await
        .context("start succeeding exec")?;
    let ok_status = ok_exec.wait().await.context("wait for succeeding exec")?;
    assert_eq!(ok_status.code, 0);
    assert!(
        ok_status.success,
        "ExitStatus.success must be true on code 0"
    );

    let mut failing = sandbox
        .exec_stream("/bin/sh", ["-c", "exit 7"])
        .await
        .context("start failing exec")?;
    let failing_status = failing.wait().await.context("wait for failing exec")?;
    assert_eq!(failing_status.code, 7);
    assert!(!failing_status.success);

    // One ExecSink::write() == one protocol frame, and the frame limit is
    // 4 MiB. Record what an oversized write does: the write is rejected AND
    // the agent connection for that exec session is torn down, so a turn's
    // stdin writer must chunk. The sandbox itself must survive.
    let mut oversized = sandbox
        .exec_stream_with("/bin/sh", |opts| {
            opts.args(["-c", "cat > /dev/null"]).stdin_pipe()
        })
        .await?;
    let oversized_sink = oversized.take_stdin().context("stdin sink")?;
    let oversized_error = oversized_sink
        .write(vec![b'x'; 5 * 1024 * 1024])
        .await
        .expect_err("a 5 MiB single write must be rejected by the 4 MiB frame limit");
    let oversized_error = format!("{oversized_error:?}");
    assert!(
        oversized_error.contains("FrameTooLarge"),
        "unexpected oversized-write error: {oversized_error}"
    );
    println!("5 MiB single ExecSink::write -> {oversized_error}");
    drop(oversized);

    let survived = sandbox
        .exec("/bin/sh", ["-c", "echo alive"])
        .await
        .context("sandbox must still be usable after a rejected oversized write")?;
    assert_eq!(survived.stdout()?.trim(), "alive");
    assert_eq!(sandbox.status().await?, SandboxStatus::Running);

    println!(
        "streamed {} stdin bytes; first stdout after {:?}; {stdout_events} stdout events, \
         {stderr_events} stderr events; total {:?}",
        payload.len(),
        first_stdout_at.context("no stdout event")?,
        started.elapsed()
    );

    drop(sandbox);
    guard.destroy().await?;
    Ok(())
}

#[tokio::test]
#[ignore = "ci-msb: runs a 60s exec against a 10s idle timeout on a live microVM"]
async fn ci_msb_long_turn_outlives_idle_timeout() -> Result<()> {
    ensure_runtime_installed().await?;
    let _slot = acquire_vm_slot();

    const IDLE_TIMEOUT_SECS: u64 = 10;
    const TICKS: usize = 12;
    const TICK_SECS: u64 = 5;

    let guard = SandboxGuard::new("exec");
    let sandbox = Sandbox::builder(guard.name())
        .image(ALPINE_IMAGE)
        .cpus(PROBE_CPUS)
        .memory(PROBE_MEMORY_MIB)
        .idle_timeout(IDLE_TIMEOUT_SECS)
        .create()
        .await
        .context("boot sandbox with a 10s idle timeout")?;

    let script = format!(
        "i=0; while [ $i -lt {TICKS} ]; do i=$((i + 1)); echo \"tick $i\"; sleep {TICK_SECS}; done"
    );

    let started = Instant::now();
    let mut handle = sandbox
        .exec_stream("/bin/sh", ["-c", script.as_str()])
        .await
        .context("start long-running exec")?;

    // Nothing here calls touch()/ping(): the exec session alone must hold the
    // idle monitor off for six consecutive idle-timeout windows.
    let mut ticks = Vec::new();
    let mut exit_code = None;
    while let Some(event) = handle.recv().await {
        match event {
            ExecEvent::Stdout(bytes) => {
                for line in String::from_utf8_lossy(&bytes).lines() {
                    if !line.is_empty() {
                        ticks.push((line.to_string(), started.elapsed()));
                    }
                }
            }
            ExecEvent::Exited { code } => exit_code = Some(code),
            ExecEvent::Failed(failed) => bail!("long exec failed: {failed:?}"),
            ExecEvent::Started { .. } | ExecEvent::Stderr(_) | ExecEvent::StdinError(_) => {}
        }
    }
    let total = started.elapsed();

    assert_eq!(exit_code, Some(0), "long exec did not complete cleanly");
    assert_eq!(ticks.len(), TICKS, "expected {TICKS} ticks, got {ticks:?}");
    assert_eq!(ticks[TICKS - 1].0, format!("tick {TICKS}"));
    let span = ticks[TICKS - 1].1;
    assert!(
        span >= Duration::from_secs((TICKS as u64 - 1) * TICK_SECS),
        "last tick arrived after only {span:?}; the turn was not long enough to cross the \
         {IDLE_TIMEOUT_SECS}s idle timeout"
    );
    assert_eq!(
        sandbox.status().await?,
        SandboxStatus::Running,
        "idle detection killed the sandbox during a long turn"
    );

    println!(
        "long turn: {} ticks over {total:?} with idle_timeout={IDLE_TIMEOUT_SECS}s; \
         first tick at {:?}, last at {span:?}",
        ticks.len(),
        ticks[0].1
    );

    // Control: with the exec finished and nothing touching the sandbox, the
    // same idle timeout must actually fire — otherwise the probe above proves
    // nothing about idle detection being active.
    let idle_deadline = Instant::now() + Duration::from_secs(IDLE_TIMEOUT_SECS * 6);
    let mut idle_status = sandbox.status().await?;
    while idle_status == SandboxStatus::Running && Instant::now() < idle_deadline {
        tokio::time::sleep(Duration::from_secs(2)).await;
        idle_status = sandbox.status().await?;
    }
    println!(
        "after the turn, idle detection moved the sandbox to {idle_status:?} within {:?}",
        Duration::from_secs(IDLE_TIMEOUT_SECS * 6)
    );
    assert_ne!(
        idle_status,
        SandboxStatus::Running,
        "idle_timeout={IDLE_TIMEOUT_SECS}s never fired even when the sandbox was idle, so the \
         long-turn result above does not demonstrate anything"
    );

    drop(sandbox);
    guard.destroy().await?;
    Ok(())
}

/// Hex SHA-256 of a host file.
fn sha256_file(path: &std::path::Path) -> Result<String> {
    use sha2::{Digest, Sha256};
    let bytes = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    Ok(hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

#[tokio::test]
#[ignore = "ci-msb: moves a >4 MiB file through the guest filesystem API on a live microVM"]
async fn ci_msb_guest_fs_roundtrip() -> Result<()> {
    ensure_runtime_installed().await?;
    let _slot = acquire_vm_slot();

    let guard = SandboxGuard::new("exec");
    let sandbox = boot_alpine(guard.name()).await?;
    let fs = sandbox.fs();

    // Deliberately larger than the 4 MiB protocol frame limit, so the transfer
    // has to chunk internally, and incompressible enough to be a real payload.
    const BIG_LEN: usize = 6 * 1024 * 1024 + 12_345;
    let mut big = Vec::with_capacity(BIG_LEN);
    let mut state = 0x243f_6a88_85a3_08d3u64;
    while big.len() < BIG_LEN {
        state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        big.extend_from_slice(&state.to_le_bytes());
    }
    big.truncate(BIG_LEN);

    let host_dir = tempfile::tempdir()?;
    let out_path = host_dir.path().join("payload.bin");
    std::fs::write(&out_path, &big)?;
    let host_digest = sha256_file(&out_path)?;

    fs.mkdir("/sandbox/work").await.context("mkdir guest dir")?;

    let push_start = Instant::now();
    fs.copy_from_host(&out_path, "/sandbox/work/payload.bin")
        .await
        .context("copy_from_host of a >4 MiB file")?;
    let push_elapsed = push_start.elapsed();

    let meta = fs.stat("/sandbox/work/payload.bin").await?;
    assert_eq!(meta.kind, microsandbox::sandbox::fs::FsEntryKind::File);
    assert_eq!(meta.size, BIG_LEN as u64, "guest file size mismatch");

    // The guest's own view of the bytes, independent of the transfer path.
    let guest_sum = sandbox
        .exec("/bin/sh", ["-c", "sha256sum /sandbox/work/payload.bin"])
        .await?;
    let guest_digest = guest_sum
        .stdout()?
        .split_whitespace()
        .next()
        .context("sha256sum produced no output")?
        .to_string();
    assert_eq!(
        guest_digest, host_digest,
        "bytes changed on the way into the guest"
    );

    let back_path = host_dir.path().join("payload.back.bin");
    let pull_start = Instant::now();
    fs.copy_to_host("/sandbox/work/payload.bin", &back_path)
        .await
        .context("copy_to_host of a >4 MiB file")?;
    let pull_elapsed = pull_start.elapsed();
    assert_eq!(
        sha256_file(&back_path)?,
        host_digest,
        "bytes changed on the way back to the host"
    );

    println!(
        "fs roundtrip: {BIG_LEN} bytes push {push_elapsed:?}, pull {pull_elapsed:?}, \
         sha256 {host_digest}"
    );

    // The small-file surface the design leans on for platform files.
    fs.write("/sandbox/work/hello.txt", b"hello guest\n")
        .await
        .context("fs write")?;
    assert_eq!(
        fs.read_to_string("/sandbox/work/hello.txt").await?,
        "hello guest\n"
    );
    assert_eq!(
        fs.read("/sandbox/work/hello.txt").await?.as_ref(),
        b"hello guest\n"
    );

    fs.mkdir("/sandbox/work/nested/deeper")
        .await
        .context("mkdir creates parents")?;
    assert!(fs.exists("/sandbox/work/nested/deeper").await?);

    let mut listed: Vec<(String, microsandbox::sandbox::fs::FsEntryKind)> = fs
        .list("/sandbox/work")
        .await?
        .into_iter()
        .map(|entry| (entry.path.clone(), entry.kind))
        .collect();
    listed.sort_by(|left, right| left.0.cmp(&right.0));
    println!("list /sandbox/work -> {listed:?}");
    assert!(listed.iter().any(|(path, kind)| path.ends_with("hello.txt")
        && *kind == microsandbox::sandbox::fs::FsEntryKind::File));
    assert!(listed.iter().any(|(path, kind)| path.ends_with("nested")
        && *kind == microsandbox::sandbox::fs::FsEntryKind::Directory));

    let hello_meta = fs.stat("/sandbox/work/hello.txt").await?;
    assert_eq!(hello_meta.size, 12);
    assert_eq!(hello_meta.uid, 0, "fs API writes as root by default");
    println!(
        "stat hello.txt -> kind={:?} size={} mode={:o} uid={} gid={} readonly={}",
        hello_meta.kind,
        hello_meta.size,
        hello_meta.mode,
        hello_meta.uid,
        hello_meta.gid,
        hello_meta.readonly
    );

    fs.remove("/sandbox/work/hello.txt")
        .await
        .context("fs remove")?;
    assert!(!fs.exists("/sandbox/work/hello.txt").await?);
    let missing = fs.read("/sandbox/work/hello.txt").await;
    assert!(missing.is_err(), "reading a removed file must fail");
    println!("read after remove -> {:?}", missing.err());

    drop(sandbox);
    guard.destroy().await?;
    Ok(())
}

/// Run `script` under `/bin/sh -c`, optionally as `user`, returning
/// `(exit code, stdout, stderr)`.
async fn run(sandbox: &Sandbox, user: Option<&str>, script: &str) -> Result<(i32, String, String)> {
    let output = sandbox
        .exec_with("/bin/sh", |opts| {
            let opts = opts.args(["-c", script]);
            match user {
                Some(user) => opts.user(user),
                None => opts,
            }
        })
        .await
        .with_context(|| format!("exec `{script}`"))?;
    Ok((output.status().code, output.stdout()?, output.stderr()?))
}

#[tokio::test]
#[ignore = "ci-msb: provisions a live microVM, restarts it, and checks persistence"]
async fn ci_msb_imperative_provisioning_persists_across_restart() -> Result<()> {
    ensure_runtime_installed().await?;
    let _slot = acquire_vm_slot();

    let guard = SandboxGuard::new("exec");
    let sandbox = boot_alpine(guard.name()).await?;

    let (code, stdout, stderr) = run(
        &sandbox,
        None,
        "set -e; id -u; apk add --no-cache jq >/dev/null 2>&1; \
         mkdir -p /sandbox; echo provisioned-once > /sandbox/state.txt; jq --version",
    )
    .await?;
    assert_eq!(code, 0, "provisioning failed: {stderr}");
    let mut lines = stdout.lines();
    assert_eq!(
        lines.next(),
        Some("0"),
        "provisioning must run as root by default"
    );
    let jq_version = lines.next().context("no jq version")?.to_string();

    sandbox.stop().await.context("stop the sandbox")?;
    drop(sandbox);

    let handle = Sandbox::get(guard.name())
        .await
        .context("re-attach by name after stop")?;
    assert_eq!(
        handle.status_snapshot(),
        SandboxStatus::Stopped,
        "sandbox should be Stopped after stop()"
    );

    let restarted = handle
        .start_detached()
        .await
        .context("start the stopped sandbox detached")?;
    assert_eq!(restarted.status().await?, SandboxStatus::Running);

    let (code, stdout, stderr) = run(
        &restarted,
        None,
        "set -e; cat /sandbox/state.txt; jq --version",
    )
    .await?;
    assert_eq!(code, 0, "post-restart check failed: {stderr}");
    let mut lines = stdout.lines();
    assert_eq!(
        lines.next(),
        Some("provisioned-once"),
        "agent-written file did not survive the restart"
    );
    assert_eq!(
        lines.next(),
        Some(jq_version.as_str()),
        "imperatively installed package did not survive the restart"
    );

    println!(
        "restart persistence: /sandbox/state.txt and jq {jq_version} both survived stop/start"
    );

    guard.destroy().await?;
    Ok(())
}

#[tokio::test]
#[ignore = "ci-msb: checks the unprivileged guest user against platform files on a live microVM"]
async fn ci_msb_unprivileged_user_cannot_write_platform_files() -> Result<()> {
    ensure_runtime_installed().await?;
    let _slot = acquire_vm_slot();

    let guard = SandboxGuard::new("exec");
    let sandbox = boot_alpine(guard.name()).await?;

    // Provisioning runs as root: create the agent user, lay down a
    // platform-owned file, and hand /sandbox itself to the agent.
    let (code, stdout, stderr) = run(
        &sandbox,
        None,
        "set -e; \
         adduser -D -h /home/sandbox sandbox; \
         mkdir -p /sandbox/.platform; \
         echo platform-owned > /sandbox/.platform/marker; \
         chown -R root:root /sandbox/.platform; \
         chmod 755 /sandbox/.platform; \
         chmod a-w /sandbox/.platform/marker; \
         chown sandbox:sandbox /sandbox; \
         id -u; id -u sandbox",
    )
    .await?;
    assert_eq!(code, 0, "provisioning failed: {stderr}");
    let mut ids = stdout.lines();
    assert_eq!(ids.next(), Some("0"), "provisioning must run as root");
    let agent_uid: u32 = ids.next().context("no agent uid")?.parse()?;
    assert_ne!(agent_uid, 0);

    // Turns run as the unprivileged user.
    let (code, stdout, _) = run(&sandbox, Some("sandbox"), "id -u; id -un").await?;
    assert_eq!(code, 0);
    assert_eq!(
        stdout.lines().collect::<Vec<_>>(),
        vec![agent_uid.to_string().as_str(), "sandbox"],
        "exec .user(\"sandbox\") did not switch users"
    );

    let (code, _, stderr) = run(
        &sandbox,
        Some("sandbox"),
        "echo tampered > /sandbox/.platform/marker",
    )
    .await?;
    assert_ne!(code, 0, "the agent user was able to write a platform file");
    println!(
        "write platform file as agent -> code {code}: {}",
        stderr.trim()
    );

    let (code, stdout, stderr) =
        run(&sandbox, Some("sandbox"), "cat /sandbox/.platform/marker").await?;
    assert_eq!(
        code, 0,
        "the agent user cannot read the platform file: {stderr}"
    );
    assert_eq!(stdout.trim(), "platform-owned");

    let (code, _, stderr) = run(
        &sandbox,
        Some("sandbox"),
        "echo agent-owned > /sandbox/notes.txt && cat /sandbox/notes.txt",
    )
    .await?;
    assert_eq!(
        code, 0,
        "the agent user cannot write its own /sandbox: {stderr}"
    );

    let (code, _, stderr) = run(
        &sandbox,
        Some("sandbox"),
        "touch /sandbox/.platform/injected",
    )
    .await?;
    assert_ne!(code, 0, "the agent user created a file in the platform dir");
    println!(
        "create in platform dir as agent -> code {code}: {}",
        stderr.trim()
    );

    let (code, _, stderr) =
        run(&sandbox, Some("sandbox"), "rm -f /sandbox/.platform/marker").await?;
    assert_ne!(code, 0, "the agent user unlinked a platform file");
    println!(
        "unlink platform file as agent -> code {code}: {}",
        stderr.trim()
    );

    // The platform directory itself lives inside an agent-writable /sandbox, so
    // record whether the agent can move it aside. Run last: it is destructive.
    let (rename_code, _, rename_stderr) = run(
        &sandbox,
        Some("sandbox"),
        "mv /sandbox/.platform /sandbox/.platform.moved",
    )
    .await?;
    println!(
        "rename the platform DIRECTORY as agent -> code {rename_code}: {}",
        rename_stderr.trim()
    );

    drop(sandbox);
    guard.destroy().await?;
    Ok(())
}

/// Read the agent's Claude OAuth token out of the host's per-agent database.
///
/// The value is returned in memory and never printed, logged, written to a
/// file, or embedded in an assertion message.
fn host_oauth_token() -> Result<String> {
    let db = PathBuf::from(std::env::var("HOME").expect("HOME is set"))
        .join(".right/agents/him/data.db");
    if !db.exists() {
        bail!("no host token database at {}", db.display());
    }
    let output = std::process::Command::new("sqlite3")
        .arg("-readonly")
        .arg(&db)
        .arg("SELECT token FROM auth_tokens LIMIT 1;")
        .output()
        .with_context(|| format!("run sqlite3 against {}", db.display()))?;
    if !output.status.success() {
        bail!(
            "sqlite3 failed on {}: {}",
            db.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let token = String::from_utf8(output.stdout)
        .context("token is not utf-8")?
        .trim()
        .to_string();
    if token.is_empty() {
        bail!("auth_tokens is empty in {}", db.display());
    }
    Ok(token)
}

/// Peak resource observation taken from the host-side metrics sampler.
#[derive(Debug, Default)]
struct Peak {
    samples: usize,
    memory_bytes: u64,
    memory_limit_bytes: u64,
    min_available_bytes: Option<u64>,
    upper_used_bytes: Option<u64>,
    max_cpu_percent: f32,
}

/// Sample `handle.metrics()` until `stop` flips, returning the peaks.
async fn sample_peaks(name: String, stop: std::sync::Arc<std::sync::atomic::AtomicBool>) -> Peak {
    use std::sync::atomic::Ordering;

    let mut peak = Peak::default();
    while !stop.load(Ordering::Relaxed) {
        if let Ok(handle) = Sandbox::get(&name).await
            && let Ok(metrics) = handle.metrics().await
        {
            peak.samples += 1;
            peak.memory_bytes = peak.memory_bytes.max(metrics.memory_bytes);
            peak.memory_limit_bytes = metrics.memory_limit_bytes;
            peak.max_cpu_percent = peak.max_cpu_percent.max(metrics.cpu_percent);
            if let Some(available) = metrics.memory_available_bytes {
                peak.min_available_bytes = Some(
                    peak.min_available_bytes
                        .map_or(available, |current| current.min(available)),
                );
            }
            if let Some(used) = metrics.upper_used_bytes {
                peak.upper_used_bytes = Some(
                    peak.upper_used_bytes
                        .map_or(used, |current| current.max(used)),
                );
            }
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    peak
}

#[tokio::test]
#[ignore = "ci-msb: 8 GiB microVM running one real Claude Code turn"]
async fn ci_msb_default_resources_sustain_a_real_turn() -> Result<()> {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    ensure_runtime_installed().await?;
    // The designed default is 8 GiB; only one such VM may exist on this host.
    let _exclusive = common::acquire_file_lock("rt-msb-eight-gib-vm");
    let _slot = acquire_vm_slot();

    let token = host_oauth_token().context(
        "BLOCKED: cannot read the host Claude OAuth token; the real-turn probe needs it",
    )?;
    let redact = |text: &str| text.replace(token.as_str(), "<redacted>");

    let guard = SandboxGuard::new("exec");
    let boot_start = Instant::now();
    let sandbox = Sandbox::builder(guard.name())
        .image(NODE_IMAGE)
        .cpus(2)
        .memory(8192u32)
        .root_disk(16384u32)
        .create()
        .await
        .context("boot the designed-default sandbox (2 vCPU / 8 GiB / 16 GiB root disk)")?;
    let boot_elapsed = boot_start.elapsed();

    let stop = Arc::new(AtomicBool::new(false));
    let sampler = tokio::spawn(sample_peaks(guard.name().to_string(), stop.clone()));

    let provision_start = Instant::now();
    let (code, _, stderr) = run(
        &sandbox,
        None,
        "set -e; \
         useradd -m -s /bin/bash agent; \
         mkdir -p /sandbox; chown agent:agent /sandbox; \
         npm i -g @anthropic-ai/claude-code >/dev/null; \
         command -v claude",
    )
    .await?;
    let provision_elapsed = provision_start.elapsed();
    assert_eq!(code, 0, "provisioning failed: {}", redact(&stderr));

    let turn_start = Instant::now();
    let output = sandbox
        .exec_with("/bin/sh", |opts| {
            opts.args([
                "-c",
                "claude -p --output-format json -- 'reply with the single word ok'",
            ])
            .user("agent")
            .cwd("/sandbox")
            .env("CLAUDE_CODE_OAUTH_TOKEN", token.as_str())
            .timeout(Duration::from_secs(600))
        })
        .await
        .context("run the claude turn")?;
    let turn_elapsed = turn_start.elapsed();
    let turn_code = output.status().code;
    let turn_stdout = redact(&output.stdout()?);
    let turn_stderr = redact(&output.stderr()?);

    let (_, meminfo, _) = run(
        &sandbox,
        None,
        "grep -E 'MemTotal|MemAvailable' /proc/meminfo; df -h / | tail -1",
    )
    .await?;

    stop.store(true, Ordering::Relaxed);
    let peak = sampler.await.context("metrics sampler")?;

    println!("boot {boot_elapsed:?}, provisioning {provision_elapsed:?}, turn {turn_elapsed:?}");
    println!("guest: {}", meminfo.replace('\n', " | "));
    println!("host metrics peak: {peak:?}");
    assert_eq!(
        turn_code, 0,
        "claude turn exited {turn_code}\nstdout: {turn_stdout}\nstderr: {turn_stderr}"
    );

    let parsed: serde_json::Value = serde_json::from_str(turn_stdout.trim())
        .with_context(|| format!("claude --output-format json produced non-JSON: {turn_stdout}"))?;
    let result = parsed
        .get("result")
        .and_then(|value| value.as_str())
        .with_context(|| format!("no string `result` field in {parsed}"))?;
    assert!(
        result.to_lowercase().contains("ok"),
        "unexpected turn result: {result}"
    );
    println!(
        "turn result = {result:?}; is_error = {:?}; duration_ms = {:?}; num_turns = {:?}",
        parsed.get("is_error"),
        parsed.get("duration_ms"),
        parsed.get("num_turns")
    );

    assert!(
        peak.samples > 0,
        "no host metrics samples were collected during the turn"
    );
    assert!(
        peak.min_available_bytes
            .is_none_or(|available| available > 0),
        "the guest ran out of available memory during the turn"
    );

    drop(sandbox);
    guard.destroy().await?;
    Ok(())
}
