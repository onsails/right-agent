# Error Reflection Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** When an agent CC invocation fails (worker safety timeout / non-zero exit, cron timeout / budget / exit), run a short `--resume`-d reflection pass so the agent itself produces a user-friendly summary instead of surfacing raw platform errors.

**Architecture:** A new `crates/bot/src/reflection.rs` module exposes `reflect_on_failure(ctx) -> Result<String, ReflectionError>`. Worker and cron call it on failure; result goes to Telegram (worker) or `notify_json` (cron). Reflection uses `--resume` on the failed session + a `⟨⟨SYSTEM_NOTICE⟩⟩`-wrapped prompt; `OPERATING_INSTRUCTIONS` is extended to teach agents how to handle these markers. Usage is accounted as `source = "reflection"` in `usage_events`, disambiguated by `chat_id` (worker) vs `job_name` (cron).

**Tech Stack:** Rust (edition 2024), tokio, rusqlite, serde, existing `ClaudeInvocation` builder, existing `build_prompt_assembly_script` helper, `TestSandbox` for integration tests.

---

### Task 1: Add "System Notices" block to OPERATING_INSTRUCTIONS template

**Files:**
- Modify: `crates/rightclaw/templates/right/prompt/OPERATING_INSTRUCTIONS.md`

- [ ] **Step 1: Append System Notices section at the end of the template**

Append to the end of `crates/rightclaw/templates/right/prompt/OPERATING_INSTRUCTIONS.md` (after the "Core Skills" section, line 183):

```markdown

## System Notices

Some of your incoming messages may be wrapped in `⟨⟨SYSTEM_NOTICE⟩⟩ … ⟨⟨/SYSTEM_NOTICE⟩⟩`.
These are platform-generated — not user messages. They appear when the platform
needs to inform you of something about your own prior execution (a timeout,
a budget cap, an exit failure, etc.) and ask you to respond with a user-facing
summary.

Rules:
- Follow the instructions inside the notice for the current turn.
- Do NOT quote the `⟨⟨SYSTEM_NOTICE⟩⟩` marker in your reply.
- On subsequent turns, do NOT treat the notice as if the user sent it —
  the user did not see it. They only see your reply.
- Do NOT reflect on, apologize for, or reference the notice in later turns
  unless the user explicitly asks about what happened.
```

- [ ] **Step 2: Verify the template still loads in existing tests**

Run:
```
cargo test -p rightclaw --lib codegen::agent_def
```
Expected: all tests pass. The constant `OPERATING_INSTRUCTIONS` is loaded via `include_str!` and has no structural assertions, so the new block is inert.

- [ ] **Step 3: Commit**

```
git add crates/rightclaw/templates/right/prompt/OPERATING_INSTRUCTIONS.md
git commit -m "docs(prompt): teach agents about ⟨⟨SYSTEM_NOTICE⟩⟩ markers"
```

---

### Task 2: Scaffold `reflection` module with core types

**Files:**
- Create: `crates/bot/src/reflection.rs`
- Modify: `crates/bot/src/lib.rs`

- [ ] **Step 1: Create `crates/bot/src/reflection.rs` with the type definitions**

```rust
//! Error reflection — on a failed CC invocation, run a short `--resume`-d pass
//! so the agent itself produces a user-friendly summary.
//!
//! Callers: `telegram::worker` (interactive) and `cron` (scheduled).
//! See: docs/superpowers/specs/2026-04-21-error-reflection-design.md

use std::path::PathBuf;
use std::time::Duration;
use std::collections::VecDeque;

use crate::telegram::stream::StreamEvent;

/// Classifies the failure we are reflecting on. Drives the human-readable
/// reason text inserted into the SYSTEM_NOTICE prompt.
#[derive(Debug, Clone)]
pub enum FailureKind {
    /// Process was killed by the 600-second safety net in worker.
    SafetyTimeout { limit_secs: u64 },
    /// CC reported `--max-budget-usd` exhaustion.
    BudgetExceeded { limit_usd: f64 },
    /// CC reported `--max-turns` exhaustion.
    MaxTurns { limit: u32 },
    /// Non-zero exit code with no auth-error classification.
    NonZeroExit { code: i32 },
}

/// Discriminator for where the reflection originated — decides how the usage
/// row is written and helps /usage render a breakdown.
#[derive(Debug, Clone)]
pub enum ParentSource {
    Worker { chat_id: i64, thread_id: i64 },
    Cron   { job_name: String },
}

/// Resource caps for a single reflection invocation.
#[derive(Debug, Clone, Copy)]
pub struct ReflectionLimits {
    pub max_turns: u32,
    pub max_budget_usd: f64,
    pub process_timeout: Duration,
}

impl ReflectionLimits {
    pub const WORKER: Self = Self {
        max_turns: 3,
        max_budget_usd: 0.20,
        process_timeout: Duration::from_secs(90),
    };
    pub const CRON: Self = Self {
        max_turns: 5,
        max_budget_usd: 0.40,
        process_timeout: Duration::from_secs(180),
    };
}

/// All inputs required to run one reflection pass.
#[derive(Debug, Clone)]
pub struct ReflectionContext {
    pub session_uuid: String,
    pub failure: FailureKind,
    pub ring_buffer_tail: VecDeque<StreamEvent>,
    pub limits: ReflectionLimits,
    pub agent_name: String,
    pub agent_dir: PathBuf,
    pub ssh_config_path: Option<PathBuf>,
    pub resolved_sandbox: Option<String>,
    pub db_path: PathBuf,
    pub parent_source: ParentSource,
    pub model: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum ReflectionError {
    #[error("reflection spawn failed: {0}")]
    Spawn(String),
    #[error("reflection timed out after {0:?}")]
    Timeout(Duration),
    #[error("reflection CC exited with code {code}: {detail}")]
    NonZeroExit { code: i32, detail: String },
    #[error("reflection output parse failed: {0}")]
    Parse(String),
    #[error("reflection I/O failed: {0}")]
    Io(#[from] std::io::Error),
}
```

- [ ] **Step 2: Wire the module into `crates/bot/src/lib.rs`**

Find the section declaring public modules in `crates/bot/src/lib.rs` (grep for `pub mod`). Add:

```rust
pub mod reflection;
```

Place it alphabetically between existing `pub mod` lines (likely after `pub mod login;` and before `pub mod sync;` or wherever alphabetical order places it).

- [ ] **Step 3: Verify the crate compiles**

Run:
```
cargo check -p rightclaw-bot
```
Expected: clean build, no warnings about unused types (the types are `pub` so usage is not required).

- [ ] **Step 4: Commit**

```
git add crates/bot/src/reflection.rs crates/bot/src/lib.rs
git commit -m "feat(bot): scaffold reflection module with core types"
```

---

### Task 3: Implement `build_reflection_prompt` (TDD)

**Files:**
- Modify: `crates/bot/src/reflection.rs`

- [ ] **Step 1: Write failing tests in `crates/bot/src/reflection.rs`**

Append to `crates/bot/src/reflection.rs`:

```rust
/// Render a human-readable reason text for the SYSTEM_NOTICE header.
pub(crate) fn failure_reason_text(kind: &FailureKind) -> String {
    match kind {
        FailureKind::SafetyTimeout { limit_secs } =>
            format!("hit the {limit_secs}-second safety limit before producing a reply"),
        FailureKind::BudgetExceeded { limit_usd } =>
            format!("exceeded the budget of ${limit_usd:.2}"),
        FailureKind::MaxTurns { limit } =>
            format!("reached the maximum turn count ({limit})"),
        FailureKind::NonZeroExit { code } =>
            format!("Claude process exited with code {code}"),
    }
}

/// Render a short, inlinable description of one ring-buffer event for the
/// "Your most recent activity" list.
pub(crate) fn format_ring_event(event: &StreamEvent) -> Option<String> {
    match event {
        StreamEvent::Text(t) => {
            let trimmed = t.trim();
            if trimmed.is_empty() { return None; }
            let snippet: String = trimmed.chars().take(80).collect();
            Some(format!("- said: {snippet}"))
        }
        StreamEvent::Thinking => Some("- was thinking".to_string()),
        StreamEvent::ToolUse { name, input } => {
            let args: String = input.chars().take(80).collect();
            Some(format!("- called {name}({args})"))
        }
        StreamEvent::Result(_) | StreamEvent::Other => None,
    }
}

/// Build the full stdin prompt for a reflection `claude -p --resume` call.
pub(crate) fn build_reflection_prompt(
    kind: &FailureKind,
    ring_buffer_tail: &VecDeque<StreamEvent>,
    max_turns: u32,
) -> String {
    let reason = failure_reason_text(kind);
    let mut activity = String::new();
    for e in ring_buffer_tail {
        if let Some(line) = format_ring_event(e) {
            activity.push_str(&line);
            activity.push('\n');
        }
    }
    let activity_block = if activity.is_empty() {
        "- (no tool activity recorded)\n".to_string()
    } else {
        activity
    };
    format!(
        "⟨⟨SYSTEM_NOTICE⟩⟩\n\
         \n\
         Your previous turn did not complete successfully.\n\
         \n\
         Reason: {reason}.\n\
         \n\
         Your most recent activity:\n\
         {activity_block}\
         \n\
         Please write a short reply for the user that:\n\
         1. Acknowledges the interruption honestly (1 sentence).\n\
         2. Summarizes what you were doing and any findings worth sharing.\n\
         3. Suggests a concrete next step (narrower scope, different approach,\n\
            or ask for clarification).\n\
         \n\
         Do NOT continue the original investigation — stay within {max_turns} turns.\n\
         Do NOT call Agent or other long-running tools.\n\
         ⟨⟨/SYSTEM_NOTICE⟩⟩\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reason_text_per_kind() {
        assert!(failure_reason_text(&FailureKind::SafetyTimeout { limit_secs: 600 })
            .contains("600-second safety limit"));
        assert!(failure_reason_text(&FailureKind::BudgetExceeded { limit_usd: 2.0 })
            .contains("$2.00"));
        assert!(failure_reason_text(&FailureKind::MaxTurns { limit: 30 })
            .contains("30"));
        assert!(failure_reason_text(&FailureKind::NonZeroExit { code: 137 })
            .contains("137"));
    }

    #[test]
    fn format_ring_event_truncates_text() {
        let ev = StreamEvent::Text("x".repeat(200));
        let out = format_ring_event(&ev).unwrap();
        assert!(out.starts_with("- said: "));
        assert!(out.len() < 120);
    }

    #[test]
    fn format_ring_event_tool_use() {
        let ev = StreamEvent::ToolUse { name: "Read".into(), input: r#"{"path":"/x"}"#.into() };
        let out = format_ring_event(&ev).unwrap();
        assert!(out.contains("called Read"));
        assert!(out.contains("/x"));
    }

    #[test]
    fn format_ring_event_skips_empty_text_and_other() {
        assert!(format_ring_event(&StreamEvent::Text("   ".into())).is_none());
        assert!(format_ring_event(&StreamEvent::Other).is_none());
        assert!(format_ring_event(&StreamEvent::Result("{}".into())).is_none());
    }

    #[test]
    fn prompt_contains_markers_and_reason() {
        let tail = VecDeque::from([
            StreamEvent::ToolUse { name: "Read".into(), input: "{}".into() },
            StreamEvent::Text("partial finding".into()),
        ]);
        let p = build_reflection_prompt(
            &FailureKind::SafetyTimeout { limit_secs: 600 },
            &tail,
            3,
        );
        assert!(p.starts_with("⟨⟨SYSTEM_NOTICE⟩⟩"));
        assert!(p.contains("⟨⟨/SYSTEM_NOTICE⟩⟩"));
        assert!(p.contains("600-second safety limit"));
        assert!(p.contains("called Read"));
        assert!(p.contains("partial finding"));
        assert!(p.contains("stay within 3 turns"));
    }

    #[test]
    fn prompt_handles_empty_ring_buffer() {
        let tail: VecDeque<StreamEvent> = VecDeque::new();
        let p = build_reflection_prompt(
            &FailureKind::NonZeroExit { code: 1 },
            &tail,
            3,
        );
        assert!(p.contains("(no tool activity recorded)"));
    }
}
```

- [ ] **Step 2: Run tests and verify they pass**

```
cargo test -p rightclaw-bot --lib reflection::tests
```
Expected: 5 tests pass.

- [ ] **Step 3: Commit**

```
git add crates/bot/src/reflection.rs
git commit -m "feat(bot): reflection prompt builder + failure-kind formatting"
```

---

### Task 4: Add `insert_reflection` to usage/insert.rs (TDD)

**Files:**
- Modify: `crates/rightclaw/src/usage/insert.rs`

- [ ] **Step 1: Write failing tests**

In `crates/rightclaw/src/usage/insert.rs`, append two tests inside the existing `#[cfg(test)] mod tests` block (after the existing `insert_cron_writes_row_with_null_chat` test):

```rust
#[test]
fn insert_reflection_from_worker_has_chat_id() {
    let dir = tempdir().unwrap();
    let conn = open_connection(dir.path(), true).unwrap();
    insert_reflection_worker(&conn, &sample_breakdown(), 42, 7).unwrap();

    let (source, chat_id, thread_id, job_name): (String, Option<i64>, Option<i64>, Option<String>) =
        conn.query_row(
            "SELECT source, chat_id, thread_id, job_name FROM usage_events LIMIT 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        ).unwrap();
    assert_eq!(source, "reflection");
    assert_eq!(chat_id, Some(42));
    assert_eq!(thread_id, Some(7));
    assert_eq!(job_name, None);
}

#[test]
fn insert_reflection_from_cron_has_job_name() {
    let dir = tempdir().unwrap();
    let conn = open_connection(dir.path(), true).unwrap();
    insert_reflection_cron(&conn, &sample_breakdown(), "my-job").unwrap();

    let (source, chat_id, job_name): (String, Option<i64>, Option<String>) =
        conn.query_row(
            "SELECT source, chat_id, job_name FROM usage_events LIMIT 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        ).unwrap();
    assert_eq!(source, "reflection");
    assert_eq!(chat_id, None);
    assert_eq!(job_name, Some("my-job".to_string()));
}
```

- [ ] **Step 2: Run tests, verify they fail with "cannot find function"**

```
cargo test -p rightclaw --lib usage::insert
```
Expected: compilation error `cannot find function insert_reflection_worker` / `insert_reflection_cron`.

- [ ] **Step 3: Implement the two helpers**

Add to `crates/rightclaw/src/usage/insert.rs`, after the existing `insert_cron` function:

```rust
/// Insert a row for a reflection invocation whose parent was a Telegram worker turn.
pub fn insert_reflection_worker(
    conn: &Connection,
    b: &UsageBreakdown,
    chat_id: i64,
    thread_id: i64,
) -> Result<(), UsageError> {
    insert_row(conn, b, "reflection", Some(chat_id), Some(thread_id), None)
}

/// Insert a row for a reflection invocation whose parent was a cron job.
pub fn insert_reflection_cron(
    conn: &Connection,
    b: &UsageBreakdown,
    job_name: &str,
) -> Result<(), UsageError> {
    insert_row(conn, b, "reflection", None, None, Some(job_name))
}
```

- [ ] **Step 4: Run tests, verify they pass**

```
cargo test -p rightclaw --lib usage::insert
```
Expected: all tests pass, including the two new ones.

- [ ] **Step 5: Commit**

```
git add crates/rightclaw/src/usage/insert.rs
git commit -m "feat(usage): insert_reflection_worker / insert_reflection_cron helpers"
```

---

### Task 5: Implement `reflect_on_failure` main function

**Files:**
- Modify: `crates/bot/src/reflection.rs`

- [ ] **Step 1: Append the main function to `crates/bot/src/reflection.rs`**

