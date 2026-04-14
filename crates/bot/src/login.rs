//! PTY-based Claude login flow via Python helper inside sandbox.
//!
//! Spawns a Python script via SSH that uses `pty.fork()` to drive
//! `claude auth login` interactively. The helper outputs structured
//! lines (URL:, READY, OK, ERROR:) on stdout and reads the auth code
//! from stdin. This avoids all issues with callback servers, port
//! discovery, and redirect_uri mismatches.

use std::path::Path;

use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _};
use tokio::sync::{mpsc, oneshot};

/// Messages sent from the login task to the auth watcher.
#[derive(Debug)]
pub enum LoginEvent {
    /// OAuth URL extracted -- send to user.
    Url(String),
    /// CC is asking for the auth code -- prompt user.
    WaitingForCode,
    /// Login completed successfully.
    Done,
    /// Login failed.
    Error(String),
}

/// Python helper script executed inside the sandbox via SSH.
///
/// Protocol (stdout lines):
///   URL:<oauth_url>    — OAuth URL extracted
///   READY              — waiting for code on stdin
///   OK                 — login succeeded
///   ERROR:<message>    — login failed
const PTY_HELPER: &str = r##"
import pty, os, sys, time, select, signal

pid, master_fd = pty.fork()
if pid == 0:
    os.execvp("claude", ["claude", "auth", "login"])
    sys.exit(1)

# Read URL from PTY output
output = b""
start = time.time()
url = None
while time.time() - start < 30:
    r, _, _ = select.select([master_fd], [], [], 0.5)
    if r:
        try:
            chunk = os.read(master_fd, 4096)
            if chunk:
                output += chunk
        except OSError:
            break
    decoded = output.decode("utf-8", errors="replace")
    if "code_challenge" in decoded:
        for line in decoded.splitlines():
            if "https://" in line:
                idx = line.find("https://")
                url = line[idx:].strip()
                break
        break

if not url:
    print("ERROR:no OAuth URL found in claude auth login output", flush=True)
    os.kill(pid, signal.SIGTERM)
    os.waitpid(pid, 0)
    sys.exit(1)

print(f"URL:{url}", flush=True)
print("READY", flush=True)

# Read code from stdin (blocking)
try:
    code = sys.stdin.readline().strip()
except EOFError:
    print("ERROR:stdin closed before code received", flush=True)
    os.kill(pid, signal.SIGTERM)
    os.waitpid(pid, 0)
    sys.exit(1)

if not code:
    print("ERROR:empty code received", flush=True)
    os.kill(pid, signal.SIGTERM)
    os.waitpid(pid, 0)
    sys.exit(1)

# Strip #STATE suffix if present (platform.claude.com shows CODE#STATE)
code = code.split("#")[0]

# Type code into PTY
os.write(master_fd, code.encode() + b"\r")

# Wait for result (up to 30s)
result = b""
start = time.time()
while time.time() - start < 30:
    r, _, _ = select.select([master_fd], [], [], 1)
    if r:
        try:
            chunk = os.read(master_fd, 4096)
            if chunk:
                result += chunk
        except OSError:
            break
    # Check if child exited
    try:
        wpid, status = os.waitpid(pid, os.WNOHANG)
        if wpid != 0:
            break
    except ChildProcessError:
        break

decoded = result.decode("utf-8", errors="replace").lower()
if "success" in decoded or "logged in" in decoded or "authenticated" in decoded:
    print("OK", flush=True)
else:
    # Check exit status
    try:
        _, status = os.waitpid(pid, 0)
        exit_code = os.WEXITSTATUS(status) if os.WIFEXITED(status) else -1
    except ChildProcessError:
        exit_code = -1
    if exit_code == 0:
        print("OK", flush=True)
    else:
        # Include first 200 chars of output for debugging
        snippet = result.decode("utf-8", errors="replace").strip()[:200]
        print(f"ERROR:exit code {exit_code}: {snippet}", flush=True)

try:
    os.kill(pid, signal.SIGTERM)
except ProcessLookupError:
    pass
