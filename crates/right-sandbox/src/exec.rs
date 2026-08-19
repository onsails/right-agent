//! Command execution inside an Agent Sandbox.
//!
//! Two surfaces: [`SandboxHandle::exec`](crate::SandboxHandle::exec) collects
//! a finished command's output; [`SandboxHandle::exec_stream`](crate::SandboxHandle::exec_stream)
//! is the turn transport — a live event stream plus an optional chunked
//! stdin writer.
//!
//! The stdin chunking is load-bearing (stage-1 correction 5): the SDK's
//! `ExecSink::write` maps one call to one protocol frame capped at
//! [`PROTOCOL_FRAME_MAX_BYTES`], and an oversized write tears the exec
//! session down. [`ChunkedStdin`] chunks below the cap.

use std::time::Duration;

use microsandbox::sandbox::ExecOptionsBuilder;
use microsandbox::sandbox::exec::ExecSink;
use microsandbox::ExecHandle;
use crate::error::SandboxError;

/// The SDK's protocol frame cap: one `ExecSink::write` becomes one frame.
pub const PROTOCOL_FRAME_MAX_BYTES: usize = 4 * 1024 * 1024;

/// Chunk size the stdin writer uses: far below the frame cap, proven in the
/// stage-1 probe (1.2 MiB pushed in 64 KiB chunks while stdout streamed).
pub const STDIN_CHUNK_BYTES: usize = 64 * 1024;

const _: () = assert!(
    STDIN_CHUNK_BYTES < PROTOCOL_FRAME_MAX_BYTES,
    "stdin chunks must stay below the protocol frame cap"
);

/// How a command's stdin is wired.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Stdin {
    /// No stdin (`/dev/null`).
    #[default]
    Null,

    /// A pipe the caller writes through [`ChunkedStdin`].
    Pipe,
}

/// A command to run in the guest.
#[derive(Debug, Clone, Default)]
pub struct ExecRequest {
    /// Program path in the guest (e.g. `"/bin/sh"`).
    pub cmd: String,

    /// Arguments, passed through unquoted (no shell).
    pub args: Vec<String>,

    /// Working directory; defaults to the sandbox's configured workdir.
    pub cwd: Option<String>,

    /// Guest user override for this command (`"1000"`, `"sandbox"`,
    /// `"1000:1000"`). Provisioning execs run as root (`"0"`); agent execs
    /// use the sandbox's default user.
    pub user: Option<String>,

    /// Per-command environment, merged over the sandbox env.
    pub env: Vec<(String, String)>,

    /// Stdin wiring.
    pub stdin: Stdin,

    /// Hard cap on runtime; the guest process is SIGKILLed on expiry.
    pub timeout: Option<Duration>,
}

impl ExecRequest {
    /// A request for `cmd` with no arguments and null stdin.
    pub fn new(cmd: impl Into<String>) -> Self {
        Self {
            cmd: cmd.into(),
            ..Self::default()
        }
    }
}

/// One event from a streaming exec session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExecEvent {
    /// The guest process started.
    Started {
        /// Guest PID.
        pid: u32,
    },

    /// A chunk of stdout.
    Stdout(Vec<u8>),

    /// A chunk of stderr.
    Stderr(Vec<u8>),

    /// The process exited with this code. Terminal.
    Exited {
        /// Process exit code.
        code: i32,
    },
}

/// A finished command's output. A non-zero exit code is data, not an error.
#[derive(Debug, Clone)]
pub struct ExecOutcome {
    /// Process exit code.
    pub code: i32,

    /// Everything the process wrote to stdout.
    pub stdout: Vec<u8>,

    /// Everything the process wrote to stderr.
    pub stderr: Vec<u8>,
}

impl ExecOutcome {
    /// True when the process exited 0.
    pub fn success(&self) -> bool {
        self.code == 0
    }
}

/// A live streaming exec session.
///
/// Spawn failures surface as `Err(SandboxError::ExecSpawn)` from
/// [`next_event`](Self::next_event)/[`wait`](Self::wait) rather than as a
/// `Started`-less stream; a non-zero exit is an [`ExecEvent::Exited`], never
/// an error.
pub struct ExecStream {
    name: String,
    cmd: String,
    inner: ExecHandle,
}

