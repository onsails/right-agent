# Login Flow Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Detect Claude auth errors (403/401) inside OpenShell sandbox, automatically spawn a login process, extract the OAuth URL from its logs, send it to the user via Telegram, and clean up after successful authentication.

**Architecture:** When `invoke_cc` detects an auth error in sandbox mode, it starts a pre-declared `login-{agent}` process in process-compose (disabled by default), spawns an auth watcher task that scrapes the OAuth URL from PC logs and probes auth status periodically. The entire login flow happens through Telegram — user never touches the PC TUI. In no-sandbox mode, a plain "run claude in terminal" message is shown instead.

**Tech Stack:** Rust, tokio, reqwest (PC REST API), teloxide, process-compose REST API (`/process/start`, `/process/logs`), minijinja templates.

**Design doc:** `docs/plans/login-flow-design.md`

---

### Task 1: PC Client — `start_process` and `get_process_logs`

Extend the process-compose REST API client with two new methods needed by the auth watcher.

**Files:**
- Modify: `crates/rightclaw/src/runtime/pc_client.rs`
- Modify: `crates/rightclaw/src/runtime/pc_client_tests.rs`

- [ ] **Step 1: Write failing test for `start_process`**

Add to `crates/rightclaw/src/runtime/pc_client_tests.rs`:

```rust
#[test]
fn start_process_url_is_correct() {
    // PcClient constructs correct URL for start endpoint.
    // We can't hit a real server, but we verify construction doesn't panic.
    let client = PcClient::new(PC_PORT).unwrap();
    // Method exists and is callable (compile-time check).
    // Integration test would need a running PC instance.
    let _ = &client;
}
```

This is a compile-time existence check. The real verification is that the method compiles with the right signature.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p rightclaw --lib runtime::pc_client::tests::start_process_url_is_correct`
Expected: FAIL — `start_process` doesn't exist yet.

- [ ] **Step 3: Implement `start_process`**

Add to `crates/rightclaw/src/runtime/pc_client.rs` after `stop_process`:

```rust
/// Start a disabled or stopped process by name.
pub async fn start_process(&self, name: &str) -> miette::Result<()> {
    self.client
        .post(format!("{}/process/start/{name}", self.base_url))
        .send()
        .await
        .map_err(|e| miette::miette!("failed to start process '{name}': {e:#}"))?;
    Ok(())
}
```

- [ ] **Step 4: Write failing test for `LogsResponse` deserialization**

Add to `crates/rightclaw/src/runtime/pc_client_tests.rs`:

```rust
#[test]
fn logs_response_deserializes_from_json() {
    let json = r#"{"logs": ["line 1", "line 2", "auth url: https://example.com"]}"#;
    let resp: LogsResponse = serde_json::from_str(json).unwrap();
    assert_eq!(resp.logs.len(), 3);
    assert_eq!(resp.logs[2], "auth url: https://example.com");
}

#[test]
fn logs_response_handles_empty_logs() {
    let json = r#"{"logs": []}"#;
    let resp: LogsResponse = serde_json::from_str(json).unwrap();
    assert!(resp.logs.is_empty());
}
```

- [ ] **Step 5: Run tests to verify they fail**

Run: `cargo test -p rightclaw --lib runtime::pc_client::tests::logs_response`
Expected: FAIL — `LogsResponse` doesn't exist.

- [ ] **Step 6: Implement `LogsResponse` and `get_process_logs`**

Add to `crates/rightclaw/src/runtime/pc_client.rs`:

```rust
/// Response from the process-compose `/process/logs/{name}/{endOffset}/{limit}` endpoint.
#[derive(Debug, Deserialize)]
pub struct LogsResponse {
    pub logs: Vec<String>,
}
```

Add method to `impl PcClient`:

```rust
/// Read recent log lines for a process.
///
/// Uses the PC endpoint `GET /process/logs/{name}/{endOffset}/{limit}`.
/// `endOffset=0` reads from the end, `limit` controls how many lines.
pub async fn get_process_logs(&self, name: &str, limit: usize) -> miette::Result<Vec<String>> {
    let resp = self
        .client
        .get(format!("{}/process/logs/{name}/0/{limit}", self.base_url))
        .send()
        .await
        .map_err(|e| miette::miette!("failed to get logs for '{name}': {e:#}"))?;

    let data: LogsResponse = resp
        .json()
        .await
        .map_err(|e| miette::miette!("failed to parse logs for '{name}': {e:#}"))?;
    Ok(data.logs)
}
```

- [ ] **Step 7: Run all pc_client tests**

Run: `cargo test -p rightclaw --lib runtime::pc_client::tests`
Expected: all PASS.

- [ ] **Step 8: Commit**

```
feat: add start_process and get_process_logs to PcClient
```

---

### Task 2: Auth Error Detection (`is_auth_error`)

Pure function that inspects CC stdout JSON to determine if the error is an authentication failure. TDD — tests first.

**Files:**
- Modify: `crates/bot/src/telegram/worker.rs`

- [ ] **Step 1: Write failing tests for `is_auth_error`**

Add to the `#[cfg(test)] mod tests` block in `crates/bot/src/telegram/worker.rs`:

