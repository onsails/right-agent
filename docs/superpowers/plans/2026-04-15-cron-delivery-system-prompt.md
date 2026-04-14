# Cron & Delivery System Prompt Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give cron and delivery CC invocations the same composite system prompt as the main worker, with MCP instructions.

**Architecture:** Extract `build_prompt_assembly_script` and helpers from `worker.rs` into shared `telegram/prompt.rs`. Thread `InternalClient` into cron and delivery. Replace `--agent` with `--system-prompt-file` using the same assembly pattern as worker. Delivery hardcodes haiku model.

**Tech Stack:** Rust, tokio, rightclaw InternalClient

---

### Task 1: Extract prompt assembly to `telegram/prompt.rs`

**Files:**
- Create: `crates/bot/src/telegram/prompt.rs`
- Modify: `crates/bot/src/telegram/mod.rs:1-10`
- Modify: `crates/bot/src/telegram/worker.rs:540-618` (remove moved code, add imports)

- [ ] **Step 1: Create `telegram/prompt.rs` with moved code**

Create `crates/bot/src/telegram/prompt.rs`:

```rust
//! Shared prompt assembly for CC invocations (worker, cron, delivery).

/// Shell-escape a string for use in generated shell scripts.
pub(crate) fn shell_escape(s: &str) -> String {
    shlex::try_quote(s)
        .expect("shlex::try_quote cannot fail for valid UTF-8")
        .into_owned()
}

/// Prompt section: a file from disk that gets a markdown header.
struct PromptSection {
    filename: &'static str,
    header: &'static str,
}

/// Identity and config files included in the system prompt (normal mode).
const PROMPT_SECTIONS: &[PromptSection] = &[
    PromptSection { filename: "IDENTITY.md", header: "## Your Identity" },
    PromptSection { filename: "SOUL.md", header: "## Your Personality and Values" },
    PromptSection { filename: "USER.md", header: "## Your User" },
    PromptSection { filename: "AGENTS.md", header: "## Agent Configuration" },
    PromptSection { filename: "TOOLS.md", header: "## Environment and Tools" },
];

/// Generate a shell script that assembles a composite system prompt and runs `claude -p`.
///
/// Parameterized by `root_path` — the directory containing agent .md files:
/// - Sandbox: `/sandbox`
/// - No-sandbox: absolute path to `agent_dir`
///
/// The script reads files from `root_path`, assembles them into `prompt_file`,
/// then runs claude from `workdir`.
pub(crate) fn build_prompt_assembly_script(
    base_prompt: &str,
    bootstrap_mode: bool,
    root_path: &str,
    prompt_file: &str,
    workdir: &str,
    claude_args: &[String],
    mcp_instructions: Option<&str>,
) -> String {
    let escaped_base = base_prompt.replace('\'', "'\\''");
    let escaped_args: Vec<String> = claude_args.iter().map(|a| shell_escape(a)).collect();
    let claude_cmd = escaped_args.join(" ");

    let file_sections = if bootstrap_mode {
        let escaped_bootstrap =
            rightclaw::codegen::BOOTSTRAP_INSTRUCTIONS.replace('\'', "'\\''");
        format!(
            "\nprintf '\\n## Bootstrap Instructions\\n'\nprintf '%s\\n' '{escaped_bootstrap}'"
        )
    } else {
        let escaped_ops =
            rightclaw::codegen::OPERATING_INSTRUCTIONS.replace('\'', "'\\''");
        let mut sections = format!(
            "\nprintf '\\n## Operating Instructions\\n'\nprintf '%s\\n' '{escaped_ops}'"
        );
        for s in PROMPT_SECTIONS {
            let filename = s.filename;
            let header = s.header;
            sections.push_str(&format!(
                r#"
if [ -f {root_path}/{filename} ]; then
  printf '\n{header}\n'
  cat {root_path}/{filename}
  printf '\n'
fi"#
            ));
        }
        sections
    };

    let mcp_section = match mcp_instructions {
        Some(instr) => {
            let escaped = instr.replace('\'', "'\\''");
            format!("\nprintf '\\n'\nprintf '%s\\n' '{escaped}'")
        }
        None => String::new(),
    };

    format!(
        "{{ printf '{escaped_base}'\n{file_sections}\n{mcp_section}\n}} > {prompt_file}\ncd {workdir} && {claude_cmd} --system-prompt-file {prompt_file}"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: build a script with sandbox-like paths for testing.
    fn test_script(base: &str, bootstrap: bool, args: &[String], mcp: Option<&str>) -> String {
        build_prompt_assembly_script(
            base,
            bootstrap,
            "/sandbox",
            "/tmp/rightclaw-system-prompt.md",
            "/sandbox",
            args,
            mcp,
        )
    }

    #[test]
    fn script_bootstrap_includes_bootstrap_md() {
        let script = test_script("Base prompt", true, &["claude".into(), "-p".into()], None);
        assert!(script.contains("Bootstrap Instructions"), "must have Bootstrap Instructions header");
        assert!(script.contains("First-Time Setup"), "must contain compiled-in bootstrap content");
        assert!(!script.contains("cat /sandbox/IDENTITY.md"), "bootstrap must not cat IDENTITY.md");
        assert!(!script.contains("cat /sandbox/SOUL.md"), "bootstrap must not cat SOUL.md");
        assert!(script.contains("claude"), "must contain claude command");
        assert!(script.contains("--system-prompt-file"), "must pass --system-prompt-file");
    }

    #[test]
    fn script_normal_includes_all_identity_files() {
        let script = test_script("Base prompt", false, &["claude".into(), "-p".into()], None);
        assert!(script.contains("IDENTITY.md"));
        assert!(script.contains("SOUL.md"));
        assert!(script.contains("USER.md"));
        assert!(script.contains("AGENTS.md"));
        assert!(script.contains("TOOLS.md"));
        assert!(script.contains("Operating Instructions"), "must have compiled-in Operating Instructions");
        assert!(!script.contains("cat /sandbox/.claude/agents/BOOTSTRAP.md"), "normal must not cat BOOTSTRAP.md");
    }

    #[test]
    fn script_escapes_single_quotes_in_base() {
        let script = test_script("It's a test", true, &["claude".into()], None);
        assert!(!script.contains("It's"), "raw single quote must be escaped");
        assert!(script.contains("It"), "content must still be present");
    }

    #[test]
    fn script_shell_escapes_claude_args() {
        let script = test_script(
            "Base",
            false,
            &["claude".into(), "-p".into(), "--json-schema".into(), r#"{"type":"object"}"#.into()],
            None,
        );
        assert!(script.contains("--json-schema"));
        assert!(script.contains("type"));
    }

    #[test]
    fn script_writes_to_prompt_file_and_uses_system_prompt_file() {
        let script = test_script("X", false, &["claude".into()], None);
        assert!(script.contains("/tmp/rightclaw-system-prompt.md"));
        assert!(script.contains("--system-prompt-file /tmp/rightclaw-system-prompt.md"));
    }

    #[test]
    fn script_custom_paths() {
        let script = build_prompt_assembly_script(
            "Base\n",
            false,
            "/home/agent",
            "/home/agent/.claude/composite-system-prompt.md",
            "/home/agent",
            &["claude".into(), "-p".into()],
            None,
        );
        assert!(script.contains("/home/agent/IDENTITY.md"), "must use custom root_path");
        assert!(script.contains("/home/agent/.claude/composite-system-prompt.md"), "must use custom prompt_file");
        assert!(script.contains("cd /home/agent"), "must cd to custom workdir");
    }

    #[test]
    fn script_bootstrap_mode_same_regardless_of_paths() {
        let script = build_prompt_assembly_script(
            "Base\n",
            true,
            "/home/agent",
            "/home/agent/.claude/composite-system-prompt.md",
            "/home/agent",
            &["claude".into()],
            None,
        );
        assert!(script.contains("## Bootstrap Instructions"));
        assert!(script.contains("First-Time Setup"), "must use compiled-in content");
        assert!(!script.contains("cat /home/agent/IDENTITY.md"), "bootstrap must not cat IDENTITY.md");
    }

    #[test]
    fn script_includes_mcp_instructions() {
        let script = test_script(
            "Base",
            false,
            &["claude".into()],
            Some("# MCP Server Instructions\n\n## composio\n\nConnect with 250+ apps.\n"),
        );
        assert!(script.contains("MCP Server Instructions"));
        assert!(script.contains("composio"));
        assert!(script.contains("printf '%s\\n'"));
    }

    #[test]
    fn script_none_mcp_instructions_omitted() {
        let script = test_script("Base", false, &["claude".into()], None);
        assert!(!script.contains("MCP Server Instructions"));
    }

    #[test]
    fn script_mcp_instructions_with_custom_paths() {
        let script = build_prompt_assembly_script(
            "Base\n",
            false,
            "/home/agent",
            "/home/agent/.claude/composite-system-prompt.md",
            "/home/agent",
            &["claude".into()],
            Some("# MCP Server Instructions\n\n## notion\n\nNotion tools.\n"),
        );
        assert!(script.contains("MCP Server Instructions"));
        assert!(script.contains("notion"));
        assert!(script.contains("Notion tools."));
    }

    #[test]
    fn script_bootstrap_uses_compiled_constant() {
        let script = test_script("Base prompt", true, &["claude".into(), "-p".into()], None);
        assert!(!script.contains("cat /sandbox"), "bootstrap must not cat any sandbox file");
        assert!(script.contains("First-Time Setup"), "must contain compiled-in bootstrap content");
        assert!(!script.contains("cat /sandbox/IDENTITY.md"), "bootstrap must not cat IDENTITY.md");
    }

    #[test]
    fn script_normal_has_operating_instructions_before_identity() {
        let script = test_script("Base prompt", false, &["claude".into()], None);
        let op_instr_pos = script.find("Operating Instructions").expect("must have Operating Instructions");
        let identity_pos = script.find("IDENTITY.md").expect("must have IDENTITY.md");
        assert!(op_instr_pos < identity_pos, "Operating Instructions must come before IDENTITY.md");
    }
}
```

