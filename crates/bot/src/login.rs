//! SSH exec-based Claude login flow.
//!
//! Spawns `claude auth login` via SSH into the sandbox, parses the OAuth URL
//! and state from stdout, discovers the callback port via `ss -tlnp`, then
//! submits the auth code via `curl` to the local callback server.

use std::path::Path;

use tokio::io::AsyncBufReadExt as _;
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

/// Holds the running `claude auth login` SSH process and extracted OAuth state.
struct AuthSession {
    url: String,
    state: String,
    child: tokio::process::Child,
    /// Keep stdout pipe alive so the SSH process doesn't get SIGPIPE.
    _stdout: tokio::io::Lines<tokio::io::BufReader<tokio::process::ChildStdout>>,
}

/// Orchestrate the Claude login flow via SSH exec and local HTTP callback.
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

    // Step 1: Start auth session, extract URL and state
    let mut auth_session = match start_auth_session(ssh_config_path, &ssh_host, agent_name).await {
        Ok(s) => s,
        Err(e) => {
            let _ = event_tx
                .send(LoginEvent::Error(format!(
                    "failed to start auth session: {e:#}"
                )))
                .await;
            return;
        }
    };

    // Step 2: Send URL to caller
    let _ = event_tx.send(LoginEvent::Url(auth_session.url.clone())).await;

    // Step 3: Discover callback port
    let port = match discover_callback_port(ssh_config_path, &ssh_host, agent_name).await {
        Ok(p) => p,
        Err(e) => {
            let _ = event_tx
                .send(LoginEvent::Error(format!(
                    "failed to discover callback port: {e:#}"
                )))
                .await;
            return;
        }
    };

    // Step 4: Signal waiting for code
    let _ = event_tx.send(LoginEvent::WaitingForCode).await;

    // Step 5: Wait for auth code from Telegram
    let code = match code_rx.await {
        Ok(c) => c,
        Err(_) => {
            let _ = event_tx
                .send(LoginEvent::Error(
                    "auth code channel closed (timeout?)".into(),
                ))
                .await;
            return;
        }
    };

    // Step 6: Submit auth code and wait for process exit
    match submit_auth_code(
        ssh_config_path,
        &ssh_host,
        agent_name,
        port,
        &code,
        &auth_session.state,
        &mut auth_session.child,
    )
    .await
    {
        Ok(()) => {
            let _ = event_tx.send(LoginEvent::Done).await;
        }
        Err(e) => {
            let _ = event_tx
                .send(LoginEvent::Error(format!("auth code submission failed: {e:#}")))
                .await;
        }
    }
}

/// Start `claude auth login` via SSH, read stdout until the OAuth URL appears.
///
/// Returns the extracted URL, OAuth state parameter, and the running child process.
async fn start_auth_session(
    ssh_config_path: &Path,
    ssh_host: &str,
    agent_name: &str,
) -> Result<AuthSession, miette::Report> {
    tracing::info!(agent = agent_name, "login: spawning claude auth login via SSH");

    let mut child = tokio::process::Command::new("ssh")
        .arg("-F")
        .arg(ssh_config_path)
        .arg(ssh_host)
        .arg("--")
        .arg("claude auth login")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .stdin(std::process::Stdio::null())
        .spawn()
        .map_err(|e| miette::miette!("failed to spawn SSH for auth: {e}"))?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| miette::miette!("no stdout from SSH process"))?;

    let mut reader = tokio::io::BufReader::new(stdout).lines();

    let url = tokio::time::timeout(std::time::Duration::from_secs(30), async {
        while let Some(line) = reader
            .next_line()
            .await
            .map_err(|e| miette::miette!("reading stdout: {e}"))?
        {
            let clean = strip_ansi(&line);
            tracing::debug!(agent = agent_name, line = %clean, "login: auth session stdout");
            if clean.contains("https://") && clean.contains("code_challenge") {
                return Ok::<String, miette::Report>(extract_url_from_text(&clean));
            }
        }
        Err(miette::miette!(
            "SSH process exited without producing OAuth URL"
        ))
    })
    .await
    .map_err(|_| miette::miette!("timed out waiting for OAuth URL (30s)"))??;

    let state = extract_state_param(&url)
        .ok_or_else(|| miette::miette!("no state parameter in OAuth URL: {url}"))?;

    tracing::info!(agent = agent_name, url_len = url.len(), state = %state, "login: OAuth URL extracted");

    Ok(AuthSession { url, state, child, _stdout: reader })
}

