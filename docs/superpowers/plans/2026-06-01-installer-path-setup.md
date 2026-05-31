# Installer PATH Setup Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `right` reachable in the user's shell after `install.sh` by idempotently adding the install dir to the user's shell rc, surfacing a non-fatal styled warning if the write fails.

**Architecture:** A new pure leaf crate `right-hostpath` detects whether the install dir is on PATH and edits the right shell rc (marker-delimited managed block). A new `right setup-path` subcommand drives it and renders the outcome via `right_ui`, exiting `0` (ensured) or `10` (couldn't write) — never failing the installer. `install.sh` runs it after the installs and re-prints any failure in its closing block. `right-agent` and `right doctor` are untouched.

**Tech Stack:** Rust 2024, `thiserror`, `right_ui` (brand CLI output), `clap`, bash (`install.sh`), `tempfile` (tests).

Spec: `docs/superpowers/specs/2026-06-01-installer-path-setup-design.md`

---

## Conventions (read first)

- Repo uses devenv — prefix cargo with `devenv shell -- cargo …`.
- Never invoke bare `right` (stale PATH copy): use `target/devenv/debug/right` or `devenv shell -- cargo run --bin right -- …`.
- Edition 2024. FAIL FAST everywhere **except** the one documented branch: an rc-write failure is a handled `CouldNotWrite` value, surfaced loudly, never an abort.
- Tests pass all config via parameters — **never** `std::env::set_var` (mirror `crates/right-ui/src/theme.rs`).
- This session works in-place on `master` (not a worktree). The spec and this plan are already committed.

## File Structure

- Create `crates/right-hostpath/Cargo.toml` — leaf crate manifest (`publish = false`).
- Create `crates/right-hostpath/src/lib.rs` — pure detection + rc-mutation logic; one responsibility: host PATH/rc integration.
- Create `crates/right-hostpath/src/lib_tests.rs` — unit tests (included via `#[path]`).
- Modify `Cargo.toml` — add the crate to `[workspace] members`.
- Modify `crates/right/Cargo.toml` — add `right-hostpath` path dependency.
- Modify `crates/right/src/main.rs` — `Commands::SetupPath` variant, dispatch arm, `cmd_setup_path`.
- Modify `install.sh` — run `setup-path`, capture its exit, reprint on rc 10, fix "Next steps", drop the stale PATH note.
- Modify `README.md` and `docs/INSTALL.md` — reconcile the post-install flow with the new-shell step.

---

### Task 1: Scaffold the `right-hostpath` crate

**Files:**
- Create: `crates/right-hostpath/Cargo.toml`
- Create: `crates/right-hostpath/src/lib.rs`
- Modify: `Cargo.toml` (workspace `members`)

- [ ] **Step 1: Create the crate manifest**

Create `crates/right-hostpath/Cargo.toml`:

```toml
[package]
name = "right-hostpath"
version.workspace = true
edition.workspace = true
publish = false

[dependencies]
thiserror = { workspace = true }

[dev-dependencies]
tempfile = "3.27"
```

- [ ] **Step 2: Add the crate to the workspace**

In `Cargo.toml`, add this line inside `[workspace] members` (e.g. after `"crates/right-ui",`):

```toml
    "crates/right-hostpath",
```

- [ ] **Step 3: Create `lib.rs` with crate docs, constants, and the public value types**

Create `crates/right-hostpath/src/lib.rs`:

```rust
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
```

- [ ] **Step 4: Verify the crate compiles**

Run: `devenv shell -- cargo build -p right-hostpath`
Expected: PASS (compiles; unused-const warnings for `BLOCK_*`/`STANDARD_DIRS` are fine — they are used in Task 2/3).

- [ ] **Step 5: Commit**

```bash
git add crates/right-hostpath/Cargo.toml crates/right-hostpath/src/lib.rs Cargo.toml
git commit -m "feat(hostpath): scaffold right-hostpath crate"
```

---

### Task 2: Detection — `bin_dir`, `rc_targets`, `is_persistently_on_path`

**Files:**
- Create: `crates/right-hostpath/src/lib_tests.rs`
- Modify: `crates/right-hostpath/src/lib.rs`

- [ ] **Step 1: Write the failing tests**

Create `crates/right-hostpath/src/lib_tests.rs`:

```rust
use super::*;
use std::path::{Path, PathBuf};
use tempfile::tempdir;

#[test]
fn bin_dir_returns_parent() {
    assert_eq!(bin_dir(Path::new("/x/y/right")), PathBuf::from("/x/y"));
}

#[test]
fn rc_targets_selects_per_shell() {
    let home = Path::new("/home/u");
    assert_eq!(
        rc_targets(Some("/bin/bash"), home),
        vec![home.join(".bashrc"), home.join(".profile")]
    );
    assert_eq!(rc_targets(Some("/usr/bin/zsh"), home), vec![home.join(".zshrc")]);
    assert_eq!(
        rc_targets(Some("/usr/bin/fish"), home),
        vec![home.join(".config/fish/config.fish")]
    );
    assert_eq!(rc_targets(None, home), vec![home.join(".profile")]);
    assert_eq!(rc_targets(Some("/usr/bin/dash"), home), vec![home.join(".profile")]);
}

#[test]
fn standard_dir_is_persistently_on_path() {
    let home = tempdir().unwrap();
    assert!(is_persistently_on_path(
        Path::new("/usr/local/bin"),
        home.path(),
        Some("/bin/bash")
    ));
}

#[test]
fn fresh_home_is_not_on_path() {
    let home = tempdir().unwrap();
    let bindir = home.path().join(".local/bin");
    assert!(!is_persistently_on_path(&bindir, home.path(), Some("/bin/bash")));
}

#[test]
fn rc_mentioning_bindir_is_on_path() {
    let home = tempdir().unwrap();
    let bindir = home.path().join(".local/bin");
    std::fs::write(
        home.path().join(".bashrc"),
        format!("export PATH=\"{}:$PATH\"\n", bindir.display()),
    )
    .unwrap();
    assert!(is_persistently_on_path(&bindir, home.path(), Some("/bin/bash")));
}
```

Then add the test-module include to the bottom of `crates/right-hostpath/src/lib.rs`:

```rust
#[cfg(test)]
#[path = "lib_tests.rs"]
mod tests;
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `devenv shell -- cargo test -p right-hostpath`
Expected: FAIL — compile error, `bin_dir` / `rc_targets` / `is_persistently_on_path` not found.

- [ ] **Step 3: Implement the detection functions**

Add to `crates/right-hostpath/src/lib.rs` (above the `#[cfg(test)]` include):

```rust
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
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `devenv shell -- cargo test -p right-hostpath`
Expected: PASS (5 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/right-hostpath/src/lib.rs crates/right-hostpath/src/lib_tests.rs
git commit -m "feat(hostpath): detect persistent PATH membership"
```

---

### Task 3: Mutation — managed block + `ensure_on_path`

**Files:**
- Modify: `crates/right-hostpath/src/lib_tests.rs`
- Modify: `crates/right-hostpath/src/lib.rs`

- [ ] **Step 1: Write the failing tests**

Append to `crates/right-hostpath/src/lib_tests.rs`:

```rust
#[test]
fn ensure_writes_block_then_is_idempotent() {
    let home = tempdir().unwrap();
    let bindir = home.path().join(".local/bin");
    let rc = home.path().join(".bashrc");

    let first = ensure_on_path(&bindir, home.path(), Some("/bin/bash")).unwrap();
    assert_eq!(first, EnsureOutcome::Wrote { file: rc.clone() });
    let content = std::fs::read_to_string(&rc).unwrap();
    assert_eq!(content.matches(BLOCK_START).count(), 1);
    assert_eq!(content.matches(BLOCK_END).count(), 1);
    assert!(content.contains(&*bindir.to_string_lossy()));

    let second = ensure_on_path(&bindir, home.path(), Some("/bin/bash")).unwrap();
    assert_eq!(second, EnsureOutcome::AlreadyOnPath);
    assert_eq!(std::fs::read_to_string(&rc).unwrap(), content);
}

#[test]
fn apply_block_normalizes_orphan_and_duplicate_markers() {
    let bindir = Path::new("/opt/bin");
    let messy = format!(
        "line1\n{BLOCK_START}\nold\n{BLOCK_END}\nline2\n{BLOCK_START}\norphan-no-end\n"
    );
    let once = apply_block(&messy, bindir, Some("/bin/bash"));
    assert_eq!(once.matches(BLOCK_START).count(), 1);
    assert_eq!(once.matches(BLOCK_END).count(), 1);
    assert!(once.contains("line1") && once.contains("line2"));
    assert!(!once.contains("old") && !once.contains("orphan-no-end"));
    assert_eq!(apply_block(&once, bindir, Some("/bin/bash")), once);
}

#[test]
fn ensure_returns_could_not_write_when_target_is_a_dir() {
    let home = tempdir().unwrap();
    let bindir = home.path().join(".local/bin");
    std::fs::create_dir(home.path().join(".bashrc")).unwrap(); // .bashrc is a dir
    match ensure_on_path(&bindir, home.path(), Some("/bin/bash")).unwrap() {
        EnsureOutcome::CouldNotWrite { file, .. } => {
            assert_eq!(file, home.path().join(".bashrc"))
        }
        other => panic!("expected CouldNotWrite, got {other:?}"),
    }
}

#[test]
fn ensure_writes_fish_config_with_fish_syntax() {
    let home = tempdir().unwrap();
    let bindir = home.path().join(".local/bin");
    let rc = home.path().join(".config/fish/config.fish");
    let outcome = ensure_on_path(&bindir, home.path(), Some("/usr/bin/fish")).unwrap();
    assert_eq!(outcome, EnsureOutcome::Wrote { file: rc.clone() });
    let content = std::fs::read_to_string(&rc).unwrap();
    assert!(content.contains("fish_add_path"));
    assert!(content.contains(&*bindir.to_string_lossy()));
}

#[test]
fn ensure_skips_when_profile_already_mentions_bindir() {
    let home = tempdir().unwrap();
    let bindir = home.path().join(".local/bin");
    std::fs::write(
        home.path().join(".profile"),
        format!("export PATH={}:$PATH\n", bindir.display()),
    )
    .unwrap();
    assert_eq!(
        ensure_on_path(&bindir, home.path(), Some("/bin/bash")).unwrap(),
        EnsureOutcome::AlreadyOnPath
    );
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `devenv shell -- cargo test -p right-hostpath`
Expected: FAIL — `ensure_on_path` / `apply_block` not found.

- [ ] **Step 3: Implement the mutation logic**

Add to `crates/right-hostpath/src/lib.rs` (above the `#[cfg(test)]` include):

```rust
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
    let mut out = strip_managed_blocks(existing).trim_end_matches('\n').to_string();
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
                reason: format!("{e}"),
            });
        }
    }
    Ok(EnsureOutcome::Wrote { file: to_write[0].clone() })
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `devenv shell -- cargo test -p right-hostpath`
Expected: PASS (10 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/right-hostpath/src/lib.rs crates/right-hostpath/src/lib_tests.rs
git commit -m "feat(hostpath): idempotently add install dir to shell rc"
```

---

### Task 4: `right setup-path` subcommand

**Files:**
- Modify: `crates/right/Cargo.toml`
- Modify: `crates/right/src/main.rs` (`Commands` enum ~421-529; dispatch ~700-720; add `cmd_setup_path`)

- [ ] **Step 1: Add the dependency**

In `crates/right/Cargo.toml`, add to `[dependencies]` (with the other `right-*` path deps):

```toml
right-hostpath = { path = "../right-hostpath", version = "*" }
```

- [ ] **Step 2: Add the `Commands` variant**

In `crates/right/src/main.rs`, add to the `enum Commands` (e.g. after the `Doctor` variant):

```rust
    /// Ensure the install directory is on your shell PATH
    SetupPath,
```

- [ ] **Step 3: Add the dispatch arm**

In the `match` that dispatches commands (next to `Commands::Doctor => cmd_doctor(&home).await,`), add:

```rust
        Commands::SetupPath => cmd_setup_path(),
```

- [ ] **Step 4: Implement `cmd_setup_path`**

Add this function near `cmd_doctor` in `crates/right/src/main.rs`:

```rust
/// `right setup-path` — ensure the install dir is on the user's shell PATH.
///
/// Never fails the installer: exits `0` when PATH is ensured, `10` when the
/// rc could not be written (after printing a warning the user can act on).
fn cmd_setup_path() -> miette::Result<()> {
    let theme = right_ui::detect();

    let exe = std::env::current_exe()
        .map_err(|e| miette::miette!("cannot resolve current executable: {e}"))?;
    let bindir = right_hostpath::bin_dir(&exe);
    let manual_fix = format!("add manually: export PATH=\"{}:$PATH\"", bindir.display());

    let Some(home) = dirs::home_dir() else {
        let line = right_ui::status(right_ui::Glyph::Warn)
            .noun("PATH")
            .verb("couldn't determine your home directory")
            .fix(manual_fix);
        println!("{}", line.render(theme));
        std::process::exit(10);
    };
    let shell = std::env::var("SHELL").ok();

    let (line, code) = match right_hostpath::ensure_on_path(&bindir, &home, shell.as_deref()) {
        Ok(right_hostpath::EnsureOutcome::AlreadyOnPath) => (
            right_ui::status(right_ui::Glyph::Ok).noun("PATH").verb("ready"),
            0,
        ),
        Ok(right_hostpath::EnsureOutcome::Wrote { file }) => (
            right_ui::status(right_ui::Glyph::Ok)
                .noun("PATH")
                .verb(format!("added to {}", file.display()))
                .fix(format!("open a new shell, or run: source {}", file.display())),
            0,
        ),
        Ok(right_hostpath::EnsureOutcome::CouldNotWrite { file, reason }) => (
            right_ui::status(right_ui::Glyph::Warn)
                .noun("PATH")
                .verb(format!("couldn't update {}", file.display()))
                .detail(reason)
                .fix(manual_fix),
            10,
        ),
        Err(e) => (
            right_ui::status(right_ui::Glyph::Warn)
                .noun("PATH")
                .verb("couldn't set up PATH")
                .detail(format!("{e:#}"))
                .fix(manual_fix),
            10,
        ),
    };

    println!("{}", line.render(theme));
    std::process::exit(code);
}
```

- [ ] **Step 5: Build**

Run: `devenv shell -- cargo build -p right`
Expected: PASS.

- [ ] **Step 6: Manual smoke against a throwaway HOME (do NOT touch your real rc)**

Run (writes into a temp dir, prints a green "added to …/.bashrc"):

```bash
H=$(mktemp -d); HOME="$H" SHELL=/bin/bash devenv shell -- cargo run --bin right -- setup-path; cat "$H/.bashrc"
```
Expected: a green `✓ PATH  added to <H>/.bashrc` line; `.bashrc` contains the `# >>> right-hostpath (PATH) >>>` block.

Run again to confirm idempotence + AlreadyOnPath:

```bash
HOME="$H" SHELL=/bin/bash devenv shell -- cargo run --bin right -- setup-path; grep -c right-hostpath "$H/.bashrc"
```
Expected: `✓ PATH  ready`; grep count `2` (one start + one end marker).

Confirm the couldn't-write path + exit code 10:

```bash
H2=$(mktemp -d); mkdir "$H2/.bashrc"; HOME="$H2" SHELL=/bin/bash devenv shell -- cargo run --bin right -- setup-path; echo "exit=$?"
```
Expected: a yellow `! PATH  couldn't update …` line and `exit=10`.

- [ ] **Step 7: Commit**

```bash
git add crates/right/Cargo.toml crates/right/src/main.rs
git commit -m "feat(install): add 'right setup-path' to ensure PATH"
```

---

### Task 5: Wire `setup-path` into `install.sh`

**Files:**
- Modify: `install.sh` (add `run_path_setup`; call it; rewrite closing block; lines ~206-237)

- [ ] **Step 1: Add the `run_path_setup` step function**

In `install.sh`, after the `install_openshell() { … }` function (before `# ── Main ──`), add:

```sh
# ── Step 3.5: Ensure PATH ──────────────────────────────────────────

run_path_setup() {
  info "Ensuring $INSTALL_DIR is on your PATH..."

  # setup-path never aborts the installer; capture its exit code so the
  # closing summary can re-surface a write failure (rc 10).
  set +e
  "$INSTALL_DIR/right" setup-path
  PATH_SETUP_RC=$?
  set -e
}
```

- [ ] **Step 2: Call it after the installs, before init**

In `main()`, change:

```sh
  install_right
  install_process_compose
  install_openshell

  echo ""
  run_init
```

to:

```sh
  install_right
  install_process_compose
  install_openshell

  echo ""
  run_path_setup

  echo ""
  run_init
```

- [ ] **Step 3: Rewrite the closing block (Next steps + drop the stale note)**

In `main()`, replace:

```sh
  echo "  Next steps:"
  echo "    1. Start your agents:  ${CYAN}right up${RESET}"
  echo "    2. View the TUI:       ${CYAN}right attach${RESET}"
  echo "    3. Check status:       ${CYAN}right status${RESET}"
  echo ""
  echo "  Make sure ${CYAN}$INSTALL_DIR${RESET} is in your PATH."
  echo "  Add this to your shell profile if needed:"
  echo "    ${CYAN}export PATH=\"\$HOME/.local/bin:\$PATH\"${RESET}"
  echo ""
```

with:

```sh
  echo "  Next steps:"
  echo "    1. Open a new shell    (so ${CYAN}right${RESET} is on your PATH)"
  echo "    2. Start your agents:  ${CYAN}right up${RESET}"
  echo "    3. View the TUI:       ${CYAN}right attach${RESET}"
  echo "    4. Check status:       ${CYAN}right status${RESET}"

  if [ "${PATH_SETUP_RC:-0}" -eq 10 ]; then
    echo ""
    warn "couldn't add $INSTALL_DIR to your shell profile automatically"
    echo "  add this line to your shell profile, then open a new shell:"
    echo "    ${CYAN}export PATH=\"$INSTALL_DIR:\$PATH\"${RESET}"
  fi
  echo ""
```

- [ ] **Step 4: Verify shell syntax**

Run: `bash -n install.sh`
Expected: no output, exit 0.

If `shellcheck` is available: `nix run nixpkgs#shellcheck -- install.sh` (expected: no new errors).

- [ ] **Step 5: Commit**

```bash
git add install.sh
git commit -m "feat(install): run setup-path and surface PATH failures"
```

---

### Task 6: Reconcile the docs

**Files:**
- Modify: `README.md` (lines ~56-62)
- Modify: `docs/INSTALL.md` (lines ~38-70)

- [ ] **Step 1: Fix the README quick start**

In `README.md`, replace the quick-start block:

````markdown
```sh
curl -LsSf https://raw.githubusercontent.com/onsails/right-agent/master/install.sh | sh
right init
right up
```
````

with:

````markdown
```sh
curl -LsSf https://raw.githubusercontent.com/onsails/right-agent/master/install.sh | sh
```

the installer runs `right init` and `right doctor` for you, and adds `~/.local/bin` to your shell profile. open a new shell (so `right` is on your `PATH`), then:

```sh
right up
```
````

- [ ] **Step 2: Add the new-shell note to `docs/INSTALL.md`**

In `docs/INSTALL.md`, in the `## After install` section, replace:

```markdown
## After install

```sh
right up
```
```

with:

```markdown
## After install

`right` installs to `~/.local/bin`. The installer adds that directory to your shell profile; open a new shell (or `source` your profile) so `right` is found, then:

```sh
right up
```
```

- [ ] **Step 3: Verify no whitespace damage**

Run: `git diff --check -- README.md docs/INSTALL.md`
Expected: no output, exit 0.

- [ ] **Step 4: Commit**

```bash
git add README.md docs/INSTALL.md
git commit -m "docs: reconcile install flow with PATH setup"
```

---

### Task 7: Release config check + final workspace verification

**Files:**
- Read-only: `release-plz.toml`
- No source changes expected.

- [ ] **Step 1: Confirm the new crate stays unreleased**

Run: `cat release-plz.toml`
Expected: release/publish is scoped to the `right` binary (per `docs/superpowers/specs/2026-04-11-binary-distribution-design.md`). `right-hostpath` is `publish = false`; make a change **only** if the config would otherwise opt it into a release. If `release-plz.toml` has no per-package opt-in for new crates, no change is needed — note that and continue.

- [ ] **Step 2: Format**

Run: `devenv shell -- cargo fmt --all`
Then: `devenv shell -- cargo fmt --check`
Expected: exit 0.

- [ ] **Step 3: Clippy**

Run: `devenv shell -- cargo clippy --workspace --all-targets`
Expected: no new warnings in `right-hostpath` or `right`.

- [ ] **Step 4: Full workspace tests (mandatory)**

Run: `devenv shell -- cargo test --workspace`
Expected: exit 0. Targeted tests do not replace this.

- [ ] **Step 5: Full workspace build**

Run: `devenv shell -- cargo build --workspace`
Expected: exit 0.

- [ ] **Step 6: Commit any formatting/config fixups**

```bash
git add -A
git commit -m "chore(install): fmt/clippy fixups for PATH setup" || echo "nothing to commit"
```

---

## Self-Review Notes

- **Spec coverage:** new crate `right-hostpath` (Tasks 1-3) = spec "Crate" section; `right setup-path` (Task 4) = spec "Binary wiring" + during-surface; `install.sh` reprint (Task 5) = spec "after" surface + non-fatal; docs (Task 6) = spec "Docs changes"; release check (Task 7) = spec files table. `right-agent`/`cmd_doctor` deliberately untouched per spec "Why not right doctor".
- **Placeholder scan:** every code step contains complete code; every run step has an exact command + expected result; each task ends with an exact commit. No TBD/TODO.
- **Type consistency:** `EnsureOutcome::{AlreadyOnPath, Wrote{file}, CouldNotWrite{file,reason}}`, `HostPathError::NoRcTarget`, `bin_dir`, `is_persistently_on_path`, `ensure_on_path`, and private `rc_targets`/`is_fish`/`managed_block`/`strip_managed_blocks`/`apply_block`/`write_block` are referenced consistently across Tasks 2-4 and `cmd_setup_path`.
- **Cadence:** targeted `cargo test -p right-hostpath` during Tasks 2-3; build + manual smoke in Task 4; one full `cargo test --workspace` + build at the end (Task 7).
- **Out of scope (spec):** no `/usr/local/bin`-for-root switch; no `right-agent::doctor` change; no current-shell mutation; no rustup-scale rc sweep.
