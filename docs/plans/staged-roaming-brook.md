# Bootstrap completion via MCP tool + file-presence detection

## Context

Bootstrap doesn't work: the agent responds with a generic greeting and immediately sets `bootstrap_complete: true` without running the onboarding flow. Root causes:

1. **`--json-schema` forces `bootstrap_complete` as required field** — model sets `true` to satisfy schema constraints, ignoring bootstrap instructions
2. **`structured_output.content` is sometimes `null`** while text is in a separate text block — bootstrap conversation text gets lost
3. **Reverse sync is fire-and-forget** — `should_accept_bootstrap` checks host files before they're synced from sandbox

Fix: replace `bootstrap_complete` JSON schema field with MCP tool signal + file-presence detection (IronClaw pattern).

## Files to modify

| File | Change |
|------|--------|
| `crates/rightclaw-cli/src/memory_server.rs` | Add `bootstrap_done` MCP tool |
| `crates/rightclaw-cli/src/memory_server_http.rs` | Add `bootstrap_done` to HTTP handler (same tool, HTTP transport) |
| `crates/bot/src/telegram/worker.rs` | Remove `bootstrap_complete` from schema/ReplyOutput, make reverse_sync blocking in bootstrap mode, move completion check after sync |
| `crates/rightclaw/src/codegen/agent_def.rs` | Remove `BOOTSTRAP_SCHEMA_JSON` constant |
| `templates/right/BOOTSTRAP.md` | Replace `bootstrap_complete` instructions with `bootstrap_done` MCP tool call |

## Implementation

### Task 1: Add `bootstrap_done` MCP tool to memory_server.rs

Add to `memory_server.rs` (same `#[tool_router] impl MemoryServer` block):

```rust
#[tool(description = "Signal that bootstrap onboarding is complete. Call this AFTER you have created IDENTITY.md, SOUL.md, and USER.md. The system will verify the files exist.")]
async fn bootstrap_done(&self) -> Result<CallToolResult, McpError> {
    let required = ["IDENTITY.md", "SOUL.md", "USER.md"];
    let missing: Vec<&str> = required
        .iter()
        .filter(|f| !self.agent_dir.join(f).exists())
        .copied()
        .collect();

    if missing.is_empty() {
        // Delete BOOTSTRAP.md to signal completion to the bot
        let bootstrap_path = self.agent_dir.join("BOOTSTRAP.md");
        if bootstrap_path.exists() {
            std::fs::remove_file(&bootstrap_path).ok();
        }
        Ok(CallToolResult::success(vec![Content::text(
            "Bootstrap complete! IDENTITY.md, SOUL.md, and USER.md verified. \
             Your identity files are now active."
        )]))
    } else {
        Ok(CallToolResult::error(vec![Content::text(format!(
            "Cannot complete bootstrap — missing files: {}. \
             Create them first, then call bootstrap_done again.",
            missing.join(", ")
        ))]))
    }
}
```

**Note on sandbox mode**: Files are in `/sandbox/` inside the container, but the MCP server checks `agent_dir` on the host. After reverse_sync pulls files to host, the check works. During the CC session the files may not be on host yet — the tool may return error. The agent can retry, or the worker catches completion after sync (see Task 3).

Mirror same tool in `memory_server_http.rs` HTTP handler.

### Task 2: Update BOOTSTRAP.md template

Replace the "CRITICAL: bootstrap_complete Field" section and "When You're Done" section with:

```markdown
## When You're Done

After writing IDENTITY.md, SOUL.md, and USER.md with your tools, call the `bootstrap_done` tool to signal completion:

```
mcp__rightclaw__bootstrap_done()
```

The system verifies that all three files exist. If any are missing, you'll get an error — create them and try again.

Do NOT call `bootstrap_done` before creating all three files.
```

Remove the CRITICAL section entirely — no more `bootstrap_complete` field.

### Task 3: Refactor worker.rs bootstrap flow

**3a. Remove `bootstrap_complete` from ReplyOutput:**

