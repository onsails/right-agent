# Sandbox Identity Mirror Bootstrap Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make sandboxed bootstrap completion verify authoritative `/sandbox` identity files while keeping the host identity mirror explicit and required.

**Architecture:** Sandbox identity files (`/sandbox/IDENTITY.md`, `/sandbox/SOUL.md`, `/sandbox/USER.md`) are the runtime source for sandboxed prompt assembly. Host identity files remain a required mirror/control-plane surface, but bootstrap acceptance must not depend on stale host state; accepting bootstrap should first verify and materialize the mirror from the sandbox. The same shared identity-mirror helper is used by bot startup, restore, and post-invocation sync.

**Tech Stack:** Rust 2024, Tokio async, OpenShell CLI download helper, existing `right-agent`, `right-bot`, `right` crates, `devenv shell -- cargo ...` verification.

---

## File Structure

- Create `crates/right-agent/src/identity_mirror.rs`
  - Owns the identity mirror contract.
  - Exposes the exact file set that must exist on host for sandboxed control-plane mirror: `IDENTITY.md`, `SOUL.md`, `USER.md`.
  - Provides host check and sandbox-to-host reconciliation helpers.

- Modify `crates/right-agent/src/lib.rs`
  - Export the new `identity_mirror` module.

- Modify `crates/bot/src/sync.rs`
  - Make `reverse_sync_md()` delegate to the shared identity mirror helper.
  - Stop syncing `TOOLS.md`; it is read from `/sandbox/TOOLS.md` for sandboxed prompt runtime and has no identified host-side consumer in the current contract.

- Modify `crates/bot/src/cc/worker_reply.rs`
  - Remove the local bootstrap required-file list.
  - Keep host-mode `should_accept_bootstrap()` as a wrapper over `right_agent::identity_mirror::host_identity_mirror_complete()`.

- Modify `crates/bot/src/telegram/worker.rs`
  - Make sandboxed bootstrap acceptance call the shared sandbox-to-host mirror reconciliation helper.
  - Keep no-sandbox bootstrap acceptance on host files.
  - Stop running a separate blocking `reverse_sync_md()` before bootstrap acceptance; the acceptance check itself performs the explicit sandbox pull.

- Modify `crates/bot/src/lib.rs`
  - After `initial_sync()` on bot startup, run a best-effort identity mirror reconciliation for sandboxed agents.
  - This makes restored/legacy agents converge without waiting for a user message.

- Modify `crates/right/src/main.rs`
  - After sandbox restore upload succeeds and `sandbox.name` is written, run identity mirror reconciliation once.
  - This makes `right agent init --from-backup ...` produce a complete host mirror before the bot starts.

- Modify docs:
  - `PROMPT_SYSTEM.md`: clarify that sandbox prompt reads `/sandbox/*.md`, and host identity files are an explicit mirror populated by identity mirror reconciliation.
  - `ARCHITECTURE.md`: clarify identity mirror lifecycle under Codegen categories or OpenShell conventions.
  - `docs/architecture/lifecycle.md`: update restore/startup lifecycle if drifted.

Allowlist backup/restore is intentionally out of scope.

---

### Task 1: Shared Identity Mirror Module

**Files:**
- Create: `crates/right-agent/src/identity_mirror.rs`
- Modify: `crates/right-agent/src/lib.rs`

- [ ] **Step 1: Write failing tests for the identity mirror contract**