- [ ] **Step 2: Register module in `telegram/mod.rs`**

Add after line 7 (`pub mod oauth_callback;`):

```rust
pub(crate) mod prompt;
```

- [ ] **Step 3: Replace moved code in `worker.rs` with imports**

Remove the following from `worker.rs` (lines 540-618):
- `fn shell_escape` (line 540-542)
- `struct PromptSection` (line 545-548)
- `const PROMPT_SECTIONS` (line 551-557)
- `fn build_prompt_assembly_script` (line 567-618)

Replace with imports at the call sites. The two call sites (lines 871 and 896) change from `build_prompt_assembly_script(...)` to `super::prompt::build_prompt_assembly_script(...)`.

Also remove the prompt assembly tests from worker.rs (lines 1605-1748) — they are now in `prompt.rs`.

- [ ] **Step 4: Run tests**

Run: `devenv shell -- cargo test -p rightclaw-bot`
Expected: All tests pass (same tests, new location).

- [ ] **Step 5: Commit**

```bash
git add crates/bot/src/telegram/prompt.rs crates/bot/src/telegram/mod.rs crates/bot/src/telegram/worker.rs
git commit -m "refactor: extract prompt assembly to telegram/prompt.rs"
```

---

### Task 2: Add system prompt to cron `execute_job`

