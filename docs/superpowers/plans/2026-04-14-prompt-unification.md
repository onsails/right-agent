# Prompt Assembly Unification Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Remove dead `--agent` code, unify system prompt assembly into a single shell template, stop forward-syncing agent-managed files, fix bootstrap file path.

**Architecture:** One `build_prompt_assembly_script()` function generates a parameterized shell script for both sandbox and no-sandbox modes. Dead `.claude/agents/` codegen is removed entirely. Forward sync of agent-managed files is eliminated — sandbox is source of truth.

**Tech Stack:** Rust, shell scripting, shlex (existing dep)

---

### Task 1: Fix bootstrap file path instructions

**Files:**
- Modify: `templates/right/agent/BOOTSTRAP.md`

- [ ] **Step 1: Update bootstrap template**

In `templates/right/agent/BOOTSTRAP.md`, change the "Files to Create" section to explicitly specify the path. Replace:

```markdown
## Files to Create

### IDENTITY.md
```

With:

```markdown
## Files to Create

Write all three files in your current working directory using the Write tool.
Do NOT create them inside `.claude/`, `.claude/agents/`, or any subdirectory.

### IDENTITY.md
```

- [ ] **Step 2: Verify compiled-in constant updates**

Run: `cargo check -p rightclaw 2>&1 | head -5`
Expected: compiles (include_str! re-reads the file at build time)

- [ ] **Step 3: Commit**

```bash
git add templates/right/agent/BOOTSTRAP.md
git commit -m "fix: bootstrap instructions — write identity files to CWD, not .claude/agents/"
```

---

### Task 2: Remove dead agent definition code from `agent_def.rs`

**Files:**
- Modify: `crates/rightclaw/src/codegen/agent_def.rs`
- Modify: `crates/rightclaw/src/codegen/agent_def_tests.rs`
- Modify: `crates/rightclaw/src/codegen/mod.rs`

- [ ] **Step 1: Remove dead functions and constants from `agent_def.rs`**

Delete:
- `CONTENT_MD_FILES` constant (lines 1-14)
- `generate_agent_definition()` function (lines 50-84)
- `generate_bootstrap_definition()` function (lines 86-104)

Keep:
- `OPERATING_INSTRUCTIONS` (lines 16-21)
- `BOOTSTRAP_INSTRUCTIONS` (lines 28-30)
- All schema constants (`REPLY_SCHEMA_JSON`, `BOOTSTRAP_SCHEMA_JSON`, `CRON_SCHEMA_JSON`)
- `generate_system_prompt()` function (lines 106-174)
- Test module declaration (lines 176-178)

After deletion, `agent_def.rs` should contain:
```rust
/// Platform operating instructions, compiled into the binary.
pub const OPERATING_INSTRUCTIONS: &str =
    include_str!("../../../../templates/right/prompt/OPERATING_INSTRUCTIONS.md");

/// Bootstrap instructions, compiled into the binary.
pub const BOOTSTRAP_INSTRUCTIONS: &str =
    include_str!("../../../../templates/right/agent/BOOTSTRAP.md");

/// JSON schema for the structured reply format.
pub const REPLY_SCHEMA_JSON: &str = r#"..."#;

/// JSON schema for bootstrap mode.
pub const BOOTSTRAP_SCHEMA_JSON: &str = r#"..."#;

/// JSON schema for cron job structured output.
pub const CRON_SCHEMA_JSON: &str = r#"..."#;

/// Generate the base system prompt for all agent modes.
pub fn generate_system_prompt(...) -> String { ... }

#[cfg(test)]
#[path = "agent_def_tests.rs"]
mod tests;
```

- [ ] **Step 2: Remove dead tests from `agent_def_tests.rs`**

Delete these tests:
- `agent_definition_has_at_references_in_cache_order` (lines 4-21)
- `agent_definition_model_none_produces_inherit` (lines 23-27)
- `agent_definition_no_embedded_file_content` (lines 29-35)
- `bootstrap_definition_has_only_bootstrap_reference` (lines 37-45)
- `bootstrap_definition_model_none_produces_inherit` (lines 47-51)

Update the import at line 1 — remove `generate_agent_definition`, `generate_bootstrap_definition`:
```rust
use crate::codegen::{generate_system_prompt, BOOTSTRAP_SCHEMA_JSON, REPLY_SCHEMA_JSON};
```

Keep all remaining tests (schema validation, system prompt tests, operating/bootstrap instructions tests).

- [ ] **Step 3: Update re-exports in `codegen/mod.rs`**

