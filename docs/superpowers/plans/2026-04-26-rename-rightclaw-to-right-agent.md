# Rename `rightclaw` → `right-agent` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Rename the project from `rightclaw` to `right-agent` across crates, binary (`right`), runtime layout (`~/.right/`), env vars (`RIGHT_HOME`), and living documentation, while auto-migrating existing `~/.rightclaw/` deployments and preserving installed OpenShell sandboxes.

**Architecture:** Three-phase mechanical refactor. Phase A (Tasks 1–4) lands sandbox-name + migration-helper changes in the *current* crate names — small, testable. Phase B (Tasks 5–7) is the atomic crate rename: `git mv` + Cargo.toml + identifier sweep, one commit. Phase C (Tasks 8–14) is peripheral: install.sh, release-plz, prose docs, e2e script, final grep gate. Historical docs (`docs/superpowers/specs/**`, `docs/plans/**`, `CHANGELOG.md` history) are out of scope.

**Tech Stack:** Rust 2024, Cargo workspace, `git mv`, `std::fs::rename` (atomic), `std::net::TcpStream` (PC-running probe), `tracing` (deprecation log).

**Spec:** `docs/superpowers/specs/2026-04-26-rename-rightclaw-to-right-agent-design.md`

---

## Phase A — Pre-rename groundwork

### Task 1: Sandbox naming for new agents (in current crate paths)

**Goal:** `agent init` writes an explicit `sandbox.name: right-{agent_name}` for new agents. Existing agents (without `sandbox.name`) continue to use the `sandbox_name()` fallback that still returns `rightclaw-{agent_name}` — this is the backward-compat shim.

**Files:**
- Modify: `crates/rightclaw/src/init.rs` (or wherever `agent init` writes `agent.yaml`) — add `sandbox.name: right-{agent}`
- Modify: `crates/rightclaw/src/openshell.rs:22-27` — leave `sandbox_name()` AND `ssh_host()` returning `rightclaw-{agent_name}` (compat). Add a comment explaining why.
- Modify: `crates/rightclaw-cli/src/main.rs:2443` and `:4060` — sandbox-migration timestamped name `format!("rightclaw-{agent_name}-{timestamp}")` → `format!("right-{agent_name}-{timestamp}")`.
- Test: `crates/rightclaw/src/init_tests.rs` (new) or extend existing init tests.

- [ ] **Step 1: Read the relevant section of `init_agent` to confirm RMW shape**

```bash
sed -n '109,135p' crates/rightclaw/src/init.rs
```

Expected: two branches around the `sandbox:` block — one for `SandboxMode::Openshell` writing `\nsandbox:\n  mode: openshell\n  policy_file: policy.yaml\n` and one for the `None` mode writing `\nsandbox:\n  mode: none\n`. These are at approximately lines 125 and 128.

- [ ] **Step 2: Write a failing test asserting `init_agent` produces `sandbox.name: right-<agent_name>`**

Add to the `#[cfg(test)] mod tests` block at the bottom of `crates/rightclaw/src/init.rs` (the file already has tests like `init_creates_default_agent_files` — pattern after them):

```rust
#[test]
fn init_agent_writes_explicit_sandbox_name_with_right_prefix() {
    let dir = tempfile::tempdir().unwrap();
    let agents_parent = dir.path();
    let agent_dir = init_agent(agents_parent, "foo", None).unwrap();

    let yaml = std::fs::read_to_string(agent_dir.join("agent.yaml")).unwrap();
    assert!(
        yaml.contains("name: right-foo"),
        "agent.yaml must contain explicit sandbox.name 'right-foo'; got:\n{yaml}"
    );
}
```

- [ ] **Step 3: Run test to confirm it fails**

```bash
cargo test -p rightclaw init_agent_writes_explicit_sandbox_name_with_right_prefix -- --nocapture
```
Expected: FAIL — `agent.yaml` lacks `name: right-foo`.

- [ ] **Step 4: Update the two `yaml.push_str` calls in `init_agent` to include the explicit name**

Edit `crates/rightclaw/src/init.rs`. Find the two lines:

- `yaml.push_str("\nsandbox:\n  mode: openshell\n  policy_file: policy.yaml\n");` (around line 125) — change to:
  `yaml.push_str(&format!("\nsandbox:\n  mode: openshell\n  policy_file: policy.yaml\n  name: right-{name}\n"));`
- `yaml.push_str("\nsandbox:\n  mode: none\n");` (around line 128) — change to:
  `yaml.push_str(&format!("\nsandbox:\n  mode: none\n  name: right-{name}\n"));`

(The `name` variable in scope is the agent name, the same one used for `agents_dir = agents_parent_dir.join(name)`.)

- [ ] **Step 5: Re-run test to confirm pass**

```bash
cargo test -p rightclaw agent_init_writes_explicit_sandbox_name_with_right_prefix
```
Expected: PASS.

- [ ] **Step 6: Update sandbox-migration timestamped name format**

Edit `crates/rightclaw-cli/src/main.rs`:
- Line ~2443: `format!("rightclaw-{agent_name}-{timestamp}")` → `format!("right-{agent_name}-{timestamp}")`
- Line ~4060: same change.

Read both lines first to confirm they match before editing. Use `Edit` with `replace_all: false` for each.

- [ ] **Step 7: Update the doc-comment on `SandboxConfig::name`**

Edit `crates/rightclaw/src/agent/types.rs:106-107`:

```rust
/// Explicit sandbox name. When set, overrides the deterministic
/// `rightclaw-{agent_name}` fallback (kept for backward compatibility
/// with agents created before the right-agent rename). New agents
/// (created via `right agent init`) get `right-{agent_name}` written
/// here explicitly.
#[serde(default)]
pub name: Option<String>,
```

- [ ] **Step 8: Add a comment to `sandbox_name()` explaining the compat behaviour**

Edit `crates/rightclaw/src/openshell.rs:21-23`:

```rust
/// Generate deterministic fallback sandbox name from agent name.
///
/// This deliberately still returns `rightclaw-{agent_name}` after the
/// `right-agent` rename to keep existing pre-rename agents working —
/// their `agent.yaml` has no explicit `sandbox.name`, so this fallback
/// is what their bot uses to find their existing sandbox.
///
/// New agents (created via `right agent init`) get an explicit
/// `sandbox.name: right-{agent_name}` written into `agent.yaml` and
/// never hit this fallback.
pub fn sandbox_name(agent_name: &str) -> String {
    format!("rightclaw-{agent_name}")
}
```

- [ ] **Step 9: Run the affected tests**

```bash
cargo test -p rightclaw
cargo test -p rightclaw-cli
```
Expected: PASS.

- [ ] **Step 10: Commit**

```bash
git add -A
git commit -m "feat: write explicit sandbox.name on agent init

New agents get sandbox.name: right-<agent> written into agent.yaml,
so they create OpenShell sandboxes with the new prefix. Existing
agents (no explicit name in yaml) keep falling back to sandbox_name()
which still returns rightclaw-<agent> for compatibility.

Sandbox-migration timestamped names also updated to right-<agent>-<ts>.
sandbox_name() fallback is intentionally unchanged — it is the
compat shim for pre-rename agents."
```

---

### Task 2: Test sandbox prefix `rightclaw-test-` → `right-test-`

**Goal:** `TestSandbox::create("foo")` produces a sandbox named `right-test-foo` instead of `rightclaw-test-foo`. `pkill_test_orphans` is parameter-driven (not prefix-hardcoded) so it needs no change. Stale `rightclaw-test-*` sandboxes from prior runs are cleaned up by their original processes' Drop / panic-hook paths; any leaks must be removed manually before the rename merge — a one-time `openshell sandbox list | grep rightclaw-test-` followed by `openshell sandbox delete <name>` for each.

**Files:**
- Modify: `crates/rightclaw/src/test_support.rs:24` — prefix change.
- Modify: `crates/rightclaw/src/test_support.rs:22, :89` — doc comment.
- Modify: hardcoded test sandbox names in `crates/rightclaw-cli/tests/cli_integration.rs:732`, `crates/rightclaw-cli/src/right_backend_tests.rs:209, :259`, `crates/bot/src/sync.rs:317`, `crates/rightclaw/tests/policy_apply.rs:40`.

- [ ] **Step 1: Read `test_cleanup::pkill_test_orphans` to understand current matching**

```bash
sed -n '1,80p' crates/rightclaw/src/test_cleanup.rs
rg 'pkill_test_orphans|pkill -' crates/rightclaw/src/test_cleanup.rs -n
```
Note the function signature and how it builds its `pgrep`/`pkill` regex.

- [ ] **Step 2: Write a failing test for the new prefix in `test_support.rs`**

Add to the test module of `test_support.rs` (or `test_support_tests.rs` if extracted):

```rust
#[test]
fn test_sandbox_uses_right_test_prefix() {
    // We don't actually call OpenShell — just inspect the format string.
    let computed = format!("right-test-{}", "lifecycle");
    assert_eq!(computed, "right-test-lifecycle");

    // Smoke check: confirm test_support.rs source file was updated.
    let src = include_str!("test_support.rs");
    assert!(
        src.contains("right-test-{test_name}"),
        "test_support.rs must use 'right-test-' prefix in sandbox name format"
    );
    assert!(
        !src.contains("rightclaw-test-{test_name}"),
        "test_support.rs must not still contain old 'rightclaw-test-' format"
    );
}
```

- [ ] **Step 3: Run test, confirm fail**

```bash
cargo test -p rightclaw --features test-support test_sandbox_uses_right_test_prefix
```
Expected: FAIL on the second/third asserts (source still has old prefix).

- [ ] **Step 4: Update prefix in `crates/rightclaw/src/test_support.rs`**

Edit line 24 (the `format!` expression) and the two doc comments at lines 22 and 89:

- `let name = format!("rightclaw-test-{test_name}");` → `let name = format!("right-test-{test_name}");`
- `/// previous runs. The sandbox name is \`rightclaw-test-<test_name>\`.` → `/// previous runs. The sandbox name is \`right-test-<test_name>\`.`
- `/// Sandbox name (already prefixed with \`rightclaw-test-\`).` → `/// Sandbox name (already prefixed with \`right-test-\`).`

- [ ] **Step 5: Verify `pkill_test_orphans` does not need changes**

`pkill_test_orphans(sandbox_name: &str)` in `crates/rightclaw/src/test_cleanup.rs` already takes the sandbox name as a parameter and uses it directly in three `pgrep`-style patterns (`openshell sandbox create --name {sandbox_name}` etc.). It has no hardcoded `rightclaw-test-` prefix, so the new caller name `right-test-foo` flows through unchanged.

Confirm by reading:

```bash
sed -n '78,100p' crates/rightclaw/src/test_cleanup.rs
```

Expected: three `format!` patterns each interpolating `{sandbox_name}`, no fixed `rightclaw` literal in the function body. No edit needed.

- [ ] **Step 6: Update hardcoded test sandbox names**

For each of these locations, replace `rightclaw-test-` with `right-test-`:

- `crates/rightclaw-cli/tests/cli_integration.rs:732` — `"rightclaw-test-policy-validate"` → `"right-test-policy-validate"`
- `crates/rightclaw-cli/src/right_backend_tests.rs:209` — `"rightclaw-test-bootstrap-present"` → `"right-test-bootstrap-present"`
- `crates/rightclaw-cli/src/right_backend_tests.rs:259` — `"rightclaw-test-bootstrap-missing"` → `"right-test-bootstrap-missing"`
- `crates/bot/src/sync.rs:317` — `"rightclaw-test-sync-upload"` → `"right-test-sync-upload"`
- `crates/rightclaw/tests/policy_apply.rs:40` — `format!("rightclaw-test-{test_name}")` → `format!("right-test-{test_name}")`

Use `Edit` with `replace_all: false` for each, reading the surrounding context first if needed.

