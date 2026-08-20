//! Guest-process transport for `claude -p`.
//!
//! Replaces the OpenShell-era `ssh -F <config> <host> -- <script>` child
//! process with a `right_sandbox` streaming exec session, behind a handle that
//! keeps the call sites' `tokio::process::Child` shape: piped stdout/stderr,
//! a stdin writer, `wait`, `wait_with_output`, `kill`, and kill-on-drop.
//!
//! Two properties are load-bearing:
//!
//! - **Stdin is chunked.** All guest stdin goes through
//!   [`right_sandbox::ChunkedStdin`], never a single oversized write: one SDK
//!   `write` is one protocol frame, and an over-cap frame tears the session
//!   down (stage-1 correction 5).
//! - **Dropping the handle kills the guest process**, the way
//!   `ProcessGroupChild::Drop` killed the ssh process group. Every call site
//!   races these handles under `tokio::time::timeout`/`select!` and relies on
//!   cancellation reaping the turn.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _, ReadHalf, SimplexStream, WriteHalf};
use tokio::sync::{mpsc, oneshot};

use right_sandbox::{ChunkedStdin, ExecEvent, ExecRequest, ExecStream, SandboxError, Stdin};

use crate::sandbox::{SANDBOX_HOME, Sandbox};

/// In-flight buffer for each guest output stream. Bounded so a guest that
/// floods stdout cannot grow the bot's heap without the reader keeping up.
const STREAM_BUFFER_BYTES: usize = 256 * 1024;

/// Read size for the stdin forwarder. [`ChunkedStdin`] re-chunks anyway; this
/// only bounds the intermediate copy.
const STDIN_READ_BYTES: usize = 64 * 1024;

/// What to do with one of the guest process's output streams.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Capture {
    /// Deliver the bytes to the caller.
    Pipe,
    /// Discard the bytes. The pump still drains them, so a guest that writes
    /// to an unread stream can never wedge the session.
    Null,
}

/// A finished guest process.
///
/// The sandbox transport reports an exit *code*, not a `std::process::Status`:
/// there is no host process and no signal to report.
#[derive(Debug, Clone)]
pub(crate) struct SandboxOutput {
    /// Guest process exit code.
    pub(crate) code: i32,
    /// Everything the process wrote to stdout (empty when captured `Null`).
    pub(crate) stdout: Vec<u8>,
    /// Everything the process wrote to stderr (empty when captured `Null`).
    pub(crate) stderr: Vec<u8>,
}

impl SandboxOutput {
    /// True when the guest process exited 0.
    pub(crate) fn success(&self) -> bool {
        self.code == 0
    }
}

/// A guest command, built but not yet started.
///
/// Always a shell invocation: every call site assembles a system-prompt
/// script plus the quoted `claude` argv, exactly as the SSH remote command did.
pub(crate) struct SandboxCommand {
    sandbox: Sandbox,
    script: String,
    env: Vec<(String, String)>,
    stdin: bool,
    stdout: Capture,
    stderr: Capture,
    timeout: Option<Duration>,
}

impl SandboxCommand {
    /// Run `script` under the guest shell. Stdin is closed and both output
    /// streams are discarded until the caller asks for them.
    pub(crate) fn shell(sandbox: &Sandbox, script: impl Into<String>) -> Self {
        Self {
            sandbox: Arc::clone(sandbox),
            script: script.into(),
            env: Vec::new(),
            stdin: false,
            stdout: Capture::Null,
            stderr: Capture::Null,
            timeout: None,
        }
    }

