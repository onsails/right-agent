# SSH Remote Argv Quoting Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Route every OpenSSH remote argv construction through one tested helper so shell metacharacters stay data and OpenSSH remote command parsing cannot regress.

**Architecture:** `right-openshell` owns the SSH remote argv quoting contract. Callers that need remote argv pass exactly one shell-quoted command string after the SSH host or `--`; callers that need authored shell behavior pass exactly one complete script string. `ARCHITECTURE.md` records this as a mandatory OpenShell convention.

**Tech Stack:** Rust 2024, `shlex`, `tokio::process::Command`, OpenSSH remote command semantics, `miette`, existing fake-ssh tests.

---

## Assumptions

- The already-fixed `ssh_exec()` regression test remains in place and passing.
- `right agent ssh <name>` without a command remains an interactive SSH login and must not append a remote command.
- `right agent ssh <name> -- <cmd> <arg...>` treats `<cmd> <arg...>` as argv, not shell source. Users who want shell syntax should pass `sh -lc '<script>'` explicitly.
- `docs/architecture/sandbox.md` is descriptive. Re-read it for cite-on-touch and update only if the implementation changes behavior described there.

## File Map

| File | Responsibility |
|------|----------------|
| `crates/right-openshell/src/openshell.rs` | Public SSH remote argv quoting helper; `ssh_exec`, tar download, tar upload callers |
| `crates/right-openshell/src/openshell_tests.rs` | Helper and fake-ssh regression tests |
| `crates/right/src/main.rs` | `right agent ssh` remote command builder and unit tests |
| `ARCHITECTURE.md` | Prescriptive SSH remote argv rule |
| `docs/architecture/sandbox.md` | Cite-on-touch descriptive doc; update only if drifted |

## Preflight

- [ ] Confirm the worktree state.

Run:

```bash
git status --short
```

Expected: either clean, or only unrelated user changes. If unrelated files are modified, do not stage or rewrite them.

- [ ] Re-read the touched architecture docs and code.

Run:

```bash
rg -n "ssh_remote_command|ssh_tar_upload|ssh_exec|ssh_tar_download" crates/right-openshell/src/openshell.rs
rg -n "ssh_tar_upload|ssh_exec_quotes|tar_download_remote_command" crates/right-openshell/src/openshell_tests.rs
rg -n "cmd_agent_ssh|command\\.join|OpenShell Integration Conventions" crates/right/src/main.rs ARCHITECTURE.md
sed -n '1,120p' docs/architecture/sandbox.md
```

Expected: current code shows `ssh_exec()` and `ssh_tar_download()` using the private helper, `ssh_tar_upload()` using `command.args([...])`, and `cmd_agent_ssh()` using `command.join(" ")`.

- [ ] Run a narrow baseline.

Run:

```bash
devenv shell -- cargo test -p right-openshell ssh_exec_quotes_remote_command_as_single_shell_string
devenv shell -- cargo test -p right-openshell sandbox_tar_download_remote_command_quotes_transform_semicolons_for_shell
```

Expected: both tests pass before new changes.

## Task 1: Promote and test the SSH remote argv helper

**Files:**
- Modify: `crates/right-openshell/src/openshell.rs`
- Modify: `crates/right-openshell/src/openshell_tests.rs`

- [ ] Add a failing public-helper test.

In `crates/right-openshell/src/openshell_tests.rs`, add this near the existing tar download quoting test:

```rust
#[test]
fn quote_ssh_remote_args_preserves_shell_metacharacters_as_data() {
    use std::process::Command;

    let remote_command = quote_ssh_remote_args([
        "probe_cmd",
        "alpha beta",
        "$(nope)",
        "semi;colon",
        "quote'arg",
    ])
    .unwrap();
    let probe = format!(
        "probe_cmd() {{ for arg in \"$@\"; do command printf '<%s>\\n' \"$arg\"; done; }}; {remote_command}"
    );

    let output = Command::new("sh").arg("-c").arg(probe).output().unwrap();
    assert!(
        output.status.success(),
        "quoted command should parse under sh; stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "<alpha beta>\n<$(nope)>\n<semi;colon>\n<quote'arg>\n"
    );
}
```

- [ ] Run the new test and verify it fails to compile because the helper is not public yet.

Run:

```bash
devenv shell -- cargo test -p right-openshell quote_ssh_remote_args_preserves_shell_metacharacters_as_data
```