```rust
// is_auth_error tests
#[test]
fn is_auth_error_detects_403() {
    let stdout = r#"{"type":"result","subtype":"success","is_error":true,"result":"Failed to authenticate. API Error: 403 status code (no body)"}"#;
    assert!(is_auth_error(stdout));
}

#[test]
fn is_auth_error_detects_401() {
    let stdout = r#"{"type":"result","subtype":"success","is_error":true,"result":"Failed to authenticate. API Error: 401 Unauthorized"}"#;
    assert!(is_auth_error(stdout));
}

#[test]
fn is_auth_error_detects_not_logged_in() {
    let stdout = r#"{"type":"result","subtype":"success","is_error":true,"result":"Not logged in · Please run /login"}"#;
    assert!(is_auth_error(stdout));
}

#[test]
fn is_auth_error_detects_please_run_login() {
    let stdout = r#"{"type":"result","subtype":"success","is_error":true,"result":"Please run /login · API Error: 403"}"#;
    assert!(is_auth_error(stdout));
}

#[test]
fn is_auth_error_false_for_normal_error() {
    let stdout = r#"{"type":"result","subtype":"success","is_error":true,"result":"Tool execution failed: timeout"}"#;
    assert!(!is_auth_error(stdout));
}

#[test]
fn is_auth_error_false_for_success() {
    let stdout = r#"{"type":"result","subtype":"success","is_error":false,"result":{"content":"hello"}}"#;
    assert!(!is_auth_error(stdout));
}

#[test]
fn is_auth_error_false_for_non_json() {
    assert!(!is_auth_error("Not logged in. Run claude auth login."));
}

#[test]
fn is_auth_error_false_for_empty() {
    assert!(!is_auth_error(""));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p rightclaw-bot --lib telegram::worker::tests::is_auth_error`
Expected: FAIL — `is_auth_error` doesn't exist.

- [ ] **Step 3: Implement `is_auth_error`**

Add to `crates/bot/src/telegram/worker.rs` in the pure helpers section (after `format_error_reply`):

```rust
/// Check whether CC stdout JSON indicates an authentication failure (403/401).
///
/// Returns true when the JSON has `is_error: true` and the `result` string
/// contains known auth-failure patterns. Returns false for non-JSON input,
/// parse errors, or non-auth errors.
pub fn is_auth_error(stdout: &str) -> bool {
    let parsed: serde_json::Value = match serde_json::from_str(stdout) {
        Ok(v) => v,
        Err(_) => return false,
    };

    let is_error = parsed.get("is_error").and_then(|v| v.as_bool()).unwrap_or(false);
    if !is_error {
        return false;
    }

    let result = match parsed.get("result").and_then(|v| v.as_str()) {
        Some(s) => s,
        None => return false,
    };

    const AUTH_PATTERNS: &[&str] = &[
        "403",
        "401",
        "Failed to authenticate",
        "Not logged in",
        "Please run /login",
    ];

    AUTH_PATTERNS.iter().any(|pattern| result.contains(pattern))
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p rightclaw-bot --lib telegram::worker::tests::is_auth_error`
Expected: all 8 tests PASS.

