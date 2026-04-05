//! Teloxide endpoint handlers: message dispatch + /reset command + /mcp + /doctor.
//!
//! handle_message: routes incoming text to the per-session worker via DashMap.
//! handle_reset: deletes the session row for the current thread.
//! handle_mcp: MCP server management (list/auth/add/remove).
//! handle_doctor: runs rightclaw doctor and returns results.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use dashmap::DashMap;
use tokio::sync::mpsc;
use teloxide::prelude::*;
use teloxide::types::Message;
use teloxide::RequestError;

use super::oauth_callback::PendingAuthMap;
use super::session::{delete_session, effective_thread_id};
use super::worker::{DebounceMsg, SessionKey, WorkerContext, spawn_worker};
use super::BotType;

/// Newtype wrapper for the agent directory passed via dptree dependencies.
/// Distinct from RightclawHome to prevent TypeId collision in dptree.
#[derive(Clone)]
pub struct AgentDir(pub PathBuf);

/// Newtype wrapper for the rightclaw home directory passed via dptree dependencies.
/// Distinct from AgentDir to prevent TypeId collision in dptree.
#[derive(Clone)]
pub struct RightclawHome(pub PathBuf);

/// Newtype wrapper for the debug flag passed via dptree dependencies.
#[derive(Clone)]
pub struct DebugFlag(pub bool);

/// Convert an arbitrary error into `RequestError::Io` so it propagates through `ResponseResult`.
fn to_request_err(e: impl std::fmt::Display) -> RequestError {
    RequestError::Io(std::io::Error::other(e.to_string()).into())
}

/// Handle an incoming text message.
///
/// 1. Compute effective_thread_id (normalise General topic).
/// 2. Look up existing sender in DashMap or spawn a new worker task.
/// 3. Send the message into the worker's mpsc channel.
///
/// Serialisation guarantee (SES-05): all messages to the same (chat_id, thread_id)
/// go through the same mpsc channel -> worker processes them serially.
pub async fn handle_message(
    bot: BotType,
    msg: Message,
    worker_map: Arc<DashMap<SessionKey, mpsc::Sender<DebounceMsg>>>,
    agent_dir: Arc<AgentDir>,
    debug_flag: Arc<DebugFlag>,
) -> ResponseResult<()> {
    // Only process messages with text (ignore stickers, photos, etc. in Phase 25)
    let text = match msg.text() {
        Some(t) => t.to_string(),
        None => return Ok(()),
    };

    let chat_id = msg.chat.id;
    let eff_thread_id = effective_thread_id(&msg);
    let key: SessionKey = (chat_id.0, eff_thread_id);
    let worker_exists = worker_map.contains_key(&key);
    tracing::info!(?key, worker_exists, "handle_message: routing");

    let debounce_msg = DebounceMsg {
        message_id: msg.id.0,
        text,
        timestamp: chrono::Utc::now(),
    };

    // Check for existing worker or spawn a new one.
    // Pitfall 7 mitigation: if send fails, the worker task has exited -- remove + respawn.
    // Note: DashMap read guard is NOT held across .await to avoid blocking. Clone the
    // sender before awaiting.
    loop {
        let maybe_tx = worker_map.get(&key).map(|entry| entry.value().clone());
        match maybe_tx {
            Some(tx) => match tx.send(debounce_msg.clone()).await {
                Ok(_) => break,
                Err(e) => {
                    // Worker task panicked or exited -- remove stale sender and respawn
                    tracing::warn!(?key, "worker send failed, respawning: {:#}", e);
                    worker_map.remove(&key);
                    // fall through to spawn new worker below on next loop iteration
                }
            },
            None => {
                // No sender yet -- spawn a new worker task
                let agent_name = agent_dir.0
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("unknown")
                    .to_string();
                let ctx = WorkerContext {
                    chat_id,
                    effective_thread_id: eff_thread_id,
                    agent_dir: agent_dir.0.clone(),
                    agent_name,
                    bot: bot.clone(),
                    db_path: agent_dir.0.clone(),
                    debug: debug_flag.0,
                };
                let tx = spawn_worker(key, ctx, Arc::clone(&worker_map));
                worker_map.insert(key, tx.clone());
                // Send to the freshly spawned worker
                if let Err(e) = tx.send(debounce_msg).await {
                    tracing::error!(?key, "send to freshly spawned worker failed: {:#}", e);
                }
                break;
            }
        }
    }

    Ok(())
}