Expected: compile failure naming missing `quote_ssh_remote_args`.

- [ ] Replace the private string helper with a public intent-revealing helper.

In `crates/right-openshell/src/openshell.rs`, replace the current helper pair:

```rust
fn ssh_remote_command(args: &[String]) -> miette::Result<String> {
    ssh_remote_command_strs(args.iter().map(|arg| arg.as_str()))
}

fn ssh_remote_command_strs<'a>(args: impl IntoIterator<Item = &'a str>) -> miette::Result<String> {
    shlex::try_join(args)
        .map_err(|e| miette::miette!("failed to quote SSH remote command args: {e}"))
}
```

with the single public helper:

```rust
/// Quote argv for OpenSSH remote command execution.
///
/// OpenSSH does not preserve remote argv. It sends one command string to the
/// remote login shell, so callers must pass the returned string as exactly one
/// argument after the SSH host or `--`.
pub fn quote_ssh_remote_args<'a>(
    args: impl IntoIterator<Item = &'a str>,
) -> miette::Result<String> {
    shlex::try_join(args)
        .map_err(|e| miette::miette!("failed to quote SSH remote command args: {e}"))
}
```

- [ ] Update `ssh_exec()` to call the public helper directly.

In `crates/right-openshell/src/openshell.rs`, change:

```rust
command.arg(ssh_remote_command_strs(cmd.iter().copied())?);
```

to:

```rust
command.arg(quote_ssh_remote_args(cmd.iter().copied())?);
```

- [ ] Update tar download code and its existing quoting test to call the public helper.

In `crates/right-openshell/src/openshell.rs`, change:

```rust
let tar_args = sandbox_tar_download_args(sandbox_path, include_rebuildable)?;
command.arg(ssh_remote_command(&tar_args)?);
```

to:

```rust
let tar_args = sandbox_tar_download_args(sandbox_path, include_rebuildable)?;
command.arg(quote_ssh_remote_args(tar_args.iter().map(String::as_str))?);
```

In `crates/right-openshell/src/openshell_tests.rs`, change:

```rust
let remote_command = ssh_remote_command(&args).unwrap();
```

to:

```rust
let remote_command = quote_ssh_remote_args(args.iter().map(String::as_str)).unwrap();
```

- [ ] Run the helper test.

Run:

```bash
devenv shell -- cargo test -p right-openshell quote_ssh_remote_args_preserves_shell_metacharacters_as_data
```

Expected: pass.

## Task 2: Route `ssh_tar_upload()` through the helper

**Files:**
- Modify: `crates/right-openshell/src/openshell.rs`
- Modify: `crates/right-openshell/src/openshell_tests.rs`

- [ ] Tighten the existing fake-ssh upload test so it fails with separate remote args.

In `crates/right-openshell/src/openshell_tests.rs`, replace the post-call assertions in `ssh_tar_upload_extracts_sandbox_archive_under_writable_sandbox_dir()` with parsing logic that asserts exactly one remote command argument after `--`:

```rust
let args = std::fs::read_to_string(args_file).unwrap();
let captured: Vec<&str> = args
    .lines()
    .map(|line| line.trim_start_matches('<').trim_end_matches('>'))
    .collect();
let separator = captured
    .iter()
    .position(|arg| *arg == "--")
    .expect("ssh args should include command separator");
let remote_args = &captured[separator + 1..];
assert_eq!(
    remote_args.len(),
    1,
    "ssh_tar_upload must pass one quoted remote command argument, got:\n{args}"
);

let probe = format!(
    "tar() {{ for arg in \"$@\"; do command printf '<%s>\\n' \"$arg\"; done; }}; {}",
    remote_args[0]
);
let output = std::process::Command::new("sh")
    .arg("-c")
    .arg(probe)
    .output()
    .unwrap();
assert!(
    output.status.success(),
    "quoted tar upload command should parse under sh; stderr: {}",
    String::from_utf8_lossy(&output.stderr)
);

let parsed_args: Vec<String> = String::from_utf8(output.stdout)
    .unwrap()
    .lines()
    .map(str::to_owned)
    .collect();
assert_eq!(
    parsed_args,
    vec![
        "<xzpf>".to_string(),
        "<->".to_string(),
        "<-C>".to_string(),
        "</sandbox>".to_string(),
        "<--strip-components=1>".to_string(),
        "<sandbox>".to_string(),
    ]
);
assert!(
    !parsed_args
        .windows(2)
        .any(|pair| pair[0] == "<-C>" && pair[1] == "</>"),
    "sandbox restore must not chdir to policy-denied /"
);
```

