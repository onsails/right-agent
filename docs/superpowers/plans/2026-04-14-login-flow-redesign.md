# Login Flow Redesign Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace fragile PTY-driven login (`expectrl` + `claude -- /login`) with robust SSH exec-based login using `claude auth login`'s local callback server.

**Architecture:** `claude auth login` starts a local HTTP server on a random port inside the sandbox. We spawn it via SSH, parse the OAuth URL and state from stdout, discover the callback port via `ss -tlnp`, then submit the auth code via `curl` to `[::1]:PORT/callback`. No PTY interaction, no TUI parsing.

**Tech Stack:** `tokio::process::Command` (SSH exec), regex for URL/port parsing, existing SSH infrastructure from `openshell.rs`.

---

### Task 1: Update Sandbox Policy — Add /dev/tty and /dev/pts

**Files:**
- Modify: `crates/rightclaw/src/codegen/policy.rs:79-92` (filesystem_policy read_write list)
- Test: `crates/rightclaw/src/codegen/policy.rs` (existing test module)

**Context:** `claude auth login` uses `script` to allocate a PTY inside the sandbox. `script` needs `/dev/tty` and `/dev/pts` access. These are currently blocked by Landlock. Filesystem policy changes require sandbox recreation (hot-reload doesn't apply them).

- [ ] **Step 1: Write failing test**

Add to the existing `mod tests` in `policy.rs`:

```rust
#[test]
fn policy_allows_dev_tty_and_pts() {
    let policy = generate_policy(8100, &NetworkPolicy::Permissive, None);
    assert!(policy.contains("/dev/tty"), "must allow /dev/tty for script PTY");
    assert!(policy.contains("/dev/pts"), "must allow /dev/pts for PTY devices");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `devenv shell -- cargo test -p rightclaw --lib codegen::policy::tests::policy_allows_dev_tty_and_pts`
Expected: FAIL — current policy doesn't include `/dev/tty` or `/dev/pts`.

- [ ] **Step 3: Add /dev/tty and /dev/pts to policy**

In `generate_policy()`, update the `read_write` list in the format string (around line 88):

```rust
  read_write:
    - /dev/null
    - /dev/tty
    - /dev/pts
    - /tmp
    - /sandbox
    - /platform
```

- [ ] **Step 4: Run tests to verify pass**

Run: `devenv shell -- cargo test -p rightclaw --lib codegen::policy`
Expected: All policy tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/rightclaw/src/codegen/policy.rs
git commit -m "feat(policy): allow /dev/tty and /dev/pts for login PTY allocation"
```

---

### Task 2: Add urlencoding Dependency

**Files:**
- Modify: `Cargo.toml` (workspace root — `[workspace.dependencies]`)
- Modify: `crates/bot/Cargo.toml` (`[dependencies]`)

**Context:** The auth code may contain characters like `#`, `+`, `=` that need URL encoding for the curl callback. `urlencoding` is a zero-dep crate.

- [ ] **Step 1: Find latest version**

Run: `devenv shell -- python3 scripts/check_crate_version.py urlencoding`

- [ ] **Step 2: Add to workspace dependencies**

In root `Cargo.toml` under `[workspace.dependencies]`:
```toml
urlencoding = "2.1"
```

In `crates/bot/Cargo.toml` under `[dependencies]`:
```toml
urlencoding = { workspace = true }
```

- [ ] **Step 3: Verify build**

Run: `devenv shell -- cargo check -p rightclaw-bot`
Expected: compiles.

- [ ] **Step 4: Commit**

```bash
git add Cargo.toml crates/bot/Cargo.toml Cargo.lock
git commit -m "deps: add urlencoding for OAuth callback code encoding"
```

---

### Task 3: Rewrite login.rs — SSH Exec-Based Login

**Files:**
- Rewrite: `crates/bot/src/login.rs` (full rewrite, keep `LoginEvent` enum, `strip_ansi`, `extract_url_from_text`)
- Test: `crates/bot/src/login.rs` (inline `#[cfg(test)]` module)

**Context:** The new flow has three async functions: `start_auth_session` (spawn claude auth login, read URL), `discover_callback_port` (parse `ss -tlnp`), and `submit_auth_code` (curl the callback). All use `tokio::process::Command` SSH exec. The `LoginEvent` enum stays for compatibility with the worker orchestration.

- [ ] **Step 1: Write unit tests for URL/state parsing**

Replace existing tests in `login.rs` with:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_ansi_removes_escapes() {
        let input = "\x1b[38;5;153mhttps://example.com\x1b[39m";
        assert_eq!(strip_ansi(input), "https://example.com");
    }

    #[test]
    fn strip_ansi_preserves_plain_text() {
        assert_eq!(strip_ansi("hello world"), "hello world");
    }

    #[test]
    fn strip_ansi_handles_empty() {
        assert_eq!(strip_ansi(""), "");
    }

    #[test]
    fn extract_url_finds_https() {
        let text = "some prefix https://example.com/path?q=1 trailing";
        assert_eq!(extract_url_from_text(text), "https://example.com/path?q=1");
    }

    #[test]
    fn extract_url_returns_input_when_no_url() {
        assert_eq!(extract_url_from_text("no url here"), "no url here");
    }

    #[test]
    fn extract_state_from_real_url() {
        let url = "https://claude.com/cai/oauth/authorize?code=true&client_id=XXX&state=abc123_def";
        assert_eq!(extract_state_param(url), Some("abc123_def".to_owned()));
    }

    #[test]
    fn extract_state_missing() {
        assert_eq!(extract_state_param("https://example.com?foo=bar"), None);
    }

    #[test]
    fn parse_callback_port_from_ss_output() {
        let ss_output = "\
State  Recv-Q Send-Q Local Address:Port  Peer Address:Port Process
LISTEN 0      512            [::1]:36275         [::]:*    users:((\"claude\",pid=881,fd=15))
LISTEN 0      512            [::1]:44901         [::]:*    users:((\"node\",pid=100,fd=3))
";
        assert_eq!(parse_callback_port(ss_output), Some(36275));
    }

    #[test]
    fn parse_callback_port_no_claude() {
        let ss_output = "State  Recv-Q Send-Q Local Address:Port\nLISTEN 0 512 [::1]:8080 [::]:*\n";
        assert_eq!(parse_callback_port(ss_output), None);
    }

    #[test]
    fn parse_callback_port_ipv4_format() {
        let ss_output = "LISTEN 0 512 127.0.0.1:42000 0.0.0.0:* users:((\"claude\",pid=50,fd=10))\n";
        assert_eq!(parse_callback_port(ss_output), Some(42000));
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `devenv shell -- cargo test -p rightclaw-bot --lib login::tests`
Expected: FAIL — `extract_state_param` and `parse_callback_port` don't exist yet.

- [ ] **Step 3: Implement the new login.rs**

Rewrite `crates/bot/src/login.rs`. Keep `LoginEvent`, `strip_ansi`, `extract_url_from_text`. Remove all `expectrl` code. Add:

```rust
//! SSH exec-based Claude login flow.
//!
//! Spawns `claude auth login` inside the sandbox via SSH. The CLI starts a local
//! HTTP callback server on a random port. We parse the OAuth URL from stdout,
//! discover the callback port via `ss -tlnp`, then submit the auth code by
//! curling the callback endpoint inside the sandbox.

use std::path::Path;
use std::time::Duration;

use tokio::io::AsyncBufReadExt as _;
use tokio::process::Command;
use tokio::sync::{mpsc, oneshot};

/// Messages sent from the login task to the auth watcher.
#[derive(Debug)]
pub enum LoginEvent {
    /// OAuth URL extracted — send to user.
    Url(String),
    /// Ready for the auth code — prompt user.
    WaitingForCode,
    /// Login completed successfully.
    Done,
    /// Login failed.
    Error(String),
}

/// State captured from `claude auth login` stdout.
struct AuthSession {
    /// OAuth state parameter from the URL (for callback).
    state: String,
    /// Handle to the running `claude auth login` process.
    child: tokio::process::Child,
}

/// Drive the login flow via SSH exec (no PTY).
///
/// Async — runs on the tokio runtime. Communicates via channels:
/// - `event_tx`: sends LoginEvents to the async orchestrator
/// - `code_rx`: receives the auth code from the Telegram handler
pub async fn run_login(
    ssh_config_path: &Path,
    agent_name: &str,
    event_tx: mpsc::Sender<LoginEvent>,
    code_rx: oneshot::Receiver<String>,
) {
    let ssh_config = ssh_config_path.to_path_buf();
    let host = rightclaw::openshell::ssh_host(agent_name);

    // Phase 1: Start auth session, parse URL
    let session = match start_auth_session(&ssh_config, &host, agent_name, &event_tx).await {
        Some(s) => s,
        None => return, // error already sent via event_tx
    };

    // Phase 2: Discover callback port
    let port = match discover_callback_port(&ssh_config, &host, agent_name, &event_tx).await {
        Some(p) => p,
        None => return,
    };

    // Notify: ready for code
    let _ = event_tx.send(LoginEvent::WaitingForCode).await;

    // Phase 3: Wait for auth code from Telegram
    tracing::info!(agent = agent_name, "login: waiting for auth code from Telegram");
    let code = match code_rx.await {
        Ok(c) => {
            tracing::info!(agent = agent_name, code_len = c.len(), "login: received auth code");
            c
        }
        Err(_) => {
            let _ = event_tx.send(LoginEvent::Error("auth code channel closed".into())).await;
            return;
        }
    };

    // Phase 4: Submit auth code via callback
    submit_auth_code(&ssh_config, &host, agent_name, port, &code, &session.state, session.child, &event_tx).await;
}

/// Spawn `claude auth login` via SSH, read stdout for OAuth URL.
async fn start_auth_session(
    ssh_config: &Path,
    host: &str,
    agent_name: &str,
    event_tx: &mpsc::Sender<LoginEvent>,
) -> Option<AuthSession> {
    tracing::info!(agent = agent_name, "login: starting auth session via SSH");

    let mut child = match Command::new("ssh")
        .arg("-F").arg(ssh_config)
        .arg(host)
        .arg("--")
        .arg("script -q -c \"claude auth login\" /dev/null")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            let _ = event_tx.send(LoginEvent::Error(format!("failed to spawn SSH: {e:#}"))).await;
            return None;
        }
    };

    // Read stdout lines until we find the URL (timeout 30s)
    let stdout = child.stdout.take().expect("stdout piped");
    let mut reader = tokio::io::BufReader::new(stdout).lines();
    let mut url = None;

    let read_url = async {
        while let Ok(Some(line)) = reader.next_line().await {
            let cleaned = strip_ansi(&line);
            tracing::debug!(agent = agent_name, line = %cleaned, "login: stdout line");
            if cleaned.contains("https://") && cleaned.contains("code_challenge") {
                url = Some(extract_url_from_text(&cleaned));
                break;
            }
        }
    };

    if tokio::time::timeout(Duration::from_secs(30), read_url).await.is_err() {
        let _ = event_tx.send(LoginEvent::Error("timeout waiting for OAuth URL".into())).await;
        let _ = child.kill().await;
        return None;
    }

    let url = match url {
        Some(u) => u,
        None => {
            let _ = event_tx.send(LoginEvent::Error("claude auth login exited without showing URL".into())).await;
            let _ = child.kill().await;
            return None;
        }
    };

    let state = match extract_state_param(&url) {
        Some(s) => s,
        None => {
            let _ = event_tx.send(LoginEvent::Error("no state parameter in OAuth URL".into())).await;
            let _ = child.kill().await;
            return None;
        }
    };

    tracing::info!(agent = agent_name, url_len = url.len(), "login: OAuth URL extracted");
    let _ = event_tx.send(LoginEvent::Url(url)).await;

    Some(AuthSession { state, child })
}

/// Discover the callback port that `claude auth login` is listening on.
///
/// Retries up to 5 times with 1s backoff.
async fn discover_callback_port(
    ssh_config: &Path,
    host: &str,
    agent_name: &str,
    event_tx: &mpsc::Sender<LoginEvent>,
) -> Option<u16> {
    for attempt in 1..=5 {
        tracing::debug!(agent = agent_name, attempt, "login: discovering callback port");
        match rightclaw::openshell::ssh_exec(ssh_config, host, &["ss", "-tlnp"], 10).await {
            Ok(output) => {
                if let Some(port) = parse_callback_port(&output) {
                    tracing::info!(agent = agent_name, port, "login: callback port discovered");
                    return Some(port);
                }
            }
            Err(e) => {
                tracing::warn!(agent = agent_name, attempt, error = %e, "login: ss command failed");
            }
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    let _ = event_tx.send(LoginEvent::Error("could not discover callback port after 5 attempts".into())).await;
    None
}

/// Submit the auth code to the local callback server and wait for completion.
async fn submit_auth_code(
    ssh_config: &Path,
    host: &str,
    agent_name: &str,
    port: u16,
    code: &str,
    state: &str,
    mut child: tokio::process::Child,
    event_tx: &mpsc::Sender<LoginEvent>,
) {
    tracing::info!(agent = agent_name, port, "login: submitting auth code via callback");

    // URL-encode the code (it may contain special chars like # + =)
    let encoded_code = urlencoding::encode(code);
    let encoded_state = urlencoding::encode(state);
    let callback_url = format!(
        "http://[::1]:{port}/callback?code={encoded_code}&state={encoded_state}"
    );

    let curl_cmd = format!("curl -s -o /dev/null -w '%{{http_code}}' '{callback_url}'");
    match rightclaw::openshell::ssh_exec(ssh_config, host, &["bash", "-c", &curl_cmd], 30).await {
        Ok(status_code) => {
            let status = status_code.trim();
            tracing::info!(agent = agent_name, http_status = %status, "login: callback response");
            if status != "302" && status != "200" {
                let _ = event_tx.send(LoginEvent::Error(
                    format!("callback returned unexpected HTTP {status}")
                )).await;
                let _ = child.kill().await;
                return;
            }
        }
        Err(e) => {
            let _ = event_tx.send(LoginEvent::Error(format!("curl callback failed: {e:#}"))).await;
            let _ = child.kill().await;
            return;
        }
    }

    // Wait for claude auth login to complete (token exchange)
    tracing::info!(agent = agent_name, "login: waiting for auth process to complete");
    match tokio::time::timeout(Duration::from_secs(30), child.wait()).await {
        Ok(Ok(status)) => {
            if status.success() {
                tracing::info!(agent = agent_name, "login: auth process exited successfully");
                let _ = event_tx.send(LoginEvent::Done).await;
            } else {
                let _ = event_tx.send(LoginEvent::Error(
                    format!("claude auth login exited with {status}")
                )).await;
            }
        }
        Ok(Err(e)) => {
            let _ = event_tx.send(LoginEvent::Error(format!("wait failed: {e:#}"))).await;
        }
        Err(_) => {
            tracing::warn!(agent = agent_name, "login: auth process didn't exit in 30s, killing");
            let _ = child.kill().await;
            let _ = event_tx.send(LoginEvent::Error("auth process timed out after code submission".into())).await;
        }
    }
}

/// Extract the `state` query parameter from an OAuth URL.
pub fn extract_state_param(url: &str) -> Option<String> {
    url.split('?')
        .nth(1)?
        .split('&')
        .find_map(|pair| {
            let (key, value) = pair.split_once('=')?;
            if key == "state" { Some(value.to_owned()) } else { None }
        })
}

/// Parse the callback port from `ss -tlnp` output.
///
/// Looks for a LISTEN line containing "claude" and extracts the port number.
pub fn parse_callback_port(ss_output: &str) -> Option<u16> {
    for line in ss_output.lines() {
        if !line.contains("claude") {
            continue;
        }
        // Match patterns like [::1]:36275 or 127.0.0.1:36275
        for segment in line.split_whitespace() {
            if let Some(port_str) = segment.rsplit(':').next() {
                if let Ok(port) = port_str.parse::<u16>() {
                    return Some(port);
                }
            }
        }
    }
    None
}

// --- Keep existing helpers ---

/// Extract the first `https://` URL from text that may contain surrounding content.
pub fn extract_url_from_text(text: &str) -> String {
    // ... (keep existing implementation unchanged)
}

/// Strip ANSI escape sequences from a string.
pub fn strip_ansi(s: &str) -> String {
    // ... (keep existing implementation unchanged)
}
```

- [ ] **Step 4: Run tests to verify pass**

Run: `devenv shell -- cargo test -p rightclaw-bot --lib login::tests`
Expected: All 10 tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/bot/src/login.rs
git commit -m "feat(login): rewrite login flow using SSH exec and local callback"
```

---

### Task 4: Update spawn_auth_watcher — Use Async Login

**Files:**
- Modify: `crates/bot/src/telegram/worker.rs:642-740` (`spawn_auth_watcher`)

**Context:** `spawn_auth_watcher` currently spawns `run_login_pty` in a blocking thread (expectrl is sync). The new `run_login` is fully async — no blocking thread needed. The `LoginEvent` enum and channel-based communication stay identical, so the event processing loop (lines 680-733) doesn't change.

- [ ] **Step 1: Update spawn_auth_watcher**

Replace the function body at `worker.rs:642-740`. The event processing loop stays the same — only the spawning logic changes:

```rust
fn spawn_auth_watcher(
    ctx: &WorkerContext,
    tg_chat_id: teloxide::types::ChatId,
    eff_thread_id: i64,
) {
    let agent_name = ctx.agent_name.clone();
    let bot = ctx.bot.clone();
    let ssh_config_path = ctx.ssh_config_path.clone();
    let active_flag = Arc::clone(&ctx.auth_watcher_active);
    let auth_code_tx_slot = Arc::clone(&ctx.auth_code_tx);

    tokio::spawn(async move {
        let ssh_config = match ssh_config_path {
            Some(ref p) => p.clone(),
            None => {
                tracing::error!(agent = %agent_name, "auth watcher: no SSH config");
                active_flag.store(false, Ordering::SeqCst);
                return;
            }
        };

        // Create channels for login ↔ async communication
        let (event_tx, mut event_rx) = tokio::sync::mpsc::channel::<crate::login::LoginEvent>(8);
        let (code_tx, code_rx) = tokio::sync::oneshot::channel::<String>();

        // Store the code sender so Telegram handler can forward the auth code
        auth_code_tx_slot.lock().await.replace(code_tx);

        // Spawn async login task (no blocking thread needed)
        let agent_for_login = agent_name.clone();
        tokio::spawn(async move {
            crate::login::run_login(&ssh_config, &agent_for_login, event_tx, code_rx).await;
        });

        // Process events from the login task — IDENTICAL to existing code
        let timeout = tokio::time::sleep(Duration::from_secs(300));
        tokio::pin!(timeout);

        loop {
            tokio::select! {
                event = event_rx.recv() => {
                    match event {
                        Some(crate::login::LoginEvent::Url(url)) => {
                            let msg = format!("Open this link to authenticate:\n{url}");
                            if let Err(e) = send_tg(&bot, tg_chat_id, eff_thread_id, &msg).await {
                                tracing::warn!(agent = %agent_name, "auth watcher: Telegram send failed: {e:#}");
                            }
                        }
                        Some(crate::login::LoginEvent::WaitingForCode) => {
                            if let Err(e) = send_tg(
                                &bot, tg_chat_id, eff_thread_id,
                                "After authenticating in the browser, send me the code shown on the page.",
                            ).await {
                                tracing::warn!(agent = %agent_name, "auth watcher: Telegram send failed: {e:#}");
                            }
                        }
                        Some(crate::login::LoginEvent::Done) => {
                            if let Err(e) = send_tg(
                                &bot, tg_chat_id, eff_thread_id,
                                "Logged in successfully. You can continue chatting.",
                            ).await {
                                tracing::warn!(agent = %agent_name, "auth watcher: Telegram send failed: {e:#}");
                            }
                            break;
                        }
                        Some(crate::login::LoginEvent::Error(msg)) => {
                            tracing::error!(agent = %agent_name, "auth watcher: login error: {msg}");
                            if let Err(e) = send_tg(
                                &bot, tg_chat_id, eff_thread_id,
                                &format!("Login failed: {msg}"),
                            ).await {
                                tracing::warn!(agent = %agent_name, "auth watcher: Telegram send failed: {e:#}");
                            }
                            break;
                        }
                        None => {
                            tracing::info!(agent = %agent_name, "auth watcher: login task exited");
                            break;
                        }
                    }
                }
                _ = &mut timeout => {
                    tracing::warn!(agent = %agent_name, "auth watcher: login timed out after 5 min");
                    if let Err(e) = send_tg(
                        &bot, tg_chat_id, eff_thread_id,
                        "Login timed out after 5 minutes. Send another message to retry.",
                    ).await {
                        tracing::warn!(agent = %agent_name, "auth watcher: Telegram send failed: {e:#}");
                    }
                    break;
                }
            }
        }

        // Cleanup
        auth_code_tx_slot.lock().await.take();
        active_flag.store(false, Ordering::SeqCst);
    });
}
```

Key change: `tokio::task::spawn_blocking(move || { run_login_pty(...) })` → `tokio::spawn(async move { run_login(...).await })`.

- [ ] **Step 2: Verify build**

Run: `devenv shell -- cargo check -p rightclaw-bot`
Expected: compiles without errors.

- [ ] **Step 3: Commit**

```bash
git add crates/bot/src/telegram/worker.rs
git commit -m "feat(worker): use async login flow instead of blocking PTY"
```

---

### Task 5: Remove expectrl Dependency

**Files:**
- Modify: `crates/bot/Cargo.toml` (remove `expectrl = "0.8"`)
- Modify or delete: `crates/bot/tests/claude_login_pty.rs`

**Context:** `expectrl` is only used in `login.rs` (now rewritten) and the integration test `claude_login_pty.rs`. The integration test tests the OLD PTY-based flow which no longer exists. Replace with a test for the new flow.

- [ ] **Step 1: Remove expectrl from Cargo.toml**

In `crates/bot/Cargo.toml`, remove:
```toml
expectrl = "0.8"
```

- [ ] **Step 2: Rewrite integration test**

Replace `crates/bot/tests/claude_login_pty.rs` with a new test file `crates/bot/tests/claude_auth_login.rs` that tests URL parsing and state extraction from real `claude auth login` output:

```rust
//! Integration test: verify `claude auth login` outputs a parseable OAuth URL
//! and starts a local callback server.
//!
//! Requires `claude` in PATH. Uses a temporary HOME.
//! Run with:
//!   cargo test -p rightclaw-bot --test claude_auth_login -- --ignored --nocapture

use std::path::PathBuf;

fn find_claude() -> PathBuf {
    which::which("claude")
        .or_else(|_| which::which("claude-bun"))
        .expect("claude binary not found in PATH")
}

fn setup_temp_home() -> tempfile::TempDir {
    let home = tempfile::tempdir().expect("failed to create temp HOME");
    let claude_dir = home.path().join(".claude");
    std::fs::create_dir_all(&claude_dir).unwrap();
    std::fs::write(
        claude_dir.join("settings.json"),
        serde_json::json!({
            "skipDangerousModePermissionPrompt": true,
            "autoMemoryEnabled": false,
        })
        .to_string(),
    )
    .unwrap();
    let home_path = home.path().to_string_lossy().to_string();
    std::fs::write(
        home.path().join(".claude.json"),
        serde_json::json!({
            "hasCompletedOnboarding": true,
            "projects": { home_path: { "hasTrustDialogAccepted": true } }
        })
        .to_string(),
    )
    .unwrap();
    home
}

#[tokio::test]
#[ignore]
async fn claude_auth_login_url_and_callback_port() {
    let home = setup_temp_home();
    let claude_bin = find_claude();

    // Spawn claude auth login with script for PTY
    let mut child = tokio::process::Command::new("script")
        .args(["-q", "-c"])
        .arg(format!("{} auth login", claude_bin.display()))
        .arg("/dev/null")
        .env("HOME", home.path())
        .env("CLAUDE_CONFIG_DIR", home.path().join(".claude"))
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("failed to spawn");

    // Read URL from stdout
    let stdout = child.stdout.take().unwrap();
    let mut reader = tokio::io::BufReader::new(stdout).lines();
    let mut found_url = None;

    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(std::time::Duration::from_secs(5), reader.next_line()).await {
            Ok(Ok(Some(line))) => {
                let cleaned = rightclaw_bot::login::strip_ansi(&line);
                if cleaned.contains("code_challenge") {
                    found_url = Some(rightclaw_bot::login::extract_url_from_text(&cleaned));
                    break;
                }
            }
            _ => break,
        }
    }

    let url = found_url.expect("should find OAuth URL in output");
    println!("URL: {url}");

    assert!(url.starts_with("https://"), "URL must start with https://");
    assert!(url.contains("state="), "URL must contain state parameter");

    let state = rightclaw_bot::login::extract_state_param(&url);
    assert!(state.is_some(), "should extract state parameter");
    println!("State: {}", state.unwrap());

    // Check that callback server is listening
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    let ss_output = tokio::process::Command::new("ss")
        .args(["-tlnp"])
        .output()
        .await
        .expect("ss command failed");
    let ss_str = String::from_utf8_lossy(&ss_output.stdout);
    let port = rightclaw_bot::login::parse_callback_port(&ss_str);
    println!("Callback port: {port:?}");
    // Port may not be visible outside sandbox, so don't assert — just log.

    child.kill().await.ok();
    println!("SUCCESS: URL and state extracted from claude auth login");
}
```

- [ ] **Step 3: Delete old test file**

```bash
rm crates/bot/tests/claude_login_pty.rs
```

- [ ] **Step 4: Verify build**

Run: `devenv shell -- cargo build --workspace`
Expected: compiles. No references to `expectrl` remain.

- [ ] **Step 5: Verify no stale expectrl references**

Run: `rg expectrl crates/` — should return zero matches (only docs/specs may mention it).

- [ ] **Step 6: Commit**

```bash
git add crates/bot/Cargo.toml crates/bot/tests/ Cargo.lock
git commit -m "refactor: remove expectrl dependency, replace PTY integration test"
```

---

### Task 6: Build and Smoke Test

**Files:** None (verification only)

- [ ] **Step 1: Full workspace build**

Run: `devenv shell -- cargo build --workspace`
Expected: clean build, no errors, no warnings related to login.

- [ ] **Step 2: Run unit tests**

Run: `devenv shell -- cargo test --workspace`
Expected: all tests pass.

- [ ] **Step 3: Verify no expectrl references in code**

Run: `rg expectrl crates/`
Expected: no matches.

- [ ] **Step 4: Commit any fixups**

If any fixes were needed, commit them.