**Files:**
- Modify: `crates/bot/src/cron.rs:108-204` (execute_job), `crates/bot/src/cron.rs:527-532` (run_cron_task), `crates/bot/src/cron.rs:697-702,755-760` (call sites)
- Modify: `crates/bot/src/lib.rs:349-358` (cron spawn)

- [ ] **Step 1: Add `internal_client` parameter to `run_cron_task`**

In `crates/bot/src/cron.rs`, change `run_cron_task` signature (line 527):

```rust
pub async fn run_cron_task(
    agent_dir: std::path::PathBuf,
    agent_name: String,
    model: Option<String>,
    ssh_config_path: Option<std::path::PathBuf>,
    internal_client: std::sync::Arc<rightclaw::mcp::internal_client::InternalClient>,
    shutdown: CancellationToken,
) {
```

Clone and pass `internal_client` to all `execute_job` calls inside `run_cron_task` (the spawns around lines 697-702 and 755-760).

- [ ] **Step 2: Add `internal_client` parameter to `execute_job`**

Change `execute_job` signature (line 108):

```rust
async fn execute_job(
    job_name: &str,
    spec: &CronSpec,
    agent_dir: &std::path::Path,
    agent_name: &str,
    model: Option<&str>,
    ssh_config_path: Option<&std::path::Path>,
    internal_client: &rightclaw::mcp::internal_client::InternalClient,
) {
```

