# Move platform store from `/platform/` to `/sandbox/.platform/`

## Context

`/platform/` inside the OpenShell sandbox sits on the **overlay filesystem** (device `0,164`), while `/sandbox/` is on a **persistent volume** (`/dev/vda1`). When a container restarts (policy reload, OOM, k8s reschedule), the overlay resets — wiping `/platform/` contents. Symlinks in `/sandbox/.claude/` survive (persistent volume) but become dangling, causing `claude -p` to fail with "MCP config file not found: /sandbox/mcp.json".

Observed 2026-04-22: initial_sync deployed files, container overlay reset before first CC invocation, background sync (5 min later) re-deployed. The 5-minute window left the agent broken.

**Fix:** Move the platform store to `/sandbox/.platform/` so content-addressed files share the persistent volume with their symlinks. Remove `/platform` from the sandbox policy (no longer needed — `/sandbox` already grants recursive read_write).

## Changes

### 1. `crates/rightclaw/src/platform_store.rs`

- **Line 71:** Change `PLATFORM_DIR` from `"/platform"` to `"/sandbox/.platform"`
- **Line 386:** Fix hardcoded warning string `"chmod u+w /platform failed"` → use `PLATFORM_DIR`

All other usages in this file reference `PLATFORM_DIR` and update automatically.

### 2. `crates/rightclaw/src/codegen/policy.rs`

- **Line 92:** Remove `    - /platform` from the `read_write` list in `generate_policy()`. `/sandbox` already covers `/sandbox/.platform/` via Landlock prefix matching.

### 3. `crates/bot/src/sync.rs`

- **Line 353:** Remove `    - /platform` from the test policy YAML string (same reason).

### 4. No other changes needed

- `SANDBOX_MCP_JSON_PATH` (`/sandbox/mcp.json`) — unchanged, it's the symlink location not the store
- `prepare_staging_dir` in `openshell.rs` — no `/platform/` references
- Templates — no `/platform/` references
- `ARCHITECTURE.md` — update the "Platform store" section to say `/sandbox/.platform/` instead of `/platform/`

## Verification

1. `cargo build --workspace` — compile check
2. `cargo test --workspace` — all tests pass
3. `cargo clippy --workspace` — no warnings
4. Live test: restart the right agent bot, SSH into sandbox, verify:
   - `/sandbox/.platform/` exists with content-addressed files
   - Symlinks in `/sandbox/.claude/` and `/sandbox/mcp.json` point to `/sandbox/.platform/...`
   - `cat /sandbox/mcp.json` works
   - `df /sandbox/.platform/` shows `/dev/vda1` (persistent volume, not overlay)
   - `/platform` directory no longer exists in policy or sandbox
