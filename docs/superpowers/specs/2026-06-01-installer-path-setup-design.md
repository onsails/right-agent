# Installer PATH Setup Design

## Problem

`install.sh` installs `right` (and `process-compose`) to `INSTALL_DIR`,
default `~/.local/bin` (`install.sh:65`). It prepends that dir to PATH
**only for the installer's own process** (`install.sh:73-78`) and prints
an "add `export PATH=…` to your shell profile" note at the end
(`install.sh:234-236`). Neither touches the shell the user is sitting
in. On a default Debian **root** shell, `~/.local/bin` is not on PATH at
all (root's PATH has `/usr/local/bin` but not `~/.local/bin`).

A `curl | sh` script cannot put `right` on the PATH of the already-open
shell — a child process cannot mutate its parent's environment. So the
first command the user runs after install (`right init` / `right up`)
fails with `right: command not found`.

The drift makes it worse: `README.md:56-60` tells the user to type
`right init` then `right up` by name (both fail), while
`docs/INSTALL.md:46,64-70` says the installer already runs init/doctor
and the user only runs `right up`. The documented flows disagree, and
both hit the PATH wall.

## Decision

Adopt **Approach A**: the installer idempotently adds the install
directory to the user's shell rc so the *next* shell works. The rc edit
and PATH detection live in a new dedicated leaf crate, **`right-hostpath`**
(pure logic, unit-tested, no rendering), invoked by the `right` binary
through a new `right setup-path` subcommand. `right-agent` is not
touched.

A failed rc write is **non-fatal** and surfaced with a brand `right_ui`
warning (`Glyph::Warn`, the yellow `!`) in two places within the single
install run:

- **during** — inline, when the write is attempted, rendered by
  `right setup-path` via `right_ui`;
- **after** — re-printed in `install.sh`'s closing block (driven by
  `setup-path`'s exit code), so the warning is among the last lines on
  screen and not lost in scrollback.

We cannot fix the *current* shell from a piped installer, so both
surfaces also print the one-line `export …` the user can paste now.

### Why not `right doctor` for the "after" surface

`right_agent::doctor::run_doctor()` already emits a `check_binary("right")`
result (doctor.rs:53) via `which::which` — the **live** PATH. During
install that is masked by the installer's session `export`
(install.sh:73-78), yielding a misleading green `✓ right`; on any later
re-run it always passes (you cannot invoke `right doctor` unless `right`
is already resolvable). It therefore cannot detect the future-shell
condition this work targets. Making it honest would mean rewriting that
check inside `right-agent`. To keep `right-agent` untouched (the whole
point of the leaf crate), the "after" surface is `install.sh` instead,
and `run_doctor`'s existing live check is left exactly as-is. Teaching
`right doctor` to report future-shell PATH persistence is a possible
follow-up, explicitly out of scope here.

## Crate: `right-hostpath`

Charter: host-side detection of whether the `right` install directory is
on the user's PATH, and idempotent editing of the user's shell rc to add
it. Host only. The sandbox's in-container `.bashrc` management is a
separate concern and stays in `right-bot`
(`crates/bot/src/cc/sandbox_env.rs`, `crates/bot/src/sync.rs`); this
crate mirrors that managed-block technique but shares no code with it
(no sane dependency edge connects them).

`publish = false` (like `right-ui`); the workspace publishes only the
`right` binary. Minimal deps: `thiserror` (workspace) for the error
type; `tempfile` as a dev-dep for tests. Pure logic, config passed as
parameters (mirrors the `EnvGet`/`IsTty` injection idiom in
`crates/right-ui/src/theme.rs` — **no `std::env::set_var` in tests**). No
`right_ui` dependency: it returns outcome enums and the binary maps them
to status lines, so rendering stays in the `cmd_*` functions where it
belongs.

```rust
pub enum EnsureOutcome {
    AlreadyOnPath,
    Wrote { file: PathBuf },
    CouldNotWrite { file: PathBuf, reason: String },
}

/// Directory portion of the running binary.
pub fn bin_dir(current_exe: &Path) -> PathBuf;

/// Will a fresh interactive shell have `bindir` on PATH?
/// True iff `bindir` is a standard system dir (`/usr/local/bin`,
/// `/usr/bin`, `/bin`, `/usr/local/sbin`, `/usr/sbin`, `/sbin`) OR a
/// candidate rc file (derived from `home` + `shell`) already references
/// the `bindir` path. **Deliberately ignores the live `$PATH`** — the
/// installer's own process has `INSTALL_DIR` exported (install.sh:73-78),
/// so a live-PATH check would be a false "ok". This is the load-bearing
/// gotcha and the reason `which`-based detection is unsuitable.
pub fn is_persistently_on_path(bindir: &Path, home: &Path, shell: Option<&str>) -> bool;

/// If not already persistently on PATH, append a guarded managed block
/// to the primary rc for `shell`. Permission-denied / read-only /
/// missing-parent map to `CouldNotWrite` (an expected branch, not Err);
/// only genuinely unexpected IO propagates as Err. `reason` is built
/// with `format!("{:#}", e)` to preserve the chain.
pub fn ensure_on_path(bindir: &Path, home: &Path, shell: Option<&str>) -> Result<EnsureOutcome, HostPathError>;
```

Managed block, delimited so re-runs replace rather than duplicate.
Mirror the idempotent replace algorithm in
`crates/bot/src/sync.rs::ensure_bashrc_sources_managed_env` (it already
handles orphaned, malformed, and duplicated markers):

```sh
# >>> right-hostpath (PATH) >>>
case ":$PATH:" in *":<bindir>:"*) ;; *) export PATH="<bindir>:$PATH" ;; esac
# <<< right-hostpath <<<
```

(fish target uses `fish_add_path <bindir>` inside equivalent markers.)

### rc-file targeting

| `$SHELL` | rc file(s) written |
|----------|--------------------|
| `*bash`  | `~/.bashrc`; also `~/.profile` if it exists (login shells, e.g. the reported root case) |
| `*zsh`   | `~/.zshrc` |
| `*fish`  | `~/.config/fish/config.fish` |
| unknown / unset | `~/.profile` |

Missing primary rc files are created (e.g. `~/.bashrc`). Smaller sweep
than rustup's; widen later if real setups need it.

## Binary wiring (`crates/right`)

`right` depends on `right-hostpath` (`{ path = "../right-hostpath",
version = "*" }`). One new callsite:

**`right setup-path`** — new `Commands::SetupPath` variant + `cmd_setup_path`,
added to the dispatch table next to the other `cmd_*`. Needs no `home`
(PATH is independent of `RIGHT_HOME`). At the boundary it reads
`current_exe()`, `$HOME`, `$SHELL`, calls `ensure_on_path`, renders via
`right_ui`, and **exits with an explicit code, never failing the
installer**:

| outcome | render | exit |
|---------|--------|------|
| `Wrote{file}` | `Glyph::Ok` `.noun("PATH").verb("added to <file>").fix("open a new shell, or run: source <file>")` | 0 |
| `AlreadyOnPath` | `Glyph::Ok` `.noun("PATH").verb("ready")` | 0 |
| `CouldNotWrite{file, reason}` | `Glyph::Warn` `.noun("PATH").verb("couldn't update <file>").detail(reason).fix("add manually: export PATH=\"<bindir>:$PATH\"")` | 10 |
| `Err(unexpected)` | `Glyph::Warn` same shape with the error text | 10 |

So exit `0` = on PATH for new shells; exit `10` = not ensured (any
reason, after printing the warning). Visible and re-runnable so a user on
an odd setup can retry. `right-agent::doctor` and `cmd_doctor` are
**unchanged**.

## `install.sh` changes

- Add `run_path_setup()` calling `"$INSTALL_DIR/right" setup-path` (full
  path, matching the existing `run_init`/`run_doctor` "Pitfall 6"
  convention), capturing its exit code without aborting under
  `set -euo pipefail`:
  ```sh
  set +e; "$INSTALL_DIR/right" setup-path; PATH_SETUP_RC=$?; set -e
  ```
  Place it **after the three installs, before `run_init`**, so it runs on
  fresh install *and* upgrade (`run_init` is skipped when `~/.right`
  exists — PATH setup must not depend on init).
- In `main`'s closing block, when `PATH_SETUP_RC` is `10`, re-print the
  warning (the **after** surface) using the installer's existing colored
  `warn()` helper plus the paste-able `export "$INSTALL_DIR:$PATH"` and
  "open a new shell" hint.
- Remove the stale hardcoded PATH note (`install.sh:234-236`) — it is
  replaced by `setup-path` (during) + the conditional reprint (after).
- Keep "Next steps" but make it accurate ("open a new shell, then
  `right up`").

## Docs changes

Reconcile the drift so documented steps actually work:

- `README.md:56-60` quick start — show that a new shell (or the printed
  `export`) is needed before `right …` works, consistent with the
  installer running init/doctor itself.
- `docs/INSTALL.md` — align the "Quick Install" / "After install"
  sections with the installer behaviour and the new-shell step.

## Data flow

- **Success:** `setup-path` writes the managed block → green `✓` (during),
  exit 0. Installer's closing block prints the normal "Next steps". User
  opens a new shell; `right up` works.
- **Couldn't write:** `setup-path` prints yellow `!` + the manual
  `export` (during), exit 10; installer continues; closing block
  re-prints the warning (after). Installer still reports success.

## Error handling

One deliberate, documented exception to the project's FAIL-FAST rule: an
rc-write failure is a handled `CouldNotWrite` outcome surfaced loudly via
`right_ui`, never an abort — the user mandated non-fatal + visible, and
this is best-effort host convenience. Genuinely unexpected IO still
propagates out of `ensure_on_path` as `HostPathError` (`thiserror`);
`cmd_setup_path` then renders it as a warning and exits `10` rather than
crashing the installer. Error chains are preserved with
`format!("{:#}", e)` when composing `reason`.

## Testing (TDD)

Unit tests in `right-hostpath` (config via params, no `set_var`), each
written red first:

- rc-target selection per `$SHELL` value (bash/zsh/fish/unknown).
- `ensure_on_path` writes the guarded block to a tempdir `home`; second
  call returns `AlreadyOnPath` with no duplicate block (idempotent);
  re-mirror the orphan/malformed-marker cases covered by
  `sync.rs`'s `ensure_bashrc_sources_managed_env` tests.
- `ensure_on_path` returns `CouldNotWrite` when the target is unwritable
  (e.g. rc path is a directory) — caller must still get a value, never a
  panic.
- `is_persistently_on_path` true for `/usr/local/bin`, true when a
  candidate rc already contains `bindir`, false otherwise, and
  unaffected by the live `$PATH`.

Targeted loop: `devenv shell -- cargo test -p right-hostpath`. Final,
mandatory: `devenv shell -- cargo test --workspace`.

## Upgrade & migration

Pure additive host behaviour; no per-agent codegen, sandbox, or on-disk
agent state changes, so the Upgrade & Migration model does not apply.
Already-installed users adopt the fix by re-running `install.sh` (or just
`right setup-path`). Backward compatible: existing installs keep working;
`INSTALL_DIR` override is still honoured (logic derives the dir from
`current_exe()`).

## What we don't do

- Do not change the default install location (no `/usr/local/bin`-for-root
  switch). Approach A only.
- Do not modify the current shell automatically (impossible from
  `curl | sh`); we print the paste-able `export` instead.
- Do not touch `right-agent::doctor` — its existing live-`which` `right`
  check stays as-is; future-shell PATH reporting in `right doctor` is a
  possible later follow-up.
- Do not move rc logic into `right-agent` or the `right-bot` sandbox path
  code.
- Do not add CLI flags beyond the `setup-path` subcommand.
- No rustup-scale multi-file rc sweep yet.

## Files to create / modify

| File | Action |
|------|--------|
| `crates/right-hostpath/Cargo.toml` | Create (leaf crate, `publish = false`) |
| `crates/right-hostpath/src/lib.rs` | Create — detection + mutation |
| `crates/right-hostpath/src/lib_tests.rs` | Create — unit tests |
| `Cargo.toml` (workspace) | Add `crates/right-hostpath` to `members` |
| `release-plz.toml` | Verify the new crate stays unreleased (it is `publish = false`; confirm config does not opt it in) |
| `crates/right/Cargo.toml` | Add `right-hostpath` path dependency |
| `crates/right/src/main.rs` | Add `Commands::SetupPath` + `cmd_setup_path` + dispatch arm |
| `install.sh` | Add `run_path_setup` (capture exit); reprint on rc 10; remove stale PATH note; fix "Next steps" |
| `README.md` | Fix quick-start PATH/new-shell step |
| `docs/INSTALL.md` | Align install/after-install sections |