/// Handle the /start command.
///
/// Sends a greeting without invoking CC. Cron runtime starts automatically
/// alongside the bot -- no explicit bootstrap needed.
pub async fn handle_start(
    bot: BotType,
    msg: Message,
) -> ResponseResult<()> {
    bot.send_message(msg.chat.id, "Agent is running. Send a message to start.").await?;
    Ok(())
}

/// Handle the /reset command.
///
/// Deletes the telegram_sessions row for the current (chat_id, effective_thread_id).
/// Also removes the worker sender from DashMap so the worker task exits cleanly.
/// Next message will create a fresh session with a new UUID (SES-06).
///
/// Both DB errors propagate -- a failed reset is surfaced to the caller so the dispatcher
/// can log it and teloxide can handle the update appropriately.
pub async fn handle_reset(
    bot: BotType,
    msg: Message,
    worker_map: Arc<DashMap<SessionKey, mpsc::Sender<DebounceMsg>>>,
    agent_dir: Arc<AgentDir>,
) -> ResponseResult<()> {
    let chat_id = msg.chat.id;
    let eff_thread_id = effective_thread_id(&msg);
    let key: SessionKey = (chat_id.0, eff_thread_id);

    // Remove the worker sender -- channel closes, worker task exits and removes its own entry
    worker_map.remove(&key);

    // Delete session from DB -- errors propagate via `?` (CLAUDE.rust.md: fail fast)
    let conn = rightclaw::memory::open_connection(&agent_dir.0)
        .map_err(|e| to_request_err(format!("reset: open DB: {:#}", e)))?;
    delete_session(&conn, chat_id.0, eff_thread_id)
        .map_err(|e| to_request_err(format!("reset: delete session: {:#}", e)))?;

    tracing::info!(?key, "session reset");

    // Send confirmation reply
    let mut send =
        bot.send_message(chat_id, "Session reset. Next message starts a fresh conversation.");
    if eff_thread_id != 0 {
        send = send.message_thread_id(teloxide::types::ThreadId(
            teloxide::types::MessageId(eff_thread_id as i32),
        ));
    }
    send.await?;

    Ok(())
}

// ---------------------------------------------------------------------------
// /mcp command handler
// ---------------------------------------------------------------------------

/// Handle the /mcp command -- routes to subcommands: list, auth, add, remove.
///
/// Teloxide captures everything after `/mcp` as a single String (RESEARCH.md Pitfall 9).
/// We split manually and dispatch.
pub async fn handle_mcp(
    bot: BotType,
    msg: Message,
    args: String,
    agent_dir: Arc<AgentDir>,
    pending_auth: PendingAuthMap,
    home: Arc<RightclawHome>,
) -> ResponseResult<()> {
    tracing::info!(agent_dir = %agent_dir.0.display(), "mcp: dispatching");
    let parts: Vec<&str> = args.split_whitespace().collect();
    let result = match parts.first().copied() {
        None | Some("list") => handle_mcp_list(&bot, &msg, &agent_dir.0).await,
        Some("auth") => {
            let server = match parts.get(1) {
                Some(s) => *s,
                None => {
                    bot.send_message(msg.chat.id, "Usage: /mcp auth <server>").await?;
                    return Ok(());
                }
            };
            handle_mcp_auth(&bot, &msg, server, &agent_dir.0, pending_auth, &home.0).await
        }
        Some("add") => {
            let rest = parts[1..].join(" ");
            handle_mcp_add(&bot, &msg, &rest, &agent_dir.0).await
        }
        Some("remove") => {
            let server = match parts.get(1) {
                Some(s) => *s,
                None => {
                    bot.send_message(msg.chat.id, "Usage: /mcp remove <server>").await?;
                    return Ok(());
                }
            };
            handle_mcp_remove(&bot, &msg, server, &agent_dir.0).await
        }
        Some(unknown) => {
            bot.send_message(
                msg.chat.id,
                format!("Unknown /mcp subcommand: {unknown}\nUsage: /mcp [list|add|remove]"),
            )
            .await
            .map(|_| ())
        }
    };
    result.map_err(|e| to_request_err(format!("{e:#}")))?;
    Ok(())
}