try:
    os.waitpid(pid, 0)
except ChildProcessError:
    pass
"##;

/// Orchestrate the Claude login flow via Python PTY helper inside sandbox.
///
/// Communicates via channels:
/// - `event_tx`: sends `LoginEvent`s to the async orchestrator
/// - `code_rx`: receives the auth code from the Telegram handler
pub async fn run_login(
    ssh_config_path: &Path,
    agent_name: &str,
    event_tx: mpsc::Sender<LoginEvent>,
    code_rx: oneshot::Receiver<String>,
) {
    let ssh_host = rightclaw::openshell::ssh_host(agent_name);
    tracing::info!(agent = agent_name, "login: starting PTY helper via SSH");

    // Spawn SSH with Python PTY helper — stdin/stdout piped for communication
    let mut child = match tokio::process::Command::new("ssh")
        .arg("-F")
        .arg(ssh_config_path)
        .arg(&ssh_host)
        .arg("--")
        .arg(format!("python3 -c {}", shell_escape(PTY_HELPER)))
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            let _ = event_tx
                .send(LoginEvent::Error(format!("failed to spawn SSH: {e:#}")))
                .await;
            return;
        }
    };

    let stdout = child.stdout.take().expect("stdout piped");
    let mut stdin = child.stdin.take().expect("stdin piped");
    let mut lines = tokio::io::BufReader::new(stdout).lines();
    let mut code_rx = Some(code_rx);

    // Read lines from helper, drive the flow
    let timeout = std::time::Duration::from_secs(300); // 5 min total
    let result = tokio::time::timeout(timeout, async {
        while let Some(line) = lines.next_line().await.map_err(|e| format!("reading stdout: {e}"))? {
            let line = line.trim().to_string();
            tracing::info!(agent = agent_name, line = %line, "login: helper output");

            if let Some(url) = line.strip_prefix("URL:") {
                let _ = event_tx.send(LoginEvent::Url(url.to_string())).await;
            } else if line == "READY" {
                let _ = event_tx.send(LoginEvent::WaitingForCode).await;

                // Wait for auth code from Telegram (one-shot)
                let rx = code_rx.take().ok_or_else(|| "code_rx already consumed".to_string())?;
                tracing::info!(agent = agent_name, "login: waiting for auth code from Telegram");
                let code = match rx.await {
                    Ok(c) => c,
                    Err(_) => {
                        return Err("auth code channel closed (timeout?)".to_string());
                    }
                };

                // Send code to helper via stdin
                tracing::info!(agent = agent_name, code_len = code.len(), "login: sending code to helper");
                stdin
                    .write_all(format!("{code}\n").as_bytes())
                    .await
                    .map_err(|e| format!("writing code to stdin: {e}"))?;
                stdin.flush().await.map_err(|e| format!("flushing stdin: {e}"))?;
            } else if line == "OK" {
                let _ = event_tx.send(LoginEvent::Done).await;
                return Ok(());
            } else if let Some(msg) = line.strip_prefix("ERROR:") {
                return Err(msg.to_string());
            }
        }
        Err("helper exited without result".to_string())
    })
    .await;

    match result {
        Ok(Ok(())) => {} // Done already sent
        Ok(Err(msg)) => {
            let _ = event_tx.send(LoginEvent::Error(msg)).await;
        }
        Err(_) => {
            let _ = event_tx
                .send(LoginEvent::Error("login timed out after 5 minutes".into()))
                .await;
        }
    }

    let _ = child.kill().await;
}