Change line 14-17 from:
```rust
pub use agent_def::{
    generate_agent_definition, generate_bootstrap_definition, generate_system_prompt,
    BOOTSTRAP_INSTRUCTIONS, BOOTSTRAP_SCHEMA_JSON, CONTENT_MD_FILES, CRON_SCHEMA_JSON,
    OPERATING_INSTRUCTIONS, REPLY_SCHEMA_JSON,
};
```

To:
```rust
pub use agent_def::{
    generate_system_prompt,
    BOOTSTRAP_INSTRUCTIONS, BOOTSTRAP_SCHEMA_JSON, CRON_SCHEMA_JSON,
    OPERATING_INSTRUCTIONS, REPLY_SCHEMA_JSON,
};
```

- [ ] **Step 4: Verify compilation**

Run: `cargo check -p rightclaw 2>&1 | tail -5`
Expected: compilation errors from downstream crates that still reference deleted items (pipeline.rs, main.rs, sync.rs). This is expected — we fix them in subsequent tasks.

- [ ] **Step 5: Commit**

```bash
git add crates/rightclaw/src/codegen/agent_def.rs crates/rightclaw/src/codegen/agent_def_tests.rs crates/rightclaw/src/codegen/mod.rs
git commit -m "refactor: remove dead agent definition code (generate_agent_definition, CONTENT_MD_FILES)"
```

---

### Task 3: Remove agent def codegen from `pipeline.rs`

**Files:**
- Modify: `crates/rightclaw/src/codegen/pipeline.rs`

- [ ] **Step 1: Remove agent def generation from `run_single_agent_codegen`**

Delete lines 69-114 (agent def generation, bootstrap def, `.claude/agents/` dir creation, CONTENT_MD_FILES copy). This is the block from `// Generate agent definition .md` through `tracing::debug!(agent = %agent.name, "copied content .md files into .claude/agents/");`.

After deletion, `run_single_agent_codegen` should go directly from the settings.json setup (line 62-67) to the reply-schema.json write (line 116-126). Adjust the line that says `tracing::debug!(agent = %agent.name, "wrote agent definitions + schemas");` (line 164) to say `tracing::debug!(agent = %agent.name, "wrote schemas");`.

- [ ] **Step 2: Update tests in `pipeline.rs`**

In `run_single_agent_codegen_generates_all_files` test (line 448): remove these assertions:
```rust
assert!(agent_dir.join(".claude/agents/test.md").exists());
assert!(agent_dir.join(".claude/agents/test-bootstrap.md").exists());
```

Delete the entire `run_agent_codegen_writes_bootstrap_definition` test (lines 569-617) — it tests `.claude/agents/test.md` and `.claude/agents/test-bootstrap.md` creation.

- [ ] **Step 3: Verify compilation**

Run: `cargo check -p rightclaw 2>&1 | tail -5`
Expected: compiles within `rightclaw` crate. Downstream `rightclaw-cli` still has errors (fixed in Task 5).

- [ ] **Step 4: Run rightclaw tests**

Run: `cargo test -p rightclaw -- codegen::pipeline 2>&1 | tail -20`
Expected: remaining pipeline tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/rightclaw/src/codegen/pipeline.rs
git commit -m "refactor: remove agent def generation from codegen pipeline"
```

---

### Task 4: Remove `.claude/agents/` from platform_store

**Files:**
- Modify: `crates/rightclaw/src/platform_store.rs`
- Modify: `crates/rightclaw/src/platform_store_tests.rs`

- [ ] **Step 1: Remove agents dir scanning from `build_manifest`**

In `platform_store.rs`, delete lines 107-135 (the entire block that scans `.claude/agents/` directory). This is the block from `// Agent def files in .claude/agents/` through the closing `}` of the `if agents_dir.exists()` block.

- [ ] **Step 2: Update tests**

In `platform_store_tests.rs`, delete the `build_manifest_skips_agent_owned_files` test (lines 83-95) — it tests `.claude/agents/` scanning which no longer exists.

- [ ] **Step 3: Verify**

Run: `cargo test -p rightclaw -- platform_store 2>&1 | tail -20`
Expected: remaining platform_store tests pass.

- [ ] **Step 4: Commit**

```bash
git add crates/rightclaw/src/platform_store.rs crates/rightclaw/src/platform_store_tests.rs
git commit -m "refactor: remove .claude/agents/ scanning from platform store"
```

---

### Task 5: Remove dead code from `main.rs`

