# Home Directory in System Prompt — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Tell CC agents their home/working directory explicitly so they don't guess wrong paths (e.g. `/root/` instead of `/sandbox/`).

**Architecture:** Add `home_dir: &str` parameter to `generate_system_prompt()`, include it in the `## Environment` section. Update BOOTSTRAP.md and OPERATING_INSTRUCTIONS.md to reference "home directory" instead of hardcoded paths. Update all callers.

**Tech Stack:** Rust, Markdown templates

---

### Task 1: Add home_dir parameter to generate_system_prompt() with TDD

**Files:**
- Modify: `crates/rightclaw/src/codegen/agent_def.rs:40-103`
- Modify: `crates/rightclaw/src/codegen/agent_def_tests.rs`

- [ ] **Step 1: Write the failing test**

Add to `crates/rightclaw/src/codegen/agent_def_tests.rs`:

```rust
#[test]
fn system_prompt_contains_home_dir() {
    let result = generate_system_prompt(
        "test",
        &crate::agent::types::SandboxMode::Openshell,
        "/my/custom/home",
    );
    assert!(
        result.contains("/my/custom/home"),
        "system prompt must contain the passed home_dir"
    );
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `devenv shell -- cargo test -p rightclaw system_prompt_contains_home_dir`
Expected: FAIL — `generate_system_prompt` takes 2 args, 3 supplied

- [ ] **Step 3: Update generate_system_prompt() signature and body**

In `crates/rightclaw/src/codegen/agent_def.rs`, change the function signature and add home_dir to the Environment section:

```rust
pub fn generate_system_prompt(agent_name: &str, sandbox_mode: &crate::agent::types::SandboxMode, home_dir: &str) -> String {
    let sandbox_desc = match sandbox_mode {
        crate::agent::types::SandboxMode::Openshell => "OpenShell sandbox (k3s container with network and filesystem policies)",
        crate::agent::types::SandboxMode::None => "no sandbox (direct host access)",
    };

    let mut prompt = format!(
        "\
You are {agent_name}, a RightClaw agent.

RightClaw is a multi-agent runtime for Claude Code built on NVIDIA OpenShell. Each agent runs \
as an independent Claude Code session inside its own sandbox with declarative YAML policies. \
Agents have persistent memory, scheduled tasks (cron), and tool management via MCP.

Source: https://github.com/onsails/rightclaw

## Environment

- Agent name: {agent_name}
- Sandbox: {sandbox_desc}
- Home / working directory: {home_dir}

## MCP

You are connected to the `right` MCP server for persistent memory, cron job management, \
and external MCP server management. Use `mcp__right__mcp_list` to see all configured servers.\n\
\n\
**Call `right` MCP tools directly by name (e.g. `mcp__right__mcp_list`). \
Do NOT use ToolSearch to find them — ToolSearch does not index MCP tools. \
They are always available.**

## Response Rules

Your final response MUST be self-contained. The user ONLY sees your final response — \
they do NOT see tool calls, intermediate text, or thinking. Never say \"see above\", \
\"as shown above\", or reference previous output. If you gathered data, include it in \
your final response.
"
    );

    if matches!(sandbox_mode, crate::agent::types::SandboxMode::Openshell) {
        prompt.push_str(&format!(
            "
## User SSH Access

If an operation requires an interactive terminal (TUI, interactive prompts, \
password input) that you cannot perform from within your sandbox — tell the \
user to run:

  rightclaw agent ssh {agent_name}
  rightclaw agent ssh {agent_name} -- <command>

Examples:
- `gh auth login`
- `gcloud auth login`
- `npm login`
- Any command with interactive prompts or TUI

Always provide the exact command with the `--` separator when passing a specific command.
"
        ));
    }

    prompt
}
```

- [ ] **Step 4: Update all existing tests to pass 3 args**

In `crates/rightclaw/src/codegen/agent_def_tests.rs`, update every `generate_system_prompt` call to include a third argument. Replace each occurrence:

Line 29: `generate_system_prompt("mybot", &crate::agent::types::SandboxMode::Openshell)` → `generate_system_prompt("mybot", &crate::agent::types::SandboxMode::Openshell, "/sandbox")`

Line 35: `generate_system_prompt("test", &crate::agent::types::SandboxMode::Openshell)` → `generate_system_prompt("test", &crate::agent::types::SandboxMode::Openshell, "/sandbox")`

Line 42: `generate_system_prompt("test", &crate::agent::types::SandboxMode::Openshell)` → `generate_system_prompt("test", &crate::agent::types::SandboxMode::Openshell, "/sandbox")`

Line 45: `generate_system_prompt("test", &crate::agent::types::SandboxMode::None)` → `generate_system_prompt("test", &crate::agent::types::SandboxMode::None, "/test/agent/home")`

Line 51: `generate_system_prompt("test", &crate::agent::types::SandboxMode::Openshell)` → `generate_system_prompt("test", &crate::agent::types::SandboxMode::Openshell, "/sandbox")`

Line 58: `generate_system_prompt("mybot", &crate::agent::types::SandboxMode::Openshell)` → `generate_system_prompt("mybot", &crate::agent::types::SandboxMode::Openshell, "/sandbox")`

Line 65: `generate_system_prompt("mybot", &crate::agent::types::SandboxMode::None)` → `generate_system_prompt("mybot", &crate::agent::types::SandboxMode::None, "/test/agent/home")`

- [ ] **Step 5: Run all codegen tests**

Run: `devenv shell -- cargo test -p rightclaw codegen`
Expected: all tests in `agent_def_tests` pass, including the new `system_prompt_contains_home_dir`

- [ ] **Step 6: Commit**

```bash
git add crates/rightclaw/src/codegen/agent_def.rs crates/rightclaw/src/codegen/agent_def_tests.rs
git commit -m "feat: add home_dir parameter to generate_system_prompt()"
```

---

### Task 2: Update BOOTSTRAP.md and OPERATING_INSTRUCTIONS.md templates

**Files:**
- Modify: `templates/right/agent/BOOTSTRAP.md:27-28`
- Modify: `templates/right/prompt/OPERATING_INSTRUCTIONS.md:92,96`

- [ ] **Step 1: Update BOOTSTRAP.md**

In `templates/right/agent/BOOTSTRAP.md`, replace line 27:

```
Write all three files in your current working directory using the Write tool.
```

With:

```
Write all three files in your home directory using the Write tool.
```

- [ ] **Step 2: Update OPERATING_INSTRUCTIONS.md — add inbox line**

In `templates/right/prompt/OPERATING_INSTRUCTIONS.md`, after line 92 (`Use the Read tool to view images and files at the given paths.`), add a blank line and:

```
Attachments are downloaded to the inbox/ directory in your home directory.
```

- [ ] **Step 3: Update OPERATING_INSTRUCTIONS.md — fix outbox line**

In `templates/right/prompt/OPERATING_INSTRUCTIONS.md`, replace line 96:

```
Write files to /sandbox/outbox/ (or the outbox/ directory in your working directory).
```

With:

```
Write files to the outbox/ directory in your home directory.
```

- [ ] **Step 4: Run template-dependent tests**

Run: `devenv shell -- cargo test -p rightclaw bootstrap_instructions`
Expected: `bootstrap_instructions_constant_is_non_empty` passes (BOOTSTRAP.md still contains expected strings)

- [ ] **Step 5: Commit**

```bash
git add templates/right/agent/BOOTSTRAP.md templates/right/prompt/OPERATING_INSTRUCTIONS.md
git commit -m "fix: use home directory in bootstrap and operating instructions"
```

---

### Task 3: Update callers — pipeline.rs, worker.rs, main.rs

**Files:**
- Modify: `crates/rightclaw/src/codegen/pipeline.rs:96`
- Modify: `crates/bot/src/telegram/worker.rs:856-863`
- Modify: `crates/rightclaw-cli/src/main.rs:2157`

- [ ] **Step 1: Update pipeline.rs**

In `crates/rightclaw/src/codegen/pipeline.rs`, replace line 96:

```rust
        crate::codegen::generate_system_prompt(&agent.name, &agent_sandbox_mode),