    /// Set a guest environment variable for this command only.
    ///
    /// This is how credentials reach the guest: never through the script text
    /// or argv, which are visible to anything that can list processes.
    pub(crate) fn env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.push((key.into(), value.into()));
        self
    }

    /// Give the command a stdin pipe.
    pub(crate) fn stdin_piped(mut self) -> Self {
        self.stdin = true;
        self
    }

    pub(crate) fn stdout(mut self, capture: Capture) -> Self {
        self.stdout = capture;
        self
    }

    pub(crate) fn stderr(mut self, capture: Capture) -> Self {
        self.stderr = capture;
        self
    }

    /// Hard cap on guest runtime. The guest process is SIGKILLed on expiry;
    /// callers that also race a host-side timeout keep doing so.
    pub(crate) fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    fn request(&self) -> ExecRequest {
        ExecRequest {
            cmd: "/bin/sh".to_owned(),
            args: vec!["-c".to_owned(), self.script.clone()],
            cwd: Some(SANDBOX_HOME.to_owned()),
            user: None,
            env: self.env.clone(),
            stdin: if self.stdin { Stdin::Pipe } else { Stdin::Null },
            timeout: self.timeout,
        }
    }

    /// Start the guest process.
    pub(crate) async fn spawn(self) -> Result<SandboxChild, SandboxProcessError> {
        let mut stream = self.sandbox.exec_stream(&self.request()).await?;
        let stdin = if self.stdin {
            stream.take_stdin()
        } else {
            None
        };
        Ok(SandboxChild::new(stream, stdin, self.stdout, self.stderr))
    }

    /// Start the guest process, collect both output streams, and wait.
    pub(crate) async fn output(self) -> Result<SandboxOutput, SandboxProcessError> {
        let mut child = self
            .stdout(Capture::Pipe)
            .stderr(Capture::Pipe)
            .spawn()
            .await?;
        child.wait_with_output().await
    }
}

/// A running guest process.
///
/// Shaped after `right_process::ProcessGroupChild`: every method takes
/// `&mut self` so `Drop` stays armed across `.await` points, and dropping the
/// handle kills the guest process.
pub(crate) struct SandboxChild {
    stdin: Option<SandboxStdin>,
    stdout: Option<ReadHalf<SimplexStream>>,
    stderr: Option<ReadHalf<SimplexStream>>,
    exit_rx: oneshot::Receiver<Result<i32, SandboxError>>,
    exit_code: Option<i32>,
    pid: Arc<AtomicU32>,
    kill_tx: mpsc::Sender<()>,
}

impl SandboxChild {
    fn new(
        stream: ExecStream,
        stdin: Option<ChunkedStdin>,
        stdout: Capture,
        stderr: Capture,
    ) -> Self {
        let (stdout_reader, stdout_writer) = pipe(stdout);
        let (stderr_reader, stderr_writer) = pipe(stderr);
        let (exit_tx, exit_rx) = oneshot::channel();
        let (kill_tx, kill_rx) = mpsc::channel(1);
        let pid = Arc::new(AtomicU32::new(0));

        tokio::spawn(pump_events(
            stream,
            kill_rx,
            stdout_writer,
            stderr_writer,
            exit_tx,
            Arc::clone(&pid),
        ));

        Self {
            stdin: stdin.map(SandboxStdin::new),
            stdout: stdout_reader,
            stderr: stderr_reader,
            exit_rx,
            exit_code: None,
            pid,
            kill_tx,
        }
    }

    /// The stdin writer, `Some` at most once and only for a command built with
    /// [`SandboxCommand::stdin_piped`]. Dropping it sends EOF.
    pub(crate) fn stdin(&mut self) -> Option<SandboxStdin> {
        self.stdin.take()
    }

    /// The stdout reader, `Some` at most once and only for `Capture::Pipe`.
    pub(crate) fn stdout(&mut self) -> Option<ReadHalf<SimplexStream>> {
        self.stdout.take()
    }

    /// The stderr reader, `Some` at most once and only for `Capture::Pipe`.
    pub(crate) fn stderr(&mut self) -> Option<ReadHalf<SimplexStream>> {
        self.stderr.take()
    }

    /// Guest PID, once the process has started.
    pub(crate) fn pid(&self) -> Option<u32> {
        match self.pid.load(Ordering::Relaxed) {
            0 => None,
            pid => Some(pid),
        }
    }

    /// Wait for the guest process to exit. Cancel-safe: the exit code is
    /// latched, so a caller that races this under a timeout can await again.
    pub(crate) async fn wait(&mut self) -> Result<i32, SandboxProcessError> {
        if let Some(code) = self.exit_code {
            return Ok(code);
        }
        let code = (&mut self.exit_rx)
            .await
            .map_err(|_| SandboxProcessError::PumpGone)??;
        self.exit_code = Some(code);
        Ok(code)
    }