**Files:**
- Modify: `crates/rightclaw-cli/src/main.rs`
- Modify: `crates/rightclaw-cli/tests/cli_integration.rs`

- [ ] **Step 1: Clean up `rightclaw init` (cmd_init)**

In the init block (~lines 556-633), remove:
1. The comment about populating `.claude/agents/` (line 558-559)
2. The `verify_sandbox_files` call and its surrounding code (lines 617-633) that verifies files in `.claude/agents/`. Keep the sandbox creation and everything else.

The `verify_sandbox_files` block to delete:
```rust
let expected_files: Vec<&str> = std::iter::once("right.md")
    .chain(std::iter::once("right-bootstrap.md"))
    .chain(rightclaw::codegen::CONTENT_MD_FILES.iter().copied()
        .filter(|f| agent_dir.join(".claude/agents").join(f).exists()))
    .collect();
tokio::task::block_in_place(|| {
    tokio::runtime::Handle::current().block_on(async {
        rightclaw::openshell::verify_sandbox_files(
            &sb_name,
            &agent_dir.join(".claude/agents"),
            "/sandbox/.claude/agents/",
            &expected_files,
        )
        .await
    })
})?;
```

- [ ] **Step 2: Clean up `rightclaw agent init` (cmd_agent_init)**

Same pattern — remove the `verify_sandbox_files` block (~lines 911-929):
```rust
let agent_def_name = format!("{name}.md");
let bootstrap_def_name = format!("{name}-bootstrap.md");
let expected_files: Vec<&str> = [agent_def_name.as_str(), bootstrap_def_name.as_str()]
    .into_iter()
    .chain(rightclaw::codegen::CONTENT_MD_FILES.iter().copied()
        .filter(|f| agent_dir.join(".claude/agents").join(f).exists()))
    .collect();
tokio::task::block_in_place(|| {
    tokio::runtime::Handle::current().block_on(async {
        rightclaw::openshell::verify_sandbox_files(
            &sb_name,
            &agent_dir.join(".claude/agents"),
            "/sandbox/.claude/agents/",
            &expected_files,
        )
        .await
    })
})?;
```

- [ ] **Step 3: Rewrite `cmd_pair` (agent exec) to use `--system-prompt-file`**

Replace the current `cmd_pair` function body (~lines 2181-2217). The current code generates an agent definition and uses `--agent`. Replace with system prompt assembly + `--system-prompt-file`.

Delete lines 2181-2214 (agent def generation + `--agent` exec). Replace with:

```rust
    // Ensure schemas exist (function may run without prior cmd_up).
    let claude_dir = agent.path.join(".claude");
    std::fs::create_dir_all(&claude_dir)
        .map_err(|e| miette::miette!("failed to create .claude dir for '{}': {e:#}", agent_name))?;
    std::fs::write(claude_dir.join("reply-schema.json"), rightclaw::codegen::REPLY_SCHEMA_JSON)
        .map_err(|e| miette::miette!("failed to write reply-schema.json for '{}': {e:#}", agent_name))?;
    std::fs::write(claude_dir.join("cron-schema.json"), rightclaw::codegen::CRON_SCHEMA_JSON)
        .map_err(|e| miette::miette!("failed to write cron-schema.json for '{}': {e:#}", agent_name))?;

    // Assemble system prompt on host.
    let sandbox_mode = agent.config.as_ref()
        .map(|c| c.sandbox_mode().clone())
        .unwrap_or_default();
    let base_prompt = rightclaw::codegen::generate_system_prompt(&agent.name, &sandbox_mode);
    let mut prompt = base_prompt;
    prompt.push_str("\n## Operating Instructions\n");
    prompt.push_str(rightclaw::codegen::OPERATING_INSTRUCTIONS);
    prompt.push('\n');
    for (file, header) in [
        ("IDENTITY.md", "## Your Identity"),
        ("SOUL.md", "## Your Personality and Values"),
        ("USER.md", "## Your User"),
        ("AGENTS.md", "## Agent Configuration"),
        ("TOOLS.md", "## Environment and Tools"),
    ] {
        if let Ok(content) = std::fs::read_to_string(agent.path.join(file)) {
            prompt.push_str(&format!("\n{header}\n"));
            prompt.push_str(&content);
            prompt.push('\n');
        }
    }
    let prompt_path = claude_dir.join("composite-system-prompt.md");
    std::fs::write(&prompt_path, &prompt)
        .map_err(|e| miette::miette!("failed to write system prompt for '{}': {e:#}", agent_name))?;

    let claude_bin = which::which("claude")
        .or_else(|_| which::which("claude-bun"))
        .map_err(|_| {
            miette::miette!("claude CLI not found in PATH (tried: claude, claude-bun)")
        })?;

    use std::os::unix::process::CommandExt;
    let err = std::process::Command::new(claude_bin)
        .arg("--system-prompt-file")
        .arg(&prompt_path)
        .arg("--dangerously-skip-permissions")
        .current_dir(&agent.path)
        .exec();

    Err(miette::miette!("failed to launch claude: {err}"))
```

