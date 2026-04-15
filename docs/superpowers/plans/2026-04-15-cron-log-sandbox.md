# Cron Log Sandbox Streaming Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Move cron NDJSON logs from host into sandbox so agents can read them directly via `Read` tool.

**Architecture:** Inject `tee` into the shell command that runs CC, so stdout is duplicated to a file inside the sandbox. Remove all host-side log writing. Add per-job retention (keep last 10 logs). Update SKILL.md and ARCHITECTURE.md.

**Tech Stack:** Rust, shell (tee), SQLite (existing schema, no migration)

---

### Task 1: Inject tee into cron shell command and update log_path

**Files:**
- Modify: `crates/bot/src/cron.rs:109-170` (execute_job — log_path computation)
- Modify: `crates/bot/src/cron.rs:224-281` (execute_job — shell command construction)

The assembly script from `build_prompt_assembly_script` ends with:
```
cd {workdir} && {claude_cmd} --system-prompt-file {prompt_file}
```

We append `| tee {log_path}` to the assembly script string so NDJSON flows to both stdout (bot reads it) and a file inside the sandbox.

- [ ] **Step 1: Update log_path computation**

In `execute_job`, replace the log_path setup (lines 142-150):

```rust
// OLD (lines 142-150):
let log_dir = agent_dir.join("crons").join("logs");
if let Err(e) = std::fs::create_dir_all(&log_dir) {
    tracing::error!(job = %job_name, "failed to create log dir: {e:#}");
    std::fs::remove_file(&lock_path).ok();
    return;
}
let log_path = log_dir.join(format!("{job_name}-{run_id}.txt"));
let log_path_str = log_path.display().to_string();
```

With:

```rust
// Compute sandbox-relative log path (agents read this via Read tool).
// For sandbox mode: /sandbox/crons/logs/{job_name}-{run_id}.ndjson
// For no-sandbox: {agent_dir}/crons/logs/{job_name}-{run_id}.ndjson
let log_filename = format!("{job_name}-{run_id}.ndjson");
let sandbox_log_dir = if ssh_config_path.is_some() {
    "/sandbox/crons/logs".to_owned()
} else {
    agent_dir.join("crons").join("logs").to_string_lossy().into_owned()
};
let log_path_str = format!("{sandbox_log_dir}/{log_filename}");
```

Remove the `create_dir_all` for the host log dir — the `mkdir -p` inside the shell script handles it.

- [ ] **Step 2: Append tee to assembly script in sandbox mode**

In `execute_job`, after building `assembly_script` for sandbox mode (line 237), append tee:

```rust
// After the existing line: assembly_script = format!("export CLAUDE_CODE_OAUTH_TOKEN=...\n{assembly_script}");
// (or after line 234 if no token)

// Inject tee: mkdir log dir + pipe claude output to tee
assembly_script = format!(
    "mkdir -p /sandbox/crons/logs\n{assembly_script} | tee /sandbox/crons/logs/{log_filename}"
);
```

The `mkdir -p` must come before the `cd && claude ...` line. Since `build_prompt_assembly_script` returns `{ printf ... } > prompt_file\ncd workdir && claude ...`, prepending `mkdir -p` and appending `| tee` gives:
```
mkdir -p /sandbox/crons/logs
{ printf ... } > /tmp/rightclaw-system-prompt.md
cd /sandbox && claude -p ... | tee /sandbox/crons/logs/job-run.ndjson
```

- [ ] **Step 3: Append tee to assembly script in no-sandbox mode**

In `execute_job`, after building `assembly_script` for no-sandbox mode (line 268), append tee. Also create the log dir on host since there's no sandbox shell to do it:

```rust
// After line 268 (c.arg(&assembly_script)):
// Create log dir on host for no-sandbox mode
let no_sandbox_log_dir = agent_dir.join("crons").join("logs");
if let Err(e) = std::fs::create_dir_all(&no_sandbox_log_dir) {
    tracing::error!(job = %job_name, "failed to create log dir: {e:#}");
    std::fs::remove_file(&lock_path).ok();
    return;
}
let assembly_script = format!("{assembly_script} | tee {}", no_sandbox_log_dir.join(&log_filename).display());
```

Then pass the modified `assembly_script` to `c.arg()`.

- [ ] **Step 4: Verify build compiles**

Run: `devenv shell -- cargo check -p rightclaw-bot`
Expected: compiles with no errors

- [ ] **Step 5: Commit**

```bash
git add crates/bot/src/cron.rs
git commit -m "feat(cron): tee NDJSON logs into sandbox, update log_path to sandbox path"
```

---

### Task 2: Remove host-side log writing

**Files:**
- Modify: `crates/bot/src/cron.rs:299-365` (stream log + text log writing)

Remove all host-side log file writing. The bot still collects stdout lines in `collected_lines` for parsing, but no longer writes them to files on the host.

- [ ] **Step 1: Remove host-side NDJSON stream log**