/// `/mcp list` -- show all MCP servers from .claude.json and .mcp.json.
async fn handle_mcp_list(
    bot: &BotType,
    msg: &Message,
    agent_dir: &Path,
) -> Result<(), RequestError> {
    tracing::info!(agent_dir = %agent_dir.display(), "mcp list");

    let statuses = match rightclaw::mcp::detect::mcp_auth_status(agent_dir) {
        Ok(s) => s,
        Err(e) => {
            bot.send_message(msg.chat.id, format!("Error reading MCP status: {e:#}"))
                .await?;
            return Ok(());
        }
    };

    if statuses.is_empty() {
        bot.send_message(msg.chat.id, "No MCP servers configured.")
            .await?;
        return Ok(());
    }

    let mut text = String::from("MCP Servers:\n\n");
    for s in &statuses {
        let icon = match s.state {
            rightclaw::mcp::detect::AuthState::Present => "ok",
            rightclaw::mcp::detect::AuthState::Missing => "needs auth",
        };
        text.push_str(&format!(
            "  {} ({}) -- {} [{}]\n",
            s.name, s.source, icon, s.url
        ));
    }
    bot.send_message(msg.chat.id, text).await?;
    Ok(())
}

/// `/mcp auth <server>` -- initiate OAuth flow: discovery, PKCE, send auth URL.
async fn handle_mcp_auth(
    bot: &BotType,
    msg: &Message,
    server_name: &str,
    agent_dir: &Path,
    pending_auth: PendingAuthMap,
    home: &Path,
) -> Result<(), RequestError> {
    tracing::info!(agent_dir = %agent_dir.display(), server = %server_name, "mcp auth");

    // 1. Read .claude.json to find server URL
    let claude_json_path = agent_dir.join(".claude.json");
    let agent_path_key = agent_dir
        .canonicalize()
        .unwrap_or_else(|_| agent_dir.to_path_buf())
        .display()
        .to_string();

    let servers = match rightclaw::mcp::credentials::list_http_servers_from_claude_json(
        &claude_json_path,
        &agent_path_key,
    ) {
        Ok(s) => s,
        Err(e) => {
            bot.send_message(msg.chat.id, format!("Cannot read .claude.json: {e:#}")).await?;
            return Ok(());
        }
    };

    let server_url = match servers.iter().find(|(name, _)| name == server_name) {
        Some((_, url)) => url.clone(),
        None => {
            bot.send_message(
                msg.chat.id,
                format!("Server '{server_name}' not found in .claude.json"),
            )
            .await?;
            return Ok(());
        }
    };

    // 2. Read tunnel config
    let global_config = match rightclaw::config::read_global_config(home) {
        Ok(c) => c,
        Err(e) => {
            bot.send_message(msg.chat.id, format!("Cannot read config.yaml: {e:#}")).await?;
            return Ok(());
        }
    };
    let tunnel = match global_config.tunnel.as_ref() {
        Some(t) => t.clone(),
        None => {
            bot.send_message(
                msg.chat.id,
                "Tunnel not configured. Run:\n  rightclaw init --tunnel-token TOKEN",
            )
            .await?;
            return Ok(());
        }
    };

    // 3. Check cloudflared binary
    if which::which("cloudflared").is_err() {
        bot.send_message(
            msg.chat.id,
            "Error: cloudflared binary not found in PATH. Install cloudflared first.",
        )
        .await?;
        return Ok(());
    }

    // 4. AS discovery
    let http_client = reqwest::Client::new();
    bot.send_message(
        msg.chat.id,
        format!("Discovering OAuth endpoints for {server_name}..."),
    )
    .await?;

    let metadata = match rightclaw::mcp::oauth::discover_as(&http_client, &server_url).await {
        Ok(m) => m,
        Err(e) => {
            bot.send_message(
                msg.chat.id,
                format!("AS discovery failed for {server_name}: {e:#}"),
            )
            .await?;
            return Ok(());
        }
    };

    // 5. DCR or static clientId
    let agent_name = agent_dir
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    if tunnel.hostname.is_empty() {
        bot.send_message(
            msg.chat.id,
            "Tunnel hostname not configured -- run `rightclaw init --tunnel-hostname HOSTNAME`",
        )
        .await?;
        return Ok(());
    }
    let redirect_uri = format!("https://{}/oauth/{agent_name}/callback", tunnel.hostname);
    let (client_id, client_secret) = match rightclaw::mcp::oauth::register_client_or_fallback(
        &http_client,
        &metadata,
        None, // no static clientId from .claude.json -- DCR only
        &redirect_uri,
    )
    .await
    {
        Ok(pair) => pair,
        Err(e) => {
            bot.send_message(
                msg.chat.id,
                format!("Client registration failed: {e:#}"),
            )
            .await?;
            return Ok(());
        }
    };

    // 6. Generate PKCE + state
    let (code_verifier, code_challenge) = rightclaw::mcp::oauth::generate_pkce();
    let state = rightclaw::mcp::oauth::generate_state();

    // 7. Tunnel healthcheck -- hit tunnel root to verify cloudflared is running
    let healthcheck_url = format!("https://{}/", tunnel.hostname);
    match http_client
        .get(&healthcheck_url)
        .timeout(std::time::Duration::from_secs(5))
        .send()
        .await
    {
        Ok(resp) if resp.status().is_server_error() => {
            bot.send_message(
                msg.chat.id,
                format!(
                    "Tunnel healthcheck returned {} -- cloudflared may be misconfigured",
                    resp.status()
                ),
            )
            .await?;
            return Ok(());
        }
        Ok(_) => {} // 2xx/3xx/4xx = tunnel is reachable
        Err(e) => {
            bot.send_message(
                msg.chat.id,
                format!("Tunnel healthcheck failed: {e:#}\nIs cloudflared running?"),
            )
            .await?;
            return Ok(());
        }
    }

    // 8. Store PendingAuth
    let pending = rightclaw::mcp::oauth::PendingAuth {
        server_name: server_name.to_string(),
        server_url: server_url.clone(),
        code_verifier,
        state: state.clone(),
        token_endpoint: metadata.token_endpoint.clone(),
        client_id: client_id.clone(),
        client_secret,
        redirect_uri: redirect_uri.clone(),
        created_at: std::time::Instant::now(),
    };
    pending_auth.lock().await.insert(state.clone(), pending);

    // 9. Build and send auth URL
    let auth_url = rightclaw::mcp::oauth::build_auth_url(
        &metadata, &client_id, &redirect_uri, &state, &code_challenge, None,
    );
    bot.send_message(
        msg.chat.id,
        format!("Authenticate {server_name}:\n\n{auth_url}"),
    )
    .await?;
    Ok(())
}

