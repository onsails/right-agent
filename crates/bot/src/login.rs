//! Token-based Claude login flow.
//!
//! On auth error, instructs the user to run `claude setup-token` on their
//! machine and send the resulting token via Telegram. The token is stored
//! in data.db and passed as `CLAUDE_CODE_OAUTH_TOKEN` env var to all
//! subsequent `claude -p` invocations.

use std::path::Path;

use tokio::sync::{mpsc, oneshot};

const TOKEN_INSTRUCTION: &str = "\
To authenticate this agent, run on your machine:\n\n\
<pre>claude setup-token</pre>\n\n\
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
/// `agent_dir` is the agent directory (data.db lives inside it).
pub async fn request_token(
    agent_dir: &Path,
    agent_name: &str,
    event_tx: mpsc::Sender<LoginEvent>,
    token_rx: oneshot::Receiver<String>,
) {
    // Delete stale token if present
    match open_db_and_delete_stale(agent_dir) {
        Ok(()) => {}
        Err(e) => {
            let _ = event_tx
                .send(LoginEvent::Error(format!("DB error: {e}")))
                .await;
            return;
        }
    }

    // Wait for token from Telegram
    tracing::info!(
        agent = agent_name,
        "login: waiting for setup-token from user"
    );
    let token = match token_rx.await {
        Ok(t) => t.trim().to_string(),
        Err(_) => {
            let _ = event_tx
                .send(LoginEvent::Error("token channel closed (timeout?)".into()))
                .await;
            return;
        }
    };

    if token.is_empty() {
        let _ = event_tx
            .send(LoginEvent::Error("received empty token".into()))
            .await;
        return;
    }

    tracing::info!(
        agent = agent_name,
        token_len = token.len(),
        "login: received token, saving"
    );

    // Save token
    match save_token(agent_dir, &token) {
        Ok(()) => {
            let _ = event_tx.send(LoginEvent::Done).await;
        }
        Err(e) => {
            let _ = event_tx
                .send(LoginEvent::Error(format!("failed to save token: {e}")))
                .await;
        }
    }
}

/// Open DB and delete any existing auth token (it's stale if we're here).
fn open_db_and_delete_stale(agent_dir: &Path) -> Result<(), String> {
    let conn = right_db::open_connection(agent_dir, false).map_err(|e| format!("open db: {e}"))?;
    right_agent::mcp::credentials::delete_auth_token(&conn)
        .map_err(|e| format!("delete stale token: {e}"))?;
    Ok(())
}

/// Save token to DB.
fn save_token(agent_dir: &Path, token: &str) -> Result<(), String> {
    let conn = right_db::open_connection(agent_dir, false).map_err(|e| format!("open db: {e}"))?;
    right_agent::mcp::credentials::save_auth_token(&conn, token)
        .map_err(|e| format!("save token: {e}"))?;
    Ok(())
}

/// Read the auth token from DB, if any.
///
/// `agent_dir` is the agent directory (data.db lives inside it).
pub fn load_auth_token(agent_dir: &Path) -> Option<String> {
    let conn = right_db::open_connection(agent_dir, false).ok()?;
    right_agent::mcp::credentials::get_auth_token(&conn)
        .ok()
        .flatten()
}

/// Instruction message sent to user when auth is needed.
pub fn auth_instruction_message() -> &'static str {
    TOKEN_INSTRUCTION
}

#[cfg(test)]
mod tests {
    use super::{LoginEvent, auth_instruction_message, load_auth_token, request_token};
    use tempfile::tempdir;
    use tokio::sync::{mpsc, oneshot};

    fn init_db(dir: &std::path::Path) {
        right_db::open_db(dir, true).unwrap();
    }

    #[test]
    fn load_auth_token_returns_none_when_no_token() {
        let dir = tempdir().unwrap();
        init_db(dir.path());
        assert!(load_auth_token(dir.path()).is_none());
    }

    #[test]
    fn load_auth_token_returns_saved_token() {
        let dir = tempdir().unwrap();
        init_db(dir.path());
        let conn = right_db::open_connection(dir.path(), false).unwrap();
        right_agent::mcp::credentials::save_auth_token(&conn, "my-token").unwrap();
        assert_eq!(load_auth_token(dir.path()).as_deref(), Some("my-token"));
    }

    #[tokio::test]
    async fn request_token_saves_and_signals_done() {
        let dir = tempdir().unwrap();
        init_db(dir.path());

        let (event_tx, mut event_rx) = mpsc::channel(8);
        let (token_tx, token_rx) = oneshot::channel();

        let agent_dir = dir.path().to_path_buf();
        let task = tokio::spawn(async move {
            request_token(&agent_dir, "test-agent", event_tx, token_rx).await;
        });

        token_tx.send("my-new-token".to_string()).unwrap();
        task.await.unwrap();

        let event = event_rx.recv().await.unwrap();
        assert!(matches!(event, LoginEvent::Done));
        assert_eq!(load_auth_token(dir.path()).as_deref(), Some("my-new-token"));
    }

    #[tokio::test]
    async fn request_token_rejects_empty_token() {
        let dir = tempdir().unwrap();
        init_db(dir.path());

        let (event_tx, mut event_rx) = mpsc::channel(8);
        let (token_tx, token_rx) = oneshot::channel();

        let agent_dir = dir.path().to_path_buf();
        let task = tokio::spawn(async move {
            request_token(&agent_dir, "test-agent", event_tx, token_rx).await;
        });

        token_tx.send(String::new()).unwrap();
        task.await.unwrap();

        let event = event_rx.recv().await.unwrap();
        assert!(matches!(event, LoginEvent::Error(_)));
    }

    #[tokio::test]
    async fn request_token_deletes_stale_before_waiting() {
        let dir = tempdir().unwrap();
        init_db(dir.path());

        // Pre-seed a stale token
        {
            let conn = right_db::open_connection(dir.path(), false).unwrap();
            right_agent::mcp::credentials::save_auth_token(&conn, "stale-token").unwrap();
        }
        assert_eq!(load_auth_token(dir.path()).as_deref(), Some("stale-token"));

        let (event_tx, mut event_rx) = mpsc::channel(8);
        let (token_tx, token_rx) = oneshot::channel::<String>();

        let agent_dir = dir.path().to_path_buf();
        // Use a barrier to confirm stale deletion happened before we send the new token.
        // Since request_token deletes stale synchronously before awaiting token_rx,
        // we can verify by completing the task and checking the final DB state.
        let task = tokio::spawn(async move {
            request_token(&agent_dir, "test-agent", event_tx, token_rx).await;
        });

        // Yield once so the task can run its synchronous delete
        tokio::task::yield_now().await;

        // Stale token should now be gone
        assert!(
            load_auth_token(dir.path()).is_none(),
            "stale token should be deleted before request_token awaits the new token"
        );

        token_tx.send("fresh-token".to_string()).unwrap();
        task.await.unwrap();

        let event = event_rx.recv().await.unwrap();
        assert!(matches!(event, LoginEvent::Done));
        assert_eq!(load_auth_token(dir.path()).as_deref(), Some("fresh-token"));
    }

    #[test]
    fn auth_instruction_message_mentions_setup_token() {
        assert!(
            auth_instruction_message().contains("claude setup-token"),
            "instruction message must mention `claude setup-token`"
        );
    }
}
