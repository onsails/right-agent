//! Token-based Claude login flow.
//!
//! On auth error, instructs the user to run `claude setup-token` on their
//! machine and send the resulting token via Telegram. The token is stored by
//! the Aggregator-owned database and passed as `CLAUDE_CODE_OAUTH_TOKEN` to
//! subsequent `claude -p` invocations.

use anyhow::Context as _;
use secrecy::ExposeSecret as _;
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

/// Save a token after local syntax validation through typed owner IPC.
async fn save_token(agent_dir: &Path, token: &str) -> Result<(), String> {
    let (client, agent) = internal_client_for_agent_dir(agent_dir)?;
    client
        .auth_token_save(&right_mcp::internal_db::AuthTokenSaveRequest {
            agent,
            request_id: uuid::Uuid::new_v4().to_string(),
            token: secrecy::SecretString::from(token.to_owned()),
        })
        .await
        .map(drop)
        .map_err(|e| format!("save token through database owner: {e:#}"))
}

/// Read the auth token from the Aggregator-owned database, if any.
///
/// Owner/transport failures propagate so callers never mistake an unavailable
/// token store for an agent that has not authenticated (#196).
pub(crate) async fn load_auth_token(agent_dir: &Path) -> anyhow::Result<Option<String>> {
    let (client, agent) = internal_client_for_agent_dir(agent_dir)
        .map_err(anyhow::Error::msg)
        .context("resolve internal database client")?;
    let response = client
        .auth_token_get(&right_mcp::internal_db::AuthTokenGetRequest { agent })
        .await
        .context("query auth token through database owner")?;
    Ok(response.token.map(|token| token.expose_secret().to_owned()))
}

fn internal_client_for_agent_dir(
    agent_dir: &Path,
) -> Result<(right_mcp::internal_client::InternalClient, String), String> {
    crate::db::client_for_agent_dir(agent_dir).map_err(|error| format!("{error:#}"))
}

/// Instruction message sent to user when auth is needed.
pub(crate) fn auth_instruction_message() -> &'static str {
    TOKEN_INSTRUCTION
}

#[cfg(test)]
#[path = "login_tests.rs"]
mod tests;
