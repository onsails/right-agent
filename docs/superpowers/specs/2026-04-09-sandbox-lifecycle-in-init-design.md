# Sandbox Lifecycle in Init

**Status:** Design

**Problem:** Sandbox creation happens at bot startup (`rightclaw up`), not during `rightclaw init`. When a user re-runs `rightclaw init` (deleting `~/.rightclaw`), the old OpenShell sandbox persists with stale files. The bot reuses it, and agent definitions inside are outdated — breaking features like bootstrap that depend on new files being present.

**Solution:** Move sandbox creation from bot startup to `rightclaw init` / `rightclaw agent init`. The bot expects a ready sandbox and fails if one doesn't exist.

## Design

### 1. Init creates sandbox

After writing agent files and running codegen, `rightclaw init` and `rightclaw agent init` create the OpenShell sandbox for each agent with `sandbox: mode: openshell`.

**Flow for `rightclaw init`:**
```
write agent files (AGENTS.md, BOOTSTRAP.md, agent.yaml, etc.)
run per-agent codegen (agent defs, settings.json, .claude.json, policy.yaml, etc.)
if sandbox mode == openshell:
    check OpenShell availability (binary + mTLS certs)
    check if sandbox rightclaw-<name> already exists (gRPC GetSandbox)
    if exists:
        interactive: prompt "Recreate or Cancel?"
        -y / --force: recreate without asking
    prepare staging dir
    spawn sandbox + wait for READY
    initial file upload (content .md files into .claude/agents/, etc.)
```

**Flow for `rightclaw agent init`:**
Same sandbox creation logic after agent files are written. Same interactive prompt if sandbox exists.

### 2. Stale sandbox prompt

When `rightclaw init` or `agent init` detects an existing sandbox:

```
⚠ Sandbox 'rightclaw-right' already exists.
  1. Recreate — delete and create fresh sandbox
  2. Cancel — use `rightclaw agent config` to update existing agent
Choose [1/2]:
```

With `-y` or `--force`: recreate without prompting (non-interactive default).

### 3. Bot startup expects ready sandbox

Change `crates/bot/src/lib.rs` sandbox lifecycle (lines 281-313):

**Current:** `is_sandbox_ready()` → if exists: reuse, if not: create
**New:** `is_sandbox_ready()` → if exists: reuse (apply policy), if not: **error**

Error message:
```
Sandbox 'rightclaw-right' not found.
Run `rightclaw init` or `rightclaw agent init right` to create it.
```

The bot no longer creates sandboxes. It only:
- Checks sandbox exists and is READY
- Applies updated policy (hot-reload)
- Runs initial_sync (upload config + content files)

### 4. Doctor checks sandbox existence

Add a per-agent check in `rightclaw doctor` for openshell agents:

| Check | Severity | Message |
|-------|----------|---------|
| Sandbox exists and is READY | Fail | "sandbox 'rightclaw-{name}' not found — run `rightclaw agent init {name}`" |

Only checked when:
- Agent has `sandbox: mode: openshell` in agent.yaml
- OpenShell binary is available
- mTLS certs are present

Skip gracefully if OpenShell is not installed or certs are missing (already handled by existing doctor checks).

### 5. OpenShell not available during init

If agent has `sandbox: mode: openshell` but OpenShell is not installed or mTLS certs are missing, init fails with:

```
Error: OpenShell is required for sandbox mode 'openshell'.
  fix: Install OpenShell and run `openshell auth login` to generate mTLS certificates.
  alt: Use `--sandbox-mode none` to run without a sandbox.
```

### 6. Code changes

#### `crates/rightclaw/src/init.rs`

After `init_agent()` writes files and runs codegen:
- Check sandbox mode from the just-written agent.yaml
- If openshell: run sandbox creation (reuse existing `prepare_staging_dir` + `spawn_sandbox` + `wait_for_ready` from lib.rs)
- Handle stale sandbox prompt

Extract sandbox creation logic from `crates/bot/src/lib.rs` into a shared function in `crates/rightclaw/src/openshell.rs`:

```rust
pub async fn create_or_replace_sandbox(
    agent_name: &str,
    policy_path: &Path,
    staging_dir: Option<&Path>,
    force_recreate: bool,
) -> miette::Result<()>
```

This function:
1. Connects via gRPC
2. Checks if sandbox exists
3. If exists and `force_recreate`: delete + create
4. If exists and not force: return error (caller handles prompt)
5. If not exists: create
6. Wait for READY

#### `crates/bot/src/lib.rs`

Remove sandbox creation block (lines 289-313). Replace with:

```rust
if !sandbox_exists {
    return Err(miette::miette!(
        help = "Run `rightclaw init` or `rightclaw agent init {name}` to create the sandbox",
        "Sandbox '{}' not found",
        sandbox
    ));
}
// Reuse: apply policy, generate SSH config, proceed to sync
```

Remove `prepare_staging_dir` function (moves to init or openshell module).

#### `crates/rightclaw/src/doctor.rs`

Add sandbox existence check for openshell agents. Requires async (gRPC call) — doctor already runs in tokio runtime.

#### `crates/rightclaw-cli/src/main.rs`

Wire sandbox creation into init and agent-init command handlers. Needs tokio runtime for gRPC + spawn_sandbox.

## Non-goals

- Sandbox migration (updating files inside existing sandbox without recreate) — out of scope
- Automatic sandbox recreation on `rightclaw up` — intentionally removed, init is the control point
- Sandbox deletion on `rightclaw agent delete` — future work

## Files changed

| File | Change |
|------|--------|
| `crates/rightclaw/src/openshell.rs` | Extract `create_or_replace_sandbox()` shared function |
| `crates/rightclaw/src/init.rs` | Call sandbox creation after agent file setup |
| `crates/bot/src/lib.rs` | Remove sandbox creation, fail if missing |
| `crates/rightclaw/src/doctor.rs` | Add sandbox existence check |
| `crates/rightclaw-cli/src/main.rs` | Wire sandbox creation into init/agent-init handlers |