Delete lines 303-323 (the `stream_log_dir`, `stream_log_path`, `stream_log` file handle setup):

```rust
// DELETE: stream_log_dir computation (lines 303-308)
// DELETE: create_dir_all for stream_log_dir (lines 309-311)
// DELETE: stream_log_path (line 312)
// DELETE: stream_log file open (lines 313-323)
```

- [ ] **Step 2: Remove stream_log write in the line-reading loop**

Replace lines 325-331:

```rust
// OLD:
let mut collected_lines: Vec<String> = Vec::new();
while let Ok(Some(line)) = lines.next_line().await {
    if let Some(ref mut log) = stream_log {
        let _ = writeln!(log, "{line}");
    }
    collected_lines.push(line);
}
```

With:

```rust
let mut collected_lines: Vec<String> = Vec::new();
while let Ok(Some(line)) = lines.next_line().await {
    collected_lines.push(line);
}
```

- [ ] **Step 3: Remove host-side text log writing**

Delete lines 355-365 (the text log file write):

```rust
// DELETE: "Write text log file (D-04)" block
// DELETE: log_content construction
// DELETE: std::fs::write(&log_path, &log_content)
```

- [ ] **Step 4: Remove the stream_log_path reference in text log**

The text log previously referenced `stream_log_path.display()` in its content. Since we removed both, nothing references it. Verify no remaining references to `stream_log_path` or `stream_log`.

- [ ] **Step 5: Remove unused import if writeln is no longer used**

Check if `writeln!` macro usage was removed. If the `use std::fmt::Write;` or similar import existed only for stream_log writes, remove it. Check top of file for `use std::io::Write` — if only used by the removed stream_log write, remove it.

- [ ] **Step 6: Verify build compiles**

Run: `devenv shell -- cargo check -p rightclaw-bot`
Expected: compiles with no errors (and no unused warnings from removed code)

- [ ] **Step 7: Commit**

```bash
git add crates/bot/src/cron.rs
git commit -m "refactor(cron): remove host-side NDJSON and text log writing"
```

---

### Task 3: Add log retention (keep last 10 per job)

**Files:**
- Modify: `crates/bot/src/cron.rs` (add retention function + call after job completion)

After each cron run, delete old log files for the same job, keeping the 10 most recent.

For sandbox mode, retention runs via SSH command. For no-sandbox, directly on filesystem.

- [ ] **Step 1: Write the retention function for no-sandbox mode**

Add a new function in `cron.rs`:

```rust
/// Delete old cron log files for a job, keeping the most recent `keep` files.
/// For no-sandbox mode: operates directly on the filesystem.
/// For sandbox mode: runs cleanup via SSH.
async fn cleanup_old_logs(
    job_name: &str,
    log_dir: &str,
    keep: usize,
    ssh_config_path: Option<&std::path::Path>,
    agent_name: &str,
) {
    if let Some(ssh_config) = ssh_config_path {
        // Sandbox mode: run cleanup via SSH
        let ssh_host = rightclaw::openshell::ssh_host(agent_name);
        // List files sorted by mtime (oldest first), skip the newest `keep`, delete the rest
        let cleanup_cmd = format!(
            "ls -1t {log_dir}/{job_name}-*.ndjson 2>/dev/null | tail -n +{} | xargs rm -f",
            keep + 1
        );
        let output = tokio::process::Command::new("ssh")
            .arg("-F").arg(ssh_config)
            .arg(&ssh_host)
            .arg("--")
            .arg(&cleanup_cmd)
            .output()
            .await;
        match output {
            Ok(o) if !o.status.success() => {
                tracing::warn!(
                    job = %job_name,
                    "log cleanup via SSH failed: {}",
                    String::from_utf8_lossy(&o.stderr)
                );
            }
            Err(e) => {
                tracing::warn!(job = %job_name, "log cleanup SSH command failed: {e:#}");
            }
            _ => {}
        }
    } else {
        // No-sandbox mode: direct filesystem cleanup
        let pattern = format!("{job_name}-");
        let dir = match std::fs::read_dir(log_dir) {
            Ok(d) => d,
            Err(_) => return,
        };
        let mut files: Vec<std::path::PathBuf> = dir
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with(&pattern) && n.ends_with(".ndjson"))
            })
            .collect();
        // Sort by mtime descending (newest first)
        files.sort_by(|a, b| {
            let ma = a.metadata().and_then(|m| m.modified()).ok();
            let mb = b.metadata().and_then(|m| m.modified()).ok();
            mb.cmp(&ma)
        });
        // Delete everything after the first `keep`
        for old in files.into_iter().skip(keep) {
            if let Err(e) = std::fs::remove_file(&old) {
                tracing::warn!(job = %job_name, path = %old.display(), "failed to delete old log: {e:#}");
            }
        }
    }
}
```

- [ ] **Step 2: Call retention after job completion**

In `execute_job`, after the lock file deletion (line 384: `std::fs::remove_file(&lock_path).ok();`), add:

