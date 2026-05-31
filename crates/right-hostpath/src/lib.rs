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

/// True when `shell` is fish (different PATH syntax).
fn is_fish(shell: Option<&str>) -> bool {
    shell
        .and_then(|s| Path::new(s).file_name())
        .and_then(|s| s.to_str())
        .map(|s| s.contains("fish"))
        .unwrap_or(false)
}

/// Managed block body for `bindir` in the shell's syntax.
fn managed_block(bindir: &Path, shell: Option<&str>) -> String {
    let dir = bindir.to_string_lossy();
    if is_fish(shell) {
        format!("{BLOCK_START}\nfish_add_path {dir}\n{BLOCK_END}\n")
    } else {
        format!(
            "{BLOCK_START}\ncase \":$PATH:\" in *\":{dir}:\"*) ;; *) export PATH=\"{dir}:$PATH\" ;; esac\n{BLOCK_END}\n"
        )
    }
}

/// Remove every existing managed block (including orphaned/stray markers),
/// line by line, so re-runs always normalize to a single block.
fn strip_managed_blocks(existing: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    let mut in_block = false;
    for line in existing.lines() {
        let t = line.trim();
        if t == BLOCK_START {
            in_block = true;
        } else if t == BLOCK_END {
            in_block = false;
        } else if !in_block {
            out.push(line);
        }
    }
    let mut s = out.join("\n");
    if existing.ends_with('\n') && !s.is_empty() {
        s.push('\n');
    }
    s
}

/// Return `existing` with exactly one managed block for `bindir` appended.
/// Idempotent: feeding the output back in yields the same string.
fn apply_block(existing: &str, bindir: &Path, shell: Option<&str>) -> String {
    let mut out = strip_managed_blocks(existing)
        .trim_end_matches('\n')
        .to_string();
    if !out.is_empty() {
        out.push_str("\n\n");
    }
    out.push_str(&managed_block(bindir, shell));
    out
}

/// Read-modify-write the managed block into `rc`, creating parent dirs.
fn write_block(rc: &Path, bindir: &Path, shell: Option<&str>) -> std::io::Result<()> {
    if let Some(parent) = rc.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let existing = std::fs::read_to_string(rc).unwrap_or_default();
    let desired = apply_block(&existing, bindir, shell);
    let name = rc.file_name().and_then(|n| n.to_str()).unwrap_or("rc");
    let tmp = rc.with_file_name(format!("{name}.right-hostpath.tmp"));
    std::fs::write(&tmp, desired.as_bytes())?;
    std::fs::rename(&tmp, rc)?;
    Ok(())
}

/// Ensure `bindir` is on PATH for future shells by editing the rc file(s).
///
/// Writes to every existing candidate rc (so login `~/.profile` and
/// interactive `~/.bashrc` both pick it up), creating the primary if none
/// exist. Returns `AlreadyOnPath` without writing when already reachable.
/// Ordinary write failures become `CouldNotWrite` (non-fatal); only an
/// absent rc target is an `Err`.
pub fn ensure_on_path(
    bindir: &Path,
    home: &Path,
    shell: Option<&str>,
) -> Result<EnsureOutcome, HostPathError> {
    if is_persistently_on_path(bindir, home, shell) {
        return Ok(EnsureOutcome::AlreadyOnPath);
    }

    let targets = rc_targets(shell, home);
    let primary = targets.first().cloned().ok_or(HostPathError::NoRcTarget)?;

    let mut to_write: Vec<PathBuf> = targets.iter().filter(|p| p.exists()).cloned().collect();
    if to_write.is_empty() {
        to_write.push(primary);
    }

    for rc in &to_write {
        if let Err(e) = write_block(rc, bindir, shell) {
            return Ok(EnsureOutcome::CouldNotWrite {
                file: rc.clone(),
                reason: format!("{e:#}"),
            });
        }
    }
    Ok(EnsureOutcome::Wrote {
        file: to_write[0].clone(),
    })
}

#[cfg(test)]
#[path = "lib_tests.rs"]
mod tests;
