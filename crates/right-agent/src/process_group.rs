//! `ProcessGroupChild` — a newtype around `tokio::process::Child` that
//! spawns the child in a new process group and kills the entire group on
//! Drop via `killpg(SIGKILL)`.
//!
//! Rationale: tokio's `kill_on_drop(true)` only SIGKILLs the direct child.
//! When the child is `ssh` (which spawns `ProxyCommand`) or `openshell
//! sandbox upload` (which spawns `ssh` which spawns `ssh-proxy`), those
//! grandchildren are reparented to launchd/init and survive indefinitely.
//! Putting the child into its own process group lets us atomically reap
//! the whole tree with one `killpg` syscall.
//!
//! All methods take `&mut self`, so Drop remains armed at every `.await`
//! suspension point. This makes `ProcessGroupChild` cancel-safe under
//! `tokio::time::timeout` and `tokio::select!` — when the awaiting task
//! is dropped, Drop fires and the whole process group is SIGKILLed.
//!
//! **Do not use for the first ssh call against a `ControlMaster auto`
//! config.** The master and its ProxyCommand share the spawning ssh's
//! process group; `killpg(SIGKILL)` would kill the ProxyCommand and
//! defeat multiplexing. See `ssh_exec` in `openshell.rs` for the pattern
//! that establishes the master safely.

use nix::sys::signal::{Signal, killpg};
use nix::unistd::Pid;
use tokio::io::AsyncReadExt;
use tokio::process::{Child, Command};

/// A child process handle that kills its entire process group on Drop.
pub struct ProcessGroupChild {
    inner: Child,
    /// Process group id. `None` only if the child was reaped before
    /// `spawn()` returned (should not happen in practice).
    pgid: Option<i32>,
}

impl ProcessGroupChild {
    /// Spawn `cmd` as the leader of a new process group. The returned
    /// handle kills the entire group on Drop via `killpg(SIGKILL)`.
    pub fn spawn(mut cmd: Command) -> std::io::Result<Self> {
        cmd.process_group(0);
        let inner = cmd.spawn()?;
        let pgid = inner.id().map(|p| p as i32);
        Ok(Self { inner, pgid })
    }

    pub fn id(&self) -> Option<u32> {
        self.inner.id()
    }

    pub async fn wait(&mut self) -> std::io::Result<std::process::ExitStatus> {
        self.inner.wait().await
    }

    /// Drives the child to completion, collecting stdout + stderr. Unlike
    /// `tokio::process::Child::wait_with_output`, this takes `&mut self` so
    /// cancellation of the outer future (timeout, select branch loss, task
    /// abort) runs `Drop` on `self`, which SIGKILLs the whole process group.
    /// The previous `self`-consuming signature was not cancel-safe.
    pub async fn wait_with_output(&mut self) -> std::io::Result<std::process::Output> {
        async fn read_to_end<A: tokio::io::AsyncRead + Unpin>(
            io: &mut Option<A>,
        ) -> std::io::Result<Vec<u8>> {
            let mut vec = Vec::new();
            if let Some(io) = io.as_mut() {
                io.read_to_end(&mut vec).await?;
            }
            Ok(vec)
        }

        let mut stdout_pipe = self.inner.stdout.take();
        let mut stderr_pipe = self.inner.stderr.take();

        let (status, stdout, stderr) = tokio::try_join!(
            self.inner.wait(),
            read_to_end(&mut stdout_pipe),
            read_to_end(&mut stderr_pipe),
        )?;

        Ok(std::process::Output {
            status,
            stdout,
            stderr,
        })
    }

    pub async fn kill(&mut self) -> std::io::Result<()> {
        self.inner.kill().await
    }

    pub fn stdout(&mut self) -> Option<tokio::process::ChildStdout> {
        self.inner.stdout.take()
    }

    pub fn stderr(&mut self) -> Option<tokio::process::ChildStderr> {
        self.inner.stderr.take()
    }