Add this test module to the bottom of the new file `crates/right-agent/src/identity_mirror.rs` while creating the file:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;
    use std::sync::Mutex;

    static PROCESS_ENV_LOCK: Mutex<()> = Mutex::new(());

    struct PathGuard(Option<OsString>);

    impl PathGuard {
        fn prepend(path: &Path) -> Self {
            let old_path = std::env::var_os("PATH");
            let mut new_path = OsString::from(path.as_os_str());
            if let Some(old_path) = &old_path {
                new_path.push(":");
                new_path.push(old_path);
            }
            unsafe {
                std::env::set_var("PATH", new_path);
            }
            Self(old_path)
        }
    }

    impl Drop for PathGuard {
        fn drop(&mut self) {
            unsafe {
                match &self.0 {
                    Some(path) => std::env::set_var("PATH", path),
                    None => std::env::remove_var("PATH"),
                }
            }
        }
    }

    #[test]
    fn host_identity_mirror_requires_only_identity_files() {
        assert_eq!(IDENTITY_MIRROR_FILES, ["IDENTITY.md", "SOUL.md", "USER.md"]);
        assert!(!IDENTITY_MIRROR_FILES.contains(&"TOOLS.md"));
    }

    #[test]
    fn host_identity_mirror_complete_requires_all_identity_files() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!host_identity_mirror_complete(dir.path()));

        std::fs::write(dir.path().join("IDENTITY.md"), "# identity\n").unwrap();
        std::fs::write(dir.path().join("SOUL.md"), "# soul\n").unwrap();
        assert!(!host_identity_mirror_complete(dir.path()));

        std::fs::write(dir.path().join("USER.md"), "# user\n").unwrap();
        assert!(host_identity_mirror_complete(dir.path()));
    }

    #[tokio::test]
    async fn sync_identity_mirror_from_sandbox_downloads_required_files() {
        let _guard = PROCESS_ENV_LOCK.lock().unwrap();

        let tmp = tempfile::tempdir().unwrap();
        let bin = tmp.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        let fake_openshell = bin.join("openshell");
        std::fs::write(
            &fake_openshell,
            r#"#!/bin/sh
set -eu
if [ "$1" != "sandbox" ] || [ "$2" != "download" ]; then
  exit 64
fi
sandbox="$3"
src="$4"
dest="$5"
if [ "$sandbox" != "right-test-sandbox" ]; then
  exit 65
fi
case "$src" in
  /sandbox/IDENTITY.md) printf '# identity\n' > "$dest/IDENTITY.md" ;;
  /sandbox/SOUL.md) printf '# soul\n' > "$dest/SOUL.md" ;;
  /sandbox/USER.md) printf '# user\n' > "$dest/USER.md" ;;
  *) exit 66 ;;
esac
"#,
        )
        .unwrap();
        std::fs::set_permissions(&fake_openshell, std::fs::Permissions::from_mode(0o755))
            .unwrap();
        let _path_guard = PathGuard::prepend(&bin);

        let agent_dir = tmp.path().join("agent");
        std::fs::create_dir(&agent_dir).unwrap();

        sync_identity_mirror_from_sandbox(&agent_dir, "right-test-sandbox")
            .await
            .unwrap();

        assert_eq!(
            std::fs::read_to_string(agent_dir.join("IDENTITY.md")).unwrap(),
            "# identity\n"
        );
        assert_eq!(
            std::fs::read_to_string(agent_dir.join("SOUL.md")).unwrap(),
            "# soul\n"
        );
        assert_eq!(
            std::fs::read_to_string(agent_dir.join("USER.md")).unwrap(),
            "# user\n"
        );
    }

    #[tokio::test]
    async fn sync_identity_mirror_from_sandbox_fails_when_any_required_file_is_missing() {
        let _guard = PROCESS_ENV_LOCK.lock().unwrap();

        let tmp = tempfile::tempdir().unwrap();
        let bin = tmp.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        let fake_openshell = bin.join("openshell");
        std::fs::write(
            &fake_openshell,
            r#"#!/bin/sh
set -eu
src="$4"
dest="$5"
case "$src" in
  /sandbox/IDENTITY.md) printf '# identity\n' > "$dest/IDENTITY.md" ;;
  /sandbox/SOUL.md) exit 1 ;;
  /sandbox/USER.md) printf '# user\n' > "$dest/USER.md" ;;
  *) exit 66 ;;
esac
"#,
        )
        .unwrap();
        std::fs::set_permissions(&fake_openshell, std::fs::Permissions::from_mode(0o755))
            .unwrap();
        let _path_guard = PathGuard::prepend(&bin);

        let agent_dir = tmp.path().join("agent");
        std::fs::create_dir(&agent_dir).unwrap();

        let err = sync_identity_mirror_from_sandbox(&agent_dir, "right-test-sandbox")
            .await
            .expect_err("missing SOUL.md must fail reconciliation");
        let msg = format!("{err:#}");
        assert!(msg.contains("SOUL.md"), "error should name missing file: {msg}");
        assert!(!host_identity_mirror_complete(&agent_dir));
    }
}
```

- [ ] **Step 2: Run the new tests to verify they fail**

Run:

```bash
devenv shell -- cargo test -p right-agent identity_mirror --lib
```

Expected: FAIL to compile with unresolved names such as `IDENTITY_MIRROR_FILES`, `host_identity_mirror_complete`, or `sync_identity_mirror_from_sandbox`.

- [ ] **Step 3: Implement the shared identity mirror helper**

Put this implementation above the test module in `crates/right-agent/src/identity_mirror.rs`:

```rust
use std::path::Path;