/// Discover the callback port that `claude auth login` is listening on.
///
/// Process names are not visible across SSH sessions in the sandbox (PID
/// namespace isolation), so we cannot use `ss -tlnp` to identify the owner.
/// Instead, list all LISTEN ports via `ss -tln`, then probe each with
/// `curl /callback` — claude's server responds HTTP 400, others give 000
/// (connection refused or non-HTTP).
async fn discover_callback_port(
    ssh_config_path: &Path,
    ssh_host: &str,
    agent_name: &str,
) -> Result<u16, miette::Report> {
    for attempt in 1..=5 {
        tracing::debug!(agent = agent_name, attempt, "login: probing for callback port");
        let ss_output =
            match rightclaw::openshell::ssh_exec(ssh_config_path, ssh_host, &["ss", "-tln"], 10)
                .await
            {
                Ok(o) => o,
                Err(e) => {
                    tracing::debug!(agent = agent_name, attempt, error = %e, "login: ss failed");
                    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                    continue;
                }
            };

        let ports = parse_listen_ports(&ss_output);
        tracing::info!(agent = agent_name, attempt, ?ports, ss_output = %ss_output.trim(), "login: found LISTEN ports");
        for port in &ports {
            let url = format!("http://localhost:{port}/callback");
            if let Ok(status) = rightclaw::openshell::ssh_exec(
                ssh_config_path,
                ssh_host,
                &["curl", "-s", "-m", "2", "-o", "/dev/null", "-w", "%{http_code}", &url],
                5,
            )
            .await
            {
                let code = status.trim();
                tracing::info!(agent = agent_name, port, http_status = %code, "login: probe result");
                if code == "400" || code == "404" || code == "200" || code == "302" {
                    tracing::info!(agent = agent_name, port, "login: discovered callback port");
                    return Ok(*port);
                }
            }
        }

        if attempt < 5 {
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        }
    }
    Err(miette::miette!(
        "failed to discover claude callback port after 5 attempts"
    ))
}

/// Submit the auth code to the callback server and wait for the process to exit.
async fn submit_auth_code(
    ssh_config_path: &Path,
    ssh_host: &str,
    agent_name: &str,
    port: u16,
    code: &str,
    state: &str,
    child: &mut tokio::process::Child,
) -> Result<(), miette::Report> {
    let encoded_code = urlencoding::encode(code);
    let encoded_state = urlencoding::encode(state);
    let callback_url = format!(
        "http://localhost:{port}/callback?code={encoded_code}&state={encoded_state}"
    );

    // Check if auth process is still alive before submitting
    let child_alive = child.try_wait().map_or(true, |s| s.is_none());
    tracing::info!(
        agent = agent_name,
        port,
        child_alive,
        callback_url = %callback_url,
        code_len = code.len(),
        "login: submitting auth code via curl"
    );

    // First check if the port is still listening
    let port_check = rightclaw::openshell::ssh_exec(
        ssh_config_path,
        ssh_host,
        &["ss", "-tln"],
        5,
    )
    .await;
    tracing::info!(
        agent = agent_name,
        ss_output = %port_check.as_deref().unwrap_or("FAILED"),
        "login: port check before curl"
    );

    // Use -v for verbose curl output on stderr, capture both stdout and stderr
    let output = rightclaw::openshell::ssh_exec(
        ssh_config_path,
        ssh_host,
        &["curl", "-sv", "-o", "/dev/null", "-w", "%{http_code}", &callback_url],
        30,
    )
    .await
    .map_err(|e| miette::miette!("curl callback failed: {e:#}"))?;

    let status_code = output.trim();
    tracing::info!(agent = agent_name, status = %status_code, "login: callback HTTP status");

    if status_code != "302" && status_code != "200" {
        return Err(miette::miette!(
            "callback returned unexpected HTTP status: {status_code}"
        ));
    }

    // Wait for claude auth login process to exit (success confirmation)
    let exit = tokio::time::timeout(std::time::Duration::from_secs(30), child.wait())
        .await
        .map_err(|_| miette::miette!("timed out waiting for claude auth login to exit (30s)"))?
        .map_err(|e| miette::miette!("waiting for process exit: {e}"))?;

    if exit.success() {
        tracing::info!(agent = agent_name, "login: claude auth login exited successfully");
        Ok(())
    } else {
        Err(miette::miette!(
            "claude auth login exited with status: {exit}"
        ))
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
///
/// Parses lines like:
/// ```text
/// LISTEN 0  512  [::1]:36275  [::]:*
/// LISTEN 0  512  127.0.0.1:42000  0.0.0.0:*
/// ```
pub fn parse_listen_ports(ss_output: &str) -> Vec<u16> {
    let mut ports = Vec::new();
    for line in ss_output.lines() {
        if !line.contains("LISTEN") {
            continue;
        }
        // The 4th whitespace-separated field is Local Address:Port
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
                // CSI sequence -- consume until letter
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
}
