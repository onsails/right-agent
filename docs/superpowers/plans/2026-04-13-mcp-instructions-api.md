# MCP Instructions API + AGENTS.md Fix — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace file-based MCP_INSTRUCTIONS.md with an internal API endpoint, fix AGENTS.md so Sonnet-class models reliably direct users to `/mcp add` and `/mcp auth`.

**Architecture:** New `POST /mcp-instructions` endpoint on the aggregator's Unix socket API reads from SQLite and returns markdown. Bot fetches instructions in `invoke_cc` before prompt assembly and inlines them. Dead file-based code is removed.

**Tech Stack:** Rust, axum, rusqlite, tokio, serde_json

**Spec:** `docs/superpowers/specs/2026-04-13-mcp-instructions-api-design.md`

---

## Task Dependency Graph

```
Task 1 (server endpoint) → Task 2 (client method) → Task 3 (bot integration)
Task 4 (cleanup: agent_def + pipeline) — independent
Task 5 (cleanup: aggregator + handler) — independent, after Task 3
Task 6 (AGENTS.md) — independent
Task 7 (docs: ARCHITECTURE.md + PROMPT_SYSTEM.md) — after all code tasks
```

Tasks 1→2→3 are sequential (each depends on the previous). Tasks 4, 5, 6 can run after Task 3 in any order. Task 7 is last.

---

### Task 1: Server endpoint — `POST /mcp-instructions`

**Files:**
- Modify: `crates/rightclaw-cli/src/internal_api.rs`

- [ ] **Step 1: Write the failing tests**

Add to the `#[cfg(test)] mod tests` block at the bottom of `crates/rightclaw-cli/src/internal_api.rs`, after the existing `mcp_list_unknown_agent_returns_404` test:

```rust
    #[tokio::test]
    async fn mcp_instructions_returns_header_for_no_servers() {
        let tmp = tempfile::tempdir().unwrap();
        let dispatcher = make_test_dispatcher(tmp.path());
        let app = internal_router(dispatcher);

        let (status, body) = send_json(
            app,
            "/mcp-instructions",
            serde_json::json!({ "agent": "test-agent" }),
        )
        .await;

        assert_eq!(status, StatusCode::OK);
        let instructions = body["instructions"].as_str().unwrap();
        assert_eq!(instructions, "# MCP Server Instructions\n");
    }

    #[tokio::test]
    async fn mcp_instructions_unknown_agent_returns_404() {
        let tmp = tempfile::tempdir().unwrap();
        let dispatcher = make_test_dispatcher(tmp.path());
        let app = internal_router(dispatcher);

        let (status, _body) = send_json(
            app,
            "/mcp-instructions",
            serde_json::json!({ "agent": "nonexistent" }),
        )
        .await;

        assert_eq!(status, StatusCode::NOT_FOUND);
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `devenv shell -- cargo test -p rightclaw-cli internal_api::tests::mcp_instructions -- --nocapture`
Expected: FAIL — 404 because no route registered for `/mcp-instructions`.

- [ ] **Step 3: Add request/response types and handler**

In `crates/rightclaw-cli/src/internal_api.rs`, add the types after `McpListResponse` / `McpServerStatus` (around line 83):

```rust
#[derive(Deserialize)]
pub(crate) struct McpInstructionsRequest {
    pub agent: String,
}

