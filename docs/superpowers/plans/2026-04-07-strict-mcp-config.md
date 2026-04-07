# Strict MCP Config Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Block cloud MCP servers and guarantee rightmemory availability in sandbox by adding `--mcp-config` + `--strict-mcp-config` to CC invocations and syncing `.mcp.json` to sandbox.

**Architecture:** Two surgical changes — (1) `invoke_cc` in worker.rs gets MCP isolation flags for both SSH and direct execution branches, (2) `sync_cycle` in sync.rs gets `.mcp.json` upload to sandbox. No new files, no new dependencies.

**Tech Stack:** Rust, tokio, OpenShell gRPC (existing `upload_file`)

**Spec:** `docs/superpowers/specs/2026-04-07-strict-mcp-config-design.md`

---

### Task 1: Add `--mcp-config` + `--strict-mcp-config` to `invoke_cc`

**Files:**
- Modify: `crates/bot/src/telegram/worker.rs:575-606`

- [ ] **Step 1: Add MCP isolation flags to `claude_args`**

In `invoke_cc()`, after line 581 (`"--dangerously-skip-permissions".into(),`) and before the `for arg in &cmd_args` loop (line 586), insert the MCP config args.

The path depends on execution mode: `/sandbox/.mcp.json` for SSH, `<agent_dir>/.mcp.json` for direct. Since `claude_args` is built before the branch split, we need to determine the path first.

Replace this block in `invoke_cc()` (lines 575-607):

```rust
    // Build claude -p args for execution inside OpenShell sandbox
    let reply_schema_path = ctx.agent_dir.join(".claude").join("reply-schema.json");
    let mut claude_args: Vec<String> = vec![
        "claude".into(),
        "-p".into(),
        "--dangerously-skip-permissions".into(),
    ];
    // NOTE: --verbose is intentionally NOT passed even in debug mode.
    // --verbose combined with --output-format json switches CC to stream-json array output,
    // breaking parse_reply_output which expects a single JSON object.
    // CC stderr is already captured and logged at debug level below.
    for arg in &cmd_args {
        claude_args.push(arg.clone());
    }
    claude_args.push("--output-format".into());
    claude_args.push("json".into());

    // --agent only on first call (AGDEF-02); resume inherits from session (AGDEF-03)
    if is_first_call {
        claude_args.push("--agent".into());
        claude_args.push(ctx.agent_name.clone());
    }

    // --json-schema on BOTH first and resume calls (D-01, Pitfall 4)
    // CC expects inline JSON string, NOT a file path — read and inline the content
    // Schema file lives on HOST; bot reads it before exec into sandbox.
    let reply_schema = std::fs::read_to_string(&reply_schema_path)
        .map_err(|e| format_error_reply(-1, &format!("reply-schema.json read failed: {:#}", e)))?;
    claude_args.push("--json-schema".into());
    claude_args.push(reply_schema);

    claude_args.push("--".into());
    claude_args.push(xml.to_string());
```

With this (only change is the `--mcp-config` + `--strict-mcp-config` block after `--dangerously-skip-permissions`):

```rust
    // Build claude -p args for execution inside OpenShell sandbox
    let reply_schema_path = ctx.agent_dir.join(".claude").join("reply-schema.json");
    let mut claude_args: Vec<String> = vec![
        "claude".into(),
        "-p".into(),
        "--dangerously-skip-permissions".into(),
    ];

    // MCP isolation: only use servers from our .mcp.json, block cloud MCPs.
    // Path differs by execution mode: /sandbox/ inside container, agent_dir on host.
    let mcp_config_path = if ctx.ssh_config_path.is_some() {
        "/sandbox/.mcp.json".to_string()
    } else {
        ctx.agent_dir.join(".mcp.json").to_string_lossy().into_owned()
    };
    claude_args.push("--mcp-config".into());
    claude_args.push(mcp_config_path);
    claude_args.push("--strict-mcp-config".into());

    // NOTE: --verbose is intentionally NOT passed even in debug mode.
    // --verbose combined with --output-format json switches CC to stream-json array output,
    // breaking parse_reply_output which expects a single JSON object.
    // CC stderr is already captured and logged at debug level below.
    for arg in &cmd_args {
        claude_args.push(arg.clone());
    }
    claude_args.push("--output-format".into());
    claude_args.push("json".into());

    // --agent only on first call (AGDEF-02); resume inherits from session (AGDEF-03)
    if is_first_call {
        claude_args.push("--agent".into());
        claude_args.push(ctx.agent_name.clone());
    }

    // --json-schema on BOTH first and resume calls (D-01, Pitfall 4)
    // CC expects inline JSON string, NOT a file path — read and inline the content
    // Schema file lives on HOST; bot reads it before exec into sandbox.
    let reply_schema = std::fs::read_to_string(&reply_schema_path)
        .map_err(|e| format_error_reply(-1, &format!("reply-schema.json read failed: {:#}", e)))?;
    claude_args.push("--json-schema".into());
    claude_args.push(reply_schema);

    claude_args.push("--".into());
    claude_args.push(xml.to_string());
```

- [ ] **Step 2: Build workspace to verify compilation**

Run: `cargo build --workspace`
Expected: clean build, no errors.

- [ ] **Step 3: Commit**