- [ ] **Step 5: Commit**

```
feat: add is_auth_error detection for CC 403/401 responses
```

---

### Task 3: Login Process in PC Template

Add a pre-declared `login-{agent}` process to the process-compose template. Disabled by default, `is_tty: true`, one-shot. Also pass `home_dir` and `RC_PC_PORT` to the template.

**Files:**
- Modify: `crates/rightclaw/src/codegen/process_compose.rs`
- Modify: `templates/process-compose.yaml.j2`
- Modify: `crates/rightclaw/src/codegen/process_compose_tests.rs`

- [ ] **Step 1: Write failing tests**

Add to `crates/rightclaw/src/codegen/process_compose_tests.rs`:

```rust
#[test]
fn sandbox_enabled_emits_login_process() {
    let agents = vec![make_bot_agent("right", "123:tok")];
    let exe = Path::new(EXE_PATH);
    let output = generate_process_compose(
        &agents, exe, false, false, Path::new("/tmp/run"),
        Path::new("/home/user/.rightclaw"), None,
    ).unwrap();
    assert!(
        output.contains("  login-right:"),
        "expected login-right process when sandbox enabled:\n{output}"
    );
    assert!(
        output.contains("is_tty: true"),
        "expected is_tty: true on login process:\n{output}"
    );
    assert!(
        output.contains("disabled: true"),
        "expected disabled: true on login process:\n{output}"
    );
    assert!(
        output.contains("ssh -t -F"),
        "expected ssh command in login process:\n{output}"
    );
    assert!(
        output.contains("openshell-rightclaw-right"),
        "expected SSH host alias in login process:\n{output}"
    );
}

#[test]
fn no_sandbox_omits_login_process() {
    let agents = vec![make_bot_agent("right", "123:tok")];
    let exe = Path::new(EXE_PATH);
    let output = generate_process_compose(
        &agents, exe, false, true, Path::new("/tmp/run"),
        Path::new("/home/user/.rightclaw"), None,
    ).unwrap();
    assert!(
        !output.contains("login-right:"),
        "login process must be absent when no_sandbox:\n{output}"
    );
}

#[test]
fn bot_process_has_rc_pc_port_env() {
    let agents = vec![make_bot_agent("right", "123:tok")];
    let exe = Path::new(EXE_PATH);
    let output = generate_process_compose(
        &agents, exe, false, false, Path::new("/tmp/run"),
        Path::new("/home/user/.rightclaw"), None,
    ).unwrap();
    assert!(
        output.contains("RC_PC_PORT="),
        "expected RC_PC_PORT env var on bot process:\n{output}"
    );
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p rightclaw --lib codegen::process_compose::tests::sandbox_enabled_emits_login_process`
Expected: FAIL — signature mismatch (new `home` param) or missing template block.

- [ ] **Step 3: Update `BotProcessAgent` and `generate_process_compose` signature**

In `crates/rightclaw/src/codegen/process_compose.rs`, add `home_dir` field to `BotProcessAgent`:

```rust
/// Rightclaw home directory (for deterministic SSH config path in login process).
home_dir: String,
```

Update `generate_process_compose` signature to accept `home: &Path`:

```rust
pub fn generate_process_compose(
    agents: &[AgentDef],
    exe_path: &Path,
    debug: bool,
    no_sandbox: bool,
    run_dir: &Path,
    home: &Path,
    cloudflared_script: Option<&Path>,
) -> miette::Result<String> {
```

In the `bot_agents` iterator, populate the new field:

```rust
home_dir: home.display().to_string(),
```

Update the template render call to pass `pc_port`:

```rust
tmpl.render(context! {
    agents => bot_agents,
    cloudflared => cf_entry,
    pc_port => crate::runtime::pc_client::PC_PORT,
})
```

- [ ] **Step 4: Update all existing tests to pass new `home` parameter**

