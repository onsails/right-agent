//! PTY-driven Claude login flow.
//!
//! Spawns `claude --dangerously-skip-permissions -- /login` via SSH into the sandbox,
//! drives through the login method menu, extracts the OAuth URL, waits for the auth
//! code from Telegram, and pipes it into the PTY session.

use std::path::Path;
use std::time::Duration;

use expectrl::Expect as _;
use tokio::sync::{mpsc, oneshot};

/// Messages sent from the login task to the auth watcher.
#[derive(Debug)]
pub enum LoginEvent {
    /// OAuth URL extracted — send to user.
    Url(String),
    /// CC is asking for the auth code — prompt user.
    WaitingForCode,
    /// Login completed successfully.
    Done,
    /// Login failed.
    Error(String),
}

/// Drive the interactive claude login flow via PTY over SSH.
///
/// Runs in a blocking thread (expectrl is sync). Communicates via channels:
/// - `event_tx`: sends LoginEvents to the async orchestrator
/// - `code_rx`: receives the auth code from the Telegram handler
pub fn run_login_pty(
    ssh_config_path: &Path,
    agent_name: &str,
    event_tx: mpsc::Sender<LoginEvent>,
    code_rx: oneshot::Receiver<String>,
) {
    let ssh_config = ssh_config_path.to_path_buf();
    let ssh_host = rightclaw::openshell::ssh_host(agent_name);

    // Build SSH command to launch claude inside sandbox
    let mut cmd = std::process::Command::new("ssh");
    cmd.arg("-t");
    cmd.arg("-F").arg(&ssh_config);
    cmd.arg(&ssh_host);
    cmd.arg("--");
    cmd.args(["claude", "--dangerously-skip-permissions", "--", "/login"]);

    tracing::info!(agent = agent_name, "login: spawning PTY session via SSH");

    let mut session = match expectrl::Session::spawn(cmd) {
        Ok(s) => s,
        Err(e) => {
            let _ = event_tx.blocking_send(LoginEvent::Error(format!("failed to spawn PTY: {e:#}")));
            return;
        }
    };
    // Set wide terminal so URLs don't wrap across lines.
    if let Err(e) = session.get_process_mut().set_window_size(500, 50) {
        tracing::warn!(agent = agent_name, "login: failed to set PTY window size: {e}");
    }

    // Step 1: Wait for login method menu
    tracing::info!(agent = agent_name, "login: step 1 — waiting for login method menu (30s timeout)");
    session.set_expect_timeout(Some(Duration::from_secs(30)));
    match session.expect(expectrl::Regex("(Select login method|Claude account|subscription)")) {
        Ok(found) => {
            let text = strip_ansi(&String::from_utf8_lossy(found.as_bytes()));
            tracing::info!(agent = agent_name, matched = %text.chars().take(100).collect::<String>(), "login: step 1 — menu appeared");
        }
        Err(e) => {
            let _ = event_tx.blocking_send(LoginEvent::Error(format!("login menu did not appear: {e:#}")));
            return;
        }
    }

    // Step 2: Press Enter to select option 1 (Claude subscription)
    tracing::info!(agent = agent_name, "login: step 2 — sending \\r to select Claude subscription");
    if let Err(e) = session.send("\r") {
        let _ = event_tx.blocking_send(LoginEvent::Error(format!("failed to send Enter: {e:#}")));
        return;
    }

    // Step 3: Wait for "Browser didn't open" text which precedes the URL
    tracing::info!(agent = agent_name, "login: step 3 — waiting for browser prompt (30s timeout)");
    session.set_expect_timeout(Some(Duration::from_secs(30)));
    match session.expect(expectrl::Regex("Browser didn't open")) {
        Ok(_) => {
            tracing::info!(agent = agent_name, "login: step 3 — browser prompt appeared, extracting URL");
            session.set_expect_timeout(Some(Duration::from_secs(5)));
            match session.expect(expectrl::Regex("https://[^\\s\\x1b]+")) {
                Ok(found) => {
                    let raw = String::from_utf8_lossy(found.as_bytes());
                    let cleaned = strip_ansi(&raw);
                    let url = extract_url_from_text(&cleaned);
                    tracing::info!(agent = agent_name, url_len = url.len(), "login: step 3 — OAuth URL extracted");
                    let _ = event_tx.blocking_send(LoginEvent::Url(url));
                }
                Err(e) => {
                    let _ = event_tx.blocking_send(LoginEvent::Error(format!("URL not found after browser prompt: {e:#}")));
                    return;
                }
            }
        }
        Err(e) => {
            let _ = event_tx.blocking_send(LoginEvent::Error(format!("browser prompt did not appear: {e:#}")));
            return;
        }
    }

    // Step 4: Wait for code paste prompt
    // CC shows something like "Paste the code" or just an input field.
    // Use "paste" specifically to avoid matching "code" from URL params.
    tracing::info!(agent = agent_name, "login: step 4 — waiting for paste prompt (10s timeout)");
    session.set_expect_timeout(Some(Duration::from_secs(10)));
    match session.expect(expectrl::Regex("(?i)(paste|enter the code|enter code|authorization)")) {
        Ok(found) => {
            let text = strip_ansi(&String::from_utf8_lossy(found.as_bytes()));
            tracing::info!(agent = agent_name, prompt = %text.chars().take(100).collect::<String>(), "login: step 4 — paste prompt detected");
        }
        Err(_) => {
            tracing::info!(agent = agent_name, "login: step 4 — no explicit paste prompt (timeout). Proceeding anyway.");
        }
    }
    let _ = event_tx.blocking_send(LoginEvent::WaitingForCode);

    // Step 5: Wait for auth code from Telegram
    tracing::info!(agent = agent_name, "login: step 5 — waiting for auth code from Telegram");
    let code = match code_rx.blocking_recv() {
        Ok(c) => {
            tracing::info!(agent = agent_name, code_len = c.len(), "login: step 5 — received auth code from Telegram");
            c
        }
        Err(_) => {
            let _ = event_tx.blocking_send(LoginEvent::Error("auth code channel closed (timeout?)".into()));
            return;
        }
    };

    // Step 6: Send auth code to CC
    std::thread::sleep(Duration::from_millis(500));
    let code_with_cr = format!("{code}\r");
    tracing::info!(agent = agent_name, code_len = code.len(), "login: step 6 — sending auth code to PTY (with \\r)");
    if let Err(e) = session.send(&code_with_cr) {
        let _ = event_tx.blocking_send(LoginEvent::Error(format!("failed to send auth code: {e:#}")));
        return;
    }

    // Step 7: Wait for success or failure indication (10s)
    tracing::info!(agent = agent_name, "login: step 7 — waiting for login result (10s timeout), looking for: logged in|success|authenticated|welcome|API key|signed in|error|failed|invalid");
    session.set_expect_timeout(Some(Duration::from_secs(10)));
    match session.expect(expectrl::Regex("(?i)(logged in|success|authenticated|welcome|API key|signed in|error|failed|invalid)")) {
        Ok(found) => {
            let raw = String::from_utf8_lossy(found.as_bytes());
            let text = strip_ansi(&raw);
            let summary = text.chars().take(300).collect::<String>();
            tracing::info!(agent = agent_name, response = %summary, "login: step 7 — got response from CC");

            // Check if it's an error
            let lower = summary.to_lowercase();
            if lower.contains("error") || lower.contains("failed") || lower.contains("invalid") {
                let _ = event_tx.blocking_send(LoginEvent::Error(format!("CC reported error: {summary}")));
            } else {
                let _ = event_tx.blocking_send(LoginEvent::Done);
            }
        }
        Err(_) => {
            // Timeout — dump raw buffer for debugging
            tracing::warn!(agent = agent_name, "login: step 7 — no response within 10s. Reading remaining PTY buffer for diagnostics...");

            // Try to read whatever is in the buffer
            session.set_expect_timeout(Some(Duration::from_millis(500)));
            match session.expect(expectrl::Regex(".+")) {
                Ok(found) => {
                    let raw = String::from_utf8_lossy(found.as_bytes());
                    let cleaned = strip_ansi(&raw);
                    tracing::info!(agent = agent_name, buffer = %cleaned.chars().take(500).collect::<String>(), "login: PTY buffer after timeout");
                }
                Err(_) => {
                    tracing::info!(agent = agent_name, "login: PTY buffer empty after timeout");
                }
            }

            let _ = event_tx.blocking_send(LoginEvent::Error(
                "No response from Claude after sending code. Check logs for PTY buffer dump.".into()
            ));
        }
    }

    // Cleanup
    tracing::info!(agent = agent_name, "login: cleaning up PTY session");
    let _ = session.send("/exit\r");
    std::thread::sleep(Duration::from_secs(1));
}

/// Extract the first `https://` URL from text that may contain surrounding content.
pub fn extract_url_from_text(text: &str) -> String {
    if let Some(start) = text.find("https://") {
        let url_part = &text[start..];
        let end = url_part.find(|c: char| c.is_whitespace() || c == '\x1b').unwrap_or(url_part.len());
        url_part[..end].to_string()
    } else {
        text.trim().to_string()
    }
}

/// Strip ANSI escape sequences from a string.
pub fn strip_ansi(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut in_escape = false;
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            in_escape = true;
            continue;
        }
        if in_escape {
            if c == '[' {
                // CSI sequence — consume until letter
                while let Some(&next) = chars.peek() {
                    chars.next();
                    if next.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
            in_escape = false;
            continue;
        }
        result.push(c);
    }
    result
}

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
}
