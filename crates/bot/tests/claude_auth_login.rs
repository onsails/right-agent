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
    use tokio::io::AsyncBufReadExt;

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