- [ ] **Step 3: Replace `--agent` with system prompt assembly in `execute_job`**

Replace the claude_args construction (lines 169-189) and the SSH/direct command building (lines 191-240) with:

```rust
    // Build claude CLI arguments — no --agent, prompt comes via --system-prompt-file.
    let mut claude_args: Vec<String> = vec![
        "claude".into(),
        "-p".into(),
        "--dangerously-skip-permissions".into(),
    ];
    if let Some(model) = model {
        claude_args.push("--model".into());
        claude_args.push(model.into());
    }
    claude_args.push("--max-budget-usd".into());
    claude_args.push(format!("{:.2}", spec.max_budget_usd));
    claude_args.push("--verbose".into());
    claude_args.push("--output-format".into());
    claude_args.push("stream-json".into());
    claude_args.push("--json-schema".into());
    claude_args.push(rightclaw::codegen::CRON_SCHEMA_JSON.into());
    claude_args.push("--".into());
    claude_args.push(spec.prompt.clone());

    // Derive sandbox_mode and home_dir from ssh_config_path (same as worker).
    let (sandbox_mode, home_dir) = if ssh_config_path.is_some() {
        (rightclaw::agent::types::SandboxMode::Openshell, "/sandbox".to_owned())
    } else {
        (rightclaw::agent::types::SandboxMode::None, agent_dir.to_string_lossy().into_owned())
    };
    let base_prompt = rightclaw::codegen::generate_system_prompt(agent_name, &sandbox_mode, &home_dir);

    // Fetch MCP instructions from aggregator (non-fatal).
    let mcp_instructions: Option<String> = match internal_client.mcp_instructions(agent_name).await {
        Ok(resp) => {
            if resp.instructions.trim().len() > rightclaw::codegen::mcp_instructions::MCP_INSTRUCTIONS_HEADER.trim().len() {
                Some(resp.instructions)
            } else {
                None
            }
        }
        Err(e) => {
            tracing::warn!(job = %job_name, "failed to fetch MCP instructions: {e:#}");
            None
        }
    };

    let mut cmd = if let Some(ssh_config) = ssh_config_path {
        let mut assembly_script = crate::telegram::prompt::build_prompt_assembly_script(
            &base_prompt,
            false,
            "/sandbox",
            "/tmp/rightclaw-system-prompt.md",
            "/sandbox",
            &claude_args,
            mcp_instructions.as_deref(),
        );
        // Inject auth token as env var in the remote shell.
        if let Some(token) = crate::login::load_auth_token(agent_dir) {
            let escaped = token.replace('\'', "'\\''");
            assembly_script = format!("export CLAUDE_CODE_OAUTH_TOKEN='{escaped}'\n{assembly_script}");
        }
        let ssh_host = rightclaw::openshell::ssh_host(agent_name);
        let mut c = tokio::process::Command::new("ssh");
        c.arg("-F").arg(ssh_config);
        c.arg(&ssh_host);
        c.arg("--");
        c.arg(assembly_script);
        c
    } else {
        let agent_dir_str = agent_dir.to_string_lossy();
        let prompt_path = agent_dir.join(".claude").join("cron-system-prompt.md");
        let prompt_path_str = prompt_path.to_string_lossy();
        let assembly_script = crate::telegram::prompt::build_prompt_assembly_script(
            &base_prompt,
            false,
            &agent_dir_str,
            &prompt_path_str,
            &agent_dir_str,
            &claude_args,
            mcp_instructions.as_deref(),
        );
        let cc_bin = match which::which("claude").or_else(|_| which::which("claude-bun")) {
            Ok(p) => p,
            Err(_) => {
                tracing::error!(job = %job_name, "claude binary not found in PATH");
                update_run_record(&conn, &run_id, None, "failed");
                std::fs::remove_file(&lock_path).ok();
                return;
            }
        };
        let mut c = tokio::process::Command::new("bash");
        c.arg("-c");
        c.arg(&assembly_script);
        c.env("HOME", agent_dir);
        c.env("USE_BUILTIN_RIPGREP", "0");
        if let Some(token) = crate::login::load_auth_token(agent_dir) {
            c.env("CLAUDE_CODE_OAUTH_TOKEN", &token);
        }
        c.current_dir(agent_dir);
        c
    };
```