#[derive(Serialize)]
pub(crate) struct McpInstructionsResponse {
    pub instructions: String,
}
```

Add the handler after `handle_mcp_list` (around line 375):

```rust
async fn handle_mcp_instructions(
    State(dispatcher): State<Arc<ToolDispatcher>>,
    Json(req): Json<McpInstructionsRequest>,
) -> axum::response::Response {
    let conn_arc = {
        let Some(registry) = dispatcher.agents.get(&req.agent) else {
            return not_found(format!("agent '{}' not found", req.agent)).into_response();
        };
        match registry.right.get_conn(&req.agent) {
            Ok(c) => c,
            Err(e) => return internal_error(format!("db open: {e:#}")).into_response(),
        }
    };

    let servers = {
        let conn = match conn_arc.lock() {
            Ok(c) => c,
            Err(e) => return internal_error(format!("mutex poisoned: {e}")).into_response(),
        };
        match credentials::db_list_servers(&conn) {
            Ok(s) => s,
            Err(e) => return internal_error(format!("db_list_servers: {e:#}")).into_response(),
        }
    };

    let content = rightclaw::codegen::generate_mcp_instructions_md(&servers);
    Json(McpInstructionsResponse { instructions: content }).into_response()
}
```

Register the route in `internal_router`:

```rust
pub(crate) fn internal_router(dispatcher: Arc<ToolDispatcher>) -> Router {
    Router::new()
        .route("/mcp-add", post(handle_mcp_add))
        .route("/mcp-remove", post(handle_mcp_remove))
        .route("/set-token", post(handle_set_token))
        .route("/mcp-list", post(handle_mcp_list))
        .route("/mcp-instructions", post(handle_mcp_instructions))
        .with_state(dispatcher)
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `devenv shell -- cargo test -p rightclaw-cli internal_api::tests::mcp_instructions -- --nocapture`
Expected: PASS — both tests green.

- [ ] **Step 5: Commit**

```bash
git add crates/rightclaw-cli/src/internal_api.rs
git commit -m "feat(api): add POST /mcp-instructions endpoint"
```

---

### Task 2: Client method — `mcp_instructions()`

**Files:**
- Modify: `crates/rightclaw/src/mcp/internal_client.rs`

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)] mod tests` block at the bottom of `crates/rightclaw/src/mcp/internal_client.rs`:

```rust
    #[test]
    fn mcp_instructions_response_deserializes() {
        let json = r#"{"instructions":"# MCP Server Instructions\n\n## composio\n\nConnect apps.\n"}"#;
        let resp: McpInstructionsResponse = serde_json::from_str(json).unwrap();
        assert!(resp.instructions.contains("composio"));
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `devenv shell -- cargo test -p rightclaw mcp_instructions_response_deserializes`
Expected: FAIL — `McpInstructionsResponse` not defined.

- [ ] **Step 3: Add the response type and client method**

In `crates/rightclaw/src/mcp/internal_client.rs`, add the response type after `SetTokenResponse` (around line 199):

```rust
#[derive(Debug, Deserialize)]
pub struct McpInstructionsResponse {
    pub instructions: String,
}
```

Add the method to `impl InternalClient`, after `mcp_list` (around line 141):

```rust
    /// Fetch MCP server instructions markdown for the given agent.
    pub async fn mcp_instructions(&self, agent: &str) -> Result<McpInstructionsResponse, InternalClientError> {
        self.post("/mcp-instructions", &serde_json::json!({"agent": agent}))
            .await
    }
```

- [ ] **Step 4: Run test to verify it passes**

Run: `devenv shell -- cargo test -p rightclaw mcp_instructions_response_deserializes`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/rightclaw/src/mcp/internal_client.rs
git commit -m "feat(client): add mcp_instructions() to InternalClient"
```

---

### Task 3: Bot integration — fetch instructions in `invoke_cc`

**Files:**
- Modify: `crates/bot/src/telegram/worker.rs`
- Modify: `crates/bot/src/telegram/handler.rs` (WorkerContext construction)

- [ ] **Step 1: Add `internal_client` field to `WorkerContext`**

In `crates/bot/src/telegram/worker.rs`, add after the `idle_timestamp` field (line 84):

```rust
    /// Internal API client for aggregator IPC (Unix socket).
    pub internal_client: Arc<rightclaw::mcp::internal_client::InternalClient>,
```

Add import at top of file if not present:

```rust
use std::sync::Arc;
```

(`Arc` is likely already imported — check before adding.)

- [ ] **Step 2: Set the field in handler.rs where WorkerContext is constructed**

In `crates/bot/src/telegram/handler.rs`, the `WorkerContext` is constructed at line 180. Add the new field. The `InternalApi` newtype wrapping `Arc<InternalClient>` is available as `internal_api` via dptree DI. Add `internal_api: InternalApi` to the handler's parameter extraction (it's already extracted for `/mcp` commands). Then set:

```rust
                let ctx = WorkerContext {
                    chat_id,
                    effective_thread_id: eff_thread_id,
                    agent_dir: agent_dir.0.clone(),
                    agent_name,
                    bot: bot.clone(),
                    db_path: agent_dir.0.clone(),
                    debug: debug_flag.0,
                    ssh_config_path: ssh_config.0.clone(),
                    auth_watcher_active: Arc::clone(&auth_watcher_flag.0),
                    auth_code_tx: Arc::clone(&auth_code_slot.0),
                    max_turns: settings.max_turns,
                    max_budget_usd: settings.max_budget_usd,
                    show_thinking: settings.show_thinking,
                    model: settings.model.clone(),
                    stop_tokens: Arc::clone(&stop_tokens),
                    idle_timestamp: Arc::clone(&idle_ts.0),
                    internal_client: Arc::clone(&internal_api.0),
                };
```

Check that `internal_api` is available in the handler function's DI parameters. It is already injected at `dispatch.rs:99` as `Arc<InternalApi>`. If the handler function parameter list doesn't include `internal_api: Arc<InternalApi>`, add it. Look at the dptree handler parameter extraction — follow the existing pattern for how other DI values are accessed.

- [ ] **Step 3: Run `cargo check` to verify the struct compiles**

Run: `devenv shell -- cargo check -p rightclaw-bot`
Expected: PASS (may have warnings about unused field, that's fine)

- [ ] **Step 4: Add `mcp_instructions` parameter to `build_sandbox_prompt_assembly_script`**

In `crates/bot/src/telegram/worker.rs`, modify the function signature at line 547:

```rust
fn build_sandbox_prompt_assembly_script(
    base_prompt: &str,
    bootstrap_mode: bool,
    claude_args: &[String],
    mcp_instructions: Option<&str>,
) -> String {
```

In the normal-mode branch (the `else` block starting around line 563), add after the TOOLS.md section (after line 589):

```rust
        r#"
if [ -f /sandbox/IDENTITY.md ]; then
  printf '\n## Your Identity\n'
  cat /sandbox/IDENTITY.md
  printf '\n'
fi
if [ -f /sandbox/SOUL.md ]; then
  printf '\n## Your Personality and Values\n'
  cat /sandbox/SOUL.md
  printf '\n'
fi
if [ -f /sandbox/USER.md ]; then
  printf '\n## Your User\n'
  cat /sandbox/USER.md
  printf '\n'
fi
if [ -f /sandbox/.claude/agents/AGENTS.md ]; then
  printf '\n## Operating Instructions\n'
  cat /sandbox/.claude/agents/AGENTS.md
  printf '\n'
fi
if [ -f /sandbox/.claude/agents/TOOLS.md ]; then
  printf '\n## Environment and Tools\n'
  cat /sandbox/.claude/agents/TOOLS.md
  printf '\n'
fi"#
```

This is the existing content. **After** the closing `"#` of the normal-mode branch, before the closing `};`, add the MCP instructions injection. The cleanest approach: instead of modifying the `file_sections` string, append MCP instructions separately in the final format string.

Actually, the simpler approach — add MCP instructions as a separate shell `printf` block in the final `format!` call. Change the format string at line 592:

Current:
```rust
    format!(
        "{{ printf '{escaped_base}'\n{file_sections}\n}} > /tmp/rightclaw-system-prompt.md\ncd /sandbox && {claude_cmd} --system-prompt-file /tmp/rightclaw-system-prompt.md"
    )
```

New:
```rust
    let mcp_section = match mcp_instructions {
        Some(instr) => {
            let escaped = instr.replace('\'', "'\\''");
            format!("\nprintf '\\n{escaped}\\n'")
        }
        None => String::new(),
    };

    format!(
        "{{ printf '{escaped_base}'\n{file_sections}{mcp_section}\n}} > /tmp/rightclaw-system-prompt.md\ncd /sandbox && {claude_cmd} --system-prompt-file /tmp/rightclaw-system-prompt.md"
    )
```

- [ ] **Step 5: Add `mcp_instructions` parameter to `assemble_host_system_prompt`**

In `crates/bot/src/telegram/worker.rs`, modify the function signature at line 600:

```rust
fn assemble_host_system_prompt(
    base_prompt: &str,
    bootstrap_mode: bool,
    agent_dir: &Path,
    mcp_instructions: Option<&str>,
) -> String {
```

At the end of the function, before `prompt` is returned (just before the final `prompt` on line ~641), add:

```rust
    if let Some(instr) = mcp_instructions {
        prompt.push('\n');
        prompt.push_str(instr);
        prompt.push('\n');
    }

    prompt
```

- [ ] **Step 6: Fetch instructions in `invoke_cc` and pass to assembly functions**

In `crates/bot/src/telegram/worker.rs`, in `invoke_cc` (around line 872, just before the `base_prompt` generation), add:

```rust
    // Fetch MCP server instructions from aggregator (non-fatal on error).
    let mcp_instructions: Option<String> = match ctx.internal_client.mcp_instructions(&ctx.agent_name).await {
        Ok(resp) => {
            // Only include if there's actual content beyond the header
            if resp.instructions.trim().len() > "# MCP Server Instructions".len() {
                Some(resp.instructions)
            } else {
                None
            }
        }
        Err(e) => {
            tracing::warn!("failed to fetch MCP instructions from aggregator: {e:#}");
            None
        }
    };
```

Then update the two call sites. In the sandbox branch (around line 888):

```rust
        let assembly_script =
            build_sandbox_prompt_assembly_script(&base_prompt, bootstrap_mode, &claude_args, mcp_instructions.as_deref());
```

In the host branch (around line 897):

```rust
        let composite = assemble_host_system_prompt(
            &base_prompt,
            bootstrap_mode,
            &ctx.agent_dir,
            mcp_instructions.as_deref(),
        );
```

- [ ] **Step 7: Fix existing test call sites**

Update all existing test calls to `build_sandbox_prompt_assembly_script` and `assemble_host_system_prompt` to pass `None` as the new parameter.

In `crates/bot/src/telegram/worker.rs` tests section, update each call:

```rust
    // build_sandbox_prompt_assembly_script calls — add None as 4th arg:
    let script = build_sandbox_prompt_assembly_script("Base prompt", true, &["claude".into(), "-p".into()], None);
    let script = build_sandbox_prompt_assembly_script("Base prompt", false, &["claude".into(), "-p".into()], None);
    let script = build_sandbox_prompt_assembly_script("It's a test", true, &["claude".into()], None);
    let script = build_sandbox_prompt_assembly_script("Base", false, &["claude".into(), "-p".into(), "--json-schema".into(), r#"{"type":"object"}"#.into()], None);
    let script = build_sandbox_prompt_assembly_script("X", false, &["claude".into()], None);

    // assemble_host_system_prompt calls — add None as 4th arg:
    let result = assemble_host_system_prompt("Base\n", true, dir.path(), None);
    let result = assemble_host_system_prompt("Base\n", false, dir.path(), None);
    let result = assemble_host_system_prompt("Base\n", false, dir.path(), None);
    let result = assemble_host_system_prompt("Base\n", true, dir.path(), None);
```

- [ ] **Step 8: Add tests for MCP instructions in prompt assembly**

Add after the existing `host_prompt_bootstrap_skips_missing_bootstrap` test:

```rust
    #[test]
    fn sandbox_script_includes_mcp_instructions() {
        let script = build_sandbox_prompt_assembly_script(
            "Base",
            false,
            &["claude".into()],
            Some("# MCP Server Instructions\n\n## composio\n\nConnect with 250+ apps.\n"),
        );
        assert!(script.contains("MCP Server Instructions"));
        assert!(script.contains("composio"));
    }

    #[test]
    fn sandbox_script_none_mcp_instructions_omitted() {
        let script = build_sandbox_prompt_assembly_script(
            "Base",
            false,
            &["claude".into()],
            None,
        );
        assert!(!script.contains("MCP Server Instructions"));
    }

    #[test]
    fn host_prompt_includes_mcp_instructions() {
        let dir = tempfile::tempdir().unwrap();
        let agents_dir = dir.path().join(".claude").join("agents");
        std::fs::create_dir_all(&agents_dir).unwrap();
        std::fs::write(agents_dir.join("AGENTS.md"), "Procedures").unwrap();

        let result = assemble_host_system_prompt(
            "Base\n",
            false,
            dir.path(),
            Some("# MCP Server Instructions\n\n## notion\n\nNotion tools.\n"),
        );
        assert!(result.contains("MCP Server Instructions"));
        assert!(result.contains("notion"));
        assert!(result.contains("Notion tools."));
    }

    #[test]
    fn host_prompt_none_mcp_instructions_omitted() {
        let dir = tempfile::tempdir().unwrap();
        let result = assemble_host_system_prompt("Base\n", false, dir.path(), None);
        assert!(!result.contains("MCP Server Instructions"));
    }
```

- [ ] **Step 9: Run all bot tests**

Run: `devenv shell -- cargo test -p rightclaw-bot -- --nocapture`
Expected: PASS

- [ ] **Step 10: Commit**

```bash
git add crates/bot/src/telegram/worker.rs crates/bot/src/telegram/handler.rs
git commit -m "feat(bot): fetch MCP instructions from aggregator at prompt assembly time"
```

---

### Task 4: Cleanup — remove MCP_INSTRUCTIONS.md from agent_def + pipeline

**Files:**
- Modify: `crates/rightclaw/src/codegen/agent_def.rs`
- Modify: `crates/rightclaw/src/codegen/agent_def_tests.rs`
- Modify: `crates/rightclaw/src/codegen/pipeline.rs`

- [ ] **Step 1: Remove from `CONTENT_MD_FILES`**

In `crates/rightclaw/src/codegen/agent_def.rs`, change the array at line 6:

```rust
pub const CONTENT_MD_FILES: &[&str] = &[
    "BOOTSTRAP.md",
    "AGENTS.md",
    "TOOLS.md",
    "IDENTITY.md",
    "SOUL.md",
    "USER.md",
    "MEMORY.md",
];
```

- [ ] **Step 2: Remove `@./MCP_INSTRUCTIONS.md` from agent definition**

In `crates/rightclaw/src/codegen/agent_def.rs`, update `generate_agent_definition` (line 40). Remove the last `---` separator and `@./MCP_INSTRUCTIONS.md` reference. The function should end with:

```rust
pub fn generate_agent_definition(name: &str, model: Option<&str>) -> String {
    let model = model.unwrap_or("inherit");
    format!(
        "\
---
name: {name}
model: {model}
description: \"RightClaw agent: {name}\"
---

@./AGENTS.md

---

@./SOUL.md

---

@./IDENTITY.md

---

@./USER.md

---

@./TOOLS.md
"
    )
}
```

- [ ] **Step 3: Update `agent_def_tests.rs`**

In `crates/rightclaw/src/codegen/agent_def_tests.rs`:

Remove lines 16 and 22 from `agent_definition_has_at_references_in_cache_order`:

```rust
    let mcp_instr_pos = result.find("@./MCP_INSTRUCTIONS.md").expect("missing @./MCP_INSTRUCTIONS.md");
```
and
```rust
    assert!(tools_pos < mcp_instr_pos, "TOOLS must come before MCP_INSTRUCTIONS");
```

The test should become:

```rust
#[test]
fn agent_definition_has_at_references_in_cache_order() {
    let result = generate_agent_definition("myagent", Some("sonnet"));
    assert!(result.contains("name: myagent"));
    assert!(result.contains("model: sonnet"));
    assert!(result.contains("description: \"RightClaw agent: myagent\""));

    // Verify order: AGENTS → SOUL → IDENTITY → USER → TOOLS
    let agents_pos = result.find("@./AGENTS.md").expect("missing @./AGENTS.md");
    let soul_pos = result.find("@./SOUL.md").expect("missing @./SOUL.md");
    let identity_pos = result.find("@./IDENTITY.md").expect("missing @./IDENTITY.md");
    let user_pos = result.find("@./USER.md").expect("missing @./USER.md");
    let tools_pos = result.find("@./TOOLS.md").expect("missing @./TOOLS.md");

    assert!(agents_pos < soul_pos, "AGENTS must come before SOUL");
    assert!(soul_pos < identity_pos, "SOUL must come before IDENTITY");
    assert!(identity_pos < user_pos, "IDENTITY must come before USER");
    assert!(user_pos < tools_pos, "USER must come before TOOLS");
}
```

Also verify the test `agent_definition_no_embedded_file_content` doesn't reference MCP_INSTRUCTIONS — it doesn't, so no change needed.

- [ ] **Step 4: Remove MCP_INSTRUCTIONS.md create-if-missing from pipeline**

In `crates/rightclaw/src/codegen/pipeline.rs`, remove lines 283–293:

```rust
    // Create MCP_INSTRUCTIONS.md if missing (agent-owned, never overwritten by codegen).
    let mcp_instr_path = agent.path.join("MCP_INSTRUCTIONS.md");
    if !mcp_instr_path.exists() {
        std::fs::write(&mcp_instr_path, "# MCP Server Instructions\n").map_err(|e| {
            miette::miette!(
                "failed to create MCP_INSTRUCTIONS.md for '{}': {e:#}",
                agent.name
            )
        })?;
        tracing::debug!(agent = %agent.name, "created empty MCP_INSTRUCTIONS.md");
    }
```

- [ ] **Step 5: Remove related tests from pipeline**

In `crates/rightclaw/src/codegen/pipeline.rs`, remove these four tests:

1. `mcp_instructions_md_created_if_missing` (lines 591–611)
2. `mcp_instructions_md_not_overwritten_if_exists` (lines 613–634)
3. `mcp_instructions_in_content_md_files` (lines 636–641)
4. `agent_def_includes_mcp_instructions_ref` (lines 644–651)

- [ ] **Step 6: Run tests**

Run: `devenv shell -- cargo test -p rightclaw -- --nocapture`
Expected: PASS

- [ ] **Step 7: Commit**

```bash
git add crates/rightclaw/src/codegen/agent_def.rs crates/rightclaw/src/codegen/agent_def_tests.rs crates/rightclaw/src/codegen/pipeline.rs
git commit -m "cleanup: remove MCP_INSTRUCTIONS.md from agent_def and pipeline"
```

---

### Task 5: Cleanup — remove `regenerate_mcp_instructions` + `sync_mcp_instructions`

**Files:**
- Modify: `crates/rightclaw-cli/src/aggregator.rs`
- Modify: `crates/bot/src/telegram/handler.rs`

- [ ] **Step 1: Remove `regenerate_mcp_instructions` from aggregator**

In `crates/rightclaw-cli/src/aggregator.rs`, remove the entire method (lines 162–175):

```rust
    /// Regenerate MCP_INSTRUCTIONS.md from SQLite-cached instructions.
    pub(crate) fn regenerate_mcp_instructions(&self) -> Result<(), anyhow::Error> {
        let conn = rightclaw::memory::open_connection(&self.agent_dir)?;
        let servers = rightclaw::mcp::credentials::db_list_servers(&conn)?;
        let content = rightclaw::codegen::generate_mcp_instructions_md(&servers);
        std::fs::write(self.agent_dir.join("MCP_INSTRUCTIONS.md"), &content)?;
        // Also copy to .claude/agents/ for @ ref resolution
        let agents_dir = self.agent_dir.join(".claude/agents");
        if agents_dir.exists() {
            std::fs::write(agents_dir.join("MCP_INSTRUCTIONS.md"), &content)?;
        }
        tracing::debug!(agent_dir = %self.agent_dir.display(), "regenerated MCP_INSTRUCTIONS.md");
        Ok(())
    }
```

- [ ] **Step 2: Remove `sync_mcp_instructions` from handler**

In `crates/bot/src/telegram/handler.rs`, remove the function (lines 693–707):

```rust
/// Sync MCP_INSTRUCTIONS.md to .claude/agents/ for @ ref resolution.
///
/// The periodic background sync handles sandbox upload, so we only do the
/// local copy here to avoid needing sandbox name / SSH config in these handlers.
fn sync_mcp_instructions(agent_dir: &Path) -> Result<(), std::io::Error> {
    let src = agent_dir.join("MCP_INSTRUCTIONS.md");
    if !src.exists() {
        return Ok(());
    }
    let agents_subdir = agent_dir.join(".claude/agents");
    if agents_subdir.exists() {
        std::fs::copy(&src, agents_subdir.join("MCP_INSTRUCTIONS.md"))?;
    }
    Ok(())
}
```

- [ ] **Step 3: Remove `sync_mcp_instructions` call from `handle_mcp_add`**

In `crates/bot/src/telegram/handler.rs`, in `handle_mcp_add` (around line 747–750), remove:

```rust
            // Non-fatal: background sync will catch up if local copy fails.
            if let Err(e) = sync_mcp_instructions(agent_dir) {
                tracing::warn!("sync MCP_INSTRUCTIONS.md failed: {e:#}");
            }
```

- [ ] **Step 4: Remove `sync_mcp_instructions` call from `handle_mcp_remove`**

In `crates/bot/src/telegram/handler.rs`, in `handle_mcp_remove` (around line 795–798), remove:

```rust
            // Non-fatal: background sync will catch up if local copy fails.
            if let Err(e) = sync_mcp_instructions(agent_dir) {
                tracing::warn!("sync MCP_INSTRUCTIONS.md failed: {e:#}");
            }
```

- [ ] **Step 5: Run tests**

Run: `devenv shell -- cargo test -p rightclaw-bot -- --nocapture && cargo test -p rightclaw-cli -- --nocapture`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add crates/rightclaw-cli/src/aggregator.rs crates/bot/src/telegram/handler.rs
git commit -m "cleanup: remove file-based MCP instructions sync"
```

---

### Task 6: Rewrite AGENTS.md MCP Management section

**Files:**
- Modify: `templates/right/AGENTS.md`

- [ ] **Step 1: Replace MCP Management section**

In `templates/right/AGENTS.md`, replace lines 32–43 (from `## MCP Management` through `via MCP_INSTRUCTIONS.md.`):

Old:
```markdown
## MCP Management

MCP servers are managed by the user via Telegram commands — agents cannot add or remove servers directly (security: prevents sandbox escape via arbitrary URL registration).

- `/mcp add <name> <url>` — register an external MCP server
- `/mcp remove <name>` — unregister a server (`right` is protected)
- `/mcp auth <name>` — start OAuth flow for a server
- `/mcp list` — show all servers with status

To check registered servers from code, use the `mcp_list()` tool.

Usage instructions from connected servers are automatically included in your context
via MCP_INSTRUCTIONS.md.
```

New:
```markdown
## MCP Management

You CANNOT add, remove, or authenticate MCP servers yourself.
The user manages them via Telegram commands:

- `/mcp add <name> <url>` — register an external MCP server
- `/mcp remove <name>` — unregister a server (`right` is protected)
- `/mcp auth <name>` — start OAuth flow (for servers requiring authentication)
- `/mcp list` — show all servers with status

**When the user asks to connect an MCP server:**
1. Help them find the correct MCP URL (search docs if needed)
2. Tell them to run: `/mcp add <name> <url>`
3. If the server requires OAuth, tell them to also run: `/mcp auth <name>`
4. NEVER ask the user for API keys or tokens directly — `/mcp auth` handles authentication

To check registered servers from code, use the `mcp_list()` tool.
```

- [ ] **Step 2: Commit**

```bash
git add templates/right/AGENTS.md
git commit -m "fix(agents): rewrite MCP section with actionable instructions for /mcp add"
```

---

### Task 7: Update documentation — ARCHITECTURE.md + PROMPT_SYSTEM.md

**Files:**
- Modify: `ARCHITECTURE.md`
- Modify: `PROMPT_SYSTEM.md`

- [ ] **Step 1: Update module map in ARCHITECTURE.md**

In `ARCHITECTURE.md`, change line 32:

Old:
```
│   ├── mcp_instructions.rs  # Generate MCP_INSTRUCTIONS.md from SQLite mcp_servers cache
```

New:
```
│   ├── mcp_instructions.rs  # Generate MCP instructions markdown from SQLite mcp_servers cache
```

- [ ] **Step 2: Update internal_api.rs description**

In `ARCHITECTURE.md`, change line 64:

Old:
```
├── internal_api.rs       # Internal REST API on Unix socket (mcp-add, mcp-remove, set-token)
```

New:
```
├── internal_api.rs       # Internal REST API on Unix socket (mcp-add, mcp-remove, set-token, mcp-instructions)
```

- [ ] **Step 3: Update Internal REST API description**

In `ARCHITECTURE.md`, after line 241 (`- POST /set-token — deliver OAuth tokens after authentication`), add:

```
  - POST /mcp-list — list MCP servers with status
  - POST /mcp-instructions — fetch MCP server instructions markdown
```

- [ ] **Step 4: Remove MCP_INSTRUCTIONS.md from Configuration Hierarchy table**

In `ARCHITECTURE.md`, delete line 281:

```
| Generated | `agents/<name>/MCP_INSTRUCTIONS.md` | Generated by Aggregator from SQLite mcp_servers cache |
```

- [ ] **Step 5: Remove MCP_INSTRUCTIONS.md from Directory Layout**

In `ARCHITECTURE.md`, delete line 362:

```
│   ├── MCP_INSTRUCTIONS.md            # generated by Aggregator from SQLite mcp_servers cache
```

- [ ] **Step 6: Update PROMPT_SYSTEM.md — add MCP instructions section to prompt structure**

In `PROMPT_SYSTEM.md`, update the normal mode prompt structure (around line 40). After the `{TOOLS.md}` line, add:

```
## MCP Server Instructions  (if any external MCP servers have instructions)
{fetched from aggregator via POST /mcp-instructions at prompt assembly time}
```

So the full structure becomes:

```
[Base: RightClaw agent description, sandbox info, MCP reference]

## Your Identity
{IDENTITY.md — name, creature, vibe, emoji, principles}

## Your Personality and Values
{SOUL.md — core values, communication style, boundaries}

## Your User
{USER.md — user name, timezone, preferences}

## Operating Instructions
{AGENTS.md — procedures, session routine}

## Environment and Tools
{TOOLS.md — sandbox paths, inbox/outbox, MCP notes}

## MCP Server Instructions  (if any external MCP servers have instructions)
{fetched from aggregator via POST /mcp-instructions at prompt assembly time}
```

Also update line 59 to say:

```
Missing files are silently skipped. MCP instructions are fetched from the aggregator's
internal API (non-fatal if unavailable).
```

- [ ] **Step 7: Update the MCP Server Instructions section in PROMPT_SYSTEM.md**

In `PROMPT_SYSTEM.md`, the section at lines 136–143 talks about `with_instructions()`. This section is about the `right` MCP server's tool instructions, NOT about upstream MCP server instructions. It's still correct. But add a new subsection after it:

```markdown
## Upstream MCP Server Instructions

When external MCP servers are registered (via `/mcp add`), their usage instructions are
fetched from the aggregator's internal API (`POST /mcp-instructions`) at prompt assembly
time and inlined into the composite system prompt. This replaces the previous file-based
approach (MCP_INSTRUCTIONS.md).

Instructions are persisted in SQLite (`mcp_servers.instructions` column) by ProxyBackend
on each `connect()`. The endpoint reads from SQLite via `db_list_servers()` and generates
markdown via `generate_mcp_instructions_md()`.
```

- [ ] **Step 8: Build workspace to verify no breakage**

Run: `devenv shell -- cargo build --workspace`
Expected: PASS (warnings OK)

- [ ] **Step 9: Commit**

```bash
git add ARCHITECTURE.md PROMPT_SYSTEM.md
git commit -m "docs: update MCP instructions delivery in ARCHITECTURE.md and PROMPT_SYSTEM.md"
```

---

## Self-Review Checklist

**Spec coverage:**
- ✅ `POST /mcp-instructions` endpoint (Task 1)
- ✅ `InternalClient::mcp_instructions()` (Task 2)
- ✅ Bot fetches + inlines in system prompt (Task 3)
- ✅ WorkerContext gets `internal_client` field (Task 3)
- ✅ Non-fatal error handling for fetch (Task 3, Step 6)
- ✅ Remove `MCP_INSTRUCTIONS.md` from CONTENT_MD_FILES (Task 4)
- ✅ Remove `@./MCP_INSTRUCTIONS.md` from agent def (Task 4)
- ✅ Remove create-if-missing in pipeline (Task 4)
- ✅ Remove `regenerate_mcp_instructions()` (Task 5)
- ✅ Remove `sync_mcp_instructions()` and calls (Task 5)
- ✅ Remove 4 pipeline tests + 1 agent_def_tests assertion (Tasks 4)
- ✅ Keep `generate_mcp_instructions_md()` (reused by endpoint)
- ✅ Keep `db_update_instructions()` (ProxyBackend still writes)
- ✅ AGENTS.md rewrite (Task 6)
- ✅ ARCHITECTURE.md updates (Task 7)
- ✅ PROMPT_SYSTEM.md updates (Task 7)

**Placeholder scan:** None found.

**Type consistency:** `McpInstructionsResponse` used consistently in Tasks 1 and 2. `mcp_instructions` field name consistent across endpoint, client, and worker.