    /// Drive the process to completion, collecting whatever was piped.
    ///
    /// Takes `&mut self` for the same reason `ProcessGroupChild` does:
    /// cancelling the outer future runs `Drop`, which kills the guest process.
    pub(crate) async fn wait_with_output(&mut self) -> Result<SandboxOutput, SandboxProcessError> {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let mut stdout_reader = self.stdout.take();
        let mut stderr_reader = self.stderr.take();
        // Drop the stdin writer first: a guest reading to EOF would otherwise
        // never exit, exactly as with a leaked `ChildStdin`.
        self.stdin = None;

        let drain_stdout = async {
            match stdout_reader.as_mut() {
                Some(reader) => reader.read_to_end(&mut stdout).await.map(|_| ()),
                None => Ok(()),
            }
        };
        let drain_stderr = async {
            match stderr_reader.as_mut() {
                Some(reader) => reader.read_to_end(&mut stderr).await.map(|_| ()),
                None => Ok(()),
            }
        };
        let (stdout_result, stderr_result, code) =
            tokio::join!(drain_stdout, drain_stderr, self.wait());
        // A read error on an in-process pipe means the pump died mid-stream;
        // surface it rather than reporting a truncated capture as complete.
        stdout_result.map_err(|source| SandboxProcessError::Pipe {
            stream: "stdout",
            source,
        })?;
        stderr_result.map_err(|source| SandboxProcessError::Pipe {
            stream: "stderr",
            source,
        })?;

        Ok(SandboxOutput {
            code: code?,
            stdout,
            stderr,
        })
    }

    /// Kill the guest process. Idempotent; a finished process is a no-op, and
    /// the kill itself is reported by the pump task, not awaited here.
    pub(crate) async fn kill(&mut self) {
        if self.kill_tx.send(()).await.is_err() {
            tracing::trace!("sandbox child killed after its exec session ended");
        }
    }
}

impl Drop for SandboxChild {
    fn drop(&mut self) {
        // Mirrors `ProcessGroupChild::Drop`: the guest process must not
        // outlive its handle. A failed send means the pump task already
        // finished, so there is nothing left to kill.
        if self.kill_tx.try_send(()).is_err() {
            tracing::trace!("sandbox child dropped after its exec session ended");
        }
    }
}

