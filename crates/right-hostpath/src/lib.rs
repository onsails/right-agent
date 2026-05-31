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

/// Directory portion of the running binary (e.g. `/root/.local/bin`).
pub fn bin_dir(current_exe: &Path) -> PathBuf {
    current_exe
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// rc files to consider for `shell`, most-specific first.
fn rc_targets(shell: Option<&str>, home: &Path) -> Vec<PathBuf> {
    let name = shell
        .and_then(|s| Path::new(s).file_name())
        .and_then(|s| s.to_str())
        .unwrap_or("");
    if name.contains("zsh") {
        vec![home.join(".zshrc")]
    } else if name.contains("fish") {
        vec![home.join(".config/fish/config.fish")]
    } else if name.contains("bash") {
        vec![home.join(".bashrc"), home.join(".profile")]
    } else {
        vec![home.join(".profile")]
    }
}

/// Whether a fresh interactive shell will have `bindir` on its PATH.
///
/// True iff `bindir` is a standard system dir, or a candidate rc file for
/// `shell` already mentions `bindir`. **Deliberately ignores the live
/// `$PATH`** — the installer's own process has the install dir exported
/// (install.sh), so a live-PATH check would be a false "ok".
pub fn is_persistently_on_path(bindir: &Path, home: &Path, shell: Option<&str>) -> bool {
    if STANDARD_DIRS.iter().any(|d| Path::new(d) == bindir) {
        return true;
    }
    let needle = bindir.to_string_lossy();
    rc_targets(shell, home).iter().any(|rc| {
        std::fs::read_to_string(rc)
            .map(|c| c.contains(needle.as_ref()))
            .unwrap_or(false)
    })
}

#[cfg(test)]
#[path = "lib_tests.rs"]
mod tests;