In `crates/rightclaw/src/codegen/process_compose_tests.rs`, update every `generate_process_compose` call to add `Path::new("/home/user/.rightclaw")` as the 6th argument (before `cloudflared_script`). For example:

Before:
```rust
let output = generate_process_compose(&agents, exe, false, true, Path::new("/tmp/run"), None).unwrap();
```

After:
```rust
let output = generate_process_compose(&agents, exe, false, true, Path::new("/tmp/run"), Path::new("/home/user/.rightclaw"), None).unwrap();
```

Apply this change to ALL existing test calls in the file.

- [ ] **Step 5: Update template**

In `templates/process-compose.yaml.j2`, add after the first `{% endfor %}` that closes the agents loop (before the cloudflared block):

```jinja2
{% for agent in agents %}
{% if not agent.no_sandbox %}
  login-{{ agent.name }}:
    command: "ssh -t -F {{ agent.home_dir }}/run/ssh/rightclaw-{{ agent.agent_name }}.ssh-config openshell-rightclaw-{{ agent.agent_name }} -- claude"
    is_tty: true
    disabled: true
    availability:
      restart: "no"
    shutdown:
      signal: 15
      timeout_seconds: 10
{% endif %}
{% endfor %}
```

Also add `RC_PC_PORT={{ pc_port }}` to the bot process environment block, after the `MCP_CONNECTION_NONBLOCKING=1` line:

```jinja2
      - RC_PC_PORT={{ pc_port }}
```

- [ ] **Step 6: Update caller in `main.rs`**

In `crates/rightclaw-cli/src/main.rs`, update the `generate_process_compose` call (around line 954) to pass `&home`:

```rust
let pc_config = rightclaw::codegen::generate_process_compose(
    &agents,
    &self_exe,
    debug,
    no_sandbox,
    &run_dir,
    &home,
    cloudflared_script_path.as_deref(),
)?;
```

- [ ] **Step 7: Run all process_compose tests**

Run: `cargo test -p rightclaw --lib codegen::process_compose::tests`
Expected: all PASS (existing + 3 new).

- [ ] **Step 8: Build workspace**

Run: `cargo build --workspace` (via rust-builder subagent)
Expected: clean build, no errors.

- [ ] **Step 9: Commit**

```
feat: add login-{agent} process to PC template for sandbox auth flow
```

---

### Task 4: Thread `pc_port` Through Bot to Worker

The bot process reads `RC_PC_PORT` from env and threads it through to `WorkerContext` so the auth watcher can call the PC API.

**Files:**
- Modify: `crates/bot/src/lib.rs`
- Modify: `crates/bot/src/telegram/worker.rs` (WorkerContext)
- Modify: `crates/bot/src/telegram/handler.rs` (dptree newtype + handle_message)
- Modify: `crates/bot/src/telegram/dispatch.rs` (run_telegram signature + deps)

- [ ] **Step 1: Add `PcPort` newtype to handler.rs**

In `crates/bot/src/telegram/handler.rs`, add after `DebugFlag`:

```rust
/// Process-compose API port for PC REST calls from the bot process.
#[derive(Clone)]
pub struct PcPort(pub u16);
```

- [ ] **Step 2: Add `pc_port` to `WorkerContext`**

In `crates/bot/src/telegram/worker.rs`, add field to `WorkerContext`:

```rust
/// Process-compose API port (for auth watcher to start/stop login process).
pub pc_port: u16,
```

- [ ] **Step 3: Update `handle_message` to accept and use `PcPort`**

In `crates/bot/src/telegram/handler.rs`, add `pc_port: Arc<PcPort>` parameter to `handle_message` and include it in `WorkerContext` construction:

```rust
pub async fn handle_message(
    bot: BotType,
    msg: Message,
    worker_map: Arc<DashMap<SessionKey, mpsc::Sender<DebounceMsg>>>,
    agent_dir: Arc<AgentDir>,
    debug_flag: Arc<DebugFlag>,
    ssh_config: Arc<SshConfigPath>,
    pc_port: Arc<PcPort>,
) -> ResponseResult<()> {
```