Note: `agent exec` is interactive (not `-p` mode), so it doesn't use `--json-schema` or `--mcp-config`. It's a simple interactive session with the system prompt.

- [ ] **Step 4: Update integration tests**

In `cli_integration.rs`, remove these assertions (lines 57-58):
```rust
assert!(agents_dir.join("right.md").exists(), "missing .claude/agents/right.md");
assert!(agents_dir.join("right-bootstrap.md").exists(), "missing .claude/agents/right-bootstrap.md");
```

Also remove the `agents_dir` variable declaration if it's no longer used:
```rust
let agents_dir = claude_dir.join("agents");
```

- [ ] **Step 5: Verify compilation**

Run: `cargo check --workspace 2>&1 | tail -10`
Expected: compiles. The only remaining reference to deleted items is in `sync.rs` (Task 6).

- [ ] **Step 6: Commit**

```bash
git add crates/rightclaw-cli/src/main.rs crates/rightclaw-cli/tests/cli_integration.rs
git commit -m "refactor: remove dead .claude/agents/ code from CLI, rewrite agent exec to use --system-prompt-file"
```

---

### Task 6: Remove forward sync of agent-managed files

**Files:**
- Modify: `crates/bot/src/sync.rs`

- [ ] **Step 1: Remove forward sync from `initial_sync`**

Delete lines 12 (`use rightclaw::codegen::CONTENT_MD_FILES;`).

Delete lines 22-60 (the two blocks that upload files to sandbox):
1. The `CONTENT_MD_FILES` upload loop (lines 22-33)
2. The `AGENTS.md`/`TOOLS.md` "only if missing" upload loop (lines 39-60)

After deletion, `initial_sync` should contain only:
```rust
pub async fn initial_sync(
    agent_dir: &Path,
    sbox: &rightclaw::sandbox_exec::SandboxExec,
) -> miette::Result<()> {
    tracing::info!(sandbox = sbox.sandbox_name(), "sync: initial cycle (blocking)");
    sync_cycle(agent_dir, sbox).await?;

    // Ensure /sandbox/.local/bin is in PATH for agent-installed CLI tools.
    ensure_local_bin_in_path(sbox).await?;

    Ok(())
}
```

- [ ] **Step 2: Remove MEMORY.md from `REVERSE_SYNC_FILES`**

Change `REVERSE_SYNC_FILES` (lines 111-118) from:
```rust
const REVERSE_SYNC_FILES: &[&str] = &[
    "AGENTS.md",
    "TOOLS.md",
    "IDENTITY.md",
    "SOUL.md",
    "USER.md",
    "MEMORY.md",
];
```

To:
```rust
const REVERSE_SYNC_FILES: &[&str] = &[
    "AGENTS.md",
    "TOOLS.md",
    "IDENTITY.md",
    "SOUL.md",
    "USER.md",
];
```

- [ ] **Step 3: Update the integration test**

The `initial_sync_uploads_content_md_files` test (lines 361-499) tests forward sync of content files. This test needs a rewrite since we're removing that functionality.

Replace the test with one that verifies initial_sync runs sync_cycle (platform store) and ensure_local_bin_in_path, but does NOT upload content .md files:

```rust
/// Verify that initial_sync does NOT upload agent-managed .md files.
/// Agent-managed files live in the sandbox — no forward sync.
///
/// Requires: running OpenShell gateway.
#[tokio::test]
async fn initial_sync_does_not_upload_agent_md_files() {
    let sandbox_name = "rightclaw-test-sync-no-forward";

    let mtls_dir = match rightclaw::openshell::preflight_check() {
        rightclaw::openshell::OpenShellStatus::Ready(dir) => dir,
        other => panic!("OpenShell not ready: {other:?}"),
    };

    // Clean up leftover from a previous failed run.
    let mut grpc_client = rightclaw::openshell::connect_grpc(&mtls_dir)
        .await
        .expect("gRPC connect");
    if rightclaw::openshell::sandbox_exists(&mut grpc_client, sandbox_name)
        .await
        .unwrap()
    {
        rightclaw::openshell::delete_sandbox(sandbox_name).await;
        rightclaw::openshell::wait_for_deleted(&mut grpc_client, sandbox_name, 60, 2)
            .await
            .expect("cleanup of leftover sandbox failed");
    }

    // Create a fresh sandbox with minimal policy.
    let policy_dir = tempfile::tempdir().unwrap();
    let policy_path = policy_dir.path().join("policy.yaml");
    std::fs::write(
        &policy_path,
        "\
version: 1
filesystem_policy:
  include_workdir: true
  read_write:
    - /tmp
    - /sandbox
    - /platform
process:
  run_as_user: sandbox
  run_as_group: sandbox
network_policies:
  outbound:
    endpoints:
      - host: \"**.*\"
        port: 443
        protocol: rest
        access: full
        tls: terminate
    binaries:
      - path: \"**\"
",
    )
    .unwrap();

    let _child = rightclaw::openshell::spawn_sandbox(sandbox_name, &policy_path, None)
        .expect("failed to spawn sandbox");
    rightclaw::openshell::wait_for_ready(&mut grpc_client, sandbox_name, 120, 2)
        .await
        .expect("sandbox did not become READY");

    let sandbox_id =
        rightclaw::openshell::resolve_sandbox_id(&mut grpc_client, sandbox_name)
            .await
            .expect("resolve sandbox_id");

    let sbox = rightclaw::sandbox_exec::SandboxExec::new(
        mtls_dir,
        sandbox_name.to_owned(),
        sandbox_id,
    );
    for attempt in 1..=20 {
        match sbox.exec(&["echo", "ready"]).await {
            Ok((out, 0)) if out.trim() == "ready" => break,
            _ if attempt == 20 => panic!("exec not ready after 20 attempts"),
            _ => tokio::time::sleep(std::time::Duration::from_secs(2)).await,
        }
    }

    // Build a fake agent dir with known content .md files on host.
    let agent_dir = tempfile::tempdir().unwrap();
    let root = agent_dir.path();

    // These should NOT be uploaded to sandbox
    std::fs::write(root.join("IDENTITY.md"), "# should not be uploaded\n").unwrap();
    std::fs::write(root.join("AGENTS.md"), "# should not be uploaded\n").unwrap();
    std::fs::write(root.join("TOOLS.md"), "# should not be uploaded\n").unwrap();

    // Minimal .claude/ infrastructure so sync_cycle doesn't fail.
    let claude_dir = root.join(".claude");
    std::fs::create_dir_all(&claude_dir).unwrap();
    std::fs::write(claude_dir.join("settings.json"), "{}").unwrap();
    std::fs::write(claude_dir.join("reply-schema.json"), "{}").unwrap();
    std::fs::write(claude_dir.join("cron-schema.json"), "{}").unwrap();
    std::fs::write(claude_dir.join("bootstrap-schema.json"), "{}").unwrap();
    std::fs::write(
        claude_dir.join("system-prompt.md"),
        "# test system prompt\n",
    )
    .unwrap();
    std::fs::write(root.join("mcp.json"), "{}").unwrap();

    // Run initial_sync.
    initial_sync(root, &sbox)
        .await
        .expect("initial_sync should succeed");

    // Verify that agent-managed files were NOT uploaded.
    for filename in &["IDENTITY.md", "AGENTS.md", "TOOLS.md"] {
        let sandbox_path = format!("/sandbox/{filename}");
        let (_, exit_code) = sbox.exec(&["test", "-f", &sandbox_path]).await
            .unwrap_or_else(|e| panic!("exec test -f {sandbox_path}: {e:#}"));
        assert_ne!(
            exit_code, 0,
            "{filename} should NOT exist in sandbox — forward sync must not upload agent-managed files"
        );
    }

    // Clean up.
    rightclaw::openshell::delete_sandbox(sandbox_name).await;
}
```

- [ ] **Step 4: Verify compilation**

Run: `cargo check --workspace 2>&1 | tail -5`
Expected: clean compilation.

- [ ] **Step 5: Commit**

```bash
git add crates/bot/src/sync.rs
git commit -m "refactor: remove forward sync of agent-managed files, remove MEMORY.md from reverse sync"
```

---

### Task 7: Unify prompt assembly into single shell template

**Files:**
- Modify: `crates/bot/src/telegram/worker.rs`

- [ ] **Step 1: Write the unified `build_prompt_assembly_script` function**