- [ ] **Step 7: Update doc comment on line 227 of `right_backend_tests.rs`**

Read line ~227 first. The comment likely says `// Agent name must match: sandbox_name = "rightclaw-{agent_name}"`. This is referring to `sandbox_name()` which is intentionally unchanged (Task 1 / Step 8). Leave this comment as-is — it's accurate.

- [ ] **Step 8: Run tests**

```bash
cargo test -p rightclaw --features test-support test_sandbox_uses_right_test_prefix
cargo build --workspace
```
Expected: PASS, build succeeds. Don't run the full test suite that touches OpenShell yet — that needs a live cluster.

- [ ] **Step 9: Commit**

```bash
git add -A
git commit -m "refactor(test): rename test sandbox prefix to right-test-

TestSandbox::create now produces right-test-<name> sandboxes.
pkill_test_orphans is parameter-driven so no changes there. Any
leftover rightclaw-test-* sandboxes from prior runs need a one-time
manual cleanup (openshell sandbox list | grep rightclaw-test-)."
```

---

### Task 3: Home directory auto-migration helper

**Goal:** A pure, testable function that takes the resolved home directory and an optional override path, detects if the *old* `~/.rightclaw/` directory needs migrating, and atomically renames it. Refuses migration if PC is running on the old port.

**Files:**
- Modify: `crates/rightclaw/src/config/mod.rs` — add `migrate_old_home()` helper, wire into `resolve_home()`.
- Test: `crates/rightclaw/src/config/mod.rs` `#[cfg(test)]` — unit tests for migration behaviour.

- [ ] **Step 1: Write failing tests for `migrate_old_home`**

In `crates/rightclaw/src/config/mod.rs`'s test module, add:

```rust
#[test]
fn migrate_old_home_renames_when_only_old_exists() {
    let tmp = tempfile::tempdir().unwrap();
    let old = tmp.path().join(".rightclaw");
    let new = tmp.path().join(".right");
    std::fs::create_dir_all(&old).unwrap();
    std::fs::write(old.join("marker"), b"hello").unwrap();

    let result = migrate_old_home(&old, &new);
    assert!(result.is_ok(), "migrate_old_home failed: {:?}", result);
    assert!(!old.exists(), "old dir must be gone after migration");
    assert!(new.exists(), "new dir must exist after migration");
    assert_eq!(
        std::fs::read(new.join("marker")).unwrap(),
        b"hello",
        "contents must be preserved"
    );
}

#[test]
fn migrate_old_home_noop_when_new_exists() {
    let tmp = tempfile::tempdir().unwrap();
    let old = tmp.path().join(".rightclaw");
    let new = tmp.path().join(".right");
    std::fs::create_dir_all(&old).unwrap();
    std::fs::create_dir_all(&new).unwrap();

    let result = migrate_old_home(&old, &new);
    assert!(result.is_ok(), "must noop when new dir exists");
    assert!(old.exists(), "old dir must still exist (no rename)");
    assert!(new.exists(), "new dir must still exist");
}

#[test]
fn migrate_old_home_noop_when_old_absent() {
    let tmp = tempfile::tempdir().unwrap();
    let old = tmp.path().join(".rightclaw");
    let new = tmp.path().join(".right");

    let result = migrate_old_home(&old, &new);
    assert!(result.is_ok(), "must noop when old dir absent");
    assert!(!old.exists());
    assert!(!new.exists());
}

#[test]
fn migrate_old_home_refuses_when_pc_running() {
    use std::net::TcpListener;

    let tmp = tempfile::tempdir().unwrap();
    let old = tmp.path().join(".rightclaw");
    let new = tmp.path().join(".right");
    std::fs::create_dir_all(old.join("run")).unwrap();

    // Bind a local TCP listener and write its port into state.json.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::fs::write(
        old.join("run").join("state.json"),
        format!(
            r#"{{"agents":[],"socket_path":"","started_at":"x","pc_port":{port},"pc_api_token":null}}"#
        ),
    )
    .unwrap();

    let err = migrate_old_home(&old, &new).expect_err("must refuse migration");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("running") || msg.contains("process-compose") || msg.contains("rightclaw"),
        "error must mention PC running; got: {msg}"
    );
    assert!(old.exists(), "no rename on refusal");
    assert!(!new.exists());
}
```

- [ ] **Step 2: Run tests, confirm fail**

```bash
cargo test -p rightclaw migrate_old_home
```
Expected: FAIL — `migrate_old_home` doesn't exist yet.

- [ ] **Step 3: Implement `migrate_old_home`**

Add to `crates/rightclaw/src/config/mod.rs` (above `resolve_home`):

```rust
/// Migrate `~/.rightclaw/` to `~/.right/` if needed.
///
/// Returns `Ok(())` when:
/// - The old dir doesn't exist (fresh install or already migrated).
/// - Both dirs exist (already migrated; old dir left alone — operator can decide).
/// - The old dir was successfully renamed.
///
/// Returns `Err` when:
/// - process-compose is running against the old `state.json` port.
/// - The atomic rename failed (cross-filesystem `EXDEV`, permissions).
fn migrate_old_home(old: &Path, new: &Path) -> miette::Result<()> {
    if !old.exists() {
        return Ok(());
    }
    if new.exists() {
        return Ok(());
    }

    // PC-running probe: if state.json carries a port and that port is
    // accepting connections, a process-compose instance is alive. Refuse
    // migration to avoid breaking open file handles.
    let state_path = old.join("run").join("state.json");
    if state_path.exists()
        && let Ok(content) = std::fs::read_to_string(&state_path)
        && let Ok(json) = serde_json::from_str::<serde_json::Value>(&content)
        && let Some(port) = json.get("pc_port").and_then(|v| v.as_u64())
    {
        let addr = format!("127.0.0.1:{port}");
        if std::net::TcpStream::connect_timeout(
            &addr.parse().expect("loopback addr always parses"),
            std::time::Duration::from_millis(500),
        )
        .is_ok()
        {
            return Err(miette::miette!(
                "Detected {} with a running process-compose on port {}. \
                 Stop it before upgrade — run the old `rightclaw down` (or kill the PC), then re-run.",
                old.display(),
                port,
            ));
        }
    }

    std::fs::rename(old, new).map_err(|e| {
        miette::miette!(
            "Failed to rename {} → {}: {}. \
             If the dirs are on different filesystems, run `mv {} {}` manually and re-run.",
            old.display(),
            new.display(),
            e,
            old.display(),
            new.display(),
        )
    })?;

    tracing::info!("Migrated {} → {}", old.display(), new.display());
    Ok(())
}
```