impl ExecStream {
    pub(crate) fn new(name: String, cmd: String, inner: ExecHandle) -> Self {
        Self { name, cmd, inner }
    }

    /// The next event, or `None` after the session ends.
    ///
    /// Guest-side spawn failures and stdin failures are errors, not events —
    /// FAIL FAST applies to the stream too.
    pub async fn next_event(&mut self) -> Result<Option<ExecEvent>, SandboxError> {
        match self.inner.recv().await {
            None => Ok(None),
            Some(microsandbox::ExecEvent::Started { pid }) => Ok(Some(ExecEvent::Started { pid })),
            Some(microsandbox::ExecEvent::Stdout(bytes)) => {
                Ok(Some(ExecEvent::Stdout(bytes.to_vec())))
            }
            Some(microsandbox::ExecEvent::Stderr(bytes)) => {
                Ok(Some(ExecEvent::Stderr(bytes.to_vec())))
            }
            Some(microsandbox::ExecEvent::Exited { code }) => Ok(Some(ExecEvent::Exited { code })),
            Some(microsandbox::ExecEvent::Failed(failed)) => Err(SandboxError::ExecSpawn {
                name: self.name.clone(),
                cmd: self.cmd.clone(),
                kind: format!("{:?}", failed.kind),
                message: format_exec_failed(&failed),
            }),
            Some(microsandbox::ExecEvent::StdinError(err)) => {
                let message = match &err.errno_name {
                    Some(errno) => format!("{} [{errno}]", err.message),
                    None => err.message,
                };
                Err(SandboxError::ExecStdin {
                    name: self.name.clone(),
                    cmd: self.cmd.clone(),
                    message,
                })
            }
        }
    }

    /// Take the stdin writer. `Some` exactly once, only when the request
    /// asked for [`Stdin::Pipe`].
    pub fn take_stdin(&mut self) -> Option<ChunkedStdin> {
        self.inner
            .take_stdin()
            .map(|sink| ChunkedStdin {
                name: self.name.clone(),
                cmd: self.cmd.clone(),
                sink,
            })
    }

    /// Drain the event stream and return the exit code. Output events are
    /// discarded; callers that want output drive [`next_event`](Self::next_event)
    /// themselves.
    pub async fn wait(&mut self) -> Result<i32, SandboxError> {
        loop {
            match self.next_event().await? {
                Some(ExecEvent::Exited { code }) => return Ok(code),
                None => {
                    return Err(SandboxError::ExecLost {
                        name: self.name.clone(),
                        cmd: self.cmd.clone(),
                    });
                }
                _ => {}
            }
        }
    }

    /// Send a signal to the guest process.
    pub async fn signal(&self, signal: i32) -> Result<(), SandboxError> {
        self.inner
            .signal(signal)
            .await
            .map_err(|source| SandboxError::Operation {
                name: self.name.clone(),
                operation: "exec signal",
                source: Box::new(crate::error::SdkError(source)),
            })
    }

    /// SIGKILL the guest process.
    pub async fn kill(&self) -> Result<(), SandboxError> {
        self.inner
            .kill()
            .await
            .map_err(|source| SandboxError::Operation {
                name: self.name.clone(),
                operation: "exec kill",
                source: Box::new(crate::error::SdkError(source)),
            })
    }
}

/// A chunked writer for a guest process's stdin.
///
/// Writes of any size are safe: data goes out in [`STDIN_CHUNK_BYTES`]
/// frames, below the protocol cap that would otherwise tear the session down.
pub struct ChunkedStdin {
    name: String,
    cmd: String,
    sink: ExecSink,
}

impl ChunkedStdin {
    /// Write all of `data`, chunking below the protocol frame cap. Empty
    /// input writes nothing (an empty write is the SDK's EOF marker — use
    /// [`close`](Self::close) for that).
    pub async fn write_all(&self, data: &[u8]) -> Result<(), SandboxError> {
        for chunk in stdin_chunks(data) {
            self.sink
                .write(chunk)
                .await
                .map_err(|source| SandboxError::ExecStdin {
                    name: self.name.clone(),
                    cmd: self.cmd.clone(),
                    message: format!("{source:#}"),
                })?;
        }
        Ok(())
    }

