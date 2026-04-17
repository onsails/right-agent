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
//! Note: `wait_with_output(self)` is NOT cancel-safe. If the awaiting
//! task is cancelled (task abort or losing a `tokio::select!` branch),
//! the process group is orphaned — `mem::forget(self)` has already
//! disarmed the Drop-based cleanup. Callers that need cancel-safe
//! cleanup should call `wait()`/`kill()` on `&mut self` instead.

use std::mem::ManuallyDrop;

use nix::sys::signal::{Signal, killpg};
use nix::unistd::Pid;
use tokio::process::{Child, Command};

/// A child process handle that kills its entire process group on Drop.
pub struct ProcessGroupChild {
    inner: ManuallyDrop<Child>,
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
        Ok(Self {
            inner: ManuallyDrop::new(inner),
            pgid,
        })
    }

    pub fn id(&self) -> Option<u32> {
        self.inner.id()
    }

    pub async fn wait(&mut self) -> std::io::Result<std::process::ExitStatus> {
        self.inner.wait().await
    }

    pub async fn wait_with_output(mut self) -> std::io::Result<std::process::Output> {
        // SAFETY: Without `mem::forget(self)`, Drop would call
        // `ManuallyDrop::drop` on an already-moved value (UB) and also send
        // SIGKILL to the group that the awaited child still occupies.
        // `wait_with_output` drives the child to completion, so group cleanup
        // at this point is redundant.
        let inner = unsafe { ManuallyDrop::take(&mut self.inner) };
        std::mem::forget(self);
        inner.wait_with_output().await
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
            // Best-effort. ESRCH (group already gone) is fine to ignore.
            if let Err(e) = killpg(Pid::from_raw(pgid), Signal::SIGKILL) {
                if e != nix::errno::Errno::ESRCH {
                    tracing::warn!(
                        pgid,
                        error = %e,
                        "killpg failed during ProcessGroupChild drop"
                    );
                }
            }
        }
        // SAFETY: `inner` is only taken out via `wait_with_output`, which
        // forgets `self` before this Drop can run. So here the ManuallyDrop
        // still owns a live Child that we must drop. tokio's Child::Drop
        // schedules a non-blocking waitpid via its internal reaper; the
        // leader zombie is reaped asynchronously.
        unsafe {
            ManuallyDrop::drop(&mut self.inner);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tempfile::NamedTempFile;

    /// Given a bash parent that spawns a `sleep 600` grandchild, dropping
    /// the `ProcessGroupChild` must kill both within ~200ms.
    #[tokio::test(flavor = "multi_thread")]
    async fn drop_kills_grandchild() {
        let tmp = NamedTempFile::new().expect("tempfile");
        let pid_path = tmp.path().to_str().expect("utf-8");

        let mut cmd = Command::new("bash");
        cmd.arg("-c")
            .arg(format!("sleep 600 & echo $! > {pid_path}; wait"));
        cmd.stdout(std::process::Stdio::null());
        cmd.stderr(std::process::Stdio::null());

        let child = ProcessGroupChild::spawn(cmd).expect("spawn");
        let parent_pid = child.id().expect("pid");

        // Give bash time to spawn the sleep and write the pid file.
        tokio::time::sleep(Duration::from_millis(200)).await;
        let grandchild_pid: i32 = std::fs::read_to_string(tmp.path())
            .expect("grandchild pid file")
            .trim()
            .parse()
            .expect("parse pid");

        // Both alive before drop.
        assert!(is_alive(parent_pid as i32), "parent should be alive before drop");
        assert!(is_alive(grandchild_pid), "grandchild should be alive before drop");

        drop(child);

        // Give the signal time to propagate.
        tokio::time::sleep(Duration::from_millis(200)).await;

        assert!(!is_alive(parent_pid as i32), "parent must be dead after drop");
        assert!(!is_alive(grandchild_pid), "grandchild must be dead after drop");
    }

    /// Sanity check: without `process_group(0)`, a plain `Child` drop with
    /// `kill_on_drop(true)` kills the direct child but leaves the grandchild
    /// alive. This is the bug ProcessGroupChild exists to fix.
    #[tokio::test(flavor = "multi_thread")]
    async fn control_without_group_leaks_grandchild() {
        let tmp = NamedTempFile::new().expect("tempfile");
        let pid_path = tmp.path().to_str().expect("utf-8");

        let mut cmd = Command::new("bash");
        cmd.kill_on_drop(true);
        cmd.arg("-c")
            .arg(format!("sleep 600 & echo $! > {pid_path}; wait"));
        cmd.stdout(std::process::Stdio::null());
        cmd.stderr(std::process::Stdio::null());

        let child = cmd.spawn().expect("spawn");
        let parent_pid = child.id().expect("pid") as i32;

        tokio::time::sleep(Duration::from_millis(200)).await;
        let grandchild_pid: i32 = std::fs::read_to_string(tmp.path())
            .expect("grandchild pid file")
            .trim()
            .parse()
            .expect("parse pid");

        assert!(is_alive(parent_pid), "parent alive before drop");
        assert!(is_alive(grandchild_pid), "grandchild alive before drop");

        drop(child);
        tokio::time::sleep(Duration::from_millis(200)).await;

        assert!(!is_alive(parent_pid), "parent killed by kill_on_drop");
        assert!(
            is_alive(grandchild_pid),
            "control: grandchild must survive without process_group(0)"
        );

        // Cleanup the leaked grandchild so the test doesn't itself leak.
        let _ = nix::sys::signal::kill(
            nix::unistd::Pid::from_raw(grandchild_pid),
            nix::sys::signal::Signal::SIGKILL,
        );
    }

    fn is_alive(pid: i32) -> bool {
        // Signal 0 is a liveness probe: returns Ok if the process exists,
        // Err(ESRCH) if not.
        nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None).is_ok()
    }
}
