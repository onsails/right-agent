//! Teloxide endpoint handlers: message dispatch + /new, /list, /switch + /mcp + /cron + /doctor.
//!
//! handle_message: routes incoming text to the per-session worker via DashMap.
//! handle_new: deactivates current session, optionally creates a named one.
//! handle_list: shows all sessions for the current chat+thread.
//! handle_switch: switches to a different session by partial UUID match.
//! handle_mcp: MCP server management (list/auth/add/remove).
//! handle_doctor: runs rightclaw doctor and returns results.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use dashmap::DashMap;
use tokio::sync::mpsc;
use teloxide::prelude::*;
use teloxide::types::{CallbackQuery, Message};
use teloxide::RequestError;

use super::oauth_callback::PendingAuthMap;
use super::session::{activate_session, create_session, deactivate_current, effective_thread_id, find_sessions_by_uuid, list_sessions, truncate_label};
use super::worker::{DebounceMsg, SessionKey, WorkerContext, spawn_worker};
use super::BotType;

/// Newtype wrapper for the agent directory passed via dptree dependencies.
/// Distinct from RightclawHome to prevent TypeId collision in dptree.
#[derive(Clone)]
pub struct AgentDir(pub PathBuf);

/// SSH config path for the agent's OpenShell sandbox.
#[derive(Clone)]
pub struct SshConfigPath(pub Option<PathBuf>);

/// Newtype wrapper for the rightclaw home directory passed via dptree dependencies.
/// Distinct from AgentDir to prevent TypeId collision in dptree.
#[derive(Clone)]
pub struct RightclawHome(pub PathBuf);

/// Newtype wrapper for the debug flag passed via dptree dependencies.
#[derive(Clone)]
pub struct DebugFlag(pub bool);

/// Shared flag: true when an auth watcher task is active for this agent.
/// One per bot process (one agent per process), shared across all workers.
#[derive(Clone)]
pub struct AuthWatcherFlag(pub Arc<AtomicBool>);

/// Shared slot for auth code sender. When login flow waits for a code,
/// a oneshot::Sender is placed here. Message handler checks before routing to worker.
#[derive(Clone)]
pub struct AuthCodeSlot(pub Arc<tokio::sync::Mutex<Option<tokio::sync::oneshot::Sender<String>>>>);

/// Channel sender for notifying the refresh scheduler about server removals.
#[derive(Clone)]
pub struct RefreshTx(pub tokio::sync::mpsc::Sender<rightclaw::mcp::refresh::RefreshMessage>);

/// Newtype wrapper for the InternalClient used to communicate with the MCP aggregator.
#[derive(Clone)]
pub struct InternalApi(pub Arc<rightclaw::mcp::internal_client::InternalClient>);

/// Shared timestamp of last interaction (unix seconds).
/// Updated by handler on incoming messages and by worker after sending replies.
#[derive(Clone)]
pub struct IdleTimestamp(pub Arc<std::sync::atomic::AtomicI64>);

/// Bundled agent invocation settings (reduces dptree injectable arity).
#[derive(Clone)]
pub struct AgentSettings {
    pub max_turns: u32,
    pub max_budget_usd: f64,
    pub show_thinking: bool,
    /// Claude model override (passed as --model). None = inherit CLI default.
    pub model: Option<String>,
}

/// Convert an arbitrary error into `RequestError::Io` so it propagates through `ResponseResult`.
fn to_request_err(e: impl std::fmt::Display) -> RequestError {
    RequestError::Io(std::io::Error::other(e.to_string()).into())
}

/// Send an HTML-formatted message, respecting thread_id for topic replies.
async fn send_html_reply(
    bot: &BotType,
    chat_id: teloxide::types::ChatId,
    eff_thread_id: i64,
    text: &str,
) -> Result<teloxide::types::Message, RequestError> {
    let mut send = bot
        .send_message(chat_id, text)
        .parse_mode(teloxide::types::ParseMode::Html);
    if eff_thread_id != 0 {
        send = send.message_thread_id(teloxide::types::ThreadId(
            teloxide::types::MessageId(eff_thread_id as i32),
        ));
    }
    send.await
}