```bash
git add crates/bot/src/telegram/worker.rs
git commit -m "fix: add --mcp-config + --strict-mcp-config to invoke_cc

Block cloud MCP servers (Blockscout, Canva, Figma, etc.) from loading
on every claude -p call. Use only servers from our .mcp.json.
Sandbox mode: /sandbox/.mcp.json, direct mode: <agent_dir>/.mcp.json."
```

---

### Task 2: Add `.mcp.json` upload to `sync_cycle`

**Files:**
- Modify: `crates/bot/src/sync.rs:31-65`

- [ ] **Step 1: Add `.mcp.json` upload to `sync_cycle`**

In `sync_cycle()`, between step 2 (reply-schema.json upload, lines 41-48) and step 3 (builtin skills, lines 50-58), insert:

```rust
    // 3. Upload .mcp.json (rightmemory + external MCP servers with Bearer tokens)
    let mcp_json = agent_dir.join(".mcp.json");
    if mcp_json.exists() {
        rightclaw::openshell::upload_file(sandbox, &mcp_json, "/sandbox/")
            .await
            .map_err(|e| miette::miette!("sync .mcp.json: {e:#}"))?;
        tracing::debug!("sync: uploaded .mcp.json");
    }
```

Update the existing step 3 comment to step 4, and step 4 to step 5:

The full `sync_cycle` function after the change:

```rust
async fn sync_cycle(agent_dir: &Path, sandbox: &str) -> miette::Result<()> {
    // 1. Upload settings.json
    let settings = agent_dir.join(".claude").join("settings.json");
    if settings.exists() {
        rightclaw::openshell::upload_file(sandbox, &settings, "/sandbox/.claude/")
            .await
            .map_err(|e| miette::miette!("sync settings.json: {e:#}"))?;
        tracing::debug!("sync: uploaded settings.json");
    }

    // 2. Upload reply-schema.json
    let schema = agent_dir.join(".claude").join("reply-schema.json");
    if schema.exists() {
        rightclaw::openshell::upload_file(sandbox, &schema, "/sandbox/.claude/")
            .await
            .map_err(|e| miette::miette!("sync reply-schema.json: {e:#}"))?;
        tracing::debug!("sync: uploaded reply-schema.json");
    }

    // 3. Upload .mcp.json (rightmemory + external MCP servers with Bearer tokens)
    let mcp_json = agent_dir.join(".mcp.json");
    if mcp_json.exists() {
        rightclaw::openshell::upload_file(sandbox, &mcp_json, "/sandbox/")
            .await
            .map_err(|e| miette::miette!("sync .mcp.json: {e:#}"))?;
        tracing::debug!("sync: uploaded .mcp.json");
    }

    // 4. Upload rightclaw builtin skills
    for skill_name in &["rightskills", "cronsync"] {
        let skill_dir = agent_dir.join(".claude").join("skills").join(skill_name);
        if skill_dir.exists() {
            rightclaw::openshell::upload_file(sandbox, &skill_dir, "/sandbox/.claude/skills/")
                .await
                .map_err(|e| miette::miette!("sync skill {skill_name}: {e:#}"))?;
        }
    }

    // 5. Verify .claude.json -- download, check rightclaw keys, fix if needed
    verify_claude_json(agent_dir, sandbox).await?;

    tracing::debug!("sync: cycle complete");
    Ok(())
}
```

- [ ] **Step 2: Build workspace to verify compilation**

Run: `cargo build --workspace`
Expected: clean build, no errors.

- [ ] **Step 3: Commit**

```bash
git add crates/bot/src/sync.rs
git commit -m "fix: add .mcp.json to sync_cycle

Ensures rightmemory MCP and Bearer tokens are always present
in sandbox after reuse. Previously only uploaded during sandbox
creation (staging), missing on persistent sandbox restarts."
```

---

### Task 3: Integration verification on live sandbox

- [ ] **Step 1: Restart the bot**

Stop and restart rightclaw to pick up the changes:

```bash
rightclaw down
cargo build --workspace
rightclaw up --agents right
```

- [ ] **Step 2: Verify `.mcp.json` is in sandbox**

```bash
ssh -F ~/.rightclaw/run/ssh/rightclaw-right.ssh-config openshell-rightclaw-right 'cat /sandbox/.mcp.json'
```

Expected: JSON with `rightmemory` server entry.

- [ ] **Step 3: Send a test message via Telegram and check logs**

Send "hi" to the bot. Check the bot log for:
- No cloud MCP server initialization (no Blockscout/Canva/Figma mentions)
- `claude -p finished` with `exit_code=0`
- Response time under ~5s (was ~12s)

```bash
tail -f ~/.rightclaw/logs/right.log.$(date +%Y-%m-%d)
```

- [ ] **Step 4: Verify no cloud MCPs in session JSONL**

```bash
ssh -F ~/.rightclaw/run/ssh/rightclaw-right.ssh-config openshell-rightclaw-right \
  'cat /sandbox/.claude/projects/-sandbox/*.jsonl' | grep -c 'claude_ai_'
```

Expected: 0 matches (no cloud MCP tool references).

- [ ] **Step 5: Commit spec and plan**

```bash
git add docs/superpowers/specs/2026-04-07-strict-mcp-config-design.md
git add docs/superpowers/plans/2026-04-07-strict-mcp-config.md
git commit -m "docs: strict-mcp-config spec and implementation plan"
```