```rust
// Retention: keep last 10 log files per job
cleanup_old_logs(job_name, &sandbox_log_dir, 10, ssh_config_path, agent_name).await;
```

`sandbox_log_dir` is already computed in Task 1 Step 1.

- [ ] **Step 3: Verify build compiles**

Run: `devenv shell -- cargo check -p rightclaw-bot`
Expected: compiles with no errors

- [ ] **Step 4: Commit**

```bash
git add crates/bot/src/cron.rs
git commit -m "feat(cron): add log retention — keep last 10 logs per job"
```

---

### Task 4: Update SKILL.md

**Files:**
- Modify: `skills/rightcron/SKILL.md`

Replace `cat <log_path>` with `Read` tool usage. Add "Watching a running job" section. Emphasize reading the tail for large logs.

- [ ] **Step 1: Replace the "Reading logs" subsection**

Replace the current "Reading logs" subsection (under "Checking Run History"):

```markdown
### Reading logs

The `log_path` field in each run record points to the NDJSON log file inside the agent's working directory. Read the tail to see recent activity:

```
Read(file_path: "<log_path>")
```

For large logs, prefer reading just the tail — use a high `offset` value to skip to the end.
```

- [ ] **Step 2: Add "Watching a Running Job" section**

Add a new section after "Checking Run History":

```markdown
## Watching a Running Job

To see what a cron job is currently doing:

1. Find the running job:
```
mcp__right__cron_list_runs(job_name="health-check", limit=1)
```
Check the `status` field — `"running"` means the job is active.

2. Read the tail of the log file to see current activity:
```
Read(file_path: "<log_path from step 1>")
```
The log is NDJSON (one JSON event per line) — look for `"type": "assistant"` events to see what the job is doing.

3. To follow progress, read the tail again after some time has passed.
```

- [ ] **Step 3: Update the debugging example**

Replace the current debugging example:

```markdown
### Debugging example

```
User: "Why did morning-briefing fail?"

1. mcp__right__cron_list_runs(job_name="morning-briefing", limit=5)
   -> Find the failed run (status="failed")
2. mcp__right__cron_show_run(run_id="<run_id from step 1>")
   -> Get full metadata including log_path
3. Read(file_path: "<log_path>", offset: -200)
   -> Read the tail of the log to diagnose the failure
```
```

- [ ] **Step 4: Update version**

Bump version to `3.1.0` in the frontmatter.

- [ ] **Step 5: Commit**

```bash
git add skills/rightcron/SKILL.md
git commit -m "docs(skill): update rightcron — Read logs from sandbox, add watching section"
```

---

### Task 5: Update ARCHITECTURE.md

**Files:**
- Modify: `ARCHITECTURE.md`

Two changes: (1) clarify sandbox lifetime, (2) update stream logging section.

- [ ] **Step 1: Clarify sandbox lifetime**

Find the line (around line 165):
```
Sandboxes are **persistent** — never deleted automatically. Survive bot restarts.
```

Replace with:
```
Sandboxes are **persistent** — never deleted automatically. They live as long as the agent lives and survive bot restarts.
```

- [ ] **Step 2: Update Stream Logging section**

Find the "Stream Logging" paragraph (around line 305-307):
```
CC is invoked with `--verbose --output-format stream-json`. Worker reads stdout
line-by-line via `tokio::io::AsyncBufReadExt`. Each event is written to a per-session
NDJSON log at `~/.rightclaw/logs/streams/<session-uuid>.ndjson`.
```

Replace with:
```
CC is invoked with `--verbose --output-format stream-json`. Worker reads stdout
line-by-line via `tokio::io::AsyncBufReadExt`. For cron jobs, stdout is tee'd into
an NDJSON log inside the sandbox at `/sandbox/crons/logs/{job_name}-{run_id}.ndjson`
(agents can read these directly via `Read`). Per-job retention keeps the last 10 logs.
Worker sessions do not write stream logs.
```

- [ ] **Step 3: Update Directory Layout**

In the runtime directory layout section, remove `crons/*.yaml` if it's only the old format, and update the logs references. Find:
```
│   ├── crons/*.yaml
```
Keep it (cron specs may still use yaml). But check for any `logs/streams/` reference in the layout and remove it if present.

- [ ] **Step 4: Commit**

```bash
git add ARCHITECTURE.md
git commit -m "docs: update ARCHITECTURE.md — sandbox persistence, cron logs in sandbox"
```

---

### Task 6: Full build and test

**Files:**
- None (verification only)

- [ ] **Step 1: Full workspace build**

Run: `devenv shell -- cargo build --workspace`
Expected: builds successfully

- [ ] **Step 2: Run all tests**

Run: `devenv shell -- cargo test --workspace`
Expected: all tests pass

- [ ] **Step 3: Run clippy**

Run: `devenv shell -- cargo clippy --workspace -- -D warnings`
Expected: no warnings