/// Shell-escape a string for use as a single argument in a remote shell command.
/// Wraps in single quotes, escaping any embedded single quotes.
fn shell_escape(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Extract the first `https://` URL from text that may contain surrounding content.
pub fn extract_url_from_text(text: &str) -> String {
    if let Some(start) = text.find("https://") {
        let url_part = &text[start..];
        let end = url_part
            .find(|c: char| c.is_whitespace() || c == '\x1b')
            .unwrap_or(url_part.len());
        url_part[..end].to_string()
    } else {
        text.trim().to_string()
    }
}

/// Extract the `state` query parameter from an OAuth URL.
pub fn extract_state_param(url: &str) -> Option<String> {
    let query = url.split('?').nth(1)?;
    for pair in query.split('&') {
        if let Some(value) = pair.strip_prefix("state=") {
            return Some(value.to_owned());
        }
    }
    None
}

/// Extract all LISTEN ports from `ss -tln` output.
pub fn parse_listen_ports(ss_output: &str) -> Vec<u16> {
    let mut ports = Vec::new();
    for line in ss_output.lines() {
        if !line.contains("LISTEN") {
            continue;
        }
        if let Some(addr) = line.split_whitespace().nth(3) {
            if let Some(port_str) = addr.rsplit(':').next() {
                if let Ok(port) = port_str.parse::<u16>() {
                    if port > 0 {
                        ports.push(port);
                    }
                }
            }
        }
    }
    ports
}

/// Return ports present in `current` but not in `baseline`.
pub fn diff_ports(current: &[u16], baseline: &[u16]) -> Vec<u16> {
    current
        .iter()
        .copied()
        .filter(|p| !baseline.contains(p))
        .collect()
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

    #[test]
    fn extract_state_from_real_url() {
        let url =
            "https://claude.com/cai/oauth/authorize?code=true&client_id=XXX&state=abc123_def";
        assert_eq!(extract_state_param(url), Some("abc123_def".to_owned()));
    }

    #[test]
    fn extract_state_missing() {
        assert_eq!(extract_state_param("https://example.com?foo=bar"), None);
    }

    #[test]
    fn parse_listen_ports_extracts_all() {
        let ss_output = "\
State  Recv-Q Send-Q Local Address:Port  Peer Address:Port Process
LISTEN 0      512            [::1]:36275         [::]:*    users:((\"claude\",pid=881,fd=15))
LISTEN 0      512            [::1]:44901         [::]:*    users:((\"node\",pid=100,fd=3))
";
        assert_eq!(parse_listen_ports(ss_output), vec![36275, 44901]);
    }

    #[test]
    fn parse_listen_ports_empty() {
        let ss_output = "State  Recv-Q Send-Q Local Address:Port\n";
        assert!(parse_listen_ports(ss_output).is_empty());
    }

    #[test]
    fn parse_listen_ports_ipv4() {
        let ss_output = "LISTEN 0 512 127.0.0.1:42000 0.0.0.0:*\n";
        assert_eq!(parse_listen_ports(ss_output), vec![42000]);
    }

    #[test]
    fn diff_ports_finds_new() {
        let baseline = vec![8080, 3000];
        let current = vec![8080, 3000, 42561];
        assert_eq!(diff_ports(&current, &baseline), vec![42561]);
    }

    #[test]
    fn diff_ports_no_change() {
        let baseline = vec![8080, 3000];
        let current = vec![8080, 3000];
        assert!(diff_ports(&current, &baseline).is_empty());
    }

    #[test]
    fn diff_ports_removed_port_ignored() {
        let baseline = vec![8080, 3000];
        let current = vec![8080, 42561];
        assert_eq!(diff_ports(&current, &baseline), vec![42561]);
    }

    #[test]
    fn diff_ports_empty_baseline() {
        let current = vec![42561];
        assert_eq!(diff_ports(&current, &[]), vec![42561]);
    }

    #[test]
    fn diff_ports_multiple_new() {
        let baseline = vec![8080];
        let current = vec![8080, 42561, 43000];
        assert_eq!(diff_ports(&current, &baseline), vec![42561, 43000]);
    }

    #[test]
    fn shell_escape_simple() {
        assert_eq!(shell_escape("hello"), "'hello'");
    }

    #[test]
    fn shell_escape_with_quotes() {
        assert_eq!(shell_escape("it's"), "'it'\\''s'");
    }

    #[test]
    fn shell_escape_with_special_chars() {
        assert_eq!(shell_escape("a & b | c"), "'a & b | c'");
    }
}