/// Handle an incoming text message.
///
/// 1. Compute effective_thread_id (normalise General topic).
/// 2. Look up existing sender in DashMap or spawn a new worker task.
/// 3. Send the message into the worker's mpsc channel.
///
/// Serialisation guarantee (SES-05): all messages to the same (chat_id, thread_id)
/// go through the same mpsc channel -> worker processes them serially.
#[allow(clippy::too_many_arguments)]
pub async fn handle_message(
    bot: BotType,
    msg: Message,
    worker_map: Arc<DashMap<SessionKey, mpsc::Sender<DebounceMsg>>>,
    agent_dir: Arc<AgentDir>,
    debug_flag: Arc<DebugFlag>,
    ssh_config: Arc<SshConfigPath>,
    auth_watcher_flag: Arc<AuthWatcherFlag>,
    auth_code_slot: Arc<AuthCodeSlot>,
    settings: Arc<AgentSettings>,
    stop_tokens: super::StopTokens,
    idle_ts: Arc<IdleTimestamp>,
) -> ResponseResult<()> {
    idle_ts.0.store(chrono::Utc::now().timestamp(), std::sync::atomic::Ordering::Relaxed);

    // Extract text from message body OR caption (media messages use captions)
    let text = msg.text().or(msg.caption()).map(|t| t.to_string());

    // Extract attachments from all media types
    let attachments = super::attachments::extract_attachments(&msg);

    // Skip messages with neither text nor attachments
    if text.is_none() && attachments.is_empty() {
        return Ok(());
    }

    // Intercept auth code: if login flow is waiting for a code, forward this message.
    if let Some(ref text_val) = text {
        let mut slot = auth_code_slot.0.lock().await;
        if let Some(sender) = slot.take() {
            tracing::info!("handle_message: forwarding message as auth code");
            let _ = sender.send(text_val.clone());
            return Ok(());
        }
    }

    let chat_id = msg.chat.id;
    let eff_thread_id = effective_thread_id(&msg);
    let key: SessionKey = (chat_id.0, eff_thread_id);
    let worker_exists = worker_map.contains_key(&key);
    tracing::info!(?key, worker_exists, has_text = text.is_some(), attachment_count = attachments.len(), "handle_message: routing");

    let debounce_msg = DebounceMsg {
        message_id: msg.id.0,
        text,
        timestamp: chrono::Utc::now(),
        attachments,
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
                    ssh_config_path: ssh_config.0.clone(),
                    auth_watcher_active: Arc::clone(&auth_watcher_flag.0),
                    auth_code_tx: Arc::clone(&auth_code_slot.0),
                    max_turns: settings.max_turns,
                    max_budget_usd: settings.max_budget_usd,
                    show_thinking: settings.show_thinking,
                    model: settings.model.clone(),
                    stop_tokens: Arc::clone(&stop_tokens),
                    idle_timestamp: Arc::clone(&idle_ts.0),
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

/// Handle the /new command — start a new session.
pub async fn handle_new(
    bot: BotType,
    msg: Message,
    name: String,
    worker_map: Arc<DashMap<SessionKey, mpsc::Sender<DebounceMsg>>>,
    agent_dir: Arc<AgentDir>,
) -> ResponseResult<()> {
    let chat_id = msg.chat.id;
    let eff_thread_id = effective_thread_id(&msg);
    let key: SessionKey = (chat_id.0, eff_thread_id);

    let conn = rightclaw::memory::open_connection(&agent_dir.0)
        .map_err(|e| to_request_err(format!("new: open DB: {:#}", e)))?;

    let prev_uuid = deactivate_current(&conn, chat_id.0, eff_thread_id)
        .map_err(|e| to_request_err(format!("new: deactivate: {:#}", e)))?;

    // Kill worker — channel closes, CC subprocess killed via kill_on_drop
    worker_map.remove(&key);

    let name = name.trim().to_string();
    let mut reply = String::new();

    if !name.is_empty() {
        let new_uuid = uuid::Uuid::new_v4().to_string();
        let label = truncate_label(&name);
        create_session(&conn, chat_id.0, eff_thread_id, &new_uuid, Some(label))
            .map_err(|e| to_request_err(format!("new: create session: {:#}", e)))?;
        reply.push_str(&format!("New session: {name}\n"));
    } else {
        reply.push_str("Session cleared.\n");
    }

    if let Some(prev) = prev_uuid {
        reply.push_str(&format!(
            "Previous session:\n<pre>/switch {prev}</pre>\nTap to copy to return."
        ));
    }

    if name.is_empty() {
        reply.push_str("\nSend a message to start a new conversation.");
    }

    send_html_reply(&bot, chat_id, eff_thread_id, &reply).await?;

    tracing::info!(?key, "new session");
    Ok(())
}

/// Handle the /list command — show all sessions for this chat+thread.
pub async fn handle_list(
    bot: BotType,
    msg: Message,
    agent_dir: Arc<AgentDir>,
) -> ResponseResult<()> {
    let chat_id = msg.chat.id;
    let eff_thread_id = effective_thread_id(&msg);

    let conn = rightclaw::memory::open_connection(&agent_dir.0)
        .map_err(|e| to_request_err(format!("list: open DB: {:#}", e)))?;

    let sessions = list_sessions(&conn, chat_id.0, eff_thread_id)
        .map_err(|e| to_request_err(format!("list: query: {:#}", e)))?;

    if sessions.is_empty() {
        bot.send_message(chat_id, "No sessions yet. Send a message to start one.")
            .await?;
        return Ok(());
    }

    let mut text = String::from("Sessions:\n");
    for s in &sessions {
        text.push_str(&format_session_line(s));
    }

    send_html_reply(&bot, chat_id, eff_thread_id, &text).await?;
    Ok(())
}

/// Format a session row as an HTML line for /list and /switch display.
fn format_session_line(s: &super::session::SessionRow) -> String {
    let marker = if s.is_active { "●" } else { " " };
    let label = s.label.as_deref().unwrap_or("(unnamed)");
    let ago = format_relative_time(&s.last_used_at);
    format!("{marker} {label} — {ago}\n<pre>{}</pre>\n", s.root_session_id)
}

/// Map cron run status to a Unicode icon.
fn status_icon(status: &str) -> &'static str {
    match status {
        "success" => "\u{2705}",
        "failed" => "\u{274c}",
        "running" => "\u{23f3}",
        _ => "?",
    }
}

/// Format an ISO timestamp as a relative time string.
fn format_relative_time(iso_timestamp: &str) -> String {
    let Ok(then) = chrono::NaiveDateTime::parse_from_str(iso_timestamp, "%Y-%m-%dT%H:%M:%SZ")
    else {
        return iso_timestamp.to_string();
    };
    let then_utc = then.and_utc();
    let now = chrono::Utc::now();
    let delta = now - then_utc;

    if delta.num_minutes() < 1 {
        "just now".to_string()
    } else if delta.num_minutes() < 60 {
        format!("{}m ago", delta.num_minutes())
    } else if delta.num_hours() < 24 {
        format!("{}h ago", delta.num_hours())
    } else {
        format!("{}d ago", delta.num_days())
    }
}

/// Handle the /switch command — switch to a different session.
pub async fn handle_switch(
    bot: BotType,
    msg: Message,
    uuid: String,
    worker_map: Arc<DashMap<SessionKey, mpsc::Sender<DebounceMsg>>>,
    agent_dir: Arc<AgentDir>,
) -> ResponseResult<()> {
    let chat_id = msg.chat.id;
    let eff_thread_id = effective_thread_id(&msg);
    let key: SessionKey = (chat_id.0, eff_thread_id);
    let uuid = uuid.trim().to_string();

    if uuid.is_empty() {
        bot.send_message(
            chat_id,
            "Usage: /switch <uuid>\nUse /list to see available sessions.",
        )
        .await?;
        return Ok(());
    }

    let conn = rightclaw::memory::open_connection(&agent_dir.0)
        .map_err(|e| to_request_err(format!("switch: open DB: {:#}", e)))?;

    let matches = find_sessions_by_uuid(&conn, chat_id.0, eff_thread_id, &uuid)
        .map_err(|e| to_request_err(format!("switch: query: {:#}", e)))?;

    match matches.len() {
        0 => {
            send_html_reply(
                &bot,
                chat_id,
                eff_thread_id,
                &format!("No session matching <pre>{uuid}</pre>. Use /list to see available sessions."),
            )
            .await?;
        }
        1 => {
            let target = &matches[0];
            if target.is_active {
                bot.send_message(chat_id, "Already active.").await?;
                return Ok(());
            }

            // activate_session atomically deactivates any other active session
            activate_session(&conn, target.id)
                .map_err(|e| to_request_err(format!("switch: activate: {:#}", e)))?;

            worker_map.remove(&key);

            let label = target.label.as_deref().unwrap_or("(unnamed)");
            send_html_reply(
                &bot,
                chat_id,
                eff_thread_id,
                &format!("Switched to: {label}\n<pre>{}</pre>", target.root_session_id),
            )
            .await?;

            tracing::info!(?key, session = %target.root_session_id, "switched session");
        }
        _ => {
            let mut text = format!("Multiple sessions match <pre>{uuid}</pre>:\n\n");
            for m in &matches {
                text.push_str(&format_session_line(m));
            }
            text.push_str("\nBe more specific.");
            send_html_reply(&bot, chat_id, eff_thread_id, &text).await?;
        }
    }
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
    internal: Arc<InternalApi>,
) -> ResponseResult<()> {
    tracing::info!(agent_dir = %agent_dir.0.display(), "mcp: dispatching");
    let agent_name = agent_dir.0
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown");

    let parts: Vec<&str> = args.split_whitespace().collect();
    let result = match parts.first().copied() {
        None | Some("list") => handle_mcp_list(&bot, &msg, agent_name, &internal.0).await,
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
            handle_mcp_add(&bot, &msg, &rest, &agent_dir.0, &internal.0).await
        }
        Some("remove") => {
            let server = match parts.get(1) {
                Some(s) => *s,
                None => {
                    bot.send_message(msg.chat.id, "Usage: /mcp remove <server>").await?;
                    return Ok(());
                }
            };
            handle_mcp_remove(&bot, &msg, server, &agent_dir.0, &internal.0).await
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

/// `/mcp list` -- show all MCP servers via the internal aggregator API.
async fn handle_mcp_list(
    bot: &BotType,
    msg: &Message,
    agent_name: &str,
    internal: &rightclaw::mcp::internal_client::InternalClient,
) -> Result<(), RequestError> {
    tracing::info!(agent = %agent_name, "mcp list");

    let result = match internal.mcp_list(agent_name).await {
        Ok(r) => r,
        Err(e) => {
            bot.send_message(msg.chat.id, format!("Error listing MCP servers: {e:#}"))
                .await?;
            return Ok(());
        }
    };

    if result.servers.is_empty() {
        bot.send_message(msg.chat.id, "No MCP servers configured.")
            .await?;
        return Ok(());
    }

    let mut text = String::from("MCP Servers:\n\n");
    for s in &result.servers {
        let url_part = s.url.as_deref().map(|u| format!(" [{u}]")).unwrap_or_default();
        text.push_str(&format!("  {} -- {} ({} tools){}\n", s.name, s.status, s.tool_count, url_part));
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

    // 1. Read mcp.json to find server URL
    let mcp_json_path = agent_dir.join("mcp.json");

    let servers = match rightclaw::mcp::credentials::list_http_servers(
        &mcp_json_path,
    ) {
        Ok(s) => s,
        Err(e) => {
            bot.send_message(msg.chat.id, format!("Cannot read mcp.json: {e:#}")).await?;
            return Ok(());
        }
    };

    let server_url = match servers.iter().find(|(name, _)| name == server_name) {
        Some((_, url)) => url.clone(),
        None => {
            bot.send_message(
                msg.chat.id,
                format!("Server '{server_name}' not found in mcp.json"),
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

/// Sync MCP_INSTRUCTIONS.md to .claude/agents/ for @ ref resolution.
///
/// The periodic background sync handles sandbox upload, so we only do the
/// local copy here to avoid needing sandbox name / SSH config in these handlers.
fn sync_mcp_instructions(agent_dir: &Path) {
    let src = agent_dir.join("MCP_INSTRUCTIONS.md");
    if !src.exists() {
        return;
    }
    let agents_subdir = agent_dir.join(".claude/agents");
    if agents_subdir.exists() {
        if let Err(e) = std::fs::copy(&src, agents_subdir.join("MCP_INSTRUCTIONS.md")) {
            tracing::warn!("failed to copy MCP_INSTRUCTIONS.md to .claude/agents/: {e:#}");
        }
    }
}

/// `/mcp add <name> <url>` -- add an MCP server via the internal aggregator API.
async fn handle_mcp_add(
    bot: &BotType,
    msg: &Message,
    config_str: &str,
    agent_dir: &Path,
    internal: &rightclaw::mcp::internal_client::InternalClient,
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

    let agent_name = agent_dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown");

    let eff_thread_id = effective_thread_id(msg);

    match internal.mcp_add(agent_name, name, url).await {
        Ok(resp) => {
            let escaped_name = super::markdown::html_escape(name);
            let mut reply = format!("Added MCP server <b>{escaped_name}</b>.");
            if resp.tools_count > 0 {
                reply.push_str(&format!(" {} tools available.", resp.tools_count));
            }
            if let Some(ref warning) = resp.warning {
                let escaped_warning = super::markdown::html_escape(warning);
                reply.push_str(&format!("\n{escaped_warning}"));
            }
            reply.push_str("\nTools available on agent's next session.");
            send_html_reply(bot, msg.chat.id, eff_thread_id, &reply).await?;
            sync_mcp_instructions(agent_dir);
        }
        Err(e) => {
            send_html_reply(bot, msg.chat.id, eff_thread_id, &format!("Failed: {e:#}")).await?;
        }
    }
    Ok(())
}

/// `/mcp remove <server>` -- remove an MCP server via the internal aggregator API.
async fn handle_mcp_remove(
    bot: &BotType,
    msg: &Message,
    server_name: &str,
    agent_dir: &Path,
    internal: &rightclaw::mcp::internal_client::InternalClient,
) -> Result<(), RequestError> {
    tracing::info!(agent_dir = %agent_dir.display(), server = %server_name, "mcp remove");

    if server_name == rightclaw::mcp::PROTECTED_MCP_SERVER {
        bot.send_message(
            msg.chat.id,
            format!("Cannot remove '{server_name}' — required for core functionality."),
        )
        .await?;
        return Ok(());
    }

    let agent_name = agent_dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown");

    let eff_thread_id = effective_thread_id(msg);
    let escaped_name = super::markdown::html_escape(server_name);

    match internal.mcp_remove(agent_name, server_name).await {
        Ok(_) => {
            send_html_reply(
                bot,
                msg.chat.id,
                eff_thread_id,
                &format!("Removed MCP server <b>{escaped_name}</b>."),
            )
            .await?;
            sync_mcp_instructions(agent_dir);
        }
        Err(e) => {
            send_html_reply(
                bot,
                msg.chat.id,
                eff_thread_id,
                &format!("Failed: {e:#}"),
            )
            .await?;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// /cron command handler
// ---------------------------------------------------------------------------

/// Handle the /cron command — routes to list (no args) or detail (job name).
pub async fn handle_cron(
    bot: BotType,
    msg: Message,
    args: String,
    agent_dir: Arc<AgentDir>,
) -> ResponseResult<()> {
    let result = if args.trim().is_empty() {
        handle_cron_list(&bot, &msg, &agent_dir.0).await
    } else {
        handle_cron_detail(&bot, &msg, args.trim(), &agent_dir.0).await
    };
    result.map_err(|e| to_request_err(format!("{e:#}")))?;
    Ok(())
}

/// `/cron` — list all cron jobs with human-readable schedule and last run status.
async fn handle_cron_list(
    bot: &BotType,
    msg: &Message,
    agent_dir: &Path,
) -> Result<(), RequestError> {
    let conn = rightclaw::memory::open_connection(agent_dir)
        .map_err(|e| to_request_err(format!("DB open failed: {e:#}")))?;

    let specs = rightclaw::cron_spec::load_specs_from_db(&conn)
        .map_err(|e| to_request_err(format!("load specs failed: {e:#}")))?;

    if specs.is_empty() {
        bot.send_message(msg.chat.id, "No cron jobs configured.")
            .await?;
        return Ok(());
    }

    let mut text = String::from("Cron Jobs:\n\n");
    let mut names: Vec<&String> = specs.keys().collect();
    names.sort();

    for name in names {
        let spec = &specs[name];
        let desc = super::markdown::html_escape(
            &rightclaw::cron_spec::describe_schedule(&spec.schedule),
        );

        let last_run = rightclaw::cron_spec::get_recent_runs(&conn, name, 1)
            .map_err(|e| to_request_err(format!("get runs failed: {e:#}")))?;

        let status_str = match last_run.first() {
            Some(run) => {
                let icon = status_icon(&run.status);
                let ago = format_relative_time(&run.started_at);
                format!("last: {ago} {icon}")
            }
            None => "never run".to_string(),
        };

        text.push_str(&format!(
            "\u{2022} {name} \u{2014} {desc} \u{2014} {status_str}\n"
        ));
    }

    let eff_thread_id = effective_thread_id(msg);
    send_html_reply(bot, msg.chat.id, eff_thread_id, &text).await?;
    Ok(())
}

/// `/cron <job-name>` — show job detail + last 5 runs.
async fn handle_cron_detail(
    bot: &BotType,
    msg: &Message,
    job_name: &str,
    agent_dir: &Path,
) -> Result<(), RequestError> {
    let conn = rightclaw::memory::open_connection(agent_dir)
        .map_err(|e| to_request_err(format!("DB open failed: {e:#}")))?;

    let detail = rightclaw::cron_spec::get_spec_detail(&conn, job_name)
        .map_err(|e| to_request_err(format!("query failed: {e:#}")))?;

    let Some(detail) = detail else {
        bot.send_message(msg.chat.id, format!("Cron job '{job_name}' not found."))
            .await?;
        return Ok(());
    };

    let desc = super::markdown::html_escape(
        &rightclaw::cron_spec::describe_schedule(&detail.schedule),
    );
    let schedule_escaped = super::markdown::html_escape(&detail.schedule);
    let mut text = format!(
        "<b>{}</b>\nSchedule: {} (<code>{}</code>)\nBudget: ${:.2}",
        detail.job_name, desc, schedule_escaped, detail.max_budget_usd,
    );
    if let Some(ref ttl) = detail.lock_ttl {
        let ttl_escaped = super::markdown::html_escape(ttl);
        text.push_str(&format!("\nLock TTL: {ttl_escaped}"));
    }
    if detail.triggered_at.is_some() {
        text.push_str("\n\u{26a1} Trigger pending");
    }

    let runs = rightclaw::cron_spec::get_recent_runs(&conn, job_name, 5)
        .map_err(|e| to_request_err(format!("get runs failed: {e:#}")))?;

    if runs.is_empty() {
        text.push_str("\n\nNo runs yet.");
    } else {
        text.push_str("\n\nRecent runs:");
        for (i, run) in runs.iter().enumerate() {
            let icon = status_icon(&run.status);
            let ago = format_relative_time(&run.started_at);
            let duration = match &run.finished_at {
                Some(end) => format_duration(&run.started_at, end),
                None => String::new(),
            };
            text.push_str(&format!(
                "\n  {}. {ago} \u{2014} {icon} {}{duration}",
                i + 1,
                run.status
            ));
        }
    }

    let eff_thread_id = effective_thread_id(msg);
    send_html_reply(bot, msg.chat.id, eff_thread_id, &text).await?;
    Ok(())
}

/// Format duration between two ISO 8601 timestamps (e.g. " (12s)", " (2m 30s)").
fn format_duration(start_iso: &str, end_iso: &str) -> String {
    let Ok(start) = chrono::NaiveDateTime::parse_from_str(start_iso, "%Y-%m-%dT%H:%M:%SZ") else {
        return String::new();
    };
    let Ok(end) = chrono::NaiveDateTime::parse_from_str(end_iso, "%Y-%m-%dT%H:%M:%SZ") else {
        return String::new();
    };
    let secs = (end - start).num_seconds();
    if secs < 60 {
        format!(" ({secs}s)")
    } else {
        format!(" ({}m {}s)", secs / 60, secs % 60)
    }
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

// ---------------------------------------------------------------------------
// Stop button callback query handler
// ---------------------------------------------------------------------------

/// Handle the Stop button callback query from thinking messages.
///
/// Callback data format: `stop:{chat_id}:{eff_thread_id}`
/// Looks up the CancellationToken in StopTokens and cancels it.
pub async fn handle_stop_callback(
    bot: BotType,
    q: CallbackQuery,
    stop_tokens: super::StopTokens,
) -> ResponseResult<()> {
    let data = q.data.as_deref().unwrap_or("");
    let parts: Vec<&str> = data.splitn(3, ':').collect();
    let qid = q.id;

    let text = if parts.len() == 3
        && parts[0] == "stop"
        && let Ok(chat_id) = parts[1].parse::<i64>()
        && let Ok(thread_id) = parts[2].parse::<i64>()
    {
        let key = (chat_id, thread_id);
        if let Some(entry) = stop_tokens.get(&key) {
            entry.value().cancel();
            drop(entry); // release DashMap read guard before await
            Some("Stopping...")
        } else {
            Some("Already finished")
        }
    } else {
        None
    };

    let mut answer = bot.answer_callback_query(qid);
    if let Some(t) = text {
        answer = answer.text(t);
    }
    answer.await?;

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
    fn parse_stop_callback_data_valid() {
        let data = "stop:12345:678";
        let parts: Vec<&str> = data.splitn(3, ':').collect();
        assert_eq!(parts.len(), 3);
        assert_eq!(parts[0], "stop");
        assert_eq!(parts[1].parse::<i64>().unwrap(), 12345);
        assert_eq!(parts[2].parse::<i64>().unwrap(), 678);
    }

    #[test]
    fn parse_stop_callback_data_zero_thread() {
        let data = "stop:12345:0";
        let parts: Vec<&str> = data.splitn(3, ':').collect();
        assert_eq!(parts[2].parse::<i64>().unwrap(), 0);
    }

    #[test]
    fn parse_stop_callback_data_invalid() {
        let data = "stop:notanumber:0";
        let parts: Vec<&str> = data.splitn(3, ':').collect();
        assert!(parts[1].parse::<i64>().is_err());
    }

    #[test]
    fn stop_token_cancel_via_dashmap_lookup() {
        use dashmap::DashMap;
        use tokio_util::sync::CancellationToken;

        let map = DashMap::new();
        let token = CancellationToken::new();
        let key = (12345_i64, 0_i64);
        map.insert(key, token.clone());

        // Simulate callback handler lookup + cancel
        let entry = map.get(&key).unwrap();
        entry.value().cancel();
        drop(entry);

        assert!(token.is_cancelled());

        // After removal, lookup returns None (race: stop after finish)
        map.remove(&key);
        assert!(map.get(&key).is_none());
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

    #[test]
    fn format_relative_time_just_now() {
        let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
        assert_eq!(format_relative_time(&now), "just now");
    }

    #[test]
    fn format_relative_time_minutes() {
        let then = (chrono::Utc::now() - chrono::Duration::minutes(15))
            .format("%Y-%m-%dT%H:%M:%SZ")
            .to_string();
        assert_eq!(format_relative_time(&then), "15m ago");
    }

    #[test]
    fn format_relative_time_hours() {
        let then = (chrono::Utc::now() - chrono::Duration::hours(3))
            .format("%Y-%m-%dT%H:%M:%SZ")
            .to_string();
        assert_eq!(format_relative_time(&then), "3h ago");
    }

    #[test]
    fn format_relative_time_days() {
        let then = (chrono::Utc::now() - chrono::Duration::days(5))
            .format("%Y-%m-%dT%H:%M:%SZ")
            .to_string();
        assert_eq!(format_relative_time(&then), "5d ago");
    }

    #[test]
    fn format_relative_time_malformed() {
        assert_eq!(format_relative_time("not-a-timestamp"), "not-a-timestamp");
    }

    #[test]
    fn format_duration_seconds() {
        assert_eq!(
            format_duration("2026-04-11T10:00:00Z", "2026-04-11T10:00:12Z"),
            " (12s)"
        );
    }

    #[test]
    fn format_duration_minutes() {
        assert_eq!(
            format_duration("2026-04-11T10:00:00Z", "2026-04-11T10:02:30Z"),
            " (2m 30s)"
        );
    }

    #[test]
    fn format_duration_malformed() {
        assert_eq!(format_duration("bad", "2026-04-11T10:00:00Z"), "");
    }
}