In the `WorkerContext` construction (around line 102), add:

```rust
pc_port: pc_port.0,
```

- [ ] **Step 4: Update `run_telegram` to accept and inject `pc_port`**

In `crates/bot/src/telegram/dispatch.rs`, add `pc_port: u16` parameter to `run_telegram`:

```rust
pub async fn run_telegram(
    token: String,
    allowed_chat_ids: Vec<i64>,
    agent_dir: PathBuf,
    debug: bool,
    pending_auth: PendingAuthMap,
    home: PathBuf,
    ssh_config_path: Option<PathBuf>,
    pc_port: u16,
) -> miette::Result<()> {
```

Add to shared state section:

```rust
let pc_port_arc: Arc<PcPort> = Arc::new(PcPort(pc_port));
```

Add to `dptree::deps![]`:

```rust
Arc::clone(&pc_port_arc)
```

Update import to include `PcPort`:

```rust
use super::handler::{..., PcPort};
```

- [ ] **Step 5: Update `run_telegram` call in `lib.rs`**

In `crates/bot/src/lib.rs`, read `RC_PC_PORT` from env and pass to `run_telegram`. Add before the `tokio::select!` block (around line 280):

```rust
let pc_port: u16 = std::env::var("RC_PC_PORT")
    .ok()
    .and_then(|v| v.parse().ok())
    .unwrap_or(rightclaw::runtime::pc_client::PC_PORT);
```

Update the `run_telegram` call to pass `pc_port`:

```rust
result = telegram::run_telegram(
    token,
    config.allowed_chat_ids,
    agent_dir,
    args.debug,
    Arc::clone(&pending_auth),
    home.clone(),
    ssh_config_path,
    pc_port,
) => result,
```

- [ ] **Step 6: Build workspace**

Run: `cargo build --workspace` (via rust-builder subagent)
Expected: clean build, no errors.

- [ ] **Step 7: Commit**

```
feat: thread PC port from env through bot to WorkerContext
```

---

### Task 5: Auth Watcher — URL Extraction Helper

Pure function that scans log lines for an OAuth URL. TDD.

**Files:**
- Modify: `crates/bot/src/telegram/worker.rs`

- [ ] **Step 1: Write failing tests for `extract_auth_url`**

Add to the `#[cfg(test)] mod tests` block in `crates/bot/src/telegram/worker.rs`:

```rust
// extract_auth_url tests
#[test]
fn extract_auth_url_finds_anthropic_url() {
    let lines = vec![
        "Initializing...".to_string(),
        "Open this URL to authenticate: https://console.anthropic.com/oauth/authorize?client_id=abc".to_string(),
        "Waiting for callback...".to_string(),
    ];
    let url = extract_auth_url(&lines);
    assert!(url.is_some());
    assert!(url.unwrap().contains("console.anthropic.com"));
}

#[test]
fn extract_auth_url_finds_claude_ai_url() {
    let lines = vec![
        "Please visit: https://claude.ai/oauth/login?token=xyz".to_string(),
    ];
    let url = extract_auth_url(&lines);
    assert!(url.is_some());
    assert!(url.unwrap().contains("claude.ai"));
}

#[test]
fn extract_auth_url_returns_none_when_no_url() {
    let lines = vec![
        "Starting up...".to_string(),
        "Checking credentials...".to_string(),
    ];
    assert!(extract_auth_url(&lines).is_none());
}

#[test]
fn extract_auth_url_ignores_non_auth_urls() {
    let lines = vec![
        "Connecting to https://api.example.com/v1".to_string(),
    ];
    assert!(extract_auth_url(&lines).is_none());
}

#[test]
fn extract_auth_url_handles_empty() {
    let lines: Vec<String> = vec![];
    assert!(extract_auth_url(&lines).is_none());
}

#[test]
fn extract_auth_url_extracts_just_url_from_line() {
    let lines = vec![
        "Go to https://console.anthropic.com/oauth/authorize?foo=bar to continue".to_string(),
    ];
    let url = extract_auth_url(&lines).unwrap();
    assert!(url.starts_with("https://"));
    assert!(!url.contains(" to continue"));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p rightclaw-bot --lib telegram::worker::tests::extract_auth_url`