```rust
use crate::telegram::invocation::{ClaudeInvocation, OutputFormat};
use rightclaw::usage::insert::{insert_reflection_cron, insert_reflection_worker};
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

/// Run one reflection pass for a failed CC invocation.
///
/// Resumes the failed session, pipes a SYSTEM_NOTICE-wrapped prompt via stdin,
/// parses the final `result` stream event, accounts the usage row, and returns
/// the agent's reply text. Any failure of the reflection itself returns `Err`
/// — the caller is responsible for a raw-error fallback.
pub async fn reflect_on_failure(ctx: ReflectionContext) -> Result<String, ReflectionError> {
    let span = tracing::info_span!(
        "reflection",
        session_uuid = %ctx.session_uuid,
        parent = ?ctx.parent_source,
        failure = ?ctx.failure,
    );
    let _enter = span.enter();

    tracing::info!("reflection starting");

    // 1. Build stdin prompt.
    let input = build_reflection_prompt(
        &ctx.failure,
        &ctx.ring_buffer_tail,
        ctx.limits.max_turns,
    );

    // 2. Read the reply JSON schema (reuse worker's reply schema).
    let schema_path = ctx.agent_dir.join(".claude").join("reply-schema.json");
    let reply_schema = std::fs::read_to_string(&schema_path).map_err(ReflectionError::Io)?;

    // 3. Build the MCP config path.
    let mcp_path = crate::telegram::invocation::mcp_config_path(
        ctx.ssh_config_path.as_deref(),
        &ctx.agent_dir,
    );

    // 4. Construct the ClaudeInvocation (resume, stream-json, budget + turns caps,
    //    Agent disallowed).
    let invocation = ClaudeInvocation {
        mcp_config_path: mcp_path,
        json_schema: reply_schema,
        output_format: OutputFormat::StreamJson,
        model: ctx.model.clone(),
        max_budget_usd: Some(ctx.limits.max_budget_usd),
        max_turns: Some(ctx.limits.max_turns),
        resume_session_id: Some(ctx.session_uuid.clone()),
        new_session_id: None,
        disallowed_tools: vec!["Agent".into()],
        extra_args: vec![],
        prompt: None,
    };
    let claude_args = invocation.into_args();

    // 5. Build the system-prompt assembly script (same as worker — no MCP
    //    instructions refresh, no memory section for reflection; keep base only).
    let (sandbox_mode, home_dir) = if ctx.ssh_config_path.is_some() {
        (rightclaw::agent::types::SandboxMode::Openshell, "/sandbox".to_owned())
    } else {
        (
            rightclaw::agent::types::SandboxMode::None,
            ctx.agent_dir.to_string_lossy().into_owned(),
        )
    };
    let base_prompt =
        rightclaw::codegen::generate_system_prompt(&ctx.agent_name, &sandbox_mode, &home_dir);

    let mut cmd = if let Some(ref ssh_config) = ctx.ssh_config_path {
        let ssh_host = rightclaw::openshell::ssh_host_for_sandbox(
            ctx.resolved_sandbox.as_deref().unwrap(),
        );
        let mut assembly_script = crate::telegram::prompt::build_prompt_assembly_script(
            &base_prompt,
            false, // not bootstrap mode
            "/sandbox",
            "/tmp/rightclaw-reflection-prompt.md",
            "/sandbox",
            &claude_args,
            None, // no MCP instructions refresh
            None, // no memory section
        );
        if let Some(token) = crate::login::load_auth_token(&ctx.db_path) {
            let escaped = token.replace('\'', "'\\''");
            assembly_script =
                format!("export CLAUDE_CODE_OAUTH_TOKEN='{escaped}'\n{assembly_script}");
        }
        let mut c = tokio::process::Command::new("ssh");
        c.arg("-F").arg(ssh_config);
        c.arg(&ssh_host);
        c.arg("--");
        c.arg(assembly_script);
        c
    } else {
        let agent_dir_str = ctx.agent_dir.to_string_lossy();
        let prompt_path = ctx.agent_dir.join(".claude").join("composite-reflection-prompt.md");
        let prompt_path_str = prompt_path.to_string_lossy();
        let assembly_script = crate::telegram::prompt::build_prompt_assembly_script(
            &base_prompt,
            false,
            &agent_dir_str,
            &prompt_path_str,
            &agent_dir_str,
            &claude_args,
            None,
            None,
        );
        let mut c = tokio::process::Command::new("bash");
        c.arg("-c");
        c.arg(&assembly_script);
        c.env("HOME", &ctx.agent_dir);
        c.env("USE_BUILTIN_RIPGREP", "0");
        if let Some(token) = crate::login::load_auth_token(&ctx.db_path) {
            c.env("CLAUDE_CODE_OAUTH_TOKEN", &token);
        }
        c.current_dir(&ctx.agent_dir);
        c
    };
    cmd.stdin(Stdio::piped());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    let mut child = rightclaw::process_group::ProcessGroupChild::spawn(cmd)
        .map_err(|e| ReflectionError::Spawn(format!("{e:#}")))?;

    if let Some(mut stdin) = child.stdin() {
        stdin.write_all(input.as_bytes()).await?;
    }

    // 6. Read stdout streaming, extract the final `result` line; enforce process timeout.
    let stdout = child.stdout()
        .ok_or_else(|| ReflectionError::Spawn("no stdout handle".into()))?;
    let mut lines = BufReader::new(stdout).lines();

    let mut last_result_line: Option<String> = None;
    let read_fut = async {
        while let Ok(Some(line)) = lines.next_line().await {
            if let crate::telegram::stream::StreamEvent::Result(raw) =
                crate::telegram::stream::parse_stream_event(&line)
            {
                last_result_line = Some(raw);
            }
        }
    };

    let timed_out = tokio::time::timeout(ctx.limits.process_timeout, read_fut)
        .await
        .is_err();
    let _ = child.kill().await;
    let exit = child.wait().await.ok().and_then(|s| s.code()).unwrap_or(-1);

    if timed_out {
        tracing::warn!(duration_ms = ctx.limits.process_timeout.as_millis(), "reflection timed out");
        return Err(ReflectionError::Timeout(ctx.limits.process_timeout));
    }

    let result_line = last_result_line.ok_or_else(|| {
        ReflectionError::Parse("no `result` stream event in reflection stdout".into())
    })?;

    if exit != 0 {
        return Err(ReflectionError::NonZeroExit {
            code: exit,
            detail: result_line.chars().take(400).collect(),
        });
    }

    // 7. Parse content out of the result JSON, same shape as worker's reply_schema.
    let parsed: serde_json::Value = serde_json::from_str(&result_line)
        .map_err(|e| ReflectionError::Parse(format!("result JSON: {e}")))?;

    // The `result_line` is the full stream-json `result` event; the agent's
    // final text comes through `result.result` (stringified reply JSON).
    let reply_raw = parsed.get("result").and_then(|v| v.as_str())
        .ok_or_else(|| ReflectionError::Parse("missing `result.result` field".into()))?;
    let reply_json: serde_json::Value = serde_json::from_str(reply_raw)
        .map_err(|e| ReflectionError::Parse(format!("inner reply JSON: {e}")))?;
    let content = reply_json.get("content").and_then(|v| v.as_str())
        .ok_or_else(|| ReflectionError::Parse("missing content field in reply".into()))?
        .to_string();

    // 8. Account usage.
    if let Some(breakdown) = crate::telegram::stream::parse_usage_full(&result_line) {
        let conn = rightclaw::memory::open_connection(&ctx.agent_dir, false)
            .map_err(|e| ReflectionError::Parse(format!("db open: {e:#}")))?;
        let res = match &ctx.parent_source {
            ParentSource::Worker { chat_id, thread_id } =>
                insert_reflection_worker(&conn, &breakdown, *chat_id, *thread_id),
            ParentSource::Cron { job_name } =>
                insert_reflection_cron(&conn, &breakdown, job_name),
        };
        if let Err(e) = res {
            tracing::warn!("reflection usage insert failed: {e:#}");
        }
    }

    tracing::info!(
        cost_usd = parsed.get("total_cost_usd").and_then(|v| v.as_f64()).unwrap_or(0.0),
        turns = parsed.get("num_turns").and_then(|v| v.as_u64()).unwrap_or(0),
        "reflection completed"
    );

    Ok(content)
}
```

