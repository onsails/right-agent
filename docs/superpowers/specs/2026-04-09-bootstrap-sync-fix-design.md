# Bootstrap Sync Fix

**Status:** Design

**Problem:** Bootstrap doesn't work in OpenShell sandbox mode. The agent definition `right-bootstrap.md` contains `@./BOOTSTRAP.md`, but `BOOTSTRAP.md` is never uploaded to the sandbox. CC resolves the `@` reference to `/sandbox/BOOTSTRAP.md` which doesn't exist, gets an effectively empty agent definition, responds as a generic assistant, and immediately signals `bootstrap_complete: true`. The same gap affects `AGENTS.md` and `TOOLS.md` — none of the content `.md` files that agent definitions reference via `@` are synced to the sandbox.

**Root cause:** The spec for the bootstrap rework (2026-04-09) only added `.claude/agents/` (the agent definition files) to `sync_cycle`, but missed the content files those definitions reference. The plan faithfully implemented the spec's gap.

**Secondary issues:**
- `AGENTS.md` and `TOOLS.md` are not in `REVERSE_SYNC_FILES`, so agent edits to them don't persist to host.
- Bot startup logs don't show agent configuration (sandbox mode, model, etc.), making debugging harder.
- No integration test covers the actual sandbox upload/download path.

## Design

### 1. Forward-sync content `.md` files on `initial_sync` only

All content `.md` files live at the agent root on the host. They need to reach `/sandbox/` inside the container for CC's `@` references to resolve.

**Files to forward-sync:**

| File | Present after init | Present after bootstrap | Editable by agent |
|------|-------------------|------------------------|-------------------|
| `BOOTSTRAP.md` | Yes | No (deleted) | Read-only |
| `AGENTS.md` | Yes | Yes | Yes (skills, subagents, routing) |
| `TOOLS.md` | Yes (codegen) | Yes | Yes |
| `IDENTITY.md` | No | Yes (created by bootstrap) | Yes |
| `SOUL.md` | No | Yes (created by bootstrap) | Yes |
| `USER.md` | No | Yes (created by bootstrap) | Yes |
| `MEMORY.md` | No | Maybe (CC auto-manages) | Yes |

**When to upload:** Only during `initial_sync` (bot startup, before teloxide starts). NOT during the periodic 5-minute `sync_cycle`.

**Why not periodic:** All these files can be edited by the agent inside the sandbox. The sandbox is the source of truth after startup. Periodic forward-sync would overwrite agent edits with stale host copies. The host copy is only authoritative at startup (either fresh from init, or restored from a previous reverse sync).

**Implementation:** Define a `CONTENT_MD_FILES` const with the list. In `initial_sync`, after calling `sync_cycle`, iterate the list and upload each file that exists on host. Skip files that don't exist (e.g., IDENTITY.md before bootstrap).

```rust
const CONTENT_MD_FILES: &[&str] = &[
    "BOOTSTRAP.md",
    "AGENTS.md",
    "TOOLS.md",
    "IDENTITY.md",
    "SOUL.md",
    "USER.md",
    "MEMORY.md",
];
```

### 2. Add AGENTS.md and TOOLS.md to reverse sync

Current `REVERSE_SYNC_FILES`: `IDENTITY.md, SOUL.md, USER.md, MEMORY.md, BOOTSTRAP.md`

Add: `AGENTS.md, TOOLS.md`

This ensures agent edits to these files flow back to host after every `claude -p` invocation. Without this, if the sandbox is recreated, any agent modifications to AGENTS.md (added skills, subagent definitions, routing rules) or TOOLS.md would be lost.

The reverse sync algorithm (download, compare, atomic write, delete-if-absent) already handles all edge cases correctly. Adding files to the list is the only change needed.

### 3. Bot startup logging

After config is parsed in `crates/bot/src/lib.rs` (around line 88), add an INFO-level log line with agent configuration parameters:

```
agent=right sandbox_mode=openshell model=sonnet restart=on_failure network_policy=restrictive bootstrap_pending=true
```

Fields:
- `agent`: agent name
- `sandbox_mode`: openshell or none
- `model`: model override or "inherit"
- `restart`: restart policy
- `network_policy`: restrictive or permissive
- `bootstrap_pending`: whether BOOTSTRAP.md exists (pre-onboarding state)

### 4. Integration test (real OpenShell sandbox)

A `#[tokio::test]` marked `#[ignore = "requires live OpenShell sandbox"]` in `crates/bot/src/sync.rs` (or a dedicated test module).

**Test flow:**

1. Create a tempdir simulating an agent directory with:
   - `BOOTSTRAP.md` (with known content)
   - `AGENTS.md` (with known content)
   - `TOOLS.md` (with known content)
   - `.claude/settings.json` (minimal valid JSON)
   - `.claude/agents/test.md` (minimal agent def)
   - `.claude/bootstrap-schema.json` (from const)
   - `.claude/reply-schema.json` (from const)
   - `mcp.json` (minimal valid JSON)
2. Call `initial_sync(&agent_dir, "rightclaw-right").await`
3. For each content `.md` file, call `openshell::download_file("rightclaw-right", "/sandbox/{file}", &tmp_download_dir).await`
4. Assert downloaded content matches what was uploaded

**Requires:** A running OpenShell gateway with an existing `rightclaw-right` sandbox. Same prerequisite as the existing `test_policy_validates_against_openshell` test.

**What this catches:** The exact bug we hit — content files not reaching the sandbox. If someone removes a file from the upload list, this test fails.

## Files Changed

| File | Change |
|------|--------|
| `crates/bot/src/sync.rs` | Add `CONTENT_MD_FILES` const, upload them in `initial_sync`. Add `AGENTS.md`, `TOOLS.md` to `REVERSE_SYNC_FILES`. Integration test. |
| `crates/bot/src/lib.rs` | Add agent config INFO log after config parse |

## Non-goals

- Conflict resolution between host and sandbox versions (host only uploads on startup; reverse sync only runs after CC invocation — no race possible in normal operation)
- Changing the periodic `sync_cycle` (infrastructure-only sync remains correct)
- Modifying the staging dir preparation (staging is for sandbox creation, not relevant to this fix)