/// `/mcp add <name> <url>` -- add a server entry to .claude.json.
async fn handle_mcp_add(
    bot: &BotType,
    msg: &Message,
    config_str: &str,
    agent_dir: &Path,
) -> Result<(), RequestError> {
    tracing::info!(agent_dir = %agent_dir.display(), "mcp add");
    let parts: Vec<&str> = config_str.split_whitespace().collect();
    if parts.len() < 2 {
        bot.send_message(msg.chat.id, "Usage: /mcp add <name> <url>")
            .await?;
        return Ok(());
    }
    let name = parts[0];
    let url = parts[1];

    let claude_json_path = agent_dir.join(".claude.json");
    let agent_path_key = agent_dir
        .canonicalize()
        .unwrap_or_else(|_| agent_dir.to_path_buf())
        .display()
        .to_string();

    match rightclaw::mcp::credentials::add_http_server_to_claude_json(
        &claude_json_path,
        &agent_path_key,
        name,
        url,
    ) {
        Ok(()) => {
            bot.send_message(msg.chat.id, format!("Added MCP server: {name} ({url})"))
                .await?;
        }
        Err(e) => {
            bot.send_message(msg.chat.id, format!("Failed to add server: {e:#}"))
                .await?;
        }
    }
    Ok(())
}

/// `/mcp remove <server>` -- remove a server entry from .claude.json.
async fn handle_mcp_remove(
    bot: &BotType,
    msg: &Message,
    server_name: &str,
    agent_dir: &Path,
) -> Result<(), RequestError> {
    tracing::info!(agent_dir = %agent_dir.display(), server = %server_name, "mcp remove");

    let claude_json_path = agent_dir.join(".claude.json");
    let agent_path_key = agent_dir
        .canonicalize()
        .unwrap_or_else(|_| agent_dir.to_path_buf())
        .display()
        .to_string();

    match rightclaw::mcp::credentials::remove_http_server_from_claude_json(
        &claude_json_path,
        &agent_path_key,
        server_name,
    ) {
        Ok(()) => {
            bot.send_message(msg.chat.id, format!("Removed MCP server: {server_name}"))
                .await?;
        }
        Err(rightclaw::mcp::credentials::CredentialError::ServerNotFound(_)) => {
            bot.send_message(
                msg.chat.id,
                format!("Server '{server_name}' not found in .claude.json"),
            )
            .await?;
        }
        Err(e) => {
            bot.send_message(msg.chat.id, format!("Failed to remove server: {e:#}"))
                .await?;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// /doctor command handler
// ---------------------------------------------------------------------------

/// Handle the /doctor command -- run all doctor checks and return results.
pub async fn handle_doctor(
    bot: BotType,
    msg: Message,
    home: Arc<RightclawHome>,
) -> ResponseResult<()> {
    tracing::info!("handle_doctor: running diagnostics");
    let checks = rightclaw::doctor::run_doctor(&home.0);
    let mut body = String::new();
    for check in &checks {
        body.push_str(&format!("{check}\n"));
    }
    let pass_count = checks
        .iter()
        .filter(|c| matches!(c.status, rightclaw::doctor::CheckStatus::Pass))
        .count();
    body.push_str(&format!("\n{pass_count}/{} checks passed", checks.len()));
    // HTML-escape body before wrapping in <pre> -- doctor output may contain <, >, &
    let escaped = body.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
    let text = format!("Doctor results:\n\n<pre>{}</pre>", escaped);
    if let Err(e) = bot.send_message(msg.chat.id, &text)
        .parse_mode(teloxide::types::ParseMode::Html)
        .await
    {
        tracing::error!("handle_doctor: Telegram rejected HTML message: {e:#}");
        // Fallback: send as plain text without <pre> wrapper
        bot.send_message(msg.chat.id, format!("Doctor results:\n\n{body}")).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::any::TypeId;

    /// Regression test: AgentDir and RightclawHome must have distinct TypeIds.
    /// If they shared the same type (e.g., both Arc<PathBuf>), dptree would overwrite
    /// the first registration with the second, causing all handlers to receive the
    /// wrong path for one of the two parameters.
    #[test]
    fn agent_dir_and_rightclaw_home_have_distinct_type_ids() {
        assert_ne!(
            TypeId::of::<AgentDir>(),
            TypeId::of::<RightclawHome>(),
            "AgentDir and RightclawHome must be distinct types to avoid dptree TypeId collision"
        );
    }

    #[test]
    fn agent_dir_and_rightclaw_home_hold_independent_paths() {
        let agent = AgentDir(PathBuf::from("/agents/myagent"));
        let home = RightclawHome(PathBuf::from("/home/user/.rightclaw"));

        assert_eq!(agent.0, PathBuf::from("/agents/myagent"));
        assert_eq!(home.0, PathBuf::from("/home/user/.rightclaw"));
        assert_ne!(agent.0, home.0);
    }

    #[test]
    fn agent_dir_and_rightclaw_home_clone_independently() {
        let agent = AgentDir(PathBuf::from("/agents/myagent"));
        let home = RightclawHome(PathBuf::from("/home/user/.rightclaw"));

        let agent2 = agent.clone();
        let home2 = home.clone();

        assert_eq!(agent.0, agent2.0);
        assert_eq!(home.0, home2.0);
    }
}