- [ ] **Step 2: Verify the module compiles**

```
cargo check -p rightclaw-bot
```
Expected: clean build. If `mcp_config_path` visibility blocks us, open `crates/bot/src/telegram/invocation.rs` and make it `pub(crate)` if it isn't already.

- [ ] **Step 3: Commit**

```
git add crates/bot/src/reflection.rs
git commit -m "feat(bot): implement reflect_on_failure (resume + SYSTEM_NOTICE + usage)"
```

Note: `reflect_on_failure` is exercised end-to-end by the worker and cron integration tasks below, not by a dedicated live-sandbox test. The pure helpers (`build_reflection_prompt`, `failure_reason_text`, `format_ring_event`) are covered by Task 3.

---

### Task 6: Refactor `invoke_cc` return type to `InvokeCcFailure`

**Files:**
- Modify: `crates/bot/src/telegram/worker.rs`

- [ ] **Step 1: Add the `InvokeCcFailure` enum in `worker.rs`**

Open `crates/bot/src/telegram/worker.rs`. Above the `async fn invoke_cc(...)` definition (around line 843), add:

```rust
use std::collections::VecDeque;
use crate::telegram::stream::StreamEvent;
use crate::reflection::FailureKind;

/// Failure classification returned by `invoke_cc` so the caller (`spawn_worker`)
/// can decide between sending the raw message and running a reflection pass.
#[derive(Debug)]
pub enum InvokeCcFailure {
    /// A failure we want to reflect on (safety timeout, non-zero exit).
    Reflectable {
        kind: FailureKind,
        ring_buffer_tail: VecDeque<StreamEvent>,
        session_uuid: String,
        raw_message: String,
    },
    /// A failure we do NOT want to reflect on (parse fail, internal errors,
    /// pre-CC setup errors). The `message` is sent to Telegram unchanged.
    NonReflectable { message: String },
}

impl From<String> for InvokeCcFailure {
    fn from(message: String) -> Self {
        InvokeCcFailure::NonReflectable { message }
    }
}
```

The `From<String>` impl means every existing `return Err("...".into())` / `return Err(format!(...))` / `return Err(format_error_reply(...))` site implicitly produces a `NonReflectable` — minimises churn. We will convert the two specific sites (timeout, non-auth non-zero exit) to `Reflectable` below.

- [ ] **Step 2: Change the function signature**

Find the line (worker.rs:847):

```rust
async fn invoke_cc(
    input: &str,
    first_text: Option<&str>,
    chat_id: i64,
    eff_thread_id: i64,
    is_group: bool,
    ctx: &WorkerContext,
) -> Result<(Option<ReplyOutput>, String), String> {
```

Change the return type to:

```rust
) -> Result<(Option<ReplyOutput>, String), InvokeCcFailure> {
```

- [ ] **Step 3: Convert the timeout Err-site to `Reflectable`**

Find `worker.rs:1420-1432` (the `if timed_out` block producing `Err(timeout_msg)`). Replace with:

```rust
if timed_out {
    let mut timeout_msg = format!(
        "⚠️ Agent timed out ({CC_TIMEOUT_SECS}s safety limit). Last activity:\n─────────────\n"
    );
    for event in ring_buffer.events() {
        if let Some(formatted) = super::stream::format_event(event) {
            timeout_msg.push_str(&formatted);
            timeout_msg.push('\n');
        }
    }
    timeout_msg.push_str(&format!("\nStream log: {}", stream_log_path.display()));
    return Err(InvokeCcFailure::Reflectable {
        kind: FailureKind::SafetyTimeout { limit_secs: CC_TIMEOUT_SECS },
        ring_buffer_tail: ring_buffer.events().clone(),
        session_uuid: session_uuid.clone(),
        raw_message: timeout_msg,
    });
}
```

- [ ] **Step 4: Convert the non-auth non-zero exit Err-site to `Reflectable`**

Find `worker.rs:1499-1508` (the block that builds `error_detail` and returns `Err(format_error_reply(exit_code, &error_detail))`). Replace the trailing `return Err(format_error_reply(...))` with:

```rust
let raw = format_error_reply(exit_code, &error_detail);
return Err(InvokeCcFailure::Reflectable {
    kind: FailureKind::NonZeroExit { code: exit_code },
    ring_buffer_tail: ring_buffer.events().clone(),
    session_uuid: session_uuid.clone(),
    raw_message: raw,
});
```

Leave the other `return Err(...)` / `return Err(format_error_reply(...))` sites above it untouched — they fall through `From<String>` into `NonReflectable`.

- [ ] **Step 5: Update `spawn_worker` to consume the new error type**

Find the match block in `worker.rs:471-475`:

```rust
let (reply_result, session_uuid) =
    match invoke_cc(&input, first_text, chat_id, eff_thread_id, is_group, &ctx).await {
        Ok((output, uuid)) => (Ok(output), uuid),
        Err(e) => (Err(e), String::new()),
    };
```

Change the `reply_result` type and retain both failure kinds:

```rust
let (reply_result, session_uuid) =
    match invoke_cc(&input, first_text, chat_id, eff_thread_id, is_group, &ctx).await {
        Ok((output, uuid)) => (Ok(output), uuid),
        Err(failure) => {
            // Session UUID is embedded in Reflectable variants; for non-reflectable
            // we keep the previous "" sentinel since no session context is useful.
            let uuid = match &failure {
                InvokeCcFailure::Reflectable { session_uuid, .. } => session_uuid.clone(),
                InvokeCcFailure::NonReflectable { .. } => String::new(),
            };
            (Err(failure), uuid)
        }
    };
```

Then update the `Err(err_msg)` arm in the existing match (around worker.rs:639). Currently:

```rust
Err(err_msg) => {
    tracing::info!(?key, "sending error reply to Telegram");
    let mut send = ctx.bot.send_message(tg_chat_id, &err_msg);
    ...
}
```

Replace with:

```rust
Err(InvokeCcFailure::NonReflectable { message }) => {
    tracing::info!(?key, "sending non-reflectable error reply to Telegram");
    send_error_to_telegram(&ctx, tg_chat_id, eff_thread_id, &message).await;
}
Err(InvokeCcFailure::Reflectable { .. }) => {
    // Handled in Task 7 — for now, fall through to raw-send so the crate compiles
    // and existing behavior is preserved.
    unreachable!("Reflectable path is wired up in Task 7")
}
```

Extract the existing inline send-error logic into a private helper `async fn send_error_to_telegram(...)` at the bottom of the file:

```rust
async fn send_error_to_telegram(
    ctx: &WorkerContext,
    tg_chat_id: teloxide::types::ChatId,
    eff_thread_id: i64,
    message: &str,
) {
    let mut send = ctx.bot.send_message(tg_chat_id, message);
    if eff_thread_id != 0 {
        use teloxide::types::{ThreadId, MessageId};
        send = send.message_thread_id(ThreadId(MessageId(eff_thread_id as i32)));
    }
    if let Err(e) = send.await {
        tracing::error!("failed to send error reply: {:#}", e);
    }
}
```

Temporarily, to avoid panicking in production before Task 7 lands, make the `Reflectable` arm fall back to `send_error_to_telegram(&ctx, tg_chat_id, eff_thread_id, &raw_message).await;` instead of `unreachable!`:

```rust
Err(InvokeCcFailure::Reflectable { raw_message, .. }) => {
    // TASK 7 replaces this with a reflection call.
    tracing::warn!(?key, "reflection not yet wired — sending raw error");
    send_error_to_telegram(&ctx, tg_chat_id, eff_thread_id, &raw_message).await;
}
```