/// Agent-authored identity files that must have an explicit host mirror.
///
/// For sandboxed agents the authoritative runtime copy is under `/sandbox`.
/// The host copy is a control-plane/debug/rebootstrap mirror, not the prompt
/// source for sandboxed runtime.
pub const IDENTITY_MIRROR_FILES: [&str; 3] = ["IDENTITY.md", "SOUL.md", "USER.md"];

/// Return true when every required identity mirror file exists on host.
pub fn host_identity_mirror_complete(agent_dir: &Path) -> bool {
    IDENTITY_MIRROR_FILES
        .iter()
        .all(|name| agent_dir.join(name).exists())
}

/// Download authoritative sandbox identity files into the host agent directory.
///
/// This is an explicit reconciliation step. It intentionally does not include
/// `TOOLS.md`: sandbox prompt runtime reads `/sandbox/TOOLS.md`, and current
/// host-side consumers only require identity files.
pub async fn sync_identity_mirror_from_sandbox(
    agent_dir: &Path,
    sandbox_name: &str,
) -> miette::Result<()> {
    let mut errors = Vec::new();

    for filename in IDENTITY_MIRROR_FILES {
        let sandbox_path = format!("/sandbox/{filename}");
        let host_dest = agent_dir.join(filename);
        if let Err(e) =
            right_openshell::openshell::download_file(sandbox_name, &sandbox_path, &host_dest).await
        {
            errors.push(format!("{filename}: {e:#}"));
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(miette::miette!(
            "identity mirror sync from sandbox '{}' failed: {}",
            sandbox_name,
            errors.join("; ")
        ))
    }
}
```

- [ ] **Step 4: Export the module**

Add this line to `crates/right-agent/src/lib.rs`:

```rust
pub mod identity_mirror;
```

- [ ] **Step 5: Run the task tests to verify they pass**

Run:

```bash
devenv shell -- cargo test -p right-agent identity_mirror --lib
```

Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/right-agent/src/identity_mirror.rs crates/right-agent/src/lib.rs
git commit -m "feat(agent): add identity mirror reconciliation"
```

---

### Task 2: Narrow Reverse Sync to the Explicit Host Mirror

**Files:**
- Modify: `crates/bot/src/sync.rs`
- Test: `crates/bot/src/sync.rs`

- [ ] **Step 1: Write a failing test for the reverse-sync file set**

Add this test to the existing `#[cfg(test)] mod tests` in `crates/bot/src/sync.rs`:

```rust
#[test]
fn reverse_sync_files_match_identity_mirror_contract() {
    assert_eq!(
        reverse_sync_files(),
        right_agent::identity_mirror::IDENTITY_MIRROR_FILES
    );
    assert!(!reverse_sync_files().contains(&"TOOLS.md"));
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run:

```bash
devenv shell -- cargo test -p right-bot reverse_sync_files_match_identity_mirror_contract --lib
```

Expected: FAIL to compile because `reverse_sync_files()` does not exist, or FAIL because current reverse sync includes `TOOLS.md`.

- [ ] **Step 3: Replace the local reverse sync file list**

In `crates/bot/src/sync.rs`, replace:

```rust
/// Files that CC creates/modifies inside the sandbox and should be synced back to host.
/// Excludes codegen-only files (BOOTSTRAP.md) — those are uploaded by
/// forward sync and never modified by CC.
const REVERSE_SYNC_FILES: &[&str] = &["TOOLS.md", "IDENTITY.md", "SOUL.md", "USER.md"];
```

with:

```rust
/// Files that CC creates/modifies inside the sandbox and must be mirrored to host.
///
/// This intentionally excludes TOOLS.md. Sandboxed prompt assembly reads
/// `/sandbox/TOOLS.md` directly, and current host-side consumers require only
/// the identity mirror files.
fn reverse_sync_files() -> &'static [&'static str] {
    &right_agent::identity_mirror::IDENTITY_MIRROR_FILES
}
```

Then change the loop in `reverse_sync_md()` from:

```rust
for &filename in REVERSE_SYNC_FILES {
```

to:

```rust
for &filename in reverse_sync_files() {
```

- [ ] **Step 4: Run the targeted test**

Run:

```bash
devenv shell -- cargo test -p right-bot reverse_sync_files_match_identity_mirror_contract --lib
```

Expected: PASS.

- [ ] **Step 5: Run existing sync tests**

Run:

```bash
devenv shell -- cargo test -p right-bot sync::tests --lib
```

Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/bot/src/sync.rs
git commit -m "refactor(bot): narrow reverse sync to identity mirror"
```

---

### Task 3: Make Bootstrap Acceptance Sandbox-Aware

**Files:**
- Modify: `crates/bot/src/cc/worker_reply.rs`
- Modify: `crates/bot/src/telegram/worker.rs`
- Test: `crates/bot/src/cc/worker_reply.rs`

- [ ] **Step 1: Update worker-reply tests to use the shared contract**

In `crates/bot/src/cc/worker_reply.rs`, update the existing tests that reference `BOOTSTRAP_REQUIRED_FILES`.

Replace this block in `should_accept_bootstrap_all_files_present()`:

```rust
for f in BOOTSTRAP_REQUIRED_FILES {
    std::fs::write(dir.path().join(f), "# test").unwrap();
}
```

with:

```rust
for f in right_agent::identity_mirror::IDENTITY_MIRROR_FILES {
    std::fs::write(dir.path().join(f), "# test").unwrap();
}
```

- [ ] **Step 2: Run worker-reply tests to verify current local constant coupling**

Run:

```bash
devenv shell -- cargo test -p right-bot should_accept_bootstrap --lib
```

Expected before implementation: PASS may still happen because the old local constant exists. This is acceptable for this step because the red test is covered by Task 2; this step prepares the refactor.

- [ ] **Step 3: Refactor `should_accept_bootstrap()` to host-mode only**

In `crates/bot/src/cc/worker_reply.rs`, delete:

```rust
/// Required identity files that must exist for bootstrap to be accepted as complete.
const BOOTSTRAP_REQUIRED_FILES: &[&str] = &["IDENTITY.md", "SOUL.md", "USER.md"];
```

Replace `should_accept_bootstrap()` with:

```rust
/// Host-mode bootstrap completion check.
///
/// Sandboxed bootstrap is verified in `telegram::worker` by reconciling the
/// authoritative `/sandbox` identity files into the required host mirror.
pub(crate) fn should_accept_bootstrap(agent_dir: &Path) -> bool {
    right_agent::identity_mirror::host_identity_mirror_complete(agent_dir)
}
```

- [ ] **Step 4: Add sandbox-aware bootstrap acceptance helper in worker**

In `crates/bot/src/telegram/worker.rs`, add this helper near the pure/helper functions above `spawn_worker`:

```rust
async fn should_accept_bootstrap_for_worker(ctx: &WorkerContext) -> bool {
    match ctx.resolved_sandbox.as_deref() {
        Some(sandbox_name) if ctx.ssh_config_path.is_some() => {
            match right_agent::identity_mirror::sync_identity_mirror_from_sandbox(
                &ctx.agent_dir,
                sandbox_name,
            )
            .await
            {
                Ok(()) => true,
                Err(e) => {
                    tracing::warn!(
                        agent = %ctx.agent_name,
                        sandbox = %sandbox_name,
                        "bootstrap identity mirror sync failed: {e:#}"
                    );
                    false
                }
            }
        }
        _ => should_accept_bootstrap(&ctx.agent_dir),
    }
}
```

- [ ] **Step 5: Stop blocking bootstrap on the old pre-check reverse sync**

In `crates/bot/src/telegram/worker.rs`, replace the reverse-sync block:

```rust
// Reverse sync .md changes from sandbox.
// Bootstrap mode: BLOCK so files are on host for completion check.
// Normal mode: fire-and-forget, don't delay reply.
let bootstrap_mode = ctx.agent_dir.join("BOOTSTRAP.md").exists();
if ctx.ssh_config_path.is_some() {
    let sandbox = ctx.resolved_sandbox.clone().unwrap();
    if bootstrap_mode {
        if let Err(e) = crate::sync::reverse_sync_md(&ctx.agent_dir, &sandbox).await {
            tracing::warn!(
                agent = %ctx.agent_name,
                "bootstrap reverse sync failed: {e:#}"
            );
        }
    } else {
        let agent_dir = ctx.agent_dir.clone();
        let agent_name = ctx.agent_name.clone();
        tokio::spawn(async move {
            if let Err(e) = crate::sync::reverse_sync_md(&agent_dir, &sandbox).await {
                tracing::warn!(agent = %agent_name, "reverse sync failed: {e:#}");
            }
        });
    }
}
```

with:

```rust
// Keep the host identity mirror fresh after normal sandbox turns.
// Bootstrap completion performs an explicit sandbox -> host reconciliation
// inside `should_accept_bootstrap_for_worker`, so it does not need this
// separate pre-check sync.
let bootstrap_mode = ctx.agent_dir.join("BOOTSTRAP.md").exists();
if ctx.ssh_config_path.is_some() && !bootstrap_mode {
    let sandbox = ctx.resolved_sandbox.clone().unwrap();
    let agent_dir = ctx.agent_dir.clone();
    let agent_name = ctx.agent_name.clone();
    tokio::spawn(async move {
        if let Err(e) = crate::sync::reverse_sync_md(&agent_dir, &sandbox).await {
            tracing::warn!(agent = %agent_name, "reverse sync failed: {e:#}");
        }
    });
}
```

- [ ] **Step 6: Use the sandbox-aware acceptance helper**

In `crates/bot/src/telegram/worker.rs`, replace:

```rust
if bootstrap_mode && bootstrap_signaled && should_accept_bootstrap(&ctx.agent_dir) {
```

with:

```rust
if bootstrap_mode && bootstrap_signaled && should_accept_bootstrap_for_worker(&ctx).await {
```

Update the nearby log string from:

```rust
"bootstrap complete — identity files present after sync"
```

to:

```rust
"bootstrap complete — identity files verified"
```

- [ ] **Step 7: Run targeted bot tests**

Run:

```bash
devenv shell -- cargo test -p right-bot should_accept_bootstrap --lib
devenv shell -- cargo test -p right-bot worker_reply::tests::parse_reply_output_bootstrap_complete_true --lib
```

Expected: PASS.

- [ ] **Step 8: Commit**

```bash
git add crates/bot/src/cc/worker_reply.rs crates/bot/src/telegram/worker.rs
git commit -m "fix(bot): verify sandbox bootstrap from sandbox files"
```

---

### Task 4: Reconcile Host Mirror on Bot Startup

**Files:**
- Modify: `crates/bot/src/lib.rs`

- [ ] **Step 1: Add startup mirror reconciliation after initial sync**

In `crates/bot/src/lib.rs`, find the startup block that runs:

```rust
sync::initial_sync(&agent_dir, &sbox).await?;
let sync_agent_dir = agent_dir.clone();
let sync_shutdown = shutdown.clone();
Some(tokio::spawn(sync::run_sync_task(
    sync_agent_dir,
    sbox,
    sync_shutdown,
)))
```

Replace it with:

```rust
sync::initial_sync(&agent_dir, &sbox).await?;
if let Err(e) = sync::reverse_sync_md(&agent_dir, sbox.sandbox_name()).await {
    tracing::warn!(
        agent = %agent_name,
        sandbox = %sbox.sandbox_name(),
        "startup identity mirror sync failed: {e:#}"
    );
}
let sync_agent_dir = agent_dir.clone();
let sync_shutdown = shutdown.clone();
Some(tokio::spawn(sync::run_sync_task(
    sync_agent_dir,
    sbox,
    sync_shutdown,
)))
```

If the local variable is not named `agent_name` in this exact scope, use the existing in-scope agent name variable from `run_async`; do not introduce a new config parse.

- [ ] **Step 2: Run a targeted compile check**

Run:

```bash
devenv shell -- cargo test -p right-bot --lib
```

Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add crates/bot/src/lib.rs
git commit -m "fix(bot): reconcile identity mirror on startup"
```

---

### Task 5: Reconcile Host Mirror After Restore

**Files:**
- Modify: `crates/right/src/main.rs`
- Test: `crates/right/src/main.rs`

- [ ] **Step 1: Write a failing unit test for restore mirror reconciliation**

Add this test near the existing restore tests in `crates/right/src/main.rs`:

```rust
#[test]
fn identity_mirror_files_are_not_treated_as_restore_config_files() {
    let tmp = tempfile::tempdir().unwrap();
    let backup = tmp.path().join("backup");
    let agent = tmp.path().join("agent");
    std::fs::create_dir_all(&backup).unwrap();
    std::fs::create_dir_all(&agent).unwrap();

    std::fs::write(backup.join("agent.yaml"), "sandbox:\n  mode: openshell\n").unwrap();
    std::fs::write(backup.join("policy.yaml"), "version: v1\n").unwrap();
    std::fs::write(backup.join("IDENTITY.md"), "# wrong source\n").unwrap();
    std::fs::write(backup.join("SOUL.md"), "# wrong source\n").unwrap();
    std::fs::write(backup.join("USER.md"), "# wrong source\n").unwrap();

    let config = right_agent::agent::discovery::parse_agent_config(&backup)
        .unwrap()
        .unwrap();
    copy_agent_restore_config_files(&backup, &agent, &config).unwrap();

    assert!(
        !agent.join("IDENTITY.md").exists(),
        "restore config copy must not treat host identity files as authoritative for sandboxed agents"
    );
    assert!(
        !agent.join("SOUL.md").exists(),
        "SOUL.md must come from sandbox identity mirror reconciliation"
    );
    assert!(
        !agent.join("USER.md").exists(),
        "USER.md must come from sandbox identity mirror reconciliation"
    );
}
```

This test protects the intended source of truth: for sandboxed restores, identity mirror files come from `sandbox.tar.gz` via the sandbox, not from stray files in the backup directory.

- [ ] **Step 2: Run the test**

Run:

```bash
devenv shell -- cargo test -p right identity_mirror_files_are_not_treated_as_restore_config_files --lib
```

Expected: PASS if config copy is already narrow. If it fails, remove identity files from the config copy path rather than copying them as config files.

- [ ] **Step 3: Add restore-time mirror reconciliation**

In `crates/right/src/main.rs`, inside `cmd_agent_restore()`, after:

```rust
crate::wizard::update_agent_yaml_sandbox_name(&agent_dir, &new_sandbox_name)?;
println!("sandbox.name set to '{new_sandbox_name}' in agent.yaml");
```

add:

```rust
right_agent::identity_mirror::sync_identity_mirror_from_sandbox(&agent_dir, &new_sandbox_name)
    .await
    .map_err(|e| {
        miette::miette!(
            "sandbox restored but identity mirror sync failed for '{}': {e:#}",
            new_sandbox_name
        )
    })?;
println!("identity mirror restored from sandbox");
```

- [ ] **Step 4: Run targeted restore tests**

Run:

```bash
devenv shell -- cargo test -p right restore_config_files_copy_custom_sandbox_policy_before_codegen --lib
devenv shell -- cargo test -p right identity_mirror_files_are_not_treated_as_restore_config_files --lib
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/right/src/main.rs
git commit -m "fix(restore): reconcile identity mirror after sandbox restore"
```

---

### Task 6: Documentation Updates

**Files:**
- Modify: `PROMPT_SYSTEM.md`
- Modify: `ARCHITECTURE.md`
- Modify: `docs/architecture/lifecycle.md`

- [ ] **Step 1: Update `PROMPT_SYSTEM.md` host mirror wording**

In `PROMPT_SYSTEM.md`, under the sandbox/host file tables, add this paragraph after the host table:

```markdown
For sandboxed agents, `/sandbox/IDENTITY.md`, `/sandbox/SOUL.md`, and
`/sandbox/USER.md` are the runtime source of truth for prompt assembly. The
host files under `agent_dir/` are a required explicit mirror for control-plane
operations, diagnostics, and rebootstrap. Mirror reconciliation runs after
sandbox restore, on bot startup, and after normal CC invocations.
```

- [ ] **Step 2: Update `ARCHITECTURE.md` codegen category wording**

In `ARCHITECTURE.md`, below the `AgentOwned` category table, add:

```markdown
For sandboxed agents, identity `AgentOwned` files are authoritative in
`/sandbox` once the sandbox exists. Host copies are an explicit mirror, not the
prompt source. Code that needs a complete host mirror must call the identity
mirror reconciliation helper instead of assuming a prior user message ran
reverse sync.
```

- [ ] **Step 3: Update lifecycle restore/startup docs**

In `docs/architecture/lifecycle.md`, add a restore/startup note near the sandbox restore or startup flow:

```markdown
Sandboxed identity files are restored from `sandbox.tar.gz` into `/sandbox`.
After restore and again on bot startup, Right Agent reconciles
`IDENTITY.md`, `SOUL.md`, and `USER.md` from `/sandbox` into the host
`agent_dir/` mirror. This mirror is required for control-plane checks, but
sandboxed prompt assembly reads `/sandbox` directly.
```

- [ ] **Step 4: Commit**

```bash
git add PROMPT_SYSTEM.md ARCHITECTURE.md docs/architecture/lifecycle.md
git commit -m "docs: clarify sandbox identity mirror lifecycle"
```

---

### Task 7: Verification

**Files:**
- No code changes.

- [ ] **Step 1: Run targeted package tests**

Run:

```bash
devenv shell -- cargo test -p right-agent identity_mirror --lib
devenv shell -- cargo test -p right-bot should_accept_bootstrap --lib
devenv shell -- cargo test -p right-bot reverse_sync_files_match_identity_mirror_contract --lib
devenv shell -- cargo test -p right restore_config_files_copy_custom_sandbox_policy_before_codegen --lib
```

Expected: all PASS.

- [ ] **Step 2: Run full workspace tests**

Run:

```bash
devenv shell -- cargo test --workspace
```

Expected: PASS.

- [ ] **Step 3: Run full workspace build**

Run:

```bash
devenv shell -- cargo build --workspace
```

Expected: PASS.

- [ ] **Step 4: Run diff hygiene**

Run:

```bash
git diff --check
```

Expected: no output.

- [ ] **Step 5: Optional live UAT on restored agents**

Only if OpenShell and the local Right runtime are available:

```bash
devenv shell -- cargo run -p right -- agent ssh right -- ls -l /sandbox/IDENTITY.md /sandbox/SOUL.md /sandbox/USER.md
devenv shell -- cargo run -p right -- agent ssh him -- ls -l /sandbox/IDENTITY.md /sandbox/SOUL.md /sandbox/USER.md
devenv shell -- cargo run -p right -- doctor
```

Expected:
- `right` and `him` sandbox files exist.
- Host mirrors exist after restore/startup reconciliation.
- `right doctor` no longer warns about missing `SOUL.md` or `USER.md` for `right` / `him`.
- Any unrelated `test` sandbox warning is not part of this plan.

- [ ] **Step 6: Final commit if verification required changes**

```bash
git status --short
git add crates/right-agent/src/identity_mirror.rs crates/right-agent/src/lib.rs crates/bot/src/sync.rs crates/bot/src/cc/worker_reply.rs crates/bot/src/telegram/worker.rs crates/bot/src/lib.rs crates/right/src/main.rs PROMPT_SYSTEM.md ARCHITECTURE.md docs/architecture/lifecycle.md
git commit -m "fix: reconcile sandbox identity mirror explicitly"
```

Skip this commit if all earlier task commits already cover the final state.

---

## Self-Review

- Spec coverage:
  - Sandboxed prompt source remains `/sandbox/*.md`: documented and preserved.
  - Host mirror remains required and explicit: implemented through shared reconciliation helper, startup reconciliation, restore reconciliation, and post-invocation sync.
  - Bootstrap acceptance checks sandbox authoritative files: implemented by sandbox-to-host reconciliation before acceptance.
  - Reverse sync only files needed on host: narrowed to `IDENTITY.md`, `SOUL.md`, `USER.md`.
  - Allowlist backup/restore: intentionally excluded.

- Placeholder scan:
  - No TBD/TODO/fill-in-later placeholders.
  - Each code step gives exact snippets and paths.
  - Verification commands and expected outcomes are explicit.

- Type consistency:
  - Shared constant name is consistently `IDENTITY_MIRROR_FILES`.
  - Shared helper names are consistently `host_identity_mirror_complete()` and `sync_identity_mirror_from_sandbox()`.
  - Existing bot wrapper remains `reverse_sync_md()` to avoid broad call-site churn.