- [ ] **Step 4: Update `lib.rs` to pass `internal_client` to cron**

In `crates/bot/src/lib.rs` (lines 349-358), add `internal_client` clone:

```rust
    let cron_internal_client = Arc::clone(&internal_client);
    let cron_handle = tokio::spawn(async move {
        cron::run_cron_task(cron_agent_dir, cron_agent_name, cron_model, cron_ssh_config, cron_internal_client, cron_shutdown).await;
    });
```

- [ ] **Step 5: Run check**

Run: `devenv shell -- cargo check --workspace`
Expected: No errors.

- [ ] **Step 6: Commit**

```bash
git add crates/bot/src/cron.rs crates/bot/src/lib.rs
git commit -m "feat: add system prompt and MCP instructions to cron execute_job"
```

---

### Task 3: Add system prompt to delivery `deliver_through_session`

**Files:**
- Modify: `crates/bot/src/cron_delivery.rs:160-168` (run_delivery_loop signature), `crates/bot/src/cron_delivery.rs:305-370` (deliver_through_session)
- Modify: `crates/bot/src/lib.rs:366-386` (delivery spawn)

- [ ] **Step 1: Update `run_delivery_loop` — add `internal_client`, remove `model`, hardcode haiku**

Change signature (line 160):

```rust
pub async fn run_delivery_loop(
    agent_dir: PathBuf,
    agent_name: String,
    bot: crate::telegram::BotType,
    notify_chat_ids: Vec<i64>,
    idle_ts: Arc<IdleTimestamp>,
    ssh_config_path: Option<PathBuf>,
    internal_client: Arc<rightclaw::mcp::internal_client::InternalClient>,
    shutdown: tokio_util::sync::CancellationToken,
) {
```

Note: `model: Option<String>` parameter removed.

Update the `deliver_through_session` call inside `run_delivery_loop` (around line 243):

```rust
        match deliver_through_session(
            &yaml,
            &agent_dir,
            &agent_name,
            &bot,
            &notify_chat_ids,
            ssh_config_path.as_deref(),
            session_id,
            &internal_client,
        )
```

- [ ] **Step 2: Update `deliver_through_session` — system prompt + haiku**

Change signature:

```rust
async fn deliver_through_session(
    yaml_input: &str,
    agent_dir: &Path,
    agent_name: &str,
    bot: &crate::telegram::BotType,
    notify_chat_ids: &[i64],
    ssh_config_path: Option<&Path>,
    session_id: Option<String>,
    internal_client: &rightclaw::mcp::internal_client::InternalClient,
) -> Result<(), String> {
```

Note: `model: Option<&str>` parameter removed.

Replace the claude_args construction and command building with:

```rust
    // Delivery always uses Haiku — cheap relay task.
    const DELIVERY_MODEL: &str = "claude-haiku-4-5-20251001";

    // Build claude CLI arguments — no --agent, prompt comes via --system-prompt-file.
    let mut claude_args: Vec<String> = vec![
        "claude".into(),
        "-p".into(),
        "--dangerously-skip-permissions".into(),
        "--model".into(),
        DELIVERY_MODEL.into(),
    ];
    claude_args.push("--max-budget-usd".into());
    claude_args.push("0.05".into());
    claude_args.push("--max-turns".into());
    claude_args.push("3".into());
    claude_args.push("--output-format".into());
    claude_args.push("json".into());

    if let Some(ref sid) = session_id {
        claude_args.push("--resume".into());
        claude_args.push(sid.clone());
    }

    let reply_schema_path = agent_dir.join(".claude").join("reply-schema.json");
    if let Ok(schema) = std::fs::read_to_string(&reply_schema_path) {
        claude_args.push("--json-schema".into());
        claude_args.push(schema);
    }

    // Derive sandbox_mode and home_dir from ssh_config_path.
    let (sandbox_mode, home_dir) = if ssh_config_path.is_some() {
        (rightclaw::agent::types::SandboxMode::Openshell, "/sandbox".to_owned())
    } else {
        (rightclaw::agent::types::SandboxMode::None, agent_dir.to_string_lossy().into_owned())
    };
    let base_prompt = rightclaw::codegen::generate_system_prompt(agent_name, &sandbox_mode, &home_dir);

    // Fetch MCP instructions from aggregator (non-fatal).
    let mcp_instructions: Option<String> = match internal_client.mcp_instructions(agent_name).await {
        Ok(resp) => {
            if resp.instructions.trim().len() > rightclaw::codegen::mcp_instructions::MCP_INSTRUCTIONS_HEADER.trim().len() {
                Some(resp.instructions)
            } else {
                None
            }
        }
        Err(e) => {
            tracing::warn!("delivery: failed to fetch MCP instructions: {e:#}");
            None
        }
    };

    let mut cmd = if let Some(ssh_config) = ssh_config_path {
        let mut assembly_script = crate::telegram::prompt::build_prompt_assembly_script(
            &base_prompt,
            false,
            "/sandbox",
            "/tmp/rightclaw-system-prompt.md",
            "/sandbox",
            &claude_args,
            mcp_instructions.as_deref(),
        );
        if let Some(token) = crate::login::load_auth_token(agent_dir) {
            let escaped = token.replace('\'', "'\\''");
            assembly_script = format!("export CLAUDE_CODE_OAUTH_TOKEN='{escaped}'\n{assembly_script}");
        }
        let ssh_host = rightclaw::openshell::ssh_host(agent_name);
        let mut c = tokio::process::Command::new("ssh");
        c.arg("-F").arg(ssh_config);
        c.arg(&ssh_host);
        c.arg("--");
        c.arg(assembly_script);
        c
    } else {
        let agent_dir_str = agent_dir.to_string_lossy();
        let prompt_path = agent_dir.join(".claude").join("delivery-system-prompt.md");
        let prompt_path_str = prompt_path.to_string_lossy();
        let assembly_script = crate::telegram::prompt::build_prompt_assembly_script(
            &base_prompt,
            false,
            &agent_dir_str,
            &prompt_path_str,
            &agent_dir_str,
            &claude_args,
            mcp_instructions.as_deref(),
        );
        let cc_bin = which::which("claude")
            .or_else(|_| which::which("claude-bun"))
            .map_err(|_| "claude binary not found in PATH".to_string())?;
        let mut c = tokio::process::Command::new("bash");
        c.arg("-c");
        c.arg(&assembly_script);
        c.env("HOME", agent_dir);
        c.env("USE_BUILTIN_RIPGREP", "0");
        if let Some(token) = crate::login::load_auth_token(agent_dir) {
            c.env("CLAUDE_CODE_OAUTH_TOKEN", &token);
        }
        c.current_dir(agent_dir);
        c
    };
```

The rest of `deliver_through_session` (stdin write, timeout, output parsing, Telegram send) stays unchanged.