- [ ] **Step 6: Fix other callers / error-chain conversions**

Grep for `format_error_reply(` and any `.map_err(|e| format!(...))` sites in `invoke_cc`. The `From<String> for InvokeCcFailure` impl means `?` on `Result<_, String>` works unchanged. If clippy flags "unused Result", use `let _ =` or add `.into()` conversions explicitly. Run:

```
cargo build -p rightclaw-bot
```
Fix any reported mismatches (they will point at specific lines).

- [ ] **Step 7: Run existing worker unit tests**

```
cargo test -p rightclaw-bot --lib telegram::worker::tests
```
Expected: all existing tests still pass. No new tests in this task — behavior unchanged for non-Reflectable paths, Reflectable currently delegates to raw-send.

- [ ] **Step 8: Commit**

```
git add crates/bot/src/telegram/worker.rs
git commit -m "refactor(worker): InvokeCcFailure classifies Reflectable vs NonReflectable errors"
```

---

### Task 7: Wire `reflect_on_failure` into `spawn_worker` with thinking UX

**Files:**
- Modify: `crates/bot/src/telegram/worker.rs`

- [ ] **Step 1: Replace the temporary Reflectable arm with a reflection call**

In `worker.rs`, find the arm added in Task 6:

```rust
Err(InvokeCcFailure::Reflectable { raw_message, .. }) => {
    tracing::warn!(?key, "reflection not yet wired — sending raw error");
    send_error_to_telegram(&ctx, tg_chat_id, eff_thread_id, &raw_message).await;
}
```

Replace with a full reflection flow:

```rust
Err(InvokeCcFailure::Reflectable {
    kind,
    ring_buffer_tail,
    session_uuid: failed_session_uuid,
    raw_message,
}) => {
    // Finalize the old thinking message (if any) as a short neutral banner.
    if let Some(msg_id) = thinking_msg_id_for_fallback {
        let short = match &kind {
            crate::reflection::FailureKind::SafetyTimeout { limit_secs } =>
                format!("⚠️ Hit {limit_secs}s safety limit — thinking again…"),
            _ => "⚠️ Previous turn did not complete — thinking again…".to_string(),
        };
        let _ = ctx.bot.edit_message_text(tg_chat_id, msg_id, &short)
            .parse_mode(teloxide::types::ParseMode::Html)
            .reply_markup(teloxide::types::InlineKeyboardMarkup::default())
            .await;
    }

    // Run reflection.
    let refl_ctx = crate::reflection::ReflectionContext {
        session_uuid: failed_session_uuid,
        failure: kind,
        ring_buffer_tail,
        limits: crate::reflection::ReflectionLimits::WORKER,
        agent_name: ctx.agent_name.clone(),
        agent_dir: ctx.agent_dir.clone(),
        ssh_config_path: ctx.ssh_config_path.clone(),
        resolved_sandbox: ctx.resolved_sandbox.clone(),
        db_path: ctx.db_path.clone(),
        parent_source: crate::reflection::ParentSource::Worker {
            chat_id,
            thread_id: eff_thread_id,
        },
        model: ctx.model.clone(),
    };
    match crate::reflection::reflect_on_failure(refl_ctx).await {
        Ok(reply_text) => {
            tracing::info!(?key, "reflection reply produced");
            let html = super::markdown::md_to_telegram_html(&reply_text);
            let parts = super::markdown::split_html_message(&html);
            for part in &parts {
                let mut send = ctx.bot.send_message(tg_chat_id, part);
                send = send.parse_mode(teloxide::types::ParseMode::Html);
                if eff_thread_id != 0 {
                    use teloxide::types::{ThreadId, MessageId};
                    send = send.message_thread_id(ThreadId(MessageId(eff_thread_id as i32)));
                }
                if let Err(e) = send.await {
                    tracing::warn!("reflection reply send failed, falling back to plain: {e:#}");
                    let plain = strip_html_tags(part);
                    let mut fb = ctx.bot.send_message(tg_chat_id, &plain);
                    if eff_thread_id != 0 {
                        use teloxide::types::{ThreadId, MessageId};
                        fb = fb.message_thread_id(ThreadId(MessageId(eff_thread_id as i32)));
                    }
                    if let Err(e2) = fb.await {
                        tracing::error!("reflection plain-text fallback also failed: {e2:#}");
                    }
                }
            }
        }
        Err(e) => {
            tracing::warn!(?key, "reflection failed: {e:#}; falling back to raw error");
            send_error_to_telegram(&ctx, tg_chat_id, eff_thread_id, &raw_message).await;
        }
    }
}
```

- [ ] **Step 2: Ensure `thinking_msg_id_for_fallback` is in scope**

The existing `thinking_msg_id` is captured in the success branch's finalization (worker.rs:1380-1411). The same value is also needed for the Reflectable branch's "edit to banner" step. Move the binding out of `invoke_cc` into `spawn_worker`, or pass it through.

Simpler: `invoke_cc` already returns the thinking message state inside `Ok((ReplyOutput, session_uuid))` — but for `Err` it's dropped. Since the Reflectable banner-edit runs in `spawn_worker`, `invoke_cc` would need to surface the thinking_msg_id on the error side.