- [ ] Run the upload test and verify it fails.

Run:

```bash
devenv shell -- cargo test -p right-openshell ssh_tar_upload_extracts_sandbox_archive_under_writable_sandbox_dir
```

Expected: fail because the current implementation passes seven remote args after `--`.

- [ ] Quote the tar upload argv centrally.

In `crates/right-openshell/src/openshell.rs`, change `ssh_tar_upload()` from:

```rust
command.arg("--");
command.args([
    "tar",
    "xzpf",
    "-",
    "-C",
    "/sandbox",
    "--strip-components=1",
    "sandbox",
]);
```

to:

```rust
command.arg("--");
command.arg(quote_ssh_remote_args([
    "tar",
    "xzpf",
    "-",
    "-C",
    "/sandbox",
    "--strip-components=1",
    "sandbox",
])?);
```

- [ ] Run the upload test again.

Run:

```bash
devenv shell -- cargo test -p right-openshell ssh_tar_upload_extracts_sandbox_archive_under_writable_sandbox_dir
```

Expected: pass.

## Task 3: Route `right agent ssh -- <cmd>` through the helper

**Files:**
- Modify: `crates/right/src/main.rs`

- [ ] Add failing command assembly tests.

In the existing `#[cfg(test)] mod tests`, import `build_agent_ssh_command` in the `use super::{ ... }` list. Change `use std::path::PathBuf;` to `use std::path::{Path, PathBuf};`, then add:

```rust
#[test]
fn agent_ssh_command_quotes_remote_argv_as_one_argument() {
    let command = vec![
        "probe_cmd".to_string(),
        "alpha beta".to_string(),
        "$(nope)".to_string(),
        "semi;colon".to_string(),
        "quote'arg".to_string(),
    ];

    let cmd = build_agent_ssh_command(Path::new("config"), "openshell-example", &command).unwrap();
    let args: Vec<String> = cmd
        .get_args()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    assert_eq!(args[0], "-F");
    assert_eq!(args[1], "config");
    assert_eq!(args[2], "openshell-example");
    assert_eq!(
        args[3..].len(),
        1,
        "agent ssh must pass one remote command argument"
    );

    let probe = format!(
        "probe_cmd() {{ for arg in \"$@\"; do command printf '<%s>\\n' \"$arg\"; done; }}; {}",
        args[3]
    );

    let output = std::process::Command::new("sh")
        .arg("-c")
        .arg(probe)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "quoted command should parse under sh; stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "<alpha beta>\n<$(nope)>\n<semi;colon>\n<quote'arg>\n"
    );
}

#[test]
fn agent_ssh_command_omits_remote_command_for_interactive_login() {
    let command = Vec::new();
    let cmd = build_agent_ssh_command(Path::new("config"), "openshell-example", &command).unwrap();
    let args: Vec<String> = cmd
        .get_args()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();

    assert_eq!(
        args,
        vec![
            "-F".to_string(),
            "config".to_string(),
            "openshell-example".to_string(),
        ]
    );
}
```

- [ ] Run the new CLI command assembly tests and verify they fail to compile.

Run:

```bash
devenv shell -- cargo test -p right agent_ssh_command
```

Expected: compile failure naming missing `build_agent_ssh_command`.

- [ ] Add the command builder.

In `crates/right/src/main.rs`, add these helpers near `cmd_agent_ssh()`:

```rust
fn build_agent_ssh_remote_command(command: &[String]) -> miette::Result<Option<String>> {
    if command.is_empty() {
        return Ok(None);
    }

    right_openshell::openshell::quote_ssh_remote_args(command.iter().map(String::as_str)).map(Some)
}

fn build_agent_ssh_command(
    ssh_config: &Path,
    ssh_host: &str,
    command: &[String],
) -> miette::Result<std::process::Command> {
    let mut cmd = std::process::Command::new("ssh");
    cmd.arg("-F").arg(ssh_config);
    cmd.arg(ssh_host);
    if let Some(remote_command) = build_agent_ssh_remote_command(command)? {
        cmd.arg(remote_command);
    }
    Ok(cmd)
}
```