- [ ] **Step 4: Run tests, confirm pass**

```bash
cargo test -p rightclaw migrate_old_home
```
Expected: 4 tests PASS.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(config): add migrate_old_home helper

Atomically renames ~/.rightclaw/ → ~/.right/ when only the old dir
exists. Refuses when process-compose is running on the port read
from old state.json. Idempotent on already-migrated systems."
```

---

### Task 4: Wire migration + env var into `resolve_home()`

**Goal:** `resolve_home()` reads `RIGHT_HOME` (not `RIGHTCLAW_HOME`), defaults to `~/.right/`, and triggers `migrate_old_home` when the default path resolves but the old dir exists.

**Files:**
- Modify: `crates/rightclaw/src/config/mod.rs` — `resolve_home()` body and tests.

- [ ] **Step 1: Write a failing test for the new default path and migration trigger**

Add to the test module in `crates/rightclaw/src/config/mod.rs`:

```rust
#[test]
fn resolve_home_returns_dot_right_default() {
    let result = resolve_home(None, None).unwrap();
    let expected = dirs::home_dir().unwrap().join(".right");
    assert_eq!(result, expected, "default home must be ~/.right after rename");
}

#[test]
fn resolve_home_does_not_read_rightclaw_home_env() {
    // Note: this test only verifies the contract that resolve_home does not
    // itself look at RIGHTCLAW_HOME — that responsibility belongs to the
    // caller. We verify by passing env_home: None and confirming the
    // default ~/.right is returned regardless of any ambient env.
    let result = resolve_home(None, None).unwrap();
    let expected = dirs::home_dir().unwrap().join(".right");
    assert_eq!(result, expected);
}
```

Also update the existing tests at `crates/rightclaw/src/config/mod.rs:175-189` so the asserted default path is `.right` not `.rightclaw`:

```rust
#[test]
fn resolve_home_returns_default_when_both_none() {
    let expected = dirs::home_dir().unwrap().join(".right");
    let result = resolve_home(None, None).unwrap();
    assert_eq!(result, expected);
}
```

- [ ] **Step 2: Run tests, confirm fail**

```bash
cargo test -p rightclaw resolve_home
```
Expected: FAIL — current code returns `.rightclaw`.

- [ ] **Step 3: Update `resolve_home` to return `.right` and trigger migration**

In `crates/rightclaw/src/config/mod.rs`, replace the existing function:

```rust
/// Resolve the runtime home directory: cli_home > env_home > ~/.right
///
/// When falling through to the default path, also triggers
/// `migrate_old_home` to rename a leftover `~/.rightclaw/` from a
/// pre-rename install. The migration is idempotent and fast (single
/// existence check + rename or noop).
pub fn resolve_home(cli_home: Option<&str>, env_home: Option<&str>) -> miette::Result<PathBuf> {
    if let Some(home) = cli_home {
        return Ok(PathBuf::from(home));
    }
    if let Some(home) = env_home {
        return Ok(PathBuf::from(home));
    }
    let home_dir =
        dirs::home_dir().ok_or_else(|| miette::miette!("Could not determine home directory"))?;
    let new = home_dir.join(".right");
    let old = home_dir.join(".rightclaw");
    migrate_old_home(&old, &new)?;
    Ok(new)
}
```

Also update the doc comment on line 5 from `Resolve RIGHTCLAW_HOME: cli_home > env_home > ~/.rightclaw` to match.

- [ ] **Step 4: Run tests, confirm pass**

```bash
cargo test -p rightclaw resolve_home migrate_old_home
```
Expected: PASS.

- [ ] **Step 5: Update env-var reads at all call sites — `RIGHTCLAW_HOME` → `RIGHT_HOME`**

Search and update each callsite. These are the call sites identified during exploration:

```bash
rg 'RIGHTCLAW_HOME' --no-ignore -g '!target' -g '!.git' -g '!docs/superpowers/specs/**' -g '!docs/plans/**' -n
```

Edit each one:

- `crates/rightclaw-cli/src/main.rs:19` — `#[arg(long, env = "RIGHTCLAW_HOME")]` → `#[arg(long, env = "RIGHT_HOME")]`
- `crates/rightclaw-cli/src/main.rs:264` — comment `$RIGHTCLAW_HOME/run/...` → `$RIGHT_HOME/run/...`
- `crates/rightclaw-cli/src/main.rs:324` — comment `$RIGHTCLAW_HOME/agents/...` → `$RIGHT_HOME/agents/...`
- `crates/rightclaw-cli/src/main.rs:412` — `std::env::var("RIGHTCLAW_HOME")` → `std::env::var("RIGHT_HOME")`
- `crates/rightclaw-cli/tests/allowlist_cli.rs:7` — `cmd.env("RIGHTCLAW_HOME", home)` → `cmd.env("RIGHT_HOME", home)`
- `crates/bot/src/lib.rs:58` — comment update
- `crates/bot/src/lib.rs:60` — comment update
- `crates/bot/src/lib.rs:88` — comment update
- `crates/bot/src/lib.rs:91` — `std::env::var("RIGHTCLAW_HOME")` → `std::env::var("RIGHT_HOME")`
- `crates/bot/src/stt/whisper.rs:110` — doc comment update
- `crates/bot/src/stt/whisper.rs:112` — `std::env::var_os("RIGHTCLAW_HOME")` → `std::env::var_os("RIGHT_HOME")`
- `crates/bot/src/stt/mod.rs:144, :238` — `std::env::var_os("RIGHTCLAW_HOME")` → `std::env::var_os("RIGHT_HOME")`
- `crates/rightclaw/src/stt.rs:17` — doc comment update.