/// Failure of the guest-process transport.
#[derive(Debug, thiserror::Error)]
pub(crate) enum SandboxProcessError {
    /// The sandbox backend or the guest exec session failed.
    #[error(transparent)]
    Sandbox(#[from] SandboxError),

    /// Reading one of the in-process output pipes failed.
    #[error("reading guest {stream} failed: {source}")]
    Pipe {
        stream: &'static str,
        #[source]
        source: std::io::Error,
    },

    /// The pump task ended without reporting an exit code — it panicked or
    /// the runtime is shutting down.
    #[error("the guest exec session ended without reporting an exit code")]
    PumpGone,
}

/// One step of the event pump.
enum PumpStep {
    /// The handle asked for a kill (explicitly, or by being dropped).
    Kill,
    /// The next event from the guest.
    Event(Result<Option<ExecEvent>, SandboxError>),
}

/// Drive the exec session: fan events out to the output pipes, latch the guest
/// PID, and report the exit code exactly once.
///
/// Runs until the session ends, so a killed guest is still reaped and its exit
/// code still delivered to whoever is awaiting `wait`.
async fn pump_events(
    mut stream: ExecStream,
    mut kill_rx: mpsc::Receiver<()>,
    mut stdout: Option<WriteHalf<SimplexStream>>,
    mut stderr: Option<WriteHalf<SimplexStream>>,
    exit_tx: oneshot::Sender<Result<i32, SandboxError>>,
    pid: Arc<AtomicU32>,
) {
    let mut kill_requested = false;
    let outcome = loop {
        // The kill branch's body must not touch `stream`: the `next_event`
        // future holds it mutably for the whole `select!`.
        let step = tokio::select! {
            biased;
            _ = kill_rx.recv(), if !kill_requested => PumpStep::Kill,
            event = stream.next_event() => PumpStep::Event(event),
        };
        match step {
            PumpStep::Kill => {
                kill_requested = true;
                if let Err(e) = stream.kill().await {
                    tracing::debug!("killing guest process failed: {e:#}");
                }
            }
            PumpStep::Event(Ok(Some(ExecEvent::Started { pid: guest_pid }))) => {
                pid.store(guest_pid, Ordering::Relaxed);
            }
            PumpStep::Event(Ok(Some(ExecEvent::Stdout(bytes)))) => {
                write_stream(&mut stdout, &bytes, "stdout").await;
            }
            PumpStep::Event(Ok(Some(ExecEvent::Stderr(bytes)))) => {
                write_stream(&mut stderr, &bytes, "stderr").await;
            }
            PumpStep::Event(Ok(Some(ExecEvent::Exited { code }))) => break Ok(code),
            PumpStep::Event(Ok(None)) => {
                break Err(SandboxError::ExecLost {
                    name: "sandbox".to_owned(),
                    cmd: "/bin/sh".to_owned(),
                });
            }
            PumpStep::Event(Err(e)) => break Err(e),
        }
    };

    // Close the pipes before reporting the exit code so a reader that races
    // `wait` against `read_to_end` always sees EOF.
    drop(stdout);
    drop(stderr);
    if exit_tx.send(outcome).is_err() {
        tracing::trace!("guest process exited after its handle was dropped");
    }
}

/// Forward one output chunk, dropping the pipe once the reader is gone.
///
/// A vanished reader is normal (the caller took what it needed and moved on);
/// the guest must keep running, so the write is not an error.
async fn write_stream(
    pipe: &mut Option<WriteHalf<SimplexStream>>,
    bytes: &[u8],
    stream: &'static str,
) {
    let Some(writer) = pipe.as_mut() else {
        return;
    };
    if let Err(e) = writer.write_all(bytes).await {
        tracing::trace!("guest {stream} reader is gone ({e}); discarding the rest");
        *pipe = None;
    }
}

/// The write end of a guest process's stdin.
///
/// Writes go out through [`ChunkedStdin`], so any payload size is safe.
/// Dropping the writer sends EOF.
pub(crate) struct SandboxStdin {
    writer: WriteHalf<SimplexStream>,
}

impl SandboxStdin {
    fn new(sink: ChunkedStdin) -> Self {
        let (reader, writer) = tokio::io::simplex(STREAM_BUFFER_BYTES);
        tokio::spawn(forward_stdin(reader, sink));
        Self { writer }
    }

    /// Write all of `data` to the guest's stdin.
    pub(crate) async fn write_all(&mut self, data: &[u8]) -> std::io::Result<()> {
        self.writer.write_all(data).await
    }
}

/// Copy the caller's writes into the guest's stdin, chunking below the SDK's
/// protocol frame cap, and close the guest's stdin on EOF.
async fn forward_stdin(mut reader: ReadHalf<SimplexStream>, sink: ChunkedStdin) {
    let mut buffer = vec![0_u8; STDIN_READ_BYTES];
    loop {
        match reader.read(&mut buffer).await {
            Ok(0) => break,
            Ok(n) => {
                if let Err(e) = sink.write_all(&buffer[..n]).await {
                    tracing::debug!("writing guest stdin failed: {e:#}");
                    return;
                }
            }
            Err(e) => {
                tracing::debug!("reading queued guest stdin failed: {e:#}");
                return;
            }
        }
    }
    if let Err(e) = sink.close().await {
        tracing::debug!("closing guest stdin failed: {e:#}");
    }
}
/// Build the in-process pipe for one output stream. `Capture::Null` still gets
/// a writer so the pump can drain the guest without blocking, but no reader.
fn pipe(
    capture: Capture,
) -> (
    Option<ReadHalf<SimplexStream>>,
    Option<WriteHalf<SimplexStream>>,
) {
    match capture {
        Capture::Pipe => {
            let (reader, writer) = tokio::io::simplex(STREAM_BUFFER_BYTES);
            (Some(reader), Some(writer))
        }
        Capture::Null => (None, None),
    }
}
