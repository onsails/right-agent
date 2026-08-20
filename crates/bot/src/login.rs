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
const MIN_SETUP_TOKEN_LENGTH: usize = 80;
const INVALID_TOKEN_MESSAGE: &str = "That does not look like a Claude setup token. Run `claude setup-token` and paste the full token.";

/// Events emitted during the token request flow.
#[derive(Debug)]
pub(crate) enum LoginEvent {
    /// The submitted token is being saved. Acknowledged after delivery.
    Saving(oneshot::Sender<()>),
    /// Login completed — token saved.
    Done,
    /// Login failed.
    Error(String),
}

/// Locally validate and save a token already submitted through Telegram.
///
/// Communicates saving progress and the final result to the owning request
/// task through `event_tx`. The caller owns this future for the entire save so
/// dropping the request cancels it before cleanup.
///
/// `agent_dir` is the agent directory (data.db lives inside it).
pub(crate) async fn validate_submitted_token(
    agent_dir: &Path,
    agent_name: &str,
    event_tx: mpsc::Sender<LoginEvent>,
    token: String,
) {
    tracing::info!(agent = agent_name, "login: saving submitted setup-token");
    let (validation_sent_tx, validation_sent_rx) = oneshot::channel();
    if event_tx
        .send(LoginEvent::Saving(validation_sent_tx))
        .await
        .is_err()
    {
        return;
    }
    if validation_sent_rx.await.is_err() {
        return;
    }
    match finish_token_request(agent_dir, &token).await {
        Ok(()) => {
            let _ = event_tx.send(LoginEvent::Done).await;
        }
        Err(error) => {
            let _ = event_tx.send(LoginEvent::Error(error)).await;
        }
    }
}

fn is_plausible_setup_token(token: &str) -> bool {
    token.len() >= MIN_SETUP_TOKEN_LENGTH
        && token.is_ascii()
        && token.bytes().all(|byte| !byte.is_ascii_whitespace())
}

async fn finish_token_request(agent_dir: &Path, token: &str) -> Result<(), String> {
    if !is_plausible_setup_token(token) {
        return Err(INVALID_TOKEN_MESSAGE.to_owned());
    }
    save_token(agent_dir, token).await
}

/// Save a token after local syntax validation.
async fn save_token(agent_dir: &Path, token: &str) -> Result<(), String> {
    let conn = right_db::open_connection(agent_dir, false)
        .await
        .map_err(|e| format!("open db: {e:#}"))?;
    right_mcp::credentials::save_auth_token(&conn, token)
        .await
        .map_err(|e| format!("save token: {e:#}"))?;
    Ok(())
}

/// Read the auth token from DB, if any.
///
/// `agent_dir` is the agent directory (data.db lives inside it).
pub(crate) async fn load_auth_token(agent_dir: &Path) -> Option<String> {
    let conn = right_db::open_connection(agent_dir, false).await.ok()?;
    right_mcp::credentials::get_auth_token(&conn)
        .await
        .ok()
        .flatten()
}

/// Instruction message sent to user when auth is needed.
pub(crate) fn auth_instruction_message() -> &'static str {
    TOKEN_INSTRUCTION
}

#[cfg(test)]
mod tests {
    use super::{
        INVALID_TOKEN_MESSAGE, auth_instruction_message, finish_token_request,
        is_plausible_setup_token, load_auth_token,
    };
    use tempfile::tempdir;

    async fn init_db(dir: &std::path::Path) {
        right_db::open_connection(dir, true).await.unwrap();
    }

    #[tokio::test]
    async fn load_auth_token_returns_none_when_no_token() {
        let dir = tempdir().unwrap();
        init_db(dir.path()).await;
        assert!(load_auth_token(dir.path()).await.is_none());
    }

    #[tokio::test]
    async fn load_auth_token_returns_saved_token() {
        let dir = tempdir().unwrap();
        init_db(dir.path()).await;
        let conn = right_db::open_connection(dir.path(), false).await.unwrap();
        right_mcp::credentials::save_auth_token(&conn, "my-token")
            .await
            .unwrap();
        assert_eq!(
            load_auth_token(dir.path()).await.as_deref(),
            Some("my-token")
        );
    }

    #[test]
    fn plausible_setup_token_accepts_long_opaque_value() {
        assert!(is_plausible_setup_token(&"a".repeat(108)));
    }

    #[test]
    fn plausible_setup_token_rejects_short_ordinary_text() {
        assert!(!is_plausible_setup_token("restart authentication"));
    }

    #[test]
    fn plausible_setup_token_rejects_internal_whitespace() {
        let candidate = format!("{} {}", "a".repeat(53), "b".repeat(54));

        assert_eq!(candidate.len(), 108);
        assert!(!is_plausible_setup_token(&candidate));
    }

    #[tokio::test]
    async fn locally_valid_token_is_saved() {
        let dir = tempdir().unwrap();
        init_db(dir.path()).await;
        let token = "a".repeat(108);

        finish_token_request(dir.path(), &token).await.unwrap();

        assert_eq!(
            load_auth_token(dir.path()).await.as_deref(),
            Some(token.as_str())
        );
    }

    #[tokio::test]
    async fn malformed_token_is_not_saved_or_diagnosed() {
        let dir = tempdir().unwrap();
        init_db(dir.path()).await;
        let token = "secret-invalid\ntoken";

        let error = finish_token_request(dir.path(), token)
            .await
            .expect_err("malformed candidate must be rejected");

        assert_eq!(error, INVALID_TOKEN_MESSAGE);
        assert!(!error.contains(token));
        assert!(load_auth_token(dir.path()).await.is_none());
    }

    #[tokio::test]
    async fn malformed_replacement_preserves_existing_token() {
        let dir = tempdir().unwrap();
        init_db(dir.path()).await;
        let conn = right_db::open_connection(dir.path(), false).await.unwrap();
        right_mcp::credentials::save_auth_token(&conn, "existing-token")
            .await
            .unwrap();
        drop(conn);

        finish_token_request(dir.path(), "restart authentication")
            .await
            .expect_err("malformed candidate must be rejected");

        assert_eq!(
            load_auth_token(dir.path()).await.as_deref(),
            Some("existing-token")
        );
    }

    #[tokio::test]
    async fn auth_instruction_message_mentions_setup_token() {
        assert!(
            auth_instruction_message().contains("claude setup-token"),
            "instruction message must mention `claude setup-token`"
        );
    }
}