For each, use `Edit` with `replace_all: false` after reading the surrounding context.

- [ ] **Step 6: Update `RC_RIGHTCLAW_HOME` → `RC_RIGHT_HOME`**

```bash
rg 'RC_RIGHTCLAW_HOME' --no-ignore -g '!target' -g '!.git' -g '!docs/superpowers/specs/**' -g '!docs/plans/**' -n
```

Locations:

- `crates/rightclaw-cli/src/memory_server.rs:532` — `std::env::var("RC_RIGHTCLAW_HOME")` → `std::env::var("RC_RIGHT_HOME")`
- `crates/rightclaw-cli/src/memory_server.rs:536` — log message `"RC_RIGHTCLAW_HOME not set"` → `"RC_RIGHT_HOME not set"`
- `templates/process-compose.yaml.j2:10` — `RC_RIGHTCLAW_HOME=` → `RC_RIGHT_HOME=`
- `crates/rightclaw/src/codegen/mcp_config.rs:47` — JSON key `"RC_RIGHTCLAW_HOME"` → `"RC_RIGHT_HOME"`
- `crates/rightclaw/src/codegen/mcp_config.rs:311-312` — test assertion key + message.

- [ ] **Step 7: Build and run all tests**

```bash
cargo build --workspace
cargo test --workspace --no-fail-fast
```
Expected: build succeeds, all tests pass. (Tests requiring live OpenShell may fail if no cluster — that's fine, they're outside this task's scope. Run them manually after Phase B.)

- [ ] **Step 8: Commit**

```bash
git add -A
git commit -m "refactor(config): migrate to RIGHT_HOME / ~/.right

resolve_home() now returns ~/.right as the default and triggers
migrate_old_home to atomically rename a leftover ~/.rightclaw on
fall-through. All call sites read RIGHT_HOME (hard rename — old
RIGHTCLAW_HOME is no longer read). Internal RC_RIGHTCLAW_HOME also
flipped to RC_RIGHT_HOME (process-compose passthrough)."
```

---

## Phase B — Atomic crate rename

### Task 5: `git mv` crate directories + Cargo.toml updates

**Goal:** Rename `crates/rightclaw/` → `crates/right-agent/` and `crates/rightclaw-cli/` → `crates/right/`. Update workspace and crate Cargo.tomls. Update inter-crate `path` deps. After this commit, the workspace must compile.

**Files:**
- Move: `crates/rightclaw/` → `crates/right-agent/`
- Move: `crates/rightclaw-cli/` → `crates/right/`
- Modify: `Cargo.toml` (workspace root) — `members`.
- Modify: `crates/right-agent/Cargo.toml` — `name = "right-agent"`.
- Modify: `crates/right/Cargo.toml` — `name = "right"`, `[[bin]] name = "right"`, deps `rightclaw → right-agent`, `rightclaw-bot → right-bot`.
- Modify: `crates/bot/Cargo.toml` — `name = "right-bot"`, dep `rightclaw → right-agent`.

- [ ] **Step 1: `git mv` the two crate directories**

```bash
git mv crates/rightclaw crates/right-agent
git mv crates/rightclaw-cli crates/right
git status
```
Expected: `git status` shows two clean directory renames.

- [ ] **Step 2: Update workspace `Cargo.toml`**

Read `Cargo.toml` first. Edit:

```toml
[workspace]
members = ["crates/right-agent", "crates/right", "crates/bot"]
resolver = "3"
```

(The `crates/bot` member stays — its directory name is unchanged.)

- [ ] **Step 3: Update `crates/right-agent/Cargo.toml`**

Read the file first. Change only the `[package]` `name`:

```toml
[package]
name = "right-agent"
version.workspace = true
edition.workspace = true
```

Leave dependencies, features, build-dependencies untouched.

- [ ] **Step 4: Update `crates/right/Cargo.toml`**

Read the file first. Change:

- `[package] name = "rightclaw-cli"` → `name = "right"`
- `[[bin]] name = "rightclaw"` → `name = "right"` (path stays `src/main.rs`)
- `rightclaw = { path = "../rightclaw", version = "*" }` → `right-agent = { path = "../right-agent", version = "*" }`
- `rightclaw-bot = { path = "../bot", version = "*" }` → `right-bot = { path = "../bot", version = "*" }`

Leave everything else untouched.

- [ ] **Step 5: Update `crates/bot/Cargo.toml`**

Read the file first. Change:

- `[package] name = "rightclaw-bot"` → `name = "right-bot"`
- `rightclaw = { path = "../rightclaw", version = "*" }` → `right-agent = { path = "../right-agent", version = "*" }` (regular and dev-dependencies — there are two occurrences)

Leave everything else untouched.

- [ ] **Step 6: Replace `use rightclaw` and `rightclaw::` paths in Rust source**

```bash
rg 'use rightclaw|rightclaw::|extern crate rightclaw' --no-ignore -g '*.rs' -l
```

For each file in the list, replace:

- `use rightclaw::` → `use right_agent::`
- `use rightclaw_bot::` → `use right_bot::`
- `rightclaw::` → `right_agent::` (qualified paths)
- `rightclaw_bot::` → `right_bot::` (qualified paths)
- `extern crate rightclaw` → `extern crate right_agent` (if any)

The fastest safe approach is `sed -i ''` per file (note macOS `sed` requires the empty `''` after `-i`). Do it with care — only `.rs` files, only the patterns above:

```bash
fd -e rs . crates/ | xargs sed -i '' \
    -e 's/\buse rightclaw_bot::/use right_bot::/g' \
    -e 's/\buse rightclaw::/use right_agent::/g' \
    -e 's/\brightclaw_bot::/right_bot::/g' \
    -e 's/\brightclaw::/right_agent::/g' \
    -e 's/\bextern crate rightclaw_bot\b/extern crate right_bot/g' \
    -e 's/\bextern crate rightclaw\b/extern crate right_agent/g'
```

After running, verify with:

```bash
rg '\b(use\s+)?rightclaw(_bot)?::|extern crate rightclaw(_bot)?' --no-ignore -g '*.rs'
```
Expected: zero hits.

- [ ] **Step 7: Update CLI `clap` binary name**

Read `crates/right/src/main.rs:1-30`. Locate the `#[command(name = "rightclaw", ...)]` attribute (or similar — clap may use `version`/`about` only, with `name` derived from the binary). Update:

- `#[command(name = "rightclaw", ...)]` → `#[command(name = "right", ...)]`

Also scan the same file for any user-facing strings that include the binary name:

```bash
rg 'rightclaw' crates/right/src/main.rs -n
```

For each match, decide:
- Help text / error messages mentioning the binary by name → update to `right`.
- Path literals like `~/.rightclaw/` (already covered by Phase A but double-check) → update.
- Comments referencing the project name → update to `right-agent` (project) or `right` (binary), whichever fits.

- [ ] **Step 8: Update lib-crate doc comments referring to "RightClaw"**

```bash
rg '\bRightClaw\b' --no-ignore -g '*.rs' -n
```

For each hit, replace `RightClaw` with `Right Agent`. These are mostly module-level doc comments. Use Edit per file.

- [ ] **Step 9: Build to verify**

```bash
cargo build --workspace 2>&1 | tail -50
```
Expected: build succeeds. If it fails, the typical causes are:
- A `use` path missed by sed (look at the error, fix manually).
- A `Cargo.toml` typo (re-read).
- A test or example referencing the old crate name (rare).

Iterate until clean.

- [ ] **Step 10: Run tests**

```bash
cargo test --workspace --no-fail-fast 2>&1 | tail -40
```
Expected: pre-rename test counts pass. (OpenShell-dependent tests may fail if no cluster — that's expected.)

- [ ] **Step 11: Commit**

```bash
git add -A
git commit -m "refactor: rename crates rightclaw → right-agent / right / right-bot

git mv:
- crates/rightclaw → crates/right-agent
- crates/rightclaw-cli → crates/right

Cargo.toml package names:
- rightclaw → right-agent
- rightclaw-cli → right (with binary name 'right')
- rightclaw-bot → right-bot

All inter-crate path deps and 'use' paths updated. Binary 'right'
replaces 'rightclaw'. Module-level RightClaw → Right Agent doc
comment updates."
```

---

### Task 6: `release-plz.toml`

**Files:** `release-plz.toml`

- [ ] **Step 1: Read current config**

```bash
cat release-plz.toml
```

- [ ] **Step 2: Replace package and tag definitions**

Replace the file contents with:

```toml
[workspace]
changelog_config = "cliff.toml"
changelog_update = false
dependencies_update = false
git_release_enable = false
git_tag_enable = false
git_only = true
publish = false
semver_check = false

[[package]]
name = "right-agent"
version_group = "workspace"
git_tag_enable = true
git_tag_name = "right-agent-v{{ version }}"

[[package]]
name = "right-bot"
version_group = "workspace"
git_tag_enable = true
git_tag_name = "right-bot-v{{ version }}"

[[package]]
name = "right"
version_group = "workspace"
changelog_update = true
changelog_path = "CHANGELOG.md"
changelog_include = ["right-agent", "right-bot"]
git_release_enable = true
git_release_name = "v{{ version }}"
git_tag_enable = true
git_tag_name = "v{{ version }}"
```

- [ ] **Step 3: Commit**

```bash
git add release-plz.toml
git commit -m "chore(release-plz): rename packages and tag prefixes

right-agent-v*, right-bot-v* per-crate tags. Aggregate v* tag now
driven by the 'right' CLI package."
```

---

## Phase C — Peripherals and prose

### Task 7: `install.sh`

**Files:** `install.sh`

- [ ] **Step 1: Read current installer**

```bash
sed -n '1,60p' install.sh
```

- [ ] **Step 2: Replace brand, repo URL, and env var**

Specific changes (read the file fully first to find all hits):

- Header comment "RightClaw Installer" → "Right Agent Installer".
- Description "rightclaw — Multi-agent runtime CLI" → "right — Multi-agent runtime CLI".
- `https://raw.githubusercontent.com/onsails/rightclaw/master/install.sh` → `https://raw.githubusercontent.com/onsails/right-agent/master/install.sh`
- All `RIGHTCLAW_VERSION` → `RIGHT_VERSION`.
- Any `rightclaw` binary references in the install path / curl-download URL → `right`.
- Any `~/.rightclaw/` literals → `~/.right/`.

Run a final check after edits:

```bash
rg 'rightclaw|RIGHTCLAW|RightClaw' install.sh
```
Expected: zero hits.

- [ ] **Step 3: Commit**

```bash
git add install.sh
git commit -m "chore(install): update brand and URLs

RIGHTCLAW_VERSION → RIGHT_VERSION; binary 'right'; repo URL points
at onsails/right-agent."
```

---

### Task 8: `templates/process-compose.yaml.j2`

**Files:** `templates/process-compose.yaml.j2`

- [ ] **Step 1: Confirm only the env var changed**

This file was edited in Phase A (Task 4 / Step 6 — `RC_RIGHTCLAW_HOME` → `RC_RIGHT_HOME`). Verify nothing else in the template needs changing:

```bash
rg 'rightclaw|RightClaw' templates/process-compose.yaml.j2
```

- [ ] **Step 2: Update any remaining hits**

For each remaining hit, decide based on context:
- A process name mentioning `rightclaw` → likely should become `right` (e.g., a process labelled `rightclaw-mcp-server`). The `right-mcp-server` name is what should remain (it was already `right-` per spec — confirm).
- A doc-comment referring to "RightClaw" → "Right Agent".

Edit accordingly. After:

```bash
rg 'rightclaw|RightClaw' templates/process-compose.yaml.j2
```
Expected: zero hits.

- [ ] **Step 3: Commit (if changes)**

```bash
git add templates/process-compose.yaml.j2
git commit -m "chore(templates): update process-compose template prose"
```

(If no changes were needed beyond Task 4, skip this commit.)

---

### Task 9: `tests/e2e/verify-sandbox.sh`

**Files:** `tests/e2e/verify-sandbox.sh`

- [ ] **Step 1: Update env var and any path literals**

```bash
sed -n '1,40p' tests/e2e/verify-sandbox.sh
```

Replace:
- `RIGHTCLAW_HOME="${RIGHTCLAW_HOME:-$HOME/.rightclaw}"` → `RIGHT_HOME="${RIGHT_HOME:-$HOME/.right}"`
- `AGENT_DIR="$RIGHTCLAW_HOME/agents/$AGENT_NAME"` → `AGENT_DIR="$RIGHT_HOME/agents/$AGENT_NAME"`
- All other references to `rightclaw` (binary calls, paths, comments) — review and update each. Binary calls become `right`.

Final check:

```bash
rg 'rightclaw|RIGHTCLAW|RightClaw' tests/e2e/verify-sandbox.sh
```
Expected: zero hits.

- [ ] **Step 2: Commit**

```bash
git add tests/e2e/verify-sandbox.sh
git commit -m "test(e2e): update verify-sandbox.sh for right rename"
```

---

### Task 10: `README.md` prose

**Files:** `README.md`

- [ ] **Step 1: Read the file**

```bash
wc -l README.md
sed -n '1,80p' README.md
```

- [ ] **Step 2: Find all instances**

```bash
rg 'rightclaw|RightClaw|RIGHTCLAW' README.md -n
```

- [ ] **Step 3: Replace based on context**

For each occurrence:

- Repo URL paths `onsails/rightclaw` → `onsails/right-agent`.
- Brand name "RightClaw" (capitalised) → "Right Agent".
- Binary references `rightclaw up`, `rightclaw down`, `rightclaw init`, etc. → `right up`, `right down`, `right init`.
- Path references `~/.rightclaw/` → `~/.right/`.
- Env var references → `RIGHT_HOME`.

Use Edit per occurrence (or sed for the obvious mechanical ones), then re-grep:

```bash
rg 'rightclaw|RightClaw|RIGHTCLAW' README.md
```
Expected: zero hits.

- [ ] **Step 4: Commit**

```bash
git add README.md
git commit -m "docs(readme): rename to Right Agent / right binary"
```

---

### Task 11: `ARCHITECTURE.md` prose

**Files:** `ARCHITECTURE.md`

- [ ] **Step 1: Find all instances**

```bash
rg 'rightclaw|RightClaw|RIGHTCLAW' ARCHITECTURE.md -n | head -60
```

- [ ] **Step 2: Replace based on context**

The patterns are the same as Task 10. Notable:

- `RIGHTCLAW_HOME` references in narrative → `RIGHT_HOME`.
- Crate references `rightclaw`, `rightclaw-cli`, `rightclaw-bot` → `right-agent`, `right`, `right-bot`.
- Path tables like `~/.rightclaw/agents/<name>/` → `~/.right/agents/<name>/`.
- Sandbox name examples `rightclaw-<agent>-<YYYYMMDD-HHMM>` → `right-<agent>-<YYYYMMDD-HHMM>` (in narrative; the deterministic fallback note can stay if it's explicitly documenting the compat shim).
- Binary references `rightclaw up/down/...` → `right up/down/...`.

Final check:

```bash
rg 'rightclaw|RightClaw|RIGHTCLAW' ARCHITECTURE.md
```
Expected: zero hits.

- [ ] **Step 3: Commit**

```bash
git add ARCHITECTURE.md
git commit -m "docs(arch): update names, paths, env vars for right rename"
```

---

### Task 12: `CLAUDE.md`, `CLAUDE.rust.md`, `PROMPT_SYSTEM.md` prose

**Files:** `CLAUDE.md`, `CLAUDE.rust.md`, `PROMPT_SYSTEM.md`

- [ ] **Step 1: Find all instances per file**

```bash
rg 'rightclaw|RightClaw|RIGHTCLAW' CLAUDE.md CLAUDE.rust.md PROMPT_SYSTEM.md -n
```

- [ ] **Step 2: Replace per the same rules as Tasks 10/11**

Same patterns. CLAUDE.md likely contains the heaviest prose (project description); CLAUDE.rust.md is mostly Rust standards (low hits expected); PROMPT_SYSTEM.md describes the prompting subsystem.

Final check:

```bash
rg 'rightclaw|RightClaw|RIGHTCLAW' CLAUDE.md CLAUDE.rust.md PROMPT_SYSTEM.md
```
Expected: zero hits.

- [ ] **Step 3: Commit**

```bash
git add CLAUDE.md CLAUDE.rust.md PROMPT_SYSTEM.md
git commit -m "docs(claude): update names and paths for right rename"
```

---

### Task 13: `docs/SECURITY.md`, `docs/brand-guidelines.html`, agent templates

**Files:** `docs/SECURITY.md`, `docs/brand-guidelines.html`, `templates/right/agent/BOOTSTRAP.md`, `templates/right/agent/agent.yaml`

- [ ] **Step 1: Find all instances**

```bash
rg 'rightclaw|RightClaw|RIGHTCLAW' docs/SECURITY.md docs/brand-guidelines.html templates/right/agent/BOOTSTRAP.md templates/right/agent/agent.yaml -n
```

- [ ] **Step 2: Replace per the same rules**

`docs/brand-guidelines.html` may have visual-design/branding terminology — replace "RightClaw" with "Right Agent" only in textual content; do not touch CSS/HTML structure.

For `templates/right/agent/agent.yaml`: this is the *static* template that `init_agent` reads at compile-time via `include_str!` and then appends RMW lines to (Task 1 modifies the Rust appender, not this template). The template currently contains:
- A header comment `# Agent configuration for the "Right" agent`
- A `# See: https://github.com/onsails/rightclaw` URL comment
- A note `# Auto-generated by rightclaw. Do not edit.`
- A note `# secret: <auto-generated on first rightclaw up>`

Update those four prose strings: URL → `onsails/right-agent`; the two `rightclaw` → `right` (both are the binary name in command examples).

Final check:

```bash
rg 'rightclaw|RightClaw|RIGHTCLAW' docs/SECURITY.md docs/brand-guidelines.html templates/right/agent/
```
Expected: zero hits (or only inside intentional compatibility notes — review each).

- [ ] **Step 3: Commit**

```bash
git add docs/SECURITY.md docs/brand-guidelines.html templates/right/agent/
git commit -m "docs: update security, brand, and agent templates for right rename"
```

---

## Phase D — Final verification

### Task 14: Grep gate, full build, full test, clippy

**Goal:** Prove the rename is complete and the workspace is healthy.

- [ ] **Step 1: Grep gate**

```bash
rg -i 'rightclaw' --no-ignore \
    -g '!target' -g '!.git' \
    -g '!docs/superpowers/specs/**' \
    -g '!docs/plans/**' \
    -g '!CHANGELOG.md'
```

Expected: zero hits, except possibly:
- `crates/right-agent/src/openshell.rs` `sandbox_name()` body and its surrounding doc comment (intentional compat shim — Task 1 / Step 8).
- `crates/right-agent/src/agent/types.rs:106-107` doc comment referencing the `rightclaw-{agent_name}` legacy fallback (intentional).

If any *other* hits remain, fix them: read context, decide whether they're real (then update) or intentional compat references (then add an explanatory comment so future grep-gate passes can ignore them).

- [ ] **Step 2: Full build (debug)**

```bash
cargo build --workspace 2>&1 | tail -20
```
Expected: succeeds, no warnings.

- [ ] **Step 3: Full test (excluding live OpenShell)**

```bash
cargo test --workspace --no-fail-fast 2>&1 | tail -40
```
Expected: passes. Tests requiring live OpenShell will pass if a cluster is available (per project policy: dev machines have OpenShell).

- [ ] **Step 4: Clippy**

```bash
cargo clippy --workspace --all-targets -- -D warnings 2>&1 | tail -20
```
Expected: no warnings.

- [ ] **Step 5: Cargo.lock regeneration check**

```bash
rg '\brightclaw\b|\brightclaw-cli\b|\brightclaw-bot\b' Cargo.lock
```
Expected: zero hits. (If hits exist, run `cargo update -p right-agent` or simply `cargo build --workspace` to regenerate.)

- [ ] **Step 6: Smoke test — `right --help`**

```bash
cargo run --bin right -- --help 2>&1 | head -30
```
Expected: help text mentions `right` (not `rightclaw`), no panics.

- [ ] **Step 7: Commit Cargo.lock if it changed**

```bash
git status
git add Cargo.lock
git commit -m "chore: regenerate Cargo.lock for right rename" || echo "Cargo.lock unchanged"
```

- [ ] **Step 8: Manual upgrade verification (optional, requires real install)**

Documented for the operator; not an automated step:

```
1. On a workstation with a populated ~/.rightclaw/ running prod agents:
   - Run the OLD binary: `rightclaw down`
   - Build new binary: `cargo install --path crates/right --force`
   - Run new binary: `right up`
2. Verify:
   - ~/.rightclaw/ no longer exists
   - ~/.right/ contains the prior contents
   - OpenShell sandboxes (named rightclaw-<agent>) reconnect via gRPC
   - Bot resumes Telegram polling
   - An inbound Telegram message produces a reply
```

- [ ] **Step 9: Final summary commit (if needed)**

If any small fixes were made during verification:

```bash
git add -A
git commit -m "fix: cleanup after rename verification" || echo "nothing to commit"
```

---

## Spec Coverage Self-Check

| Spec section | Implementing task |
|---|---|
| Naming map: binary `right` | Task 5 (Cargo.toml `[[bin]] name`) + Task 7 (install.sh) + prose tasks 10–13 |
| Naming map: crates `right-agent`/`right`/`right-bot` | Task 5 |
| Naming map: `~/.right/` runtime dir | Task 4 |
| Naming map: `RIGHT_HOME`/`RC_RIGHT_HOME` env vars | Task 4 |
| Naming map: new sandbox `right-<agent>-<ts>` | Task 1 (`agent init`, sandbox migration) |
| Naming map: test sandbox `right-test-` | Task 2 |
| Naming map: release-plz tags | Task 6 |
| Naming map: install.sh URL | Task 7 |
| Migration: home dir auto-rename | Tasks 3 + 4 |
| Migration: PC-running guard | Task 3 |
| Migration: env var hard rename | Task 4 |
| Migration: existing OpenShell sandboxes preserved | Task 1 (sandbox_name fallback unchanged) |
| Codebase: workspace + per-crate Cargo.toml | Task 5 |
| Codebase: `use rightclaw` → `use right_agent` sweep | Task 5 |
| Codebase: living-doc prose | Tasks 10–13 |
| Codebase: out-of-scope historical docs | Verified by Task 14 grep gate exclusions |
| Verification: build, test, clippy, grep, smoke | Task 14 |

All spec items mapped to tasks. No gaps.

---

## Notes for the implementer

- **Order matters.** Phase A introduces compat-friendly changes in current crate paths. Phase B is the atomic crate rename. Phase C touches non-Rust files (so order within is flexible). Don't reorder phases.
- **Phase A tasks 1–4 produce four small commits.** Phase B is one big commit. Phase C is one commit per file group. Total: ~13 commits.
- **`sandbox_name()` intentionally still says `rightclaw-{agent_name}`.** Don't "fix" this — it's the compat shim for pre-rename agents. There is an explanatory doc comment.
- **`use right_agent::` not `use right-agent::`.** Cargo replaces `-` with `_` in module names; this is correct.
- **All changes happen on the `rename` branch** (already checked out). Don't push or open a PR until Task 14 / Step 8 (manual upgrade test) is also done.