Replace both `build_sandbox_prompt_assembly_script()` (lines 549-611) and `assemble_host_system_prompt()` (lines 617-673) with a single function:

```rust
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
fn build_prompt_assembly_script(
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
```

- [ ] **Step 2: Update callers in `invoke_cc`**

In the sandbox branch (~lines 930-941), replace:
```rust
let assembly_script =
    build_sandbox_prompt_assembly_script(&base_prompt, bootstrap_mode, &claude_args, mcp_instructions.as_deref());
```

With:
```rust
let assembly_script = build_prompt_assembly_script(
    &base_prompt,
    bootstrap_mode,
    "/sandbox",
    "/tmp/rightclaw-system-prompt.md",
    "/sandbox",
    &claude_args,
    mcp_instructions.as_deref(),
);
```

In the no-sandbox branch (~lines 942-969), replace the entire block that calls `assemble_host_system_prompt` and creates a `tokio::process::Command`:

```rust
    } else {
        // No-sandbox: same shell template, but paths point to host agent_dir.
        let agent_dir_str = ctx.agent_dir.to_string_lossy();
        let prompt_path = ctx.agent_dir.join(".claude").join("composite-system-prompt.md");
        let prompt_path_str = prompt_path.to_string_lossy();
        let assembly_script = build_prompt_assembly_script(
            &base_prompt,
            bootstrap_mode,
            &agent_dir_str,
            &prompt_path_str,
            &agent_dir_str,
            &claude_args,
            mcp_instructions.as_deref(),
        );

        let mut c = tokio::process::Command::new("bash");
        c.arg("-c");
        c.arg(&assembly_script);
        c.env("HOME", &ctx.agent_dir);
        c.env("USE_BUILTIN_RIPGREP", "0");
        c.current_dir(&ctx.agent_dir);
        c
    };
```

- [ ] **Step 3: Verify compilation**

Run: `cargo check -p rightclaw-bot 2>&1 | tail -5`
Expected: compiles.

- [ ] **Step 4: Commit**

```bash
git add crates/bot/src/telegram/worker.rs
git commit -m "refactor: unify prompt assembly into single shell template for sandbox and no-sandbox"
```

---

### Task 8: Update tests for unified prompt assembly

**Files:**
- Modify: `crates/bot/src/telegram/worker.rs` (test module)

- [ ] **Step 1: Replace sandbox-specific tests with unified tests**

All existing tests for `build_sandbox_prompt_assembly_script` and `assemble_host_system_prompt` need to be replaced with tests for the new `build_prompt_assembly_script`. The function signature changed (added `root_path`, `prompt_file`, `workdir` params), so all test calls need updating.

Replace the test block (lines 1647-1889) with updated tests. Key changes:
- All calls use `build_prompt_assembly_script` with explicit path params
- Sandbox tests use `root_path="/sandbox"`, `prompt_file="/tmp/rightclaw-system-prompt.md"`, `workdir="/sandbox"`
- Host tests use `root_path="/home/agent"` (or any path), check the script has correct paths
- No more `assemble_host_system_prompt` tests — both modes produce shell scripts now