- [ ] **Step 3: Update `lib.rs` to pass `internal_client` to delivery, remove `model`**

In `crates/bot/src/lib.rs` (lines 366-386), remove `delivery_model` and add `internal_client`:

```rust
    let delivery_agent_dir = agent_dir.clone();
    let delivery_agent_name = args.agent.clone();
    let delivery_bot = telegram::bot::build_bot(token.clone());
    let delivery_chat_ids = config.allowed_chat_ids.clone();
    let delivery_idle_ts = Arc::clone(&idle_timestamp);
    let delivery_ssh_config = ssh_config_path.clone();
    let delivery_internal_client = Arc::clone(&internal_client);
    let delivery_shutdown = shutdown.clone();
    let delivery_handle = tokio::spawn(async move {
        cron_delivery::run_delivery_loop(
            delivery_agent_dir,
            delivery_agent_name,
            delivery_bot,
            delivery_chat_ids,
            delivery_idle_ts,
            delivery_ssh_config,
            delivery_internal_client,
            delivery_shutdown,
        ).await;
    });
```

- [ ] **Step 4: Run check and tests**

Run: `devenv shell -- cargo check --workspace && devenv shell -- cargo test -p rightclaw-bot`
Expected: All pass.

- [ ] **Step 5: Commit**

```bash
git add crates/bot/src/cron_delivery.rs crates/bot/src/lib.rs
git commit -m "feat: add system prompt and MCP instructions to cron delivery, use haiku"
```

---

### Task 4: Update PROMPT_SYSTEM.md and ARCHITECTURE.md

**Files:**
- Modify: `PROMPT_SYSTEM.md`
- Modify: `ARCHITECTURE.md`

- [ ] **Step 1: Update PROMPT_SYSTEM.md**

Change line 23 from:

```
A single function `build_prompt_assembly_script()` in `worker.rs` generates a
```

to:

```
A single function `build_prompt_assembly_script()` in `telegram/prompt.rs` generates a
```

Add after line 31 (before "## Prompt Structure"):

```markdown
### Callers

All three CC invocation paths use `build_prompt_assembly_script()`:

| Caller | Module | bootstrap_mode | Schema | Model |
|--------|--------|---------------|--------|-------|
| Worker (Telegram messages) | `telegram/worker.rs` | true/false | reply-schema.json | agent config |
| Cron (scheduled jobs) | `cron.rs` | false | CRON_SCHEMA_JSON | agent config |
| Delivery (cron result relay) | `cron_delivery.rs` | false | reply-schema.json | claude-haiku-4-5-20251001 |
```

- [ ] **Step 2: Update ARCHITECTURE.md module map**

In the `rightclaw-bot` module map, add `prompt.rs` under `telegram/`:

```
├── telegram/
│   ├── prompt.rs       # Shared prompt assembly: build_prompt_assembly_script, shell helpers
│   ├── attachments.rs  # ...
```

Update the `cron.rs` and `cron_delivery.rs` descriptions:

```
├── cron.rs             # Cron engine: load specs, lock check, invoke CC with system prompt, persist results
├── cron_delivery.rs    # Delivery poll loop: idle detection, dedup, CC session delivery (haiku), cleanup
```

- [ ] **Step 3: Commit**

```bash
git add PROMPT_SYSTEM.md ARCHITECTURE.md
git commit -m "docs: update PROMPT_SYSTEM.md and ARCHITECTURE.md for shared prompt assembly"
```

---

### Task 5: Final build and test

- [ ] **Step 1: Full workspace build**

Run: `devenv shell -- cargo build --workspace`
Expected: Clean build, no warnings.

- [ ] **Step 2: Run all bot tests**

Run: `devenv shell -- cargo test -p rightclaw-bot`
Expected: All tests pass.

- [ ] **Step 3: Verify no leftover `--agent` in cron/delivery**

Run: `rg -- '--agent' crates/bot/src/cron.rs crates/bot/src/cron_delivery.rs`
Expected: No matches.