    pub fn stdin(&mut self) -> Option<tokio::process::ChildStdin> {
        self.inner.stdin.take()
    }
}

impl Drop for ProcessGroupChild {
    fn drop(&mut self) {
        if let Some(pgid) = self.pgid {
            // Best-effort. ESRCH (group already gone) and EPERM (macOS returns
            // this instead of ESRCH for reaped process groups) are fine to ignore.
            if let Err(e) = killpg(Pid::from_raw(pgid), Signal::SIGKILL)
                && e != nix::errno::Errno::ESRCH
                && e != nix::errno::Errno::EPERM
            {
                tracing::warn!(pgid, error = %e, "killpg failed during ProcessGroupChild drop");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tempfile::NamedTempFile;

    fn is_alive(pid: i32) -> bool {
        nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None).is_ok()
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn drop_kills_grandchild() {
        let pid_file = NamedTempFile::new().expect("tmpfile");
        let pid_path = pid_file.path().to_str().expect("utf-8").to_owned();

        let mut cmd = Command::new("bash");
        cmd.arg("-c")
            .arg(format!("sleep 600 & echo $! > {pid_path}; wait"));
        cmd.stdout(std::process::Stdio::null());
        cmd.stderr(std::process::Stdio::null());

        let child = ProcessGroupChild::spawn(cmd).expect("spawn");
        let parent_pid = child.id().expect("pid") as i32;

        tokio::time::sleep(Duration::from_millis(200)).await;
        let grandchild_pid: i32 = std::fs::read_to_string(&pid_path)
            .expect("pid file")
            .trim()
            .parse()
            .expect("parse");

        assert!(is_alive(parent_pid), "parent alive before drop");
        assert!(is_alive(grandchild_pid), "grandchild alive before drop");

        drop(child);
        tokio::time::sleep(Duration::from_millis(200)).await;

        assert!(!is_alive(parent_pid), "parent dead after drop");
        assert!(!is_alive(grandchild_pid), "grandchild dead after drop");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn control_without_group_leaks_grandchild() {
        let pid_file = NamedTempFile::new().expect("tmpfile");
        let pid_path = pid_file.path().to_str().expect("utf-8").to_owned();

        let mut cmd = Command::new("bash");
        cmd.kill_on_drop(true);
        cmd.arg("-c")
            .arg(format!("sleep 600 & echo $! > {pid_path}; wait"));
        cmd.stdout(std::process::Stdio::null());
        cmd.stderr(std::process::Stdio::null());

        let child = cmd.spawn().expect("spawn");
        let parent_pid = child.id().expect("pid") as i32;

        tokio::time::sleep(Duration::from_millis(200)).await;
        let grandchild_pid: i32 = std::fs::read_to_string(&pid_path)
            .expect("pid file")
            .trim()
            .parse()
            .expect("parse");

        assert!(is_alive(parent_pid), "parent alive before drop");
        assert!(is_alive(grandchild_pid), "grandchild alive before drop");

        drop(child);
        tokio::time::sleep(Duration::from_millis(200)).await;

        assert!(!is_alive(parent_pid), "parent killed by kill_on_drop");
        assert!(
            is_alive(grandchild_pid),
            "control: grandchild must survive without process_group(0)"
        );

        let _ = nix::sys::signal::kill(
            nix::unistd::Pid::from_raw(grandchild_pid),
            nix::sys::signal::Signal::SIGKILL,
        );
    }

    /// Mirrors the SSH ControlMaster hang seen in production (2026-05-05):
    /// the slave ssh forwards its stdio FDs to the master via SCM_RIGHTS,
    /// so SIGKILLing the slave leaves the pipe write-end held by another
    /// process and an unbounded `read_to_string(stderr)` never sees EOF.
    ///
    /// We can't bring up a real ssh master in a unit test, but the failure
    /// mode is general: any process whose backgrounded child inherits stderr
    /// leaves the FD open after the direct child dies. This test exercises
    /// that shape and asserts the worker/cron post-break read pattern —
    /// `tokio::time::timeout` around `read_to_string` — returns within its
    /// budget instead of hanging.
    #[tokio::test(flavor = "multi_thread")]
    async fn bounded_stderr_read_returns_when_grandchild_holds_fd() {
        use tokio::io::{AsyncReadExt, BufReader};

        let mut cmd = Command::new("bash");
        // Background a sleep that inherits stderr (FD 2), then echo a marker
        // and wait. SIGKILLing bash leaves the orphan sleep holding the FD.
        cmd.arg("-c").arg("sleep 30 & echo started >&2; wait");
        cmd.stdout(std::process::Stdio::null());
        cmd.stderr(std::process::Stdio::piped());

        let mut child = ProcessGroupChild::spawn(cmd).expect("spawn");
        let stderr = child.stderr().expect("stderr piped");
        let mut reader = BufReader::new(stderr);

        // Synchronise on the marker so we know the backgrounded sleep has
        // forked and inherited the FD before we kill bash.
        let mut marker = String::new();
        tokio::time::timeout(Duration::from_secs(2), async {
            use tokio::io::AsyncBufReadExt;
            reader.read_line(&mut marker).await
        })
        .await
        .expect("marker read should not time out")
        .expect("marker read");
        assert_eq!(marker.trim(), "started");

        // Kill the direct child (bash). The backgrounded sleep survives and
        // keeps its inherited stderr FD open.
        child.kill().await.expect("kill bash");
        let _ = tokio::time::timeout(Duration::from_millis(500), child.wait()).await;

        // Bounded drain — exactly what worker.rs/cron.rs do post-break. With
        // an unbounded read this would block for the orphan sleep's full 30s
        // (or until the surrounding test runner times out).
        let started = tokio::time::Instant::now();
        let mut buf = String::new();
        let _ =
            tokio::time::timeout(Duration::from_secs(1), reader.read_to_string(&mut buf)).await;
        let elapsed = started.elapsed();

        assert!(
            elapsed < Duration::from_secs(3),
            "bounded read returned in {elapsed:?} — should be ≤ ~1s",
        );
        // child's Drop fires killpg on the whole group, reaping the orphan.
    }

    /// Regression test for the cancel-safety bug: tokio::time::timeout
    /// wrapping wait_with_output must kill the process group when the
    /// timeout elapses. The previous `wait_with_output(self)` signature
    /// used mem::forget and leaked the group.
    #[tokio::test(flavor = "multi_thread")]
    async fn timeout_on_wait_with_output_kills_group() {
        let pid_file = NamedTempFile::new().expect("tmpfile");
        let pid_path = pid_file.path().to_str().expect("utf-8").to_owned();

        let mut cmd = Command::new("bash");
        cmd.arg("-c")
            .arg(format!("sleep 600 & echo $! > {pid_path}; wait"));
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());

        let mut child = ProcessGroupChild::spawn(cmd).expect("spawn");
        let parent_pid = child.id().expect("pid") as i32;

        tokio::time::sleep(Duration::from_millis(200)).await;
        let grandchild_pid: i32 = std::fs::read_to_string(&pid_path)
            .expect("pid file")
            .trim()
            .parse()
            .expect("parse");

        assert!(is_alive(parent_pid), "parent alive before timeout");
        assert!(is_alive(grandchild_pid), "grandchild alive before timeout");

        // Wrap wait_with_output in a short timeout. When timeout fires,
        // the inner future drops — Drop on ProcessGroupChild must fire.
        let result =
            tokio::time::timeout(Duration::from_millis(200), child.wait_with_output()).await;
        assert!(result.is_err(), "timeout must elapse");

        drop(child);
        tokio::time::sleep(Duration::from_millis(200)).await;

        assert!(!is_alive(parent_pid), "parent dead after timeout + drop");
        assert!(
            !is_alive(grandchild_pid),
            "grandchild dead after timeout + drop"
        );
    }
}