```rust
#[test]
fn prompt_script_bootstrap_includes_bootstrap_md() {
    let script = build_prompt_assembly_script(
        "Base prompt",
        true,
        "/sandbox",
        "/tmp/rightclaw-system-prompt.md",
        "/sandbox",
        &["claude".into(), "-p".into()],
        None,
    );
    assert!(script.contains("Bootstrap Instructions"), "must have Bootstrap Instructions header");
    assert!(script.contains("First-Time Setup"), "must contain compiled-in bootstrap content");
    assert!(!script.contains("cat /sandbox/IDENTITY.md"), "bootstrap must not cat IDENTITY.md");
    assert!(script.contains("claude"), "must contain claude command");
    assert!(script.contains("--system-prompt-file"), "must pass --system-prompt-file");
}

#[test]
fn prompt_script_normal_includes_all_identity_files() {
    let script = build_prompt_assembly_script(
        "Base prompt",
        false,
        "/sandbox",
        "/tmp/rightclaw-system-prompt.md",
        "/sandbox",
        &["claude".into(), "-p".into()],
        None,
    );
    assert!(script.contains("cat /sandbox/IDENTITY.md"));
    assert!(script.contains("cat /sandbox/SOUL.md"));
    assert!(script.contains("cat /sandbox/USER.md"));
    assert!(script.contains("cat /sandbox/AGENTS.md"));
    assert!(script.contains("cat /sandbox/TOOLS.md"));
    assert!(script.contains("Operating Instructions"));
    assert!(!script.contains("BOOTSTRAP.md"), "normal must not reference BOOTSTRAP.md");
}

#[test]
fn prompt_script_host_mode_uses_host_paths() {
    let script = build_prompt_assembly_script(
        "Base prompt",
        false,
        "/home/user/.rightclaw/agents/right",
        "/home/user/.rightclaw/agents/right/.claude/composite-system-prompt.md",
        "/home/user/.rightclaw/agents/right",
        &["claude".into(), "-p".into()],
        None,
    );
    assert!(script.contains("cat /home/user/.rightclaw/agents/right/IDENTITY.md"));
    assert!(script.contains("cat /home/user/.rightclaw/agents/right/AGENTS.md"));
    assert!(script.contains("> /home/user/.rightclaw/agents/right/.claude/composite-system-prompt.md"));
    assert!(script.contains("cd /home/user/.rightclaw/agents/right"));
}

#[test]
fn prompt_script_escapes_single_quotes_in_base() {
    let script = build_prompt_assembly_script(
        "It's a test",
        true,
        "/sandbox",
        "/tmp/rightclaw-system-prompt.md",
        "/sandbox",
        &["claude".into()],
        None,
    );
    assert!(!script.contains("It's"), "raw single quote must be escaped");
    assert!(script.contains("It"), "content must still be present");
}

#[test]
fn prompt_script_shell_escapes_claude_args() {
    let script = build_prompt_assembly_script(
        "Base",
        false,
        "/sandbox",
        "/tmp/rightclaw-system-prompt.md",
        "/sandbox",
        &["claude".into(), "-p".into(), "--json-schema".into(), r#"{"type":"object"}"#.into()],
        None,
    );
    assert!(script.contains("--json-schema"));
    assert!(script.contains("type"));
}

#[test]
fn prompt_script_writes_to_prompt_file() {
    let script = build_prompt_assembly_script(
        "X",
        false,
        "/sandbox",
        "/tmp/rightclaw-system-prompt.md",
        "/sandbox",
        &["claude".into()],
        None,
    );
    assert!(script.contains("> /tmp/rightclaw-system-prompt.md"));
    assert!(script.contains("--system-prompt-file /tmp/rightclaw-system-prompt.md"));
}

#[test]
fn prompt_script_includes_mcp_instructions() {
    let script = build_prompt_assembly_script(
        "Base",
        false,
        "/sandbox",
        "/tmp/rightclaw-system-prompt.md",
        "/sandbox",
        &["claude".into()],
        Some("# MCP Server Instructions\n\n## composio\n\nConnect with 250+ apps.\n"),
    );
    assert!(script.contains("MCP Server Instructions"));
    assert!(script.contains("composio"));
}

#[test]
fn prompt_script_none_mcp_instructions_omitted() {
    let script = build_prompt_assembly_script(
        "Base",
        false,
        "/sandbox",
        "/tmp/rightclaw-system-prompt.md",
        "/sandbox",
        &["claude".into()],
        None,
    );
    assert!(!script.contains("MCP Server Instructions"));
}

#[test]
fn prompt_script_bootstrap_uses_compiled_constant() {
    let script = build_prompt_assembly_script(
        "Base prompt",
        true,
        "/sandbox",
        "/tmp/rightclaw-system-prompt.md",
        "/sandbox",
        &["claude".into(), "-p".into()],
        None,
    );
    assert!(!script.contains("cat /sandbox"), "bootstrap must not cat any sandbox file");
    assert!(script.contains("First-Time Setup"), "must contain compiled-in bootstrap content");
}

#[test]
fn prompt_script_operating_instructions_before_identity() {
    let script = build_prompt_assembly_script(
        "Base prompt",
        false,
        "/sandbox",
        "/tmp/rightclaw-system-prompt.md",
        "/sandbox",
        &["claude".into()],
        None,
    );
    let op_pos = script.find("Operating Instructions").expect("must have Operating Instructions");
    let id_pos = script.find("IDENTITY.md").expect("must have IDENTITY.md");
    assert!(op_pos < id_pos, "Operating Instructions must come before IDENTITY.md");
}

#[test]
fn prompt_script_has_agent_configuration_section() {
    let script = build_prompt_assembly_script(
        "Base prompt",
        false,
        "/sandbox",
        "/tmp/rightclaw-system-prompt.md",
        "/sandbox",
        &["claude".into()],
        None,
    );
    assert!(script.contains("Agent Configuration"));
    assert!(script.contains("cat /sandbox/AGENTS.md"));
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p rightclaw-bot -- worker 2>&1 | tail -20`
Expected: all tests pass.