Add a field to the Reflectable variant in `crates/bot/src/telegram/worker.rs` (and the enum definition — `reflection.rs` doesn't need to know):

```rust
pub enum InvokeCcFailure {
    Reflectable {
        kind: FailureKind,
        ring_buffer_tail: VecDeque<StreamEvent>,
        session_uuid: String,
        raw_message: String,
        thinking_msg_id: Option<teloxide::types::MessageId>,  // NEW
    },
    NonReflectable { message: String },
}
```

Populate `thinking_msg_id` from the existing local inside `invoke_cc` when building the Reflectable variant (at both the timeout and non-zero-exit sites).

Update the binding used in the Reflectable match arm: replace `thinking_msg_id_for_fallback` with the value pulled out of the enum, and remove the old `if let Some(msg_id) = thinking_msg_id_for_fallback` locally — the `kind` and `thinking_msg_id` come from the match.

- [ ] **Step 3: Disable automatic Hindsight retain for reflection turns**

Find the auto-retain block in `spawn_worker` (around `worker.rs:651-682`, starting at `if let Some(ref hs) = ctx.hindsight`). The block currently runs for every successful reply. It uses `reply_text_for_retain` — which is only set in `Ok(Some(output))`. Reflection replies are sent in the `Err(Reflectable)` arm and don't touch `reply_text_for_retain`. Confirm by inspection: reflection-produced text must NOT be pushed into Hindsight.

No code change needed in this step — just verify by reading the match block that `reply_text_for_retain` stays `None` on the Reflectable branch. Add a comment above the auto-retain block:

```rust
// reply_text_for_retain is only set on the Ok path; reflection replies are
// intentionally excluded from Hindsight (SYSTEM_NOTICE prompts are platform
// noise, not user-agent conversation).
```

- [ ] **Step 4: Manual smoke test**

```
cargo build -p rightclaw-bot
```
Expected: clean build. Start the bot locally against a dev agent and send a message that triggers a long investigation (e.g., request a deep multi-turn notion search) while reducing `CC_TIMEOUT_SECS` temporarily to 10 to force a timeout. Verify that:

1. The thinking message shows a short banner "⚠️ Hit 10s safety limit — thinking again…"
2. A reflection reply arrives within ~90s
3. Usage row with `source='reflection'` appears in `data.db`

```
sqlite3 ~/.rightclaw/agents/<name>/data.db "SELECT source, chat_id, total_cost_usd FROM usage_events ORDER BY ts DESC LIMIT 5"
```

Restore `CC_TIMEOUT_SECS` to 600 before committing.

- [ ] **Step 5: Commit**

```
git add crates/bot/src/telegram/worker.rs
git commit -m "feat(worker): reflection pass on safety-timeout / non-zero-exit"
```

---

### Task 8: Add `status` to `PendingCronResult` and add `DELIVERY_INSTRUCTION_FAILURE`

**Files:**
- Modify: `crates/bot/src/cron_delivery.rs`

- [ ] **Step 1: Add `status` field to `PendingCronResult`**

Find `PendingCronResult` in `crates/bot/src/cron_delivery.rs` (around line 12):

```rust
pub struct PendingCronResult {
    pub id: String,
    pub job_name: String,
    pub notify_json: String,
    pub summary: String,
    pub finished_at: String,
}
```

Add `pub status: String,`.

- [ ] **Step 2: Extend `fetch_pending` and the per-job variant to select `status`**

Find `fetch_pending` (cron_delivery.rs:23) and the per-job variant (cron_delivery.rs:66). In both, extend the SELECT:

```rust
// Before:
"SELECT id, job_name, notify_json, summary, finished_at FROM cron_runs \
 WHERE status IN ('success', 'failed') AND notify_json IS NOT NULL AND delivered_at IS NULL \
 ORDER BY finished_at ASC LIMIT 1"

// After:
"SELECT id, job_name, notify_json, summary, finished_at, status FROM cron_runs \
 WHERE status IN ('success', 'failed') AND notify_json IS NOT NULL AND delivered_at IS NULL \
 ORDER BY finished_at ASC LIMIT 1"
```

Update the row mapping:

```rust
Ok(PendingCronResult {
    id: row.get(0)?,
    job_name: row.get(1)?,
    notify_json: row.get(2)?,
    summary: row.get(3)?,
    finished_at: row.get(4)?,
    status: row.get(5)?,
})
```

Apply to both `fetch_pending` and `fetch_pending_for_job`.

- [ ] **Step 3: Add `DELIVERY_INSTRUCTION_FAILURE`**

Below the existing `DELIVERY_INSTRUCTION` constant (cron_delivery.rs:113), add:

```rust
const DELIVERY_INSTRUCTION_FAILURE: &str = "\
The cron job below did not complete successfully. The `content` field contains
a platform-generated summary of the failure (produced by the agent's reflection
pass). Relay it to the user in natural prose — you MAY rephrase lightly for
flow with the recent conversation, but keep all factual claims intact. Do not
invent details. Ignore the attachments field.

Here is the YAML report of the cron job:
";
```

Rename the existing `DELIVERY_INSTRUCTION` to `DELIVERY_INSTRUCTION_SUCCESS` for symmetry. Update its single reference inside `format_cron_yaml`.

- [ ] **Step 4: Branch on `status` in `format_cron_yaml`**

Find `format_cron_yaml` (cron_delivery.rs:127). Change the first line from:

```rust
let mut output = String::from(DELIVERY_INSTRUCTION);
```

to:

```rust
let instruction = match pending.status.as_str() {
    "failed" => DELIVERY_INSTRUCTION_FAILURE,
    _        => DELIVERY_INSTRUCTION_SUCCESS,
};
let mut output = String::from(instruction);
```

- [ ] **Step 5: Update existing tests to set `status`**

The test constructor `PendingCronResult { id, job_name, notify_json, ... }` appears in tests around `cron_delivery.rs:708` and later. Add `status: "success".into()` to each literal. Grep for `PendingCronResult {` to find all sites.

- [ ] **Step 6: Add a new test for failure-instruction routing**

Append to the test module:

```rust
#[test]
fn format_cron_yaml_uses_failure_instruction_when_status_failed() {
    let pending = PendingCronResult {
        id: "r1".into(),
        job_name: "watcher".into(),
        notify_json: r#"{"content":"Partial data fetched then hit budget"}"#.into(),
        summary: "failed".into(),
        finished_at: "2026-04-21T10:00:00Z".into(),
        status: "failed".into(),
    };
    let out = format_cron_yaml(&pending, 0);
    assert!(out.contains("did not complete successfully"));
    assert!(!out.contains("send it VERBATIM"));
}

#[test]
fn format_cron_yaml_uses_success_instruction_when_status_success() {
    let pending = PendingCronResult {
        id: "r2".into(),
        job_name: "watcher".into(),
        notify_json: r#"{"content":"BTC up 2%"}"#.into(),
        summary: "ok".into(),
        finished_at: "2026-04-21T10:00:00Z".into(),
        status: "success".into(),
    };
    let out = format_cron_yaml(&pending, 0);
    assert!(out.contains("send it VERBATIM"));
}
```

- [ ] **Step 7: Run the cron_delivery tests**

```
cargo test -p rightclaw-bot --lib cron_delivery
```
Expected: all tests pass, including the two new routing tests.

- [ ] **Step 8: Commit**

```
git add crates/bot/src/cron_delivery.rs
git commit -m "feat(cron-delivery): route failed runs through DELIVERY_INSTRUCTION_FAILURE"
```

---

### Task 9: Call `reflect_on_failure` in cron.rs on failure

**Files:**
- Modify: `crates/bot/src/cron.rs`

- [ ] **Step 1: Identify the failure branch**

In `crates/bot/src/cron.rs`, the failure branch is at lines 584-609 (the `else` of `if exit_status.success()`). It currently constructs a `CronNotify` with `"Cron job X failed (exit code N): …"` as content and persists it to `notify_json`.

- [ ] **Step 2: Wrap that branch with a reflection call**

Replace lines 584-609 with:

```rust
} else {
    // Reflection pass — give the agent a short chance to summarize the failure.
    // Falls back to the raw failure content on reflection failure.
    let exit_str = exit_code.map_or("unknown".to_string(), |c| c.to_string());
    let raw_detail = find_last_result_line(&collected_lines)
        .and_then(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .and_then(|v| v.get("result").and_then(|r| r.as_str()).map(String::from))
        .unwrap_or_else(|| stderr_str.to_string());
    let raw_content = format!(
        "Cron job `{job_name}` failed (exit code {exit_str}):\n{raw_detail}"
    );

    // Classify failure kind for the reflection prompt.
    let kind = classify_cron_failure(exit_code, &raw_detail, &spec);

    // Build a ring buffer tail from the collected lines (best-effort).
    let ring_tail: std::collections::VecDeque<_> = collected_lines
        .iter()
        .rev()
        .take(5)
        .map(|line| crate::telegram::stream::parse_stream_event(line))
        .filter(|e| !matches!(e, crate::telegram::stream::StreamEvent::Other))
        .collect();

    let refl_ctx = crate::reflection::ReflectionContext {
        session_uuid: session_uuid_for_cron.clone(),
        failure: kind,
        ring_buffer_tail: ring_tail,
        limits: crate::reflection::ReflectionLimits::CRON,
        agent_name: agent_name.to_string(),
        agent_dir: agent_dir.clone(),
        ssh_config_path: ssh_config_path.map(PathBuf::from),
        resolved_sandbox: resolved_sandbox.map(String::from),
        db_path: db_path.clone(),
        parent_source: crate::reflection::ParentSource::Cron {
            job_name: job_name.to_string(),
        },
        model: spec.model.clone(),
    };

    let reflected_content = match crate::reflection::reflect_on_failure(refl_ctx).await {
        Ok(text) => text,
        Err(e) => {
            tracing::warn!(job = %job_name, "cron reflection failed: {e:#}");
            raw_content.clone()
        }
    };

    let notify = CronNotify {
        content: reflected_content,
        attachments: None,
    };
    match serde_json::to_string(&notify) {
        Ok(json) => {
            if let Err(e) = conn.execute(
                "UPDATE cron_runs SET summary = ?1, notify_json = ?2, delivery_status = 'pending' WHERE id = ?3",
                rusqlite::params!["failed", json, run_id],
            ) {
                tracing::error!(job = %job_name, "failed to persist reflected failure notify to DB: {e:#}");
            }
        }
        Err(e) => {
            tracing::error!(job = %job_name, "failed to serialize reflected failure notify: {e:#}");
        }
    }
}
```

- [ ] **Step 3: Implement `classify_cron_failure` helper**

Above `run_cron_spec` (or in a private helper section at the top of `cron.rs`), add:

```rust
/// Guess the FailureKind for a cron job based on its exit code, its last result
/// event (if any), and the spec's configured limits.
fn classify_cron_failure(
    exit_code: Option<i32>,
    raw_detail: &str,
    spec: &CronSpec,
) -> crate::reflection::FailureKind {
    let lower = raw_detail.to_ascii_lowercase();
    if lower.contains("max budget") || lower.contains("budget exceeded") {
        return crate::reflection::FailureKind::BudgetExceeded {
            limit_usd: spec.max_budget_usd.unwrap_or(0.0),
        };
    }
    if lower.contains("max turns") || lower.contains("turn limit") {
        return crate::reflection::FailureKind::MaxTurns {
            limit: spec.max_turns.unwrap_or(0),
        };
    }
    crate::reflection::FailureKind::NonZeroExit {
        code: exit_code.unwrap_or(-1),
    }
}

#[cfg(test)]
mod classify_tests {
    use super::*;
    use crate::reflection::FailureKind;

    fn spec() -> CronSpec {
        // Construct a minimal CronSpec; adjust field names to match the actual struct.
        // Substitute the real constructor when implementing.
        todo!("replace with a minimal CronSpec constructor in this crate")
    }
}
```

Note: replace the `todo!()` test stub with an actual minimal `CronSpec` constructor. If `CronSpec` is cheap to build inline, add unit tests covering all three classification branches:

```rust
#[test]
fn classify_budget_exceeded() {
    let s = spec(); // max_budget_usd = Some(2.0)
    let kind = classify_cron_failure(Some(137), "exceeded max budget", &s);
    assert!(matches!(kind, FailureKind::BudgetExceeded { .. }));
}

#[test]
fn classify_max_turns() {
    let s = spec();
    let kind = classify_cron_failure(Some(137), "reached max turns", &s);
    assert!(matches!(kind, FailureKind::MaxTurns { .. }));
}

#[test]
fn classify_other_is_non_zero_exit() {
    let s = spec();
    let kind = classify_cron_failure(Some(1), "something else", &s);
    assert!(matches!(kind, FailureKind::NonZeroExit { code: 1 }));
}
```

If the `CronSpec` struct is complex to construct inline, extract the classification logic to take primitives instead:

```rust
fn classify_cron_failure(
    exit_code: Option<i32>,
    raw_detail: &str,
    max_budget_usd: Option<f64>,
    max_turns: Option<u32>,
) -> crate::reflection::FailureKind { ... }
```

Pick the primitive variant — it is simpler to test.

- [ ] **Step 4: Resolve the `session_uuid_for_cron` binding**

The reflection ctx needs the cron session's UUID. In `cron.rs`, the session ID is generated earlier (grep for `session_id` or `--session-id`). Capture it into a `let session_uuid_for_cron = ...;` local before the `run_cron_spec` call enters its subprocess wait, so it's in scope where the failure branch now lives. If the cron currently uses `--session-id <uuid>` flag, use that same value.

- [ ] **Step 5: Verify compilation**

```
cargo check -p rightclaw-bot
```
Fix any borrow / lifetime issues reported. The reflection call is `async`, so the enclosing function must already be `async` — it is (the parent function uses `.await` on child processes).

- [ ] **Step 6: Run tests**

```
cargo test -p rightclaw-bot --lib cron
```
Expected: existing cron tests pass; new classify tests pass.

- [ ] **Step 7: Commit**

```
git add crates/bot/src/cron.rs
git commit -m "feat(cron): reflection pass on job failure populates notify_json"
```

---

### Task 10: Extend `/usage` with reflection line

**Files:**
- Modify: `crates/rightclaw/src/usage/format.rs`
- Modify: `crates/bot/src/telegram/handler.rs`

- [ ] **Step 1: Extend `AllWindows` with reflection fields**

In `crates/rightclaw/src/usage/format.rs` (line 6), update:

```rust
pub struct AllWindows {
    pub today_interactive:  WindowSummary,
    pub today_cron:         WindowSummary,
    pub today_reflection:   WindowSummary,  // NEW
    pub week_interactive:   WindowSummary,
    pub week_cron:          WindowSummary,
    pub week_reflection:    WindowSummary,  // NEW
    pub month_interactive:  WindowSummary,
    pub month_cron:         WindowSummary,
    pub month_reflection:   WindowSummary,  // NEW
    pub all_interactive:    WindowSummary,
    pub all_cron:           WindowSummary,
    pub all_reflection:     WindowSummary,  // NEW
}
```

- [ ] **Step 2: Update `format_summary_message` and `render_window`**

Thread the reflection summary into `render_window`:

```rust
pub fn format_summary_message(w: &AllWindows, detail: bool) -> String {
    let total_invocations = w.all_interactive.invocations
        + w.all_cron.invocations
        + w.all_reflection.invocations;
    if total_invocations == 0 {
        return "No usage recorded yet.".to_string();
    }

    let total_cost = w.all_interactive.cost_usd + w.all_cron.cost_usd + w.all_reflection.cost_usd;
    let total_sub  = w.all_interactive.subscription_cost_usd
                   + w.all_cron.subscription_cost_usd
                   + w.all_reflection.subscription_cost_usd;
    let total_api  = w.all_interactive.api_cost_usd
                   + w.all_cron.api_cost_usd
                   + w.all_reflection.api_cost_usd;

    let mut out = String::new();
    out.push_str("\u{1f4ca} <b>Usage Summary</b> (UTC)\n\n");
    out.push_str(&render_window("Today",        &w.today_interactive,  &w.today_cron,  &w.today_reflection,  detail));
    out.push_str(&render_window("Last 7 days",  &w.week_interactive,   &w.week_cron,   &w.week_reflection,   detail));
    out.push_str(&render_window("Last 30 days", &w.month_interactive,  &w.month_cron,  &w.month_reflection,  detail));
    out.push_str(&render_window("All time",     &w.all_interactive,    &w.all_cron,    &w.all_reflection,    detail));
    // ... existing total footer unchanged (uses total_cost/sub/api) ...
    out
}
```

Update `render_window` signature and body:

```rust
fn render_window(
    title: &str,
    interactive: &WindowSummary,
    cron: &WindowSummary,
    reflection: &WindowSummary,
    detail: bool,
) -> String {
    let mut s = format!("\u{2501}\u{2501} <b>{}</b> \u{2501}\u{2501}\n", html_escape(title));
    if interactive.invocations == 0 && cron.invocations == 0 && reflection.invocations == 0 {
        s.push_str("(no activity)\n\n");
        return s;
    }
    if interactive.invocations > 0 {
        s.push_str(&render_source("\u{1f4ac} Interactive", interactive, "sessions", detail));
    }
    if cron.invocations > 0 {
        s.push_str(&render_source("\u{23f0} Cron", cron, "runs", detail));
    }
    if reflection.invocations > 0 {
        s.push_str(&render_source("\u{1f9e0} Reflection", reflection, "passes", detail));
    }
    // Web & footer use combined totals — update to include reflection.
    let web_s = interactive.web_search_requests + cron.web_search_requests + reflection.web_search_requests;
    let web_f = interactive.web_fetch_requests  + cron.web_fetch_requests  + reflection.web_fetch_requests;
    if web_s > 0 || web_f > 0 {
        s.push_str(&format!("\u{1f50e} Web: {web_s} searches, {web_f} fetches\n"));
    }
    let sub = interactive.subscription_cost_usd + cron.subscription_cost_usd + reflection.subscription_cost_usd;
    let api = interactive.api_cost_usd         + cron.api_cost_usd         + reflection.api_cost_usd;
    if sub > 0.0 && api > 0.0 {
        s.push_str(&format!("Subscription: {} · API-billed: {}\n", format_cost(sub), format_cost(api)));
    } else if sub > 0.0 {
        s.push_str("Subscription covers this (Claude subscription via setup-token)\n");
    } else if api > 0.0 {
        s.push_str("Billed via API key\n");
    }
    s.push('\n');
    s
}
```

- [ ] **Step 3: Update `build_usage_summary` in handler.rs**

In `crates/bot/src/telegram/handler.rs:1481-1518`, add reflection aggregates:

```rust
let windows = AllWindows {
    today_interactive:  aggregate(&conn, Some(today_start), "interactive") .map_err(|e| miette::miette!("aggregate today/interactive: {e:#}"))?,
    today_cron:         aggregate(&conn, Some(today_start), "cron")        .map_err(|e| miette::miette!("aggregate today/cron: {e:#}"))?,
    today_reflection:   aggregate(&conn, Some(today_start), "reflection")  .map_err(|e| miette::miette!("aggregate today/reflection: {e:#}"))?,
    week_interactive:   aggregate(&conn, Some(week_start),  "interactive") .map_err(|e| miette::miette!("aggregate week/interactive: {e:#}"))?,
    week_cron:          aggregate(&conn, Some(week_start),  "cron")        .map_err(|e| miette::miette!("aggregate week/cron: {e:#}"))?,
    week_reflection:    aggregate(&conn, Some(week_start),  "reflection")  .map_err(|e| miette::miette!("aggregate week/reflection: {e:#}"))?,
    month_interactive:  aggregate(&conn, Some(month_start), "interactive") .map_err(|e| miette::miette!("aggregate month/interactive: {e:#}"))?,
    month_cron:         aggregate(&conn, Some(month_start), "cron")        .map_err(|e| miette::miette!("aggregate month/cron: {e:#}"))?,
    month_reflection:   aggregate(&conn, Some(month_start), "reflection")  .map_err(|e| miette::miette!("aggregate month/reflection: {e:#}"))?,
    all_interactive:    aggregate(&conn, None,              "interactive") .map_err(|e| miette::miette!("aggregate all/interactive: {e:#}"))?,
    all_cron:           aggregate(&conn, None,              "cron")        .map_err(|e| miette::miette!("aggregate all/cron: {e:#}"))?,
    all_reflection:     aggregate(&conn, None,              "reflection")  .map_err(|e| miette::miette!("aggregate all/reflection: {e:#}"))?,
};
```

- [ ] **Step 4: Update any other callers of `AllWindows` / `render_window`**

Grep for `AllWindows {` and `render_window(` across the workspace. The only consumers are `handler.rs::build_usage_summary` (done) and the format-level tests. Update any format-level tests to populate the new fields with `WindowSummary::default()` or similar.

- [ ] **Step 5: Run tests**

```
cargo test -p rightclaw --lib usage::format
cargo test -p rightclaw-bot --lib telegram::handler
```
Expected: all pass.

- [ ] **Step 6: Commit**

```
git add crates/rightclaw/src/usage/format.rs crates/bot/src/telegram/handler.rs
git commit -m "feat(usage): /usage shows separate Reflection line per window"
```

---

### Task 11: Update ARCHITECTURE.md and PROMPT_SYSTEM.md

**Files:**
- Modify: `ARCHITECTURE.md`
- Modify: `PROMPT_SYSTEM.md`

- [ ] **Step 1: Add a "Reflection Primitive" subsection to ARCHITECTURE.md**

Near the "Prompting Architecture" / "Claude Invocation Contract" sections of `ARCHITECTURE.md`, add a new block:

```markdown
### Reflection Primitive

`crates/bot/src/reflection.rs` exposes `reflect_on_failure(ctx) -> Result<String, ReflectionError>`.
On CC invocation failure the worker (`telegram::worker`) and cron (`cron.rs`)
call it to give the agent a short `--resume`-d turn wrapped in
`⟨⟨SYSTEM_NOTICE⟩⟩ … ⟨⟨/SYSTEM_NOTICE⟩⟩`, so the agent produces a human-friendly
summary of the failure instead of the raw ring-buffer dump.

- Worker uses `ReflectionLimits::WORKER` (3 turns, $0.20, 90s process timeout).
  Reflection reply is sent to Telegram directly; on reflection failure, the
  caller falls back to the raw error message.
- Cron uses `ReflectionLimits::CRON` (5 turns, $0.40, 180s process timeout).
  Reflection reply is stored in `cron_runs.notify_json`; `cron_delivery` picks
  it up and relays using `DELIVERY_INSTRUCTION_FAILURE` (non-verbatim).
- `usage_events` rows for reflection use `source = "reflection"`, discriminated
  by `chat_id` (worker) vs `job_name` (cron). `/usage` shows them on a separate
  "Reflection" line.
- Reflection never reflects on itself. Hindsight `memory_retain` is skipped for
  reflection turns.
```

- [ ] **Step 2: Add a SYSTEM_NOTICE subsection to PROMPT_SYSTEM.md**

Near the top-level documentation of how system prompts are assembled, document:

```markdown
### ⟨⟨SYSTEM_NOTICE⟩⟩ Markers

When the platform needs to inject a platform-level message into the agent's
conversation (currently: only error reflection after a CC invocation failure),
it wraps the injected text in `⟨⟨SYSTEM_NOTICE⟩⟩ … ⟨⟨/SYSTEM_NOTICE⟩⟩`. The
agent is taught via `OPERATING_INSTRUCTIONS` that such messages are not from
the user and should be acted on for the current turn but not treated as user
input on subsequent turns.

The primitive lives at `crates/bot/src/reflection.rs`. See ARCHITECTURE.md for
lifecycle details.
```

- [ ] **Step 3: Commit**

```
git add ARCHITECTURE.md PROMPT_SYSTEM.md
git commit -m "docs: reflection primitive + SYSTEM_NOTICE convention"
```

---

## Self-Review

**Spec coverage check:**
- Problem / Goal / Scope: Tasks 1-11 implement the full scope. Parse failure is explicitly out of scope — Task 6's `InvokeCcFailure::NonReflectable` covers it as the pass-through path.
- Reflection Primitive (types + prompt + main function): Tasks 2, 3, 5.
- SYSTEM_NOTICE Prompt: Task 3 (prompt text), Task 1 (agent-side rules).
- OPERATING_INSTRUCTIONS Update: Task 1.
- Worker Integration: Tasks 6, 7 (return type + wiring + UX).
- Worker UX (thinking lifecycle): Task 7, Step 1.
- Cron Integration: Task 9.
- Cron Delivery Branching: Task 8.
- Usage Accounting: Tasks 4 (insert helpers), 10 (render).
- Observability (tracing spans, stream log tagging): Task 5 (spans), no stream-log tagging task — the spec marks this as *preference* not hard requirement, and the worker reuses stream log with session UUID so events are already co-located.
- Reflection-health doctor check: explicitly MVP-optional in spec; no task.
- Invariants: Tasks 5 (no recursion — reflection has no error-retry loop), 7 Step 3 (no Hindsight retain), 8 (delivery does not reflect on itself).
- Tests: pure-unit tests for prompt, reason, ring-event formatting, insert_reflection, cron classification, delivery instruction routing. Live-sandbox integration tests are documented as manual smoke tests in Task 7 (Step 4) because CI does not have stable Claude auth.

**Placeholder scan:** No "TBD" / "TODO" / "implement later" in any step. The `todo!()` in Task 9 Step 3 is flagged explicitly with an instruction to replace it when constructing `CronSpec` inline, and offers a primitive-argument alternative. Acceptable under "show the alternative inline" reading.

**Type consistency:**
- `FailureKind` used identically in `reflection.rs`, `worker.rs`, and `cron.rs`.
- `ParentSource::Worker { chat_id, thread_id }` / `Cron { job_name }` matches `insert_reflection_worker(conn, b, chat_id, thread_id)` / `insert_reflection_cron(conn, b, job_name)`.
- `ReflectionLimits::WORKER` / `::CRON` values match the spec (3/$0.20/90s, 5/$0.40/180s).
- `StreamEvent` imported consistently from `crate::telegram::stream`.
- `ReflectionError` variants align with what `reflect_on_failure` returns.