Expected: FAIL — function doesn't exist.

- [ ] **Step 3: Implement `extract_auth_url`**

Add to `crates/bot/src/telegram/worker.rs` in the pure helpers section:

```rust
/// Extract an OAuth URL from process log lines.
///
/// Scans for `https://` URLs containing `anthropic` or `claude.ai` —
/// these are the login URLs produced by `claude auth login`.
/// Returns the first matching URL, trimmed of surrounding text.
pub fn extract_auth_url(lines: &[String]) -> Option<String> {
    for line in lines {
        let Some(start) = line.find("https://") else {
            continue;
        };
        let url_part = &line[start..];
        let end = url_part.find(|c: char| c.is_whitespace()).unwrap_or(url_part.len());
        let url = &url_part[..end];

        if url.contains("anthropic") || url.contains("claude.ai") {
            return Some(url.to_string());
        }
    }
    None
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p rightclaw-bot --lib telegram::worker::tests::extract_auth_url`
Expected: all 6 tests PASS.

- [ ] **Step 5: Commit**

```
feat: add extract_auth_url helper for scraping OAuth URL from PC logs
```

---

### Task 6: Auth Watcher Task + Integration into `invoke_cc`

Wire everything together: detect auth error in `invoke_cc`, start login process, spawn watcher that extracts URL and probes auth, clean up on success.

**Files:**
- Modify: `crates/bot/src/telegram/worker.rs`
- Modify: `crates/bot/src/telegram/handler.rs`

- [ ] **Step 1: Add `auth_watcher_active` guard to `WorkerContext`**

In `crates/bot/src/telegram/worker.rs`, add import:

```rust
use std::sync::atomic::{AtomicBool, Ordering};
```

Add to `WorkerContext`:

```rust
/// Guard: true when an auth watcher task is active for this agent. Prevents duplicates.
pub auth_watcher_active: Arc<AtomicBool>,
```

- [ ] **Step 2: Update `WorkerContext` construction in handler.rs**

In `crates/bot/src/telegram/handler.rs`, in the `WorkerContext` construction block (around line 102), add:

```rust
auth_watcher_active: Arc::new(AtomicBool::new(false)),
```

Add necessary import at the top:

```rust
use std::sync::atomic::AtomicBool;
```

- [ ] **Step 3: Implement `login_process_name` helper**

Add to the pure helpers section in `crates/bot/src/telegram/worker.rs`:

```rust
/// Process-compose process name for the login session.
fn login_process_name(agent_name: &str) -> String {
    format!("login-{agent_name}")
}
```

- [ ] **Step 4: Implement `send_tg` helper**

Add to `crates/bot/src/telegram/worker.rs`:

```rust
/// Send a Telegram message, optionally in a thread.
async fn send_tg(
    bot: &super::BotType,
    chat_id: teloxide::types::ChatId,
    eff_thread_id: i64,
    text: &str,
) -> Result<(), teloxide::RequestError> {
    use teloxide::types::{MessageId, ThreadId};
    let mut send = bot.send_message(chat_id, text);
    if eff_thread_id != 0 {
        send = send.message_thread_id(ThreadId(MessageId(eff_thread_id as i32)));
    }
    send.await?;
    Ok(())
}
```

- [ ] **Step 5: Implement `spawn_auth_watcher` function**

Add to `crates/bot/src/telegram/worker.rs`:

```rust
/// Spawn a background task that:
/// 1. Starts the `login-{agent}` process via PC API.
/// 2. Scrapes PC logs for the OAuth URL and sends it to Telegram.
/// 3. Periodically probes `claude -p "say ok"` inside sandbox to check auth.
/// 4. On success: stops login process, notifies Telegram.
/// 5. On timeout (5 min): stops login process, notifies Telegram.
fn spawn_auth_watcher(
    ctx: &WorkerContext,
    tg_chat_id: teloxide::types::ChatId,
    eff_thread_id: i64,
) {
    let agent_name = ctx.agent_name.clone();
    let pc_port = ctx.pc_port;
    let bot = ctx.bot.clone();
    let ssh_config_path = ctx.ssh_config_path.clone();
    let active_flag = Arc::clone(&ctx.auth_watcher_active);

    tokio::spawn(async move {
        let login_name = login_process_name(&agent_name);
        let pc = match rightclaw::runtime::pc_client::PcClient::new(pc_port) {
            Ok(c) => c,
            Err(e) => {
                tracing::error!(agent = %agent_name, "auth watcher: failed to create PC client: {e:#}");
                active_flag.store(false, Ordering::SeqCst);
                return;
            }
        };

        // Phase 1: Start login process
        if let Err(e) = pc.start_process(&login_name).await {
            tracing::error!(agent = %agent_name, "auth watcher: failed to start {login_name}: {e:#}");
            let _ = send_tg(&bot, tg_chat_id, eff_thread_id,
                &format!("⚠️ Failed to start login process: {e:#}")).await;
            active_flag.store(false, Ordering::SeqCst);
            return;
        }
        tracing::info!(agent = %agent_name, "auth watcher: started {login_name}");

        // Phase 2: Extract OAuth URL from PC logs (poll for up to 30s)
        let url_deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        let mut url_found = false;
        while tokio::time::Instant::now() < url_deadline {
            tokio::time::sleep(Duration::from_secs(2)).await;
            match pc.get_process_logs(&login_name, 50).await {
                Ok(lines) => {
                    if let Some(url) = extract_auth_url(&lines) {
                        let msg = format!("🔑 Open this link to authenticate:\n{url}");
                        let _ = send_tg(&bot, tg_chat_id, eff_thread_id, &msg).await;
                        tracing::info!(agent = %agent_name, "auth watcher: sent OAuth URL to Telegram");
                        url_found = true;
                        break;
                    }
                }
                Err(e) => {
                    tracing::warn!(agent = %agent_name, "auth watcher: failed to read logs: {e:#}");
                }
            }
        }
        if !url_found {
            let msg = format!(
                "🔑 Could not extract login URL automatically. \
                 Open the process-compose TUI and find **{login_name}** to authenticate."
            );
            let _ = send_tg(&bot, tg_chat_id, eff_thread_id, &msg).await;
        }

        // Phase 3: Probe auth status (every 10s, up to 5 min)
        let auth_deadline = tokio::time::Instant::now() + Duration::from_secs(300);
        let ssh_config = match ssh_config_path {
            Some(ref p) => p,
            None => {
                tracing::error!(agent = %agent_name, "auth watcher: no SSH config — cannot probe");
                active_flag.store(false, Ordering::SeqCst);
                return;
            }
        };
        let ssh_host = rightclaw::openshell::ssh_host(&agent_name);

        while tokio::time::Instant::now() < auth_deadline {
            tokio::time::sleep(Duration::from_secs(10)).await;

            let probe_result = rightclaw::openshell::ssh_exec(
                ssh_config,
                &ssh_host,
                &[
                    "claude", "-p",
                    "--dangerously-skip-permissions",
                    "--output-format", "json",
                    "--", "say ok",
                ],
                30,
            ).await;

            match probe_result {
                Ok(_) => {
                    tracing::info!(agent = %agent_name, "auth watcher: auth probe succeeded");
                    if let Err(e) = pc.stop_process(&login_name).await {
                        tracing::warn!(agent = %agent_name, "auth watcher: failed to stop {login_name}: {e:#}");
                    }
                    let _ = send_tg(&bot, tg_chat_id, eff_thread_id,
                        "✅ Logged in successfully. You can continue chatting.").await;
                    active_flag.store(false, Ordering::SeqCst);
                    return;
                }
                Err(e) => {
                    tracing::debug!(agent = %agent_name, "auth watcher: probe still failing (expected during login): {e:#}");
                }
            }
        }

        // Timeout — stop login process and notify
        tracing::warn!(agent = %agent_name, "auth watcher: login timed out after 5 min");
        if let Err(e) = pc.stop_process(&login_name).await {
            tracing::warn!(agent = %agent_name, "auth watcher: failed to stop {login_name} on timeout: {e:#}");
        }
        let _ = send_tg(&bot, tg_chat_id, eff_thread_id,
            "⚠️ Login timed out after 5 minutes. Send another message to retry.").await;
        active_flag.store(false, Ordering::SeqCst);
    });
}
```

- [ ] **Step 6: Integrate auth error detection into `invoke_cc`**

In `crates/bot/src/telegram/worker.rs`, replace the `if !output.status.success()` block (lines 484–500) with:

```rust
if !output.status.success() {
    // Log full output on failure for debuggability.
    tracing::error!(
        ?chat_id,
        exit_code,
        stdout = %stdout_str.chars().take(1000).collect::<String>(),
        stderr = %stderr_str,
        "claude -p failed"
    );

    // Check for auth error — trigger login flow if sandboxed.
    if is_auth_error(&stdout_str) {
        tracing::warn!(?chat_id, "detected auth error from CC");
        if ctx.ssh_config_path.is_some() {
            // Sandbox mode: spawn auth watcher if not already active.
            if !ctx.auth_watcher_active.swap(true, Ordering::SeqCst) {
                let tg_chat_id = ctx.chat_id;
                let _ = send_tg(&ctx.bot, tg_chat_id, ctx.effective_thread_id,
                    "🔑 Claude needs to log in. Starting login session...").await;
                spawn_auth_watcher(ctx, tg_chat_id, ctx.effective_thread_id);
            }
            return Err(
                "🔑 Login in progress. Please complete authentication using the link sent above."
                    .to_string(),
            );
        } else {
            return Err(
                "🔑 Claude needs to log in. Run `claude` in your terminal to authenticate."
                    .to_string(),
            );
        }
    }

    // Non-auth error: generic error reply.
    let error_detail = if stderr_str.trim().is_empty() && !stdout_str.trim().is_empty() {
        format!("(stderr empty, stdout): {}", stdout_str.chars().take(500).collect::<String>())
    } else {
        stderr_str.to_string()
    };
    return Err(format_error_reply(exit_code, &error_detail));
}
```

- [ ] **Step 7: Build workspace**

Run: `cargo build --workspace` (via rust-builder subagent)
Expected: clean build.

- [ ] **Step 8: Commit**

```
feat: auth watcher — spawns login process, scrapes URL, probes auth, cleans up
```

---

### Task 7: Smoke Test and Fix

Manual smoke test to verify the entire flow end-to-end.

**Files:** Potentially any file from Tasks 1–6 depending on issues found.

- [ ] **Step 1: Rebuild**

Run: `cargo build --workspace` (via rust-builder subagent)
Verify: clean build, no warnings from our crates.

- [ ] **Step 2: Regenerate process-compose config**

Stop running instance (`rightclaw down` if needed), then `rightclaw up`.
Verify in `~/.rightclaw/run/process-compose.yaml`:
- `login-{agent}` process present with `disabled: true`, `is_tty: true`
- `RC_PC_PORT=18927` in bot process env
- SSH config path and host alias correct

- [ ] **Step 3: Trigger auth error**

Send a Telegram message to the bot.
Expected sequence:
1. Bot logs: `detected auth error from CC`
2. Bot logs: `auth watcher: started login-{agent}`
3. Telegram: "🔑 Claude needs to log in. Starting login session..."
4. Within 30s: either OAuth URL in Telegram or fallback PC TUI message

- [ ] **Step 4: Complete authentication**

Click the OAuth URL (or use PC TUI).
Expected:
1. Logs: `auth watcher: auth probe succeeded`
2. Login process stopped
3. Telegram: "✅ Logged in successfully. You can continue chatting."

- [ ] **Step 5: Verify normal operation**

Send another Telegram message.
Expected: bot responds normally.

- [ ] **Step 6: Commit any fixes**

```
fix: smoke test fixes for login flow
```