- [ ] Run the CLI command assembly tests again.

Run:

```bash
devenv shell -- cargo test -p right agent_ssh_command
```

Expected: pass. This proves the extracted builder quotes remote argv and omits the remote command for interactive login.

- [ ] Replace the lossy join in `cmd_agent_ssh()`.

In `crates/right/src/main.rs`, replace this block:

```rust
let mut cmd = std::process::Command::new("ssh");
cmd.arg("-F").arg(&ssh_config);
cmd.arg(&ssh_host);
if !command.is_empty() {
    cmd.arg(command.join(" "));
}
```

with:

```rust
let mut cmd = build_agent_ssh_command(&ssh_config, &ssh_host, command)?;
```

- [ ] Re-run the CLI helper tests.

Run:

```bash
devenv shell -- cargo test -p right agent_ssh_command
```

Expected: pass.

## Task 4: Add the architecture rule

**Files:**
- Modify: `ARCHITECTURE.md`
- Review: `docs/architecture/sandbox.md`

- [ ] Add a prescriptive OpenShell convention.

In `ARCHITECTURE.md`, under `## OpenShell Integration Conventions`, add this bullet after `Sandbox create stdio`:

```markdown
- **SSH remote argv must be quoted centrally**: OpenSSH does not preserve remote argv; it sends one command string to the remote login shell. For remote argv, call `right_openshell::openshell::quote_ssh_remote_args(...)` and pass exactly one argument after the SSH host or `--`. For authored shell scripts, pass exactly one complete script string. Never use `Command::args(...)` or `Vec::join(" ")` for remote argv after the SSH host.
```

- [ ] Re-read `docs/architecture/sandbox.md` after the code change.

Run:

```bash
sed -n '1,140p' docs/architecture/sandbox.md
```

Expected: no update needed unless the implementation changes the documented sandbox lifecycle, file transfer shape, or SSH config behavior. If drift is found, update the doc in the same commit.

## Task 5: Verification

- [ ] Format only the touched Rust files.

Run:

```bash
devenv shell -- cargo fmt --package right-openshell --package right
```

Expected: formatting succeeds. Check `git diff --stat` afterward to ensure only intended files changed.

- [ ] Run targeted package tests.

Run:

```bash
devenv shell -- cargo test -p right-openshell
devenv shell -- cargo test -p right agent_ssh_command
```

Expected: all targeted tests pass.

- [ ] Run final workspace tests.

Run:

```bash
devenv shell -- cargo test --workspace
```

Expected: all workspace tests pass.

- [ ] Run final workspace build.

Run:

```bash
devenv shell -- cargo build --workspace
```

Expected: debug workspace build succeeds.

## Task 6: Commit

- [ ] Inspect the final diff.

Run:

```bash
git diff -- crates/right-openshell/src/openshell.rs crates/right-openshell/src/openshell_tests.rs crates/right/src/main.rs ARCHITECTURE.md docs/architecture/sandbox.md
git status --short
```

Expected: only intended files are modified. If `docs/architecture/sandbox.md` was unchanged after review, it should not appear in the diff.

- [ ] Commit the implementation.

Run:

```bash
git add crates/right-openshell/src/openshell.rs crates/right-openshell/src/openshell_tests.rs crates/right/src/main.rs ARCHITECTURE.md
git add docs/architecture/sandbox.md
git commit -m "fix(openshell): centralize ssh remote argv quoting"
```

Expected: commit succeeds. If `docs/architecture/sandbox.md` was unchanged, `git add` is a no-op for that path.

## Acceptance Criteria

- `right_openshell::openshell::quote_ssh_remote_args(...)` is the only helper for OpenSSH remote argv quoting.
- `ssh_exec()`, `ssh_tar_download()`, and `ssh_tar_upload()` all pass exactly one remote command argument after `--`.
- `right agent ssh <name> -- <cmd> <arg...>` preserves spaces and shell metacharacters as argv data.
- `right agent ssh <name>` still opens an interactive SSH session without a remote command.
- `ARCHITECTURE.md` explicitly forbids `Command::args(...)` and `Vec::join(" ")` for SSH remote argv after the host.
- `devenv shell -- cargo test --workspace` and `devenv shell -- cargo build --workspace` pass.