    /// Send EOF and consume the writer.
    pub async fn close(self) -> Result<(), SandboxError> {
        self.sink
            .close()
            .await
            .map_err(|source| SandboxError::ExecStdin {
                name: self.name.clone(),
                cmd: self.cmd.clone(),
                message: format!("{source:#}"),
            })
    }
}

/// Render an SDK `ExecFailed` with its errno/stage context appended.
///
/// `kind` is carried separately on [`SandboxError::ExecSpawn`]; this formats
/// only the human message plus the structured `errno_name`/`stage` when the
/// agentd classifier populated them.
pub(crate) fn format_exec_failed(
    failed: &microsandbox::protocol::exec::ExecFailed,
) -> String {
    match (&failed.errno_name, &failed.stage) {
        (Some(errno), Some(stage)) => format!("{} [{errno} at {stage}]", failed.message),
        (Some(errno), None) => format!("{} [{errno}]", failed.message),
        (None, Some(stage)) => format!("{} [at {stage}]", failed.message),
        (None, None) => failed.message.clone(),
    }
}

/// Split `data` into chunks of at most [`STDIN_CHUNK_BYTES`].
fn stdin_chunks(data: &[u8]) -> std::slice::Chunks<'_, u8> {
    data.chunks(STDIN_CHUNK_BYTES)
}

/// Apply a request to an SDK exec-options builder.
pub(crate) fn apply_request(
    mut options: ExecOptionsBuilder,
    request: &ExecRequest,
) -> ExecOptionsBuilder {
    options = options.args(request.args.iter().cloned());
    if let Some(cwd) = &request.cwd {
        options = options.cwd(cwd);
    }
    if let Some(user) = &request.user {
        options = options.user(user);
    }
    for (key, value) in &request.env {
        options = options.env(key, value);
    }
    if let Some(timeout) = request.timeout {
        options = options.timeout(timeout);
    }
    match request.stdin {
        Stdin::Null => options.stdin_null(),
        Stdin::Pipe => options.stdin_pipe(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunk_size_matches_the_proven_probe_value() {
        // The const-assert above already guarantees chunk < frame cap; this
        // pins the specific 64 KiB value stage-1 proved out.
        assert_eq!(STDIN_CHUNK_BYTES, 64 * 1024);
    }

    #[test]
    fn empty_input_produces_no_chunks() {
        // An empty SDK write is the EOF marker, so the chunker must not emit
        // one for empty input.
        assert_eq!(stdin_chunks(&[]).count(), 0);
    }

    #[test]
    fn small_input_is_one_chunk() {
        let data = vec![b'x'; STDIN_CHUNK_BYTES - 1];
        let chunks: Vec<&[u8]> = stdin_chunks(&data).collect();
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].len(), STDIN_CHUNK_BYTES - 1);
    }

    #[test]
    fn exact_multiple_produces_full_chunks_only() {
        let data = vec![b'x'; 2 * STDIN_CHUNK_BYTES];
        let chunks: Vec<&[u8]> = stdin_chunks(&data).collect();
        assert_eq!(chunks.len(), 2);
        assert!(chunks.iter().all(|chunk| chunk.len() == STDIN_CHUNK_BYTES));
    }

    #[test]
    fn oversized_input_chunks_below_the_cap() {
        // Larger than the protocol frame cap: the whole point of the writer.
        let data = vec![b'x'; 5 * 1024 * 1024 + 7];
        let chunks: Vec<&[u8]> = stdin_chunks(&data).collect();
        assert_eq!(chunks.len(), 81);
        assert!(
            chunks.iter().all(|chunk| chunk.len() <= STDIN_CHUNK_BYTES),
            "every chunk stays below the cap"
        );
        assert_eq!(chunks.last().expect("last chunk").len(), 7);
        assert_eq!(
            chunks.iter().map(|chunk| chunk.len()).sum::<usize>(),
            data.len(),
            "chunking is lossless"
        );
    }
}