```rust
pub struct ReplyOutput {
    pub content: Option<String>,
    pub reply_to_message_id: Option<i32>,
    pub attachments: Option<Vec<super::attachments::OutboundAttachment>>,
    // bootstrap_complete field REMOVED
}
```

**3b. Remove bootstrap schema selection (lines ~659-677):**

Always use `reply-schema.json` — no more `bootstrap-schema.json`:

```rust
// Before:
let schema_filename = if bootstrap_mode { "bootstrap-schema.json" } else { "reply-schema.json" };

// After:
let schema_filename = "reply-schema.json";
```

**3c. Move bootstrap completion to spawn_worker, after reverse_sync:**

Current flow (broken):
```
invoke_cc → spawn(reverse_sync) → check bootstrap_complete → send reply
                ↑ fire-and-forget    ↑ files not synced yet!
```

New flow:
```
invoke_cc → reverse_sync.await (blocking in bootstrap) → check files → send reply
```

In `spawn_worker` (around line 362-375), change reverse_sync from fire-and-forget to blocking when in bootstrap mode:

```rust
// After invoke_cc returns:
if ctx.ssh_config_path.is_some() {
    let sandbox = rightclaw::openshell::sandbox_name(&ctx.agent_name);
    if bootstrap_mode {
        // Bootstrap: BLOCK on reverse_sync so files are available for completion check
        if let Err(e) = crate::sync::reverse_sync_md(&ctx.agent_dir, &sandbox).await {
            tracing::warn!(agent = %ctx.agent_name, "bootstrap reverse sync failed: {e:#}");
        }
    } else {
        // Normal: fire-and-forget (existing behavior)
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

**3d. Replace bootstrap_complete check with file-presence check:**

Remove the entire `if bootstrap_mode && reply_output.bootstrap_complete == Some(true)` block (lines 823-870).

Add bootstrap completion check AFTER reverse_sync, BEFORE sending reply:

```rust
// After reverse_sync completes (for bootstrap mode):
if bootstrap_mode && should_accept_bootstrap(&ctx.agent_dir) {
    tracing::info!(?chat_id, "bootstrap complete — identity files present after sync");
    delete_session(&conn, chat_id, eff_thread_id)
        .map_err(|e| tracing::error!(?chat_id, "delete_session after bootstrap: {:#}", e))
        .ok();
    // BOOTSTRAP.md may already be deleted by MCP tool; ensure cleanup
    let bp = ctx.agent_dir.join("BOOTSTRAP.md");
    if bp.exists() {
        std::fs::remove_file(&bp).ok();
    }
}
```

This is simpler: no parsing bootstrap_complete, no dual-path logic. Just check files after sync.

### Task 4: Remove BOOTSTRAP_SCHEMA_JSON

In `crates/rightclaw/src/codegen/agent_def.rs`:
- Remove the `BOOTSTRAP_SCHEMA_JSON` constant
- Remove `bootstrap-schema.json` from any codegen pipeline writes
- Update exports in `codegen/mod.rs` if re-exported

### Task 5: Update tests

**worker.rs tests to remove:**
- `parse_reply_output_bootstrap_complete_true`
- `parse_reply_output_bootstrap_complete_false`
- `parse_reply_output_no_bootstrap_field`

**worker.rs tests to keep:**
- `should_accept_bootstrap_*` (3 tests — still needed for file-presence check)

**New test for MCP tool:**
- Test `bootstrap_done` returns error when files missing
- Test `bootstrap_done` returns success and deletes BOOTSTRAP.md when files present

## Verification

1. `cargo build --workspace` — compile
2. `cargo test --workspace` — all tests pass
3. Manual test:
   - `rightclaw init` → `rightclaw up`
   - Send "hi" to Telegram bot
   - Agent should start bootstrap conversation (not generic greeting)
   - Agent creates IDENTITY.md, SOUL.md, USER.md, calls bootstrap_done
   - Next message uses normal agent mode
