//! Token-based Claude login flow.
//!
//! On auth error, instructs the user to run `claude setup-token` on their
//! machine and send the resulting token via Telegram. The token is stored
//! in memory.db and passed as `CLAUDE_CODE_OAUTH_TOKEN` env var to all
//! subsequent `claude -p` invocations.

use std::path::Path;

use tokio::sync::{mpsc, oneshot};

const TOKEN_INSTRUCTION: &str = "\
To authenticate this agent, run on your machine:
```
claude setup-token
```
Then send me the token it prints.";

/// Events emitted during the token request flow.
#[derive(Debug)]
pub enum LoginEvent {
    /// Login completed — token saved.
    Done,
    /// Login failed.
    Error(String),
}

/// Request an auth token from the user via Telegram and save it.
///
/// Communicates via channels:
/// - `event_tx`: sends `LoginEvent`s to the orchestrator
/// - `token_rx`: receives the token string from the Telegram handler
///
/// `agent_dir` is the agent directory (memory.db lives inside it).
pub async fn request_token(
    agent_dir: &Path,
    agent_name: &str,
    event_tx: mpsc::Sender<LoginEvent>,
    token_rx: oneshot::Receiver<String>,
) {
    let db_path = agent_dir.join("memory.db");

    // Delete stale token if present
    match open_db_and_delete_stale(&db_path) {
        Ok(()) => {}
        Err(e) => {
            let _ = event_tx.send(LoginEvent::Error(format!("DB error: {e}"))).await;
            return;
        }
    }

    // Wait for token from Telegram
    tracing::info!(agent = agent_name, "login: waiting for setup-token from user");
    let token = match token_rx.await {
        Ok(t) => t.trim().to_string(),
        Err(_) => {
            let _ = event_tx.send(LoginEvent::Error("token channel closed (timeout?)".into())).await;
            return;
        }
    };

    if token.is_empty() {
        let _ = event_tx.send(LoginEvent::Error("received empty token".into())).await;
        return;
    }

    tracing::info!(agent = agent_name, token_len = token.len(), "login: received token, saving");

    // Save token
    match save_token(&db_path, &token) {
        Ok(()) => {
            let _ = event_tx.send(LoginEvent::Done).await;
        }
        Err(e) => {
            let _ = event_tx.send(LoginEvent::Error(format!("failed to save token: {e}"))).await;
        }
    }
}

/// Open DB and delete any existing auth token (it's stale if we're here).
fn open_db_and_delete_stale(db_path: &Path) -> Result<(), String> {
    let conn = rusqlite::Connection::open(db_path)
        .map_err(|e| format!("open db: {e}"))?;
    rightclaw::memory::store::delete_auth_token(&conn)
        .map_err(|e| format!("delete stale token: {e}"))?;
    Ok(())
}

/// Save token to DB.
fn save_token(db_path: &Path, token: &str) -> Result<(), String> {
    let conn = rusqlite::Connection::open(db_path)
        .map_err(|e| format!("open db: {e}"))?;
    rightclaw::memory::store::save_auth_token(&conn, token)
        .map_err(|e| format!("save token: {e}"))?;
    Ok(())
}

/// Read the auth token from DB, if any.
///
/// `agent_dir` is the agent directory (memory.db lives inside it).
pub fn load_auth_token(agent_dir: &Path) -> Option<String> {
    let conn = rusqlite::Connection::open(agent_dir.join("memory.db")).ok()?;
    rightclaw::memory::store::get_auth_token(&conn).ok().flatten()
}

/// Instruction message sent to user when auth is needed.
pub fn auth_instruction_message() -> &'static str {
    TOKEN_INSTRUCTION
}