```

With:

```rust
        crate::codegen::generate_system_prompt(
            &agent.name,
            &agent_sandbox_mode,
            match agent_sandbox_mode {
                SandboxMode::Openshell => "/sandbox",
                SandboxMode::None => agent.path.to_str().unwrap_or("/sandbox"),
            },
        ),
```

- [ ] **Step 2: Update worker.rs**

In `crates/bot/src/telegram/worker.rs`, replace lines 856-863:

```rust
    let base_prompt = rightclaw::codegen::generate_system_prompt(
        &ctx.agent_name,
        &if ctx.ssh_config_path.is_some() {
            rightclaw::agent::types::SandboxMode::Openshell
        } else {
            rightclaw::agent::types::SandboxMode::None
        },
    );
```

With:

```rust
    let (sandbox_mode, home_dir) = if ctx.ssh_config_path.is_some() {
        (rightclaw::agent::types::SandboxMode::Openshell, "/sandbox".to_owned())
    } else {
        (rightclaw::agent::types::SandboxMode::None, ctx.agent_dir.to_string_lossy().into_owned())
    };
    let base_prompt = rightclaw::codegen::generate_system_prompt(
        &ctx.agent_name,
        &sandbox_mode,
        &home_dir,
    );
```

- [ ] **Step 3: Update main.rs (cmd_pair)**

In `crates/rightclaw-cli/src/main.rs`, replace line 2157:

```rust
    let base_prompt = rightclaw::codegen::generate_system_prompt(&agent.name, &sandbox_mode);
```

With:

```rust
    let base_prompt = rightclaw::codegen::generate_system_prompt(
        &agent.name,
        &sandbox_mode,
        &agent.path.to_string_lossy(),
    );
```

- [ ] **Step 4: Build the whole workspace**

Run: `devenv shell -- cargo build --workspace`
Expected: compiles with no errors

- [ ] **Step 5: Run full test suite**

Run: `devenv shell -- cargo test --workspace`
Expected: all tests pass

- [ ] **Step 6: Commit**

```bash
git add crates/rightclaw/src/codegen/pipeline.rs crates/bot/src/telegram/worker.rs crates/rightclaw-cli/src/main.rs
git commit -m "fix: pass home_dir to generate_system_prompt() in all callers"
```

---

### Task 4: Update PROMPT_SYSTEM.md

**Files:**
- Modify: `PROMPT_SYSTEM.md:86`

- [ ] **Step 1: Update PROMPT_SYSTEM.md**

In `PROMPT_SYSTEM.md`, replace line 86:

```
Content: agent name, RightClaw description, sandbox mode, MCP reference, repo link.
```

With:

```
Content: agent name, RightClaw description, sandbox mode, home/working directory, MCP reference, repo link.
```

- [ ] **Step 2: Commit**

```bash
git add PROMPT_SYSTEM.md
git commit -m "docs: update PROMPT_SYSTEM.md for home_dir in system prompt"
```