- [ ] **Step 3: Commit**

```bash
git add crates/bot/src/telegram/worker.rs
git commit -m "test: update prompt assembly tests for unified shell template"
```

---

### Task 9: Remove `.claude/agents/` from openshell tests

**Files:**
- Modify: `crates/rightclaw/src/openshell_tests.rs`

- [ ] **Step 1: Remove `.claude/agents/` staging setup**

In `openshell_tests.rs` around line 448, the test creates `.claude/agents/test.md` in the staging dir. Remove these lines:

```rust
std::fs::create_dir_all(staging.join(".claude/agents")).unwrap();
std::fs::write(
    staging.join(".claude/agents/test.md"),
    "# test agent def\n",
)
.unwrap();
```

Replace with a simple test file that doesn't reference `.claude/agents/`:
```rust
std::fs::create_dir_all(staging.join(".claude")).unwrap();
std::fs::write(
    staging.join(".claude/settings.json"),
    "{}",
)
.unwrap();
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p rightclaw -- openshell 2>&1 | tail -20`
Expected: passes (this test requires OpenShell running).

- [ ] **Step 3: Commit**

```bash
git add crates/rightclaw/src/openshell_tests.rs
git commit -m "test: remove .claude/agents/ from openshell test staging"
```

---

### Task 10: Update documentation

**Files:**
- Modify: `PROMPT_SYSTEM.md`
- Modify: `ARCHITECTURE.md`

- [ ] **Step 1: Update PROMPT_SYSTEM.md**

Replace the "Prompt Assembly" section (lines 22-34) with:

```markdown
### Unified prompt assembly

A single Rust function `build_prompt_assembly_script()` generates a shell script
parameterized by `root_path`:

- **Sandbox (OpenShell):** `root_path=/sandbox`, executed via SSH
- **No-sandbox:** `root_path=<agent_dir>`, executed via `bash -c`

Both modes produce the same shell script structure. The script reads .md files
from `root_path`, assembles them into a temp file, then runs `claude -p` with
`--system-prompt-file`.

Built by `build_prompt_assembly_script()` in `worker.rs`.
```

In the "File Locations" section, remove any references to `.claude/agents/` for agent def files. Specifically, remove mention of agent def files being deployed to `/platform/`.

In the "Bootstrap Completion Flow" section, add a note:
```markdown
**Important:** Bootstrap instructions explicitly tell the agent to write files
in the current working directory (not `.claude/agents/`).
```

- [ ] **Step 2: Update ARCHITECTURE.md**

In the "Module Map" for rightclaw codegen, update `agent_def.rs` description:
```
├── agent_def.rs    # System prompt generation, compiled-in constants (OPERATING_INSTRUCTIONS, BOOTSTRAP_INSTRUCTIONS), JSON schemas
```

Remove references to `.claude/agents/` from the "Directory Layout" section. Specifically, remove:
```
│   ├── .claude/
│       ├── agents/<name>.md
│       ├── agents/<name>-bootstrap.md
```

In the "Data Flow" / "Agent Lifecycle" section, update the `rightclaw init` and codegen descriptions to remove agent def generation.

In the "OpenShell Sandbox Architecture" / "Platform store" section, remove "Content-addressed agent defs" or similar.

- [ ] **Step 3: Commit**

```bash
git add PROMPT_SYSTEM.md ARCHITECTURE.md
git commit -m "docs: update PROMPT_SYSTEM.md and ARCHITECTURE.md for prompt unification"
```

---

### Task 11: Full workspace build and test

- [ ] **Step 1: Build workspace**

Run: `cargo build --workspace 2>&1 | tail -10`
Expected: clean build, no warnings about dead code.

- [ ] **Step 2: Run all tests**

Run: `cargo test --workspace 2>&1 | tail -30`
Expected: all tests pass.

- [ ] **Step 3: Check for stale references**

Run: `rg '\.claude/agents' crates/ templates/ --type rust 2>&1`
Expected: no results (all references removed).

Run: `rg 'CONTENT_MD_FILES' crates/ 2>&1`
Expected: no results.

Run: `rg 'generate_agent_definition\|generate_bootstrap_definition' crates/ --type rust 2>&1`
Expected: no results.

- [ ] **Step 4: Fix any remaining issues**

If any stale references remain, fix them and commit.
