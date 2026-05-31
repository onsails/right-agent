//! Host-side PATH integration for the `right` CLI.
//!
//! Detects whether `right`'s install directory is on the user's shell PATH
//! and idempotently adds it to the appropriate shell rc file. Host only —
//! the sandbox's in-container `.bashrc` management lives in `right-bot`
//! (`crates/bot/src/cc/sandbox_env.rs`) and shares no code with this crate.
//!
//! Pure logic: all environment inputs (`home`, `shell`, the running exe
//! path) are passed as parameters so tests never touch global state.

use std::path::{Path, PathBuf};

/// Markers delimiting our managed block in an rc file. Re-runs replace the
/// block between these markers rather than appending a duplicate.
const BLOCK_START: &str = "# >>> right-hostpath (PATH) >>>";
const BLOCK_END: &str = "# <<< right-hostpath <<<";

/// Directories conventionally already on a login shell's PATH.
const STANDARD_DIRS: &[&str] = &[
    "/usr/local/bin",
    "/usr/bin",
    "/bin",
    "/usr/local/sbin",
    "/usr/sbin",
    "/sbin",
];

/// Result of [`ensure_on_path`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnsureOutcome {
    /// `bindir` is already reachable by future shells; nothing was written.
    AlreadyOnPath,
    /// The managed block was written/updated; `file` is the primary rc.
    Wrote { file: PathBuf },
    /// Writing failed (e.g. permission denied). Non-fatal — the caller
    /// surfaces `reason` and tells the user to add the line manually.
    CouldNotWrite { file: PathBuf, reason: String },
}

/// Unexpected failure that is not an ordinary "couldn't write the rc file".
#[derive(Debug, thiserror::Error)]
pub enum HostPathError {
    #[error("no shell rc file could be determined")]
    NoRcTarget,
}
