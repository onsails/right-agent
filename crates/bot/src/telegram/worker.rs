//! Per-session worker task: debounce loop, CC subprocess invocation, reply tool parsing.
//!
//! Pure helpers are tested in isolation (TDD). `spawn_worker` and `invoke_cc` require
//! live infrastructure and are covered by code review pattern only.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use chrono::{DateTime, Utc};
use dashmap::DashMap;
use teloxide::prelude::*;
use teloxide::types::{ChatAction, MessageId, ReplyParameters, ThreadId};
use tokio::sync::mpsc;
use tokio::time::{Duration, sleep};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::reflection::FailureKind;

use super::session::{
    SessionRow, create_session, deactivate_current, get_active_session, touch_session,
    truncate_label,
};

/// Session key: `(chat_id, effective_thread_id)`.
pub type SessionKey = (i64, i64);

/// Idle debounce window in milliseconds — every new message resets the
/// timer; the batch closes after this much silence (D-01).
const DEBOUNCE_MS: u64 = 500;

/// While the current batch contains any media-group sibling, close the window
/// after this many milliseconds of inactivity from the latest arrival.
const MEDIA_GROUP_IDLE_MS: u64 = 1000;

/// Hard cap on the total time spent collecting a batch that contains
/// media-group siblings, measured from the first arrival.
const MEDIA_GROUP_HARD_CAP_MS: u64 = 2500;

/// Maximum time to wait for a CC subprocess to complete.
const CC_TIMEOUT_SECS: u64 = 600;

/// Maximum character count for Hindsight recall queries (~530 tokens, safely under the 500-token API limit).
const RECALL_MAX_CHARS: usize = 800;

/// Build the inline keyboard with a single "Stop" button for thinking messages.
fn stop_keyboard(chat_id: i64, eff_thread_id: i64) -> teloxide::types::InlineKeyboardMarkup {
    teloxide::types::InlineKeyboardMarkup::new(vec![vec![
        teloxide::types::InlineKeyboardButton::callback(
            "\u{26d4} Stop",
            format!("stop:{chat_id}:{eff_thread_id}"),
        ),
    ]])
}

/// A single Telegram message queued into the debounce channel.
#[derive(Clone)]
pub struct DebounceMsg {
    pub message_id: i32,
    pub text: Option<String>,
    pub timestamp: DateTime<Utc>,
    pub attachments: Vec<super::attachments::InboundAttachment>,
    pub author: super::attachments::MessageAuthor,
    pub forward_info: Option<super::attachments::ForwardInfo>,
    pub reply_to_id: Option<i32>,
    pub address: Option<super::mention::AddressKind>,
    pub group_open: bool,
    pub chat: super::attachments::ChatContext,
    pub reply_to_body: Option<super::attachments::ReplyToBody>,
    /// `Some(id)` when this message is part of a Telegram album (media group);
    /// shared by all siblings of the album.
    pub media_group_id: Option<String>,
}

/// Context passed to each worker task when it is spawned.
#[derive(Clone)]
pub struct WorkerContext {
    pub chat_id: teloxide::types::ChatId,
    pub effective_thread_id: i64,
    pub agent_dir: PathBuf,
    /// Agent name for --agent flag on first CC invocation (AGDEF-02).
    pub agent_name: String,
    pub bot: super::BotType,
    /// agent_dir — passed separately so worker opens its own Connection
    pub db_path: PathBuf,
    /// When true, pass --verbose to CC subprocess and log CC stderr at debug level.
    pub debug: bool,
    /// Path to the SSH config file for this agent's OpenShell sandbox (None when --no-sandbox).
    pub ssh_config_path: Option<PathBuf>,
    /// Guard: true when an auth watcher task is active for this agent. Prevents duplicates.
    pub auth_watcher_active: Arc<AtomicBool>,
    /// Slot for auth code sender — when login flow is waiting for a code from Telegram,
    /// the oneshot::Sender is stored here. Message handler checks this before routing to worker.
    pub auth_code_tx: Arc<tokio::sync::Mutex<Option<tokio::sync::oneshot::Sender<String>>>>,
    /// Resolved sandbox name (None when running without sandbox).
    pub resolved_sandbox: Option<String>,
    /// Show live thinking indicator in Telegram.
    pub show_thinking: bool,
    /// Claude model override (passed as --model). None = inherit CLI default.
    pub model: Option<String>,
    /// Shared map for stop button — worker inserts token before CC, removes after exit.
    pub stop_tokens: super::StopTokens,
    /// Shared idle timestamp — worker updates after each reply sent.
    pub idle_timestamp: Arc<std::sync::atomic::AtomicI64>,
    /// Internal API client for aggregator IPC (Unix socket).
    pub internal_client: std::sync::Arc<right_agent::mcp::internal_client::InternalClient>,
    /// Hindsight client for auto-retain/recall (None when memory.provider=file).
    pub hindsight: Option<std::sync::Arc<right_agent::memory::ResilientHindsight>>,
    /// Prefetch cache for auto-recall results (None when memory.provider=file).
    pub prefetch_cache: Option<right_agent::memory::prefetch::PrefetchCache>,
    /// RwLock gate — worker acquires read lock before invoke_cc to block during upgrades.
    pub upgrade_lock: Arc<tokio::sync::RwLock<()>>,
    /// STT context — None when stt.enabled=false or whisper model not yet cached.
    pub stt: Option<std::sync::Arc<crate::stt::SttContext>>,
}

/// Parsed output from CC structured JSON response (`result` field per D-03).
#[derive(Debug, serde::Deserialize)]
pub struct ReplyOutput {
    pub content: Option<String>,
    pub reply_to_message_id: Option<i32>,
    pub attachments: Option<Vec<super::attachments::OutboundAttachment>>,
    /// Bootstrap mode: `true` signals agent claims onboarding is complete.
    /// Server-side file check (`should_accept_bootstrap`) gates actual completion.
    pub bootstrap_complete: Option<bool>,
}

/// Required identity files that must exist for bootstrap to be accepted as complete.
const BOOTSTRAP_REQUIRED_FILES: &[&str] = &["IDENTITY.md", "SOUL.md", "USER.md"];

/// Check whether bootstrap completion should be accepted.
///
/// Returns `true` only when all required identity files exist in `agent_dir`.
/// If any are missing, the agent didn't actually complete the onboarding flow
/// and bootstrap mode should continue.
fn should_accept_bootstrap(agent_dir: &Path) -> bool {
    BOOTSTRAP_REQUIRED_FILES
        .iter()
        .all(|f| agent_dir.join(f).exists())
}

// ── Pure helpers ──────────────────────────────────────────────────────────────

/// Strip HTML tags for plain-text fallback when Telegram rejects HTML.
/// Also decodes common entities back to their characters.
use super::markdown::{html_escape, strip_html_tags};

/// Format a CC subprocess error as a Telegram message (D-16).
///
/// Returns HTML intended for `ParseMode::Html`. Callers must fall back to
/// `strip_html_tags` if Telegram rejects the HTML.
pub fn format_error_reply(exit_code: i32, stderr: &str) -> String {
    let truncated = if stderr.len() > 300 {
        &stderr[..300]
    } else {
        stderr
    };
    format!(
        "\u{26a0}\u{fe0f} Agent error (exit {exit_code}):\n<pre>{}</pre>",
        html_escape(truncated)
    )
}

/// Check whether CC stdout JSON indicates an authentication failure (403/401).
///
/// Returns true when the JSON has `is_error: true` and the `result` string
/// contains known auth-failure patterns. Returns false for non-JSON input,
/// parse errors, or non-auth errors.
pub fn is_auth_error(stdout: &str) -> bool {
    let parsed: serde_json::Value = match serde_json::from_str(stdout) {
        Ok(v) => v,
        Err(_) => return false,
    };

    let is_error = parsed
        .get("is_error")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    if !is_error {
        return false;
    }

    let result = match parsed.get("result").and_then(|v| v.as_str()) {
        Some(s) => s,
        None => return false,
    };

    const AUTH_PATTERNS: &[&str] = &[
        "API Error: 403",
        "API Error: 401",
        "Failed to authenticate",
        "Not logged in",
        "Please run /login",
    ];

    AUTH_PATTERNS.iter().any(|pattern| result.contains(pattern))
}

/// Extract an OAuth URL from process log lines.
///
/// Scans for `https://` URLs containing OAuth-specific path segments
/// (`/oauth/` or `/authorize`) on Anthropic/Claude domains.
/// Returns the first matching URL, trimmed of surrounding text.
pub fn extract_auth_url(lines: &[String]) -> Option<String> {
    for line in lines {
        let Some(start) = line.find("https://") else {
            continue;
        };
        let url_part = &line[start..];
        let end = url_part
            .find(|c: char| c.is_whitespace())
            .unwrap_or(url_part.len());
        let url = &url_part[..end];

        // Match OAuth-specific URLs on Anthropic/Claude domains.
        let is_auth_domain =
            url.contains("anthropic") || url.contains("claude.ai") || url.contains("claude.com");
        let is_auth_path = url.contains("/oauth/") || url.contains("/authorize");
        if is_auth_domain && is_auth_path {
            return Some(url.to_string());
        }
    }
    None
}

/// Parse CC structured JSON output (D-03, D-04).
///
/// Returns `Ok((ReplyOutput, Option<session_id>))` on success.
/// Returns `Err(String)` if JSON is malformed or the `result` field is missing.
/// Returns `Ok((ReplyOutput { content: None, .. }, _))` if content=null (silent response per D-04).
pub fn parse_reply_output(raw_json: &str) -> Result<(ReplyOutput, Option<String>), String> {
    tracing::debug!("CC raw JSON output: {}", raw_json);

    let parsed: serde_json::Value =
        serde_json::from_str(raw_json).map_err(|e| format!("JSON parse error: {e}"))?;

    let session_id = parsed
        .get("session_id")
        .and_then(|v| v.as_str())
        .map(str::to_string);

    let result_val = parsed
        .get("structured_output")
        .filter(|v| !v.is_null())
        .or_else(|| parsed.get("result"))
        .ok_or_else(|| {
            "CC response missing both 'structured_output' and 'result' fields".to_string()
        })?;

    // CC sometimes returns result as a plain string (e.g. after multi-turn MCP tool use)
    // instead of complying with --json-schema. Wrap it as ReplyOutput so the message is delivered.
    let output: ReplyOutput = if let Some(text) = result_val.as_str() {
        ReplyOutput {
            content: if text.is_empty() {
                None
            } else {
                Some(text.to_string())
            },
            reply_to_message_id: None,
            attachments: None,
            bootstrap_complete: None,
        }
    } else {
        serde_json::from_value(result_val.clone())
            .map_err(|e| format!("failed to deserialize result: {e}"))?
    };

    Ok((output, session_id))
}

/// Build the tag list for a Hindsight retain call.
///
/// - DM: `["chat:<chat_id>"]`.
/// - Group: `["chat:<chat_id>", "user:<sender_id>"]` plus `"topic:<thread_id>"`
///   when this is a supergroup topic (thread_id > 0).
fn retain_tags(
    chat_id: i64,
    sender_id: Option<i64>,
    thread_id: i64,
    is_group: bool,
) -> Vec<String> {
    let mut tags = vec![format!("chat:{chat_id}")];
    if is_group {
        if let Some(uid) = sender_id {
            tags.push(format!("user:{uid}"));
        }
        if thread_id > 0 {
            tags.push(format!("topic:{thread_id}"));
        }
    }
    tags
}

/// Recall tags — always just `chat:<chat_id>`, group/DM agnostic so recall
/// fetches all memories scoped to that chat.
fn recall_tags(chat_id: i64) -> Vec<String> {
    vec![format!("chat:{chat_id}")]
}

/// Truncate a string to at most `max_chars` characters (not bytes).
///
/// Hindsight recall API rejects queries over 500 tokens. At ~1 token per
/// 1.5 chars, 800 chars stays safely under that limit.
fn truncate_to_chars(s: &str, max_chars: usize) -> &str {
    match s.char_indices().nth(max_chars) {
        Some((byte_idx, _)) => &s[..byte_idx],
        None => s,
    }
}

/// Build the `<memory-status>` marker appended to composite-memory.md.
///
/// Returns `None` when memory is healthy and no retain-side drops have
/// accumulated in the last 24h — no marker is injected in that case.
fn build_memory_marker(
    status: right_agent::memory::MemoryStatus,
    client_drops_24h: usize,
) -> Option<String> {
    use right_agent::memory::MemoryStatus as S;
    match status {
        S::AuthFailed { .. } => Some(
            "<memory-status>unavailable — memory provider authentication failed, \
             memory ops will error until the user rotates the API key</memory-status>"
                .into(),
        ),
        S::Degraded { .. } => Some(
            "<memory-status>degraded — recall may be incomplete or stale, \
             retain may be queued</memory-status>"
                .into(),
        ),
        S::Healthy => {
            if client_drops_24h > 0 {
                Some(format!(
                    "<memory-status>retain-errors: {client_drops_24h} records dropped \
                     in last 24h due to bad payload — check logs</memory-status>"
                ))
            } else {
                None
            }
        }
    }
}

// ── Async worker ─────────────────────────────────────────────────────────────

/// Collect a single debounce batch starting from `first`, draining additional
/// messages from `rx` according to the windowing rules:
///
/// - If no message in the batch carries a `media_group_id`, the window is
///   "idle `DEBOUNCE_MS` from the latest arrival" — every new message resets
///   the timer.
/// - Once any message in the batch carries a `media_group_id`, the window
///   becomes "idle `MEDIA_GROUP_IDLE_MS` from the latest arrival, capped at
///   `MEDIA_GROUP_HARD_CAP_MS` from the first arrival". The flip from the
///   first regime to the second can happen mid-batch when a media-group
///   sibling arrives during a non-media batch; the deadline is recomputed
///   on every iteration so the regime change takes effect immediately.
///
/// Returns when the window closes or `rx` is closed (whichever happens first).
async fn collect_batch(
    first: DebounceMsg,
    rx: &mut mpsc::Receiver<DebounceMsg>,
) -> Vec<DebounceMsg> {
    use tokio::time::{Instant, sleep_until};

    let first_arrival = Instant::now();
    let mut last_arrival = first_arrival;
    let mut media_group_seen = first.media_group_id.is_some();
    let mut batch = vec![first];

    loop {
        let deadline = if media_group_seen {
            std::cmp::min(
                last_arrival + Duration::from_millis(MEDIA_GROUP_IDLE_MS),
                first_arrival + Duration::from_millis(MEDIA_GROUP_HARD_CAP_MS),
            )
        } else {
            last_arrival + Duration::from_millis(DEBOUNCE_MS)
        };

        tokio::select! {
            biased;
            msg = rx.recv() => {
                match msg {
                    Some(m) => {
                        if m.media_group_id.is_some() {
                            media_group_seen = true;
                        }
                        last_arrival = Instant::now();
                        batch.push(m);
                    }
                    None => break,
                }
            }
            _ = sleep_until(deadline) => break,
        }
    }
    batch
}

/// Post-debounce addressedness gate. Returns `true` if at least one message
/// in the batch was addressed to the bot. In groups this is the predicate
/// the worker uses to decide whether to invoke CC; if `false`, the batch is
/// dropped silently. DM batches always have `address: Some(DirectMessage)`
/// so the predicate trivially holds for them.
fn batch_is_addressed(batch: &[DebounceMsg]) -> bool {
    batch.iter().any(|m| m.address.is_some())
}

/// Spawn a per-session worker task.
///
/// Called by the message handler when no sender exists for the session key.
/// Returns the `Sender` to store in the DashMap. The worker task:
///   1. Waits for the first message.
///   2. Collects additional messages via `collect_batch` (idle-timeout
///      window — see `collect_batch` docs).
///   3. Batches them as XML (D-02).
///   4. Invokes `claude -p` (D-13, D-14).
///   5. Parses the `reply` tool call (D-03, D-04, D-05).
///   6. Sends the Telegram reply.
///   7. Loops back to step 1.
///
/// On channel close (DashMap entry removed on `/reset`), the task exits.
/// On worker task panic, Sender in DashMap becomes stale; handler detects
/// `SendError` and removes the entry + respawns (Pitfall 7 mitigation).
pub fn spawn_worker(
    key: SessionKey,
    ctx: WorkerContext,
    worker_map: Arc<DashMap<SessionKey, mpsc::Sender<DebounceMsg>>>,
) -> mpsc::Sender<DebounceMsg> {
    let (tx, mut rx) = mpsc::channel::<DebounceMsg>(32); // bounded — safe for debounce

    let tx_for_map = tx.clone();
    tokio::spawn(async move {
        let (chat_id, eff_thread_id) = key;
        let tg_chat_id = ctx.chat_id;

        loop {
            tracing::info!(?key, "worker waiting for message");
            // Wait for first message in this debounce cycle
            let Some(first) = rx.recv().await else {
                tracing::info!(?key, "worker channel closed — exiting");
                break;
            };
            tracing::info!(
                ?key,
                batch_size = 1,
                "worker received message, starting debounce"
            );
            let batch = collect_batch(first, &mut rx).await;

            // Group vs DM detection: used for tag derivation, live-thinking
            // suppression, and reply-to behavior across the batch.
            let is_group = matches!(
                batch.first().map(|m| &m.chat),
                Some(super::attachments::ChatContext::Group { .. })
            );
            if is_group && !batch_is_addressed(&batch) {
                tracing::debug!(
                    ?key,
                    batch_size = batch.len(),
                    "media-group batch had no addressed sibling — dropping without CC"
                );
                continue;
            }
            if is_group && ctx.show_thinking {
                tracing::debug!(?key, "show_thinking suppressed in group");
            }

            // Download attachments for all messages in batch
            let mut input_messages = Vec::with_capacity(batch.len());
            let mut skip_batch = false;
            for msg in &batch {
                let (resolved, voice_markers) = if msg.attachments.is_empty() {
                    (vec![], vec![])
                } else {
                    match super::attachments::download_attachments(
                        &msg.attachments,
                        msg.message_id,
                        &ctx.bot,
                        &ctx.agent_dir,
                        ctx.ssh_config_path.as_deref(),
                        ctx.resolved_sandbox.as_deref(),
                        tg_chat_id,
                        eff_thread_id,
                        ctx.stt.as_deref(),
                    )
                    .await
                    {
                        Ok(r) => r,
                        Err(e) => {
                            tracing::error!(?key, "attachment download failed: {:#}", e);
                            let _ = send_tg(&ctx.bot, tg_chat_id, eff_thread_id, &format!("⚠️ Failed to download attachments: {e:#}\nYour message was not forwarded.")).await;
                            skip_batch = true;
                            break;
                        }
                    }
                };
                input_messages.push(super::attachments::InputMessage {
                    message_id: msg.message_id,
                    text: crate::stt::combine_markers_with_text(
                        &voice_markers,
                        msg.text.as_deref(),
                    ),
                    timestamp: msg.timestamp,
                    attachments: resolved,
                    author: msg.author.clone(),
                    forward_info: msg.forward_info.clone(),
                    reply_to_id: msg.reply_to_id,
                    chat: msg.chat.clone(),
                    reply_to_body: msg.reply_to_body.clone(),
                });
            }
            if skip_batch {
                continue;
            }

            let Some(input) = super::attachments::format_cc_input(&input_messages) else {
                tracing::warn!(
                    ?key,
                    "empty input after formatting -- skipping CC invocation"
                );
                continue;
            };

            // Typing indicator: always active until reply is sent (D-10).
            let cancel_token = CancellationToken::new();
            let cancel_clone = cancel_token.clone();
            let bot_clone = ctx.bot.clone();
            let typing_task = tokio::spawn(async move {
                loop {
                    let mut action = bot_clone.send_chat_action(tg_chat_id, ChatAction::Typing);
                    if eff_thread_id != 0 {
                        action =
                            action.message_thread_id(ThreadId(MessageId(eff_thread_id as i32)));
                    }
                    action.await.ok();
                    tokio::select! {
                        _ = cancel_clone.cancelled() => break,
                        _ = sleep(Duration::from_secs(4)) => {}
                    }
                }
            });

            // Block while upgrade is running (upgrade holds write lock).
            let _upgrade_guard = ctx.upgrade_lock.read().await;

            // Invoke claude -p (D-13, D-14)
            // Pass first message text for session label (truncated 60 chars).
            let first_text = batch.first().and_then(|m| m.text.as_deref());
            let (reply_result, session_uuid, is_first_call) =
                match invoke_cc(&input, first_text, chat_id, eff_thread_id, is_group, &ctx).await {
                    Ok(CcReply { output, session_uuid, is_first_call }) => {
                        (Ok(output), session_uuid, is_first_call)
                    }
                    Err(failure) => {
                        let uuid = match &failure {
                            InvokeCcFailure::Reflectable { session_uuid, .. } => {
                                session_uuid.clone()
                            }
                            InvokeCcFailure::NonReflectable { .. } => String::new(),
                        };
                        // is_first_call=false: failures don't produce a normal
                        // reply, so the bootstrap welcome photo should not fire.
                        // Auth-error recovery deactivates the session, so a
                        // subsequent retry sees is_first_call=true again.
                        (Err(failure), uuid, false)
                    }
                };

            // Reverse sync .md changes from sandbox.
            // Bootstrap mode: BLOCK so files are on host for completion check.
            // Normal mode: fire-and-forget, don't delay reply.
            let bootstrap_mode = ctx.agent_dir.join("BOOTSTRAP.md").exists();
            if ctx.ssh_config_path.is_some() {
                let sandbox = ctx.resolved_sandbox.clone().unwrap();
                if bootstrap_mode {
                    if let Err(e) = crate::sync::reverse_sync_md(&ctx.agent_dir, &sandbox).await {
                        tracing::warn!(
                            agent = %ctx.agent_name,
                            "bootstrap reverse sync failed: {e:#}"
                        );
                    }
                } else {
                    let agent_dir = ctx.agent_dir.clone();
                    let agent_name = ctx.agent_name.clone();
                    tokio::spawn(async move {
                        if let Err(e) = crate::sync::reverse_sync_md(&agent_dir, &sandbox).await {
                            tracing::warn!(agent = %agent_name, "reverse sync failed: {e:#}");
                        }
                    });
                }
            }

            // Bootstrap completion: check if identity files are now on host after sync.
            // MCP tool bootstrap_done may have already deleted BOOTSTRAP.md, but
            // we also check here as a safety net (handles no-sandbox mode too).
            let bootstrap_signaled = matches!(
                &reply_result,
                Ok(Some(output)) if output.bootstrap_complete == Some(true)
            );
            if bootstrap_mode && bootstrap_signaled && should_accept_bootstrap(&ctx.agent_dir) {
                tracing::info!(
                    key = ?key,
                    "bootstrap complete — identity files present after sync"
                );
                // Open a short-lived connection to deactivate the session.
                if let Ok(conn) = right_agent::memory::open_connection(&ctx.agent_dir, false) {
                    deactivate_current(&conn, chat_id, eff_thread_id)
                        .map_err(|e| {
                            tracing::error!(
                                key = ?key,
                                "deactivate_current after bootstrap: {:#}",
                                e
                            )
                        })
                        .ok();
                }
                // BOOTSTRAP.md may already be deleted by MCP tool; ensure cleanup.
                let bp = ctx.agent_dir.join("BOOTSTRAP.md");
                if bp.exists()
                    && let Err(e) = std::fs::remove_file(&bp)
                {
                    tracing::warn!(key = ?key, "failed to delete BOOTSTRAP.md: {e:#}");
                }
            }

            // Cancel typing indicator
            cancel_token.cancel();
            typing_task.await.ok();

            // Send reply (D-04, D-05, DIS-05, DIS-06)
            let mut reply_text_for_retain: Option<String> = None;
            // Common reply-to policy:
            //  - group: always thread to the triggering message
            //  - single-message batch: thread to that message
            //  - multi-message batch: deferred to output.reply_to_message_id on the
            //    success path; for reflection replies (Err path) we fall back to the
            //    first message since we don't have a CC-picked id.
            let default_reply_to = if is_group {
                batch.first().map(|m| m.message_id)
            } else if batch.len() == 1 {
                Some(batch[0].message_id)
            } else {
                batch.first().map(|m| m.message_id)
            };
            match reply_result {
                Ok(Some(output)) => {
                    let reply_to = if is_group {
                        // Always reply-to the triggering message in groups,
                        // regardless of batch size.
                        batch.first().map(|m| m.message_id)
                    } else if batch.len() == 1 {
                        Some(batch[0].message_id)
                    } else {
                        output.reply_to_message_id
                    };

                    if let Some(content) = output.content {
                        reply_text_for_retain = Some(content.clone());
                        let html = super::markdown::md_to_telegram_html(&content);
                        let parts = super::markdown::split_html_message(&html);
                        tracing::info!(
                            ?key,
                            content_len = content.len(),
                            html_len = html.len(),
                            parts = parts.len(),
                            ?reply_to,
                            "sending reply to Telegram"
                        );

                        // Bootstrap welcome photo — first agent reply only, in
                        // bootstrap mode only. When caption fits, the first text
                        // part rides as the photo caption (single Telegram
                        // message); we then skip it in the text loop below.
                        let caption_consumed = super::bootstrap_photo::send_if_needed(
                            &ctx.bot,
                            tg_chat_id,
                            eff_thread_id,
                            bootstrap_mode,
                            is_first_call,
                            parts.first().map(|s| s.as_str()),
                            reply_to,
                        )
                        .await;

                        let start = if caption_consumed { 1 } else { 0 };
                        for part in &parts[start..] {
                            let mut send = ctx.bot.send_message(tg_chat_id, part);
                            send = send.parse_mode(teloxide::types::ParseMode::Html);
                            if eff_thread_id != 0 {
                                send = send
                                    .message_thread_id(ThreadId(MessageId(eff_thread_id as i32)));
                            }
                            if let Some(ref_id) = reply_to {
                                send = send.reply_parameters(ReplyParameters {
                                    message_id: MessageId(ref_id),
                                    ..Default::default()
                                });
                            }
                            if let Err(e) = send.await {
                                tracing::warn!(
                                    ?key,
                                    "HTML send failed, retrying plain text: {:#}",
                                    e
                                );
                                let plain = strip_html_tags(part);
                                let mut fallback = ctx.bot.send_message(tg_chat_id, &plain);
                                if eff_thread_id != 0 {
                                    fallback = fallback.message_thread_id(ThreadId(MessageId(
                                        eff_thread_id as i32,
                                    )));
                                }
                                if let Some(ref_id) = reply_to {
                                    fallback = fallback.reply_parameters(ReplyParameters {
                                        message_id: MessageId(ref_id),
                                        ..Default::default()
                                    });
                                }
                                if let Err(e2) = fallback.await {
                                    tracing::error!(
                                        ?key,
                                        "plain text fallback also failed: {:#}",
                                        e2
                                    );
                                }
                            }
                        }
                    } else {
                        tracing::warn!(?key, "CC returned content: null -- no text reply sent");
                    }

                    // Send outbound attachments
                    #[allow(clippy::collapsible_if)]
                    if let Some(ref atts) = output.attachments
                        && !atts.is_empty()
                    {
                        if let Err(e) = super::attachments::send_attachments(
                            atts,
                            &ctx.bot,
                            tg_chat_id,
                            eff_thread_id,
                            &ctx.agent_dir,
                            ctx.ssh_config_path.as_deref(),
                            ctx.resolved_sandbox.as_deref(),
                        )
                        .await
                        {
                            tracing::error!(?key, "failed to send attachments: {:#}", e);
                            let _ = send_tg(
                                &ctx.bot,
                                tg_chat_id,
                                eff_thread_id,
                                &format!("Failed to send attachments: {e}"),
                            )
                            .await;
                        }
                    }
                }
                Ok(None) => {
                    tracing::warn!(?key, "unexpected Ok(None) from invoke_cc — no reply sent");
                }
                Err(InvokeCcFailure::NonReflectable { message }) => {
                    tracing::info!(?key, "sending non-reflectable error reply to Telegram");
                    send_error_to_telegram(&ctx, tg_chat_id, eff_thread_id, &message).await;
                }
                Err(InvokeCcFailure::Reflectable {
                    kind,
                    ring_buffer_tail,
                    session_uuid: failed_session_uuid,
                    raw_message,
                    thinking_msg_id,
                }) => {
                    // 1. Edit the old thinking message to a short neutral banner
                    //    (no ring-buffer dump) and clear the stop keyboard.
                    let banner = match &kind {
                        crate::reflection::FailureKind::SafetyTimeout { limit_secs } => {
                            format!(
                                "\u{26a0}\u{fe0f} Hit {limit_secs}s safety limit — thinking again…"
                            )
                        }
                        crate::reflection::FailureKind::NonZeroExit { code } => {
                            format!(
                                "\u{26a0}\u{fe0f} Claude exited with code {code} — thinking again…"
                            )
                        }
                        _ => "\u{26a0}\u{fe0f} Previous turn did not complete — thinking again…"
                            .to_string(),
                    };
                    if let Some(msg_id) = thinking_msg_id {
                        let _ = ctx
                            .bot
                            .edit_message_text(tg_chat_id, msg_id, &banner)
                            .parse_mode(teloxide::types::ParseMode::Html)
                            .reply_markup(teloxide::types::InlineKeyboardMarkup::default())
                            .await;
                    }

                    // 2. Run reflection.
                    let refl_ctx = crate::reflection::ReflectionContext {
                        session_uuid: failed_session_uuid,
                        failure: kind,
                        ring_buffer_tail,
                        limits: crate::reflection::ReflectionLimits::WORKER,
                        agent_name: ctx.agent_name.clone(),
                        agent_dir: ctx.agent_dir.clone(),
                        ssh_config_path: ctx.ssh_config_path.clone(),
                        resolved_sandbox: ctx.resolved_sandbox.clone(),
                        parent_source: crate::reflection::ParentSource::Worker {
                            chat_id,
                            thread_id: eff_thread_id,
                        },
                        model: ctx.model.clone(),
                    };

                    match crate::reflection::reflect_on_failure(refl_ctx).await {
                        Ok(reply_text) => {
                            tracing::info!(?key, "reflection reply produced");
                            // Delete the banner — reply is the substantive update.
                            if let Some(msg_id) = thinking_msg_id {
                                let _ = ctx.bot.delete_message(tg_chat_id, msg_id).await;
                            }
                            // Send reply via the same md→html pipeline as the success path.
                            // Mirror the success path's reply-threading so reflection replies
                            // don't appear unthreaded in group chats.
                            let reply_to = default_reply_to;
                            let html = super::markdown::md_to_telegram_html(&reply_text);
                            let parts = super::markdown::split_html_message(&html);
                            for part in &parts {
                                let mut send = ctx.bot.send_message(tg_chat_id, part);
                                send = send.parse_mode(teloxide::types::ParseMode::Html);
                                if eff_thread_id != 0 {
                                    send = send.message_thread_id(ThreadId(MessageId(
                                        eff_thread_id as i32,
                                    )));
                                }
                                if let Some(ref_id) = reply_to {
                                    send = send.reply_parameters(ReplyParameters {
                                        message_id: MessageId(ref_id),
                                        ..Default::default()
                                    });
                                }
                                if let Err(e) = send.await {
                                    tracing::warn!(
                                        ?key,
                                        "reflection HTML send failed, retrying plain: {:#}",
                                        e
                                    );
                                    let plain = strip_html_tags(part);
                                    let mut fb = ctx.bot.send_message(tg_chat_id, &plain);
                                    if eff_thread_id != 0 {
                                        fb = fb.message_thread_id(ThreadId(MessageId(
                                            eff_thread_id as i32,
                                        )));
                                    }
                                    if let Some(ref_id) = reply_to {
                                        fb = fb.reply_parameters(ReplyParameters {
                                            message_id: MessageId(ref_id),
                                            ..Default::default()
                                        });
                                    }
                                    if let Err(e2) = fb.await {
                                        tracing::error!(
                                            ?key,
                                            "reflection plain-text fallback also failed: {:#}",
                                            e2
                                        );
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            tracing::warn!(?key, "reflection failed: {:#}; showing raw error", e);
                            match thinking_msg_id {
                                Some(msg_id) => {
                                    // raw_message is HTML produced by format_error_reply
                                    // (stderr is html-escaped, wrapped in <pre>). Try HTML
                                    // edit first; on failure, fall through to the plain-text
                                    // fallback path.
                                    let edit_result = ctx
                                        .bot
                                        .edit_message_text(tg_chat_id, msg_id, &raw_message)
                                        .parse_mode(teloxide::types::ParseMode::Html)
                                        .reply_markup(
                                            teloxide::types::InlineKeyboardMarkup::default(),
                                        )
                                        .await;
                                    if let Err(edit_err) = edit_result {
                                        tracing::warn!(
                                            ?key,
                                            "banner edit failed ({:#}); sending as new message",
                                            edit_err
                                        );
                                        let _ = ctx.bot.delete_message(tg_chat_id, msg_id).await;
                                        send_error_to_telegram(
                                            &ctx,
                                            tg_chat_id,
                                            eff_thread_id,
                                            &raw_message,
                                        )
                                        .await;
                                    }
                                }
                                None => {
                                    send_error_to_telegram(
                                        &ctx,
                                        tg_chat_id,
                                        eff_thread_id,
                                        &raw_message,
                                    )
                                    .await;
                                }
                            }
                        }
                    }
                }
            }

            // Auto-retain and prefetch (fire-and-forget).
            // reply_text_for_retain is only set on the Ok success path; reflection
            // replies are intentionally excluded from Hindsight (SYSTEM_NOTICE prompts
            // are platform noise, not user-agent conversation).
            if let Some(ref hs) = ctx.hindsight {
                // Auto-retain this turn.
                if let Some(ref reply_text) = reply_text_for_retain {
                    let hs_retain = Arc::clone(hs);
                    let retain_input = input.clone();
                    let retain_response = reply_text.clone();
                    let retain_doc_id = session_uuid.clone();
                    let sender_id = batch.first().and_then(|m| m.author.user_id);
                    let retain_tags_v = retain_tags(chat_id, sender_id, eff_thread_id, is_group);
                    let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
                    tokio::spawn(async move {
                        let content = serde_json::json!([
                            {"role": "user", "content": retain_input, "timestamp": now},
                            {"role": "assistant", "content": retain_response, "timestamp": now},
                        ])
                        .to_string();
                        if let Err(e) = hs_retain
                            .retain(
                                &content,
                                Some("conversation between Right Agent and the User"),
                                Some(&retain_doc_id),
                                Some("append"),
                                Some(&retain_tags_v),
                                right_agent::memory::resilient::POLICY_AUTO_RETAIN,
                            )
                            .await
                        {
                            tracing::warn!("auto-retain failed: {e:#}");
                        }
                    });
                }

                // Prefetch for next turn.
                let hs_recall = Arc::clone(hs);
                let recall_query = truncate_to_chars(&input, RECALL_MAX_CHARS).to_owned();
                let recall_tags_v = recall_tags(chat_id);
                let cache_key = format!("{}:{}", chat_id, eff_thread_id);
                let cache = ctx.prefetch_cache.clone();
                tokio::spawn(async move {
                    match hs_recall
                        .recall(
                            &recall_query,
                            Some(&recall_tags_v),
                            Some("any"),
                            right_agent::memory::resilient::POLICY_PREFETCH,
                        )
                        .await
                    {
                        Ok(results) if !results.is_empty() => {
                            let content = right_agent::memory::hindsight::join_recall_texts(&results);
                            if let Some(ref c) = cache {
                                c.put(&cache_key, content).await;
                            }
                        }
                        Ok(_) => {}
                        Err(right_agent::memory::ResilientError::CircuitOpen { .. }) => {
                            tracing::warn!("prefetch recall skipped: circuit open");
                        }
                        Err(right_agent::memory::ResilientError::Upstream(e)) => {
                            tracing::warn!("prefetch recall failed: {e:#}");
                        }
                    }
                });
            }

            ctx.idle_timestamp.store(
                chrono::Utc::now().timestamp(),
                std::sync::atomic::Ordering::Relaxed,
            );
        }

        // Worker exiting — remove DashMap entry to prevent stale sender (Pitfall 3)
        worker_map.remove(&key);
        tracing::debug!(?key, "worker task exited, DashMap entry removed");
    });

    tx_for_map
}

/// Send a Telegram message, optionally in a thread.
pub(crate) async fn send_tg(
    bot: &super::BotType,
    chat_id: teloxide::types::ChatId,
    eff_thread_id: i64,
    text: &str,
) -> Result<(), teloxide::RequestError> {
    let mut send = bot.send_message(chat_id, text);
    if eff_thread_id != 0 {
        send = send.message_thread_id(ThreadId(MessageId(eff_thread_id as i32)));
    }
    send.await?;
    Ok(())
}

/// Spawn a background task that requests a setup-token from the user.
///
/// 1. Sends instruction to user via Telegram.
/// 2. Waits for token from Telegram message intercept.
/// 3. Saves token to data.db.
fn spawn_token_request(
    ctx: &WorkerContext,
    tg_chat_id: teloxide::types::ChatId,
    eff_thread_id: i64,
) {
    let agent_name = ctx.agent_name.clone();
    let bot = ctx.bot.clone();
    let db_path = ctx.db_path.clone();
    let active_flag = Arc::clone(&ctx.auth_watcher_active);
    let auth_code_tx_slot = Arc::clone(&ctx.auth_code_tx);

    tokio::spawn(async move {
        // Send instruction to user (with HTML parse mode for <pre> formatting)
        let send_result = {
            let mut msg = bot.send_message(tg_chat_id, crate::login::auth_instruction_message());
            msg = msg.parse_mode(teloxide::types::ParseMode::Html);
            if eff_thread_id != 0 {
                msg = msg.message_thread_id(ThreadId(MessageId(eff_thread_id as i32)));
            }
            msg.await
        };
        if let Err(e) = send_result {
            tracing::warn!(agent = %agent_name, "token request: Telegram send failed: {e:#}");
            active_flag.store(false, Ordering::SeqCst);
            return;
        }

        // Create channel for token from Telegram
        let (token_tx, token_rx) = tokio::sync::oneshot::channel::<String>();
        auth_code_tx_slot.lock().await.replace(token_tx);

        // Create event channel
        let (event_tx, mut event_rx) = tokio::sync::mpsc::channel::<crate::login::LoginEvent>(4);

        // Spawn token request task
        let agent_for_login = agent_name.clone();
        tokio::spawn(async move {
            crate::login::request_token(&db_path, &agent_for_login, event_tx, token_rx).await;
        });

        // Process events with timeout
        let timeout = tokio::time::sleep(Duration::from_secs(300));
        tokio::pin!(timeout);

        tokio::select! {
            event = event_rx.recv() => {
                match event {
                    Some(crate::login::LoginEvent::Done) => {
                        if let Err(e) = send_tg(
                            &bot, tg_chat_id, eff_thread_id,
                            "Token saved. You can continue chatting.",
                        ).await {
                            tracing::warn!(agent = %agent_name, "token request: Telegram send failed: {e:#}");
                        }
                    }
                    Some(crate::login::LoginEvent::Error(msg)) => {
                        tracing::error!(agent = %agent_name, "token request: {msg}");
                        if let Err(e) = send_tg(
                            &bot, tg_chat_id, eff_thread_id,
                            &format!("Token setup failed: {msg}"),
                        ).await {
                            tracing::warn!(agent = %agent_name, "token request: Telegram send failed: {e:#}");
                        }
                    }
                    None => {
                        tracing::info!(agent = %agent_name, "token request: task exited");
                    }
                }
            }
            _ = &mut timeout => {
                tracing::warn!(agent = %agent_name, "token request: timed out after 5 min");
                if let Err(e) = send_tg(
                    &bot, tg_chat_id, eff_thread_id,
                    "Token request timed out after 5 minutes. Send another message to retry.",
                ).await {
                    tracing::warn!(agent = %agent_name, "token request: Telegram send failed: {e:#}");
                }
            }
        }

        // Cleanup
        auth_code_tx_slot.lock().await.take();
        active_flag.store(false, Ordering::SeqCst);
    });
}

/// Classification of why `invoke_cc` failed, used by `spawn_worker` to decide
/// between sending the raw error text and running a reflection pass.
#[derive(Debug)]
pub(crate) enum InvokeCcFailure {
    /// A failure we want to reflect on (safety timeout, non-zero exit of CC).
    /// The `raw_message` is preserved so callers can fall back to it if the
    /// reflection pass itself fails.
    Reflectable {
        kind: FailureKind,
        ring_buffer_tail: VecDeque<super::stream::StreamEvent>,
        session_uuid: String,
        raw_message: String,
        /// The live "thinking" message created during the failed CC run, if any.
        /// `spawn_worker` edits this into a banner before reflection and deletes
        /// it on reflection success (so the reflection reply is the substantive
        /// final update).
        thinking_msg_id: Option<teloxide::types::MessageId>,
    },
    /// A failure we do NOT want to reflect on (parse failures, pre-CC setup
    /// errors, schema read failures). The `message` is sent to Telegram verbatim.
    NonReflectable { message: String },
}

impl From<String> for InvokeCcFailure {
    fn from(message: String) -> Self {
        InvokeCcFailure::NonReflectable { message }
    }
}

/// Successful payload returned by [`invoke_cc`].
pub(crate) struct CcReply {
    /// Parsed agent reply, or `None` when CC produced an empty/no-reply result.
    pub(crate) output: Option<ReplyOutput>,
    /// CC session UUID for this invocation (new or resumed).
    pub(crate) session_uuid: String,
    /// `true` if this invocation created a brand-new CC session
    /// (i.e. the worker's first turn in this chat/thread).
    pub(crate) is_first_call: bool,
}

/// Invoke `claude -p` and parse the reply tool call from its JSON output.
///
/// Returns `Ok(CcReply { output, session_uuid, is_first_call })` whenever no
/// failure needs to be surfaced to the user. `output` is `Some(ReplyOutput)`
/// for a normal agent reply and `None` for paths that produced no user-visible
/// reply (user-triggered stop, auth-token-flow handoff). Returns
/// `Err(InvokeCcFailure)` for subprocess failures, parse failures, or other
/// conditions that require an error reply.
async fn invoke_cc(
    input: &str,
    first_text: Option<&str>,
    chat_id: i64,
    eff_thread_id: i64,
    is_group: bool,
    ctx: &WorkerContext,
) -> Result<CcReply, InvokeCcFailure> {
    // Open per-worker DB connection (rusqlite is !Send — each worker opens its own)
    let conn = right_agent::memory::open_connection(&ctx.agent_dir, false)
        .map_err(|e| format!("⚠️ Agent error: DB open failed: {:#}", e))?;

    // Session lookup / create (SES-02, SES-03)
    let (cmd_args, is_first_call, session_uuid) =
        match get_active_session(&conn, chat_id, eff_thread_id) {
            Ok(Some(SessionRow {
                root_session_id, ..
            })) => {
                // Resume: --resume <root_session_id>
                let uuid = root_session_id.clone();
                (vec!["--resume".to_string(), root_session_id], false, uuid)
            }
            Ok(None) => {
                // First message: generate UUID, --session-id <uuid>
                let new_uuid = Uuid::new_v4().to_string();
                let label = first_text.map(truncate_label);
                create_session(&conn, chat_id, eff_thread_id, &new_uuid, label)
                    .map_err(|e| format!("⚠️ Agent error: session create failed: {:#}", e))?;
                let uuid = new_uuid.clone();
                (vec!["--session-id".to_string(), new_uuid], true, uuid)
            }
            Err(e) => {
                return Err(format!("⚠️ Agent error: session lookup failed: {:#}", e).into());
            }
        };

    // Bootstrap mode detection: check if BOOTSTRAP.md exists in agent dir.
    let bootstrap_mode = ctx.agent_dir.join("BOOTSTRAP.md").exists();
    if bootstrap_mode {
        tracing::info!(?chat_id, "bootstrap mode: BOOTSTRAP.md present");
    }

    // Disallow CC built-in tools that conflict with MCP equivalents.
    let disallowed_tools: Vec<String> = [
        "CronCreate",
        "CronList",
        "CronDelete",
        "TaskCreate",
        "TaskUpdate",
        "TaskList",
        "TaskGet",
        "TaskOutput",
        "TaskStop",
        "EnterPlanMode",
        "ExitPlanMode",
        "RemoteTrigger",
    ]
    .iter()
    .map(|&s| s.into())
    .collect();

    let schema_filename = if bootstrap_mode {
        "bootstrap-schema.json"
    } else {
        "reply-schema.json"
    };
    let reply_schema_path = ctx.agent_dir.join(".claude").join(schema_filename);
    let reply_schema = std::fs::read_to_string(&reply_schema_path)
        .map_err(|e| format_error_reply(-1, &format!("{schema_filename} read failed: {:#}", e)))?;

    let mcp_path =
        super::invocation::mcp_config_path(ctx.ssh_config_path.as_deref(), &ctx.agent_dir);

    let mut invocation = super::invocation::ClaudeInvocation {
        mcp_config_path: Some(mcp_path),
        json_schema: Some(reply_schema),
        output_format: super::invocation::OutputFormat::StreamJson,
        model: ctx.model.clone(),
        max_budget_usd: None,
        max_turns: None,
        resume_session_id: None,
        new_session_id: None,
        allowed_tools: vec![],
        disallowed_tools,
        extra_args: vec![],
        prompt: None, // stdin-piped
    };

    // Session management (resume vs new).
    match &cmd_args[..] {
        [flag, sid] if flag == "--resume" => invocation.resume_session_id = Some(sid.clone()),
        [flag, sid] if flag == "--session-id" => invocation.new_session_id = Some(sid.clone()),
        _ => {}
    }

    let claude_args = invocation.into_args();

    // Fetch MCP server instructions from aggregator (non-fatal on error).
    let mcp_instructions: Option<String> =
        match ctx.internal_client.mcp_instructions(&ctx.agent_name).await {
            Ok(resp) => {
                // Only include if there's actual content beyond the header
                if resp.instructions.trim().len()
                    > right_agent::codegen::mcp_instructions::MCP_INSTRUCTIONS_HEADER
                        .trim()
                        .len()
                {
                    Some(resp.instructions)
                } else {
                    None
                }
            }
            Err(e) => {
                tracing::warn!("failed to fetch MCP instructions from aggregator: {e:#}");
                None
            }
        };

    // Generate base system prompt (identity-neutral — no agent name to avoid
    // contradicting IDENTITY.md which the agent may have customized).
    let (sandbox_mode, home_dir) = if ctx.ssh_config_path.is_some() {
        (
            right_agent::agent::types::SandboxMode::Openshell,
            "/sandbox".to_owned(),
        )
    } else {
        (
            right_agent::agent::types::SandboxMode::None,
            ctx.agent_dir.to_string_lossy().into_owned(),
        )
    };
    let base_prompt =
        right_agent::codegen::generate_system_prompt(&ctx.agent_name, &sandbox_mode, &home_dir);

    let memory_mode = if ctx.hindsight.is_some() {
        let sandbox_path = if ctx.ssh_config_path.is_some() {
            "/sandbox/.claude/composite-memory.md".to_owned()
        } else {
            ctx.agent_dir
                .join(".claude")
                .join("composite-memory.md")
                .to_string_lossy()
                .into_owned()
        };

        let cache_key = format!("{}:{}", chat_id, eff_thread_id);
        let cached = if let Some(ref cache) = ctx.prefetch_cache {
            cache.get(&cache_key).await
        } else {
            None
        };

        let recall_content = if let Some(content) = cached {
            Some(content)
        } else if let Some(ref hs) = ctx.hindsight {
            tracing::info!(?chat_id, "prefetch cache miss, blocking recall");
            let truncated_query = truncate_to_chars(input, RECALL_MAX_CHARS);
            let recall_tags_v = recall_tags(chat_id);
            match hs
                .recall(
                    truncated_query,
                    Some(&recall_tags_v),
                    Some("any"),
                    right_agent::memory::resilient::POLICY_BLOCKING_RECALL,
                )
                .await
            {
                Ok(results) if !results.is_empty() => {
                    let content = right_agent::memory::hindsight::join_recall_texts(&results);
                    if let Some(ref cache) = ctx.prefetch_cache {
                        cache.put(&cache_key, content.clone()).await;
                    }
                    Some(content)
                }
                Ok(_) => None,
                Err(right_agent::memory::ResilientError::CircuitOpen { .. }) => {
                    tracing::warn!(?chat_id, "blocking recall skipped: circuit open");
                    None
                }
                Err(right_agent::memory::ResilientError::Upstream(e)) => {
                    tracing::warn!(?chat_id, "blocking recall failed: {e:#}");
                    None
                }
            }
        } else {
            None
        };

        let wrapper_status = ctx
            .hindsight
            .as_ref()
            .map(|h| h.status())
            .unwrap_or(right_agent::memory::MemoryStatus::Healthy);
        let client_drops_24h = if let Some(ref h) = ctx.hindsight {
            h.client_drops_24h().await
        } else {
            0
        };

        let marker = build_memory_marker(wrapper_status, client_drops_24h);
        match (recall_content.as_deref(), marker.as_deref()) {
            (None, None) => {
                let sandbox_ref = match (
                    ctx.ssh_config_path.as_deref(),
                    ctx.resolved_sandbox.as_deref(),
                ) {
                    (Some(ssh_config), Some(sandbox_name)) => Some(super::prompt::SandboxRef {
                        ssh_config,
                        sandbox_name,
                    }),
                    _ => None,
                };
                super::prompt::remove_composite_memory(&ctx.agent_dir, sandbox_ref).await;
            }
            (content, marker_str) => {
                // content may be None (no recall) while marker is Some —
                // deploy a marker-only file so the agent still sees status.
                let body = content.unwrap_or("");
                if let Err(e) = super::prompt::deploy_composite_memory(
                    body,
                    "NOT new user input. Treat as background",
                    &ctx.agent_dir,
                    ctx.resolved_sandbox.as_deref(),
                    marker_str,
                )
                .await
                {
                    tracing::warn!("composite-memory deploy failed: {e:#}");
                }
            }
        }
        Some(super::prompt::MemoryMode::Hindsight {
            composite_memory_path: sandbox_path,
        })
    } else {
        Some(super::prompt::MemoryMode::File)
    };

    let mut cmd = if let Some(ref ssh_config) = ctx.ssh_config_path {
        // OpenShell sandbox: composite system prompt assembled IN the sandbox
        // from fresh files — single SSH command, no extra roundtrips.
        let ssh_host =
            right_agent::openshell::ssh_host_for_sandbox(ctx.resolved_sandbox.as_deref().unwrap());
        let mut assembly_script = super::prompt::build_prompt_assembly_script(
            &base_prompt,
            bootstrap_mode,
            "/sandbox",
            "/tmp/right-system-prompt.md",
            "/sandbox",
            &claude_args,
            mcp_instructions.as_deref(),
            memory_mode.as_ref(),
        );
        // Inject auth token as env var in the remote shell
        if let Some(token) = crate::login::load_auth_token(&ctx.db_path) {
            let escaped_token = token.replace('\'', "'\\''");
            assembly_script =
                format!("export CLAUDE_CODE_OAUTH_TOKEN='{escaped_token}'\n{assembly_script}");
        }
        let mut c = tokio::process::Command::new("ssh");
        c.arg("-F").arg(ssh_config);
        c.arg(&ssh_host);
        c.arg("--");
        c.arg(assembly_script);
        c
    } else {
        // No-sandbox: same shell template, paths point to host agent_dir.
        let agent_dir_str = ctx.agent_dir.to_string_lossy();
        let prompt_path = ctx
            .agent_dir
            .join(".claude")
            .join("composite-system-prompt.md");
        let prompt_path_str = prompt_path.to_string_lossy();
        let assembly_script = super::prompt::build_prompt_assembly_script(
            &base_prompt,
            bootstrap_mode,
            &agent_dir_str,
            &prompt_path_str,
            &agent_dir_str,
            &claude_args,
            mcp_instructions.as_deref(),
            memory_mode.as_ref(),
        );

        let mut c = tokio::process::Command::new("bash");
        c.arg("-c");
        c.arg(&assembly_script);
        c.env("HOME", &ctx.agent_dir);
        c.env("USE_BUILTIN_RIPGREP", "0");
        if let Some(token) = crate::login::load_auth_token(&ctx.db_path) {
            c.env("CLAUDE_CODE_OAUTH_TOKEN", &token);
        }
        c.current_dir(&ctx.agent_dir);
        c
    };
    cmd.stdin(Stdio::piped());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    let sandboxed = ctx.ssh_config_path.is_some();
    tracing::info!(
        ?chat_id,
        ?eff_thread_id,
        is_first_call,
        sandboxed,
        "invoking claude -p"
    );

    let mut child = right_agent::process_group::ProcessGroupChild::spawn(cmd)
        .map_err(|e| format_error_reply(-1, &format!("spawn failed: {:#}", e)))?;

    // Write input to stdin, then drop to signal EOF.
    if let Some(mut stdin) = child.stdin() {
        use tokio::io::AsyncWriteExt;
        stdin
            .write_all(input.as_bytes())
            .await
            .map_err(|e| format_error_reply(-1, &format!("stdin write failed: {:#}", e)))?;
    }

    // Insert stop token so callback handler can kill this CC session.
    let stop_token = CancellationToken::new();
    ctx.stop_tokens
        .insert((chat_id, eff_thread_id), stop_token.clone());

    // Stream stdout line-by-line: log to file, parse events, update thinking message.
    let stdout = child
        .stdout()
        .ok_or_else(|| format_error_reply(-1, "no stdout handle"))?;

    use tokio::io::{AsyncBufReadExt, BufReader};
    let mut lines = BufReader::new(stdout).lines();

    // Per-session stream log file.
    let stream_log_dir = ctx
        .agent_dir
        .parent()
        .and_then(|p| p.parent())
        .unwrap_or(&ctx.agent_dir)
        .join("logs")
        .join("streams");
    std::fs::create_dir_all(&stream_log_dir).ok();
    let session_id_for_log = cmd_args
        .first()
        .filter(|a| a.contains('-') && a.len() > 30)
        .or(cmd_args.get(1))
        .cloned()
        .unwrap_or_else(|| "unknown".to_string());
    let stream_log_path = stream_log_dir.join(format!("{session_id_for_log}.ndjson"));
    let mut stream_log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&stream_log_path)
        .ok();

    let mut ring_buffer = super::stream::EventRingBuffer::new(5);
    let mut usage = super::stream::StreamUsage::default();
    let mut result_line: Option<String> = None;
    let mut api_key_source: Option<String> = None;
    let mut thinking_msg_id: Option<teloxide::types::MessageId> = None;
    let mut last_edit = tokio::time::Instant::now();
    let mut total_assistant_events: u32 = 0;
    let tg_chat_id = ctx.chat_id;

    let deadline = tokio::time::Instant::now() + Duration::from_secs(CC_TIMEOUT_SECS);
    let mut timed_out = false;
    let mut stopped = false;

    loop {
        tokio::select! {
            line_result = lines.next_line() => {
                match line_result {
                    Ok(Some(line)) => {
                        // Write to stream log file.
                        if let Some(ref mut log) = stream_log {
                            use std::io::Write;
                            let _ = writeln!(log, "{line}");
                        }

                        if api_key_source.is_none()
                            && let Some(src) = super::stream::parse_api_key_source(&line)
                        {
                            api_key_source = Some(src);
                        }

                        let event = super::stream::parse_stream_event(&line);

                        match &event {
                            super::stream::StreamEvent::Result(json) => {
                                usage = super::stream::parse_usage(json);
                                result_line = Some(json.clone());

                                match super::stream::parse_usage_full(json) {
                                    Some(mut breakdown) => {
                                        breakdown.api_key_source = api_key_source
                                            .clone()
                                            .unwrap_or_else(|| "none".into());
                                        if let Err(e) =
                                            right_agent::usage::insert::insert_interactive(
                                                &conn,
                                                &breakdown,
                                                chat_id,
                                                eff_thread_id,
                                            )
                                        {
                                            tracing::warn!(
                                                ?chat_id,
                                                "usage insert failed: {e:#}"
                                            );
                                        }
                                    }
                                    None => tracing::warn!(
                                        ?chat_id,
                                        "result event missing required usage fields"
                                    ),
                                }
                            }
                            _ => {
                                if let Some(formatted) = super::stream::format_event(&event) {
                                    total_assistant_events += 1;
                                    tracing::info!(?chat_id, turn = total_assistant_events, "{formatted}");
                                }
                                ring_buffer.push(&event);
                                // Update turn count from assistant events.
                                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&line)
                                    && v.pointer("/message/usage/output_tokens").is_some()
                                {
                                    usage.num_turns = usage.num_turns.max(1);
                                }
                            }
                        }

                        // Thinking message: always send (Stop button anchor).
                        // show_thinking=true: update with events every 2s.
                        // show_thinking=false: send static "Working..." once, no updates.
                        if super::stream::format_event(&event).is_some() {
                            let kb = stop_keyboard(chat_id, eff_thread_id);

                            if thinking_msg_id.is_none() {
                                // First displayable event — send thinking message.
                                // In groups, always fall back to the static "Working..."
                                // placeholder to avoid noisy live updates.
                                let text = if ctx.show_thinking && !is_group {
                                    super::stream::format_thinking_message(
                                        ring_buffer.events(),
                                        &usage,
                                    )
                                } else {
                                    "\u{23f3} Working...".to_string()
                                };
                                let mut send = ctx.bot.send_message(tg_chat_id, &text)
                                    .parse_mode(teloxide::types::ParseMode::Html)
                                    .reply_markup(kb);
                                if eff_thread_id != 0 {
                                    send = send.message_thread_id(
                                        ThreadId(MessageId(eff_thread_id as i32)),
                                    );
                                }
                                if let Ok(msg) = send.await {
                                    thinking_msg_id = Some(msg.id);
                                }
                                last_edit = tokio::time::Instant::now();
                            } else if ctx.show_thinking
                                && !is_group
                                && last_edit.elapsed() >= Duration::from_secs(2)
                            {
                                // Throttled update (show_thinking=true only).
                                let text = super::stream::format_thinking_message(
                                    ring_buffer.events(),
                                    &usage,
                                );
                                if let Some(msg_id) = thinking_msg_id {
                                    let _ = ctx
                                        .bot
                                        .edit_message_text(tg_chat_id, msg_id, &text)
                                        .parse_mode(teloxide::types::ParseMode::Html)
                                        .reply_markup(kb)
                                        .await;
                                }
                                last_edit = tokio::time::Instant::now();
                            }
                        }
                    }
                    Ok(None) => break, // stdout closed — process exited
                    Err(e) => {
                        tracing::warn!(?chat_id, "stream read error: {e:#}");
                        break;
                    }
                }
            }
            _ = tokio::time::sleep_until(deadline) => {
                timed_out = true;
                child.kill().await.ok();
                break;
            }
            _ = stop_token.cancelled() => {
                stopped = true;
                child.kill().await.ok();
                break;
            }
        }
    }

    // Wait for process exit.
    let exit_status = child.wait().await.ok();
    let exit_code = exit_status.and_then(|s| s.code()).unwrap_or(-1);

    // Remove stop token — session no longer cancellable.
    ctx.stop_tokens.remove(&(chat_id, eff_thread_id));

    // Read any remaining stderr.
    let stderr_str = if let Some(mut stderr) = child.stderr() {
        let mut buf = String::new();
        use tokio::io::AsyncReadExt;
        let _ = stderr.read_to_string(&mut buf).await;
        buf
    } else {
        String::new()
    };

    tracing::info!(
        ?chat_id,
        exit_code,
        timed_out,
        stopped,
        stream_log = %stream_log_path.display(),
        sandboxed,
        "claude -p finished"
    );

    if !stderr_str.is_empty() {
        tracing::warn!(?chat_id, stderr = %stderr_str, "CC stderr");
    }

    let stdout_str = result_line.unwrap_or_default();

    // If we're about to return a Reflectable, spawn_worker will edit the
    // thinking message into a banner — skip the cost/turns finalization here
    // to avoid a visible flash of the final summary before the banner.
    let will_reflect = timed_out || (exit_code != 0 && !is_auth_error(&stdout_str));

    // Final thinking message update based on completion mode.
    if let Some(msg_id) = thinking_msg_id {
        if stopped {
            // Stopped by user — show final state, remove keyboard.
            // In groups we never rendered the thinking view, so reuse the
            // "Working..." placeholder for consistency with the initial send.
            let text = if ctx.show_thinking && !is_group {
                let mut msg = super::stream::format_thinking_message(ring_buffer.events(), &usage);
                msg.push_str("\n\u{26d4} Stopped");
                msg
            } else {
                "\u{23f3} Working...\n\u{26d4} Stopped".to_string()
            };
            let _ = ctx
                .bot
                .edit_message_text(tg_chat_id, msg_id, &text)
                .parse_mode(teloxide::types::ParseMode::Html)
                .reply_markup(teloxide::types::InlineKeyboardMarkup::default())
                .await;
        } else if !will_reflect && ctx.show_thinking && !is_group {
            // Normal finish with thinking — final cost/turns, remove keyboard.
            let text = super::stream::format_thinking_message(ring_buffer.events(), &usage);
            let _ = ctx
                .bot
                .edit_message_text(tg_chat_id, msg_id, &text)
                .parse_mode(teloxide::types::ParseMode::Html)
                .reply_markup(teloxide::types::InlineKeyboardMarkup::default())
                .await;
        } else if !will_reflect {
            // Normal finish without thinking (or group chat) — delete the anchor message.
            let _ = ctx.bot.delete_message(tg_chat_id, msg_id).await;
        }
        // When will_reflect is true, DO NOT touch the thinking message here —
        // spawn_worker will edit it into a banner.
    }

    // Handle user-initiated stop.
    if stopped {
        tracing::info!(?chat_id, "CC session stopped by user");
        // No reply to send — thinking message already updated.
        return Ok(CcReply {
            output: None,
            session_uuid,
            is_first_call,
        });
    }

    // Handle timeout.
    if timed_out {
        let mut timeout_msg = format!(
            "⚠️ Agent timed out ({CC_TIMEOUT_SECS}s safety limit). Last activity:\n─────────────\n"
        );
        for event in ring_buffer.events() {
            if let Some(formatted) = super::stream::format_event(event) {
                timeout_msg.push_str(&formatted);
                timeout_msg.push('\n');
            }
        }
        timeout_msg.push_str(&format!("\nStream log: {}", stream_log_path.display()));
        return Err(InvokeCcFailure::Reflectable {
            kind: FailureKind::SafetyTimeout {
                limit_secs: CC_TIMEOUT_SECS,
            },
            ring_buffer_tail: ring_buffer.events().clone(),
            session_uuid: session_uuid.clone(),
            raw_message: timeout_msg,
            thinking_msg_id,
        });
    }

    // DIS-06: non-zero exit → error reply
    if exit_code != 0 {
        // Log full output on failure for debuggability.
        tracing::error!(
            ?chat_id,
            exit_code,
            stdout = %stdout_str.chars().take(1000).collect::<String>(),
            stderr = %stderr_str,
            "claude -p failed"
        );

        // Check for auth error — trigger login flow if sandboxed.
        if is_auth_error(&stdout_str) {
            tracing::warn!(?chat_id, "detected auth error from CC");
            // Deactivate the session created before invoke_cc — it's from a failed auth
            // attempt and must not be resumed. Next message will start fresh.
            deactivate_current(&conn, chat_id, eff_thread_id)
                .map_err(|e| tracing::error!(?chat_id, "deactivate_current on auth error: {:#}", e))
                .ok();
            if ctx.ssh_config_path.is_some() {
                // Sandbox mode: spawn token request if not already active.
                if !ctx.auth_watcher_active.swap(true, Ordering::SeqCst) {
                    let tg_chat_id = ctx.chat_id;
                    if let Err(e) = send_tg(
                        &ctx.bot,
                        tg_chat_id,
                        ctx.effective_thread_id,
                        "Claude needs authentication. Setup instructions incoming...",
                    )
                    .await
                    {
                        tracing::warn!(?chat_id, "failed to send auth error notification: {e:#}");
                    }
                    spawn_token_request(ctx, tg_chat_id, ctx.effective_thread_id);
                    // Return Ok(None) — the initial message above is sufficient,
                    // don't send a second error message before instructions arrive.
                    return Ok(CcReply {
                        output: None,
                        session_uuid,
                        is_first_call,
                    });
                } else {
                    // Token request already running — silent, don't spam.
                    return Ok(CcReply {
                        output: None,
                        session_uuid,
                        is_first_call,
                    });
                }
            } else {
                // No-sandbox: also use token request flow.
                if !ctx.auth_watcher_active.swap(true, Ordering::SeqCst) {
                    let tg_chat_id = ctx.chat_id;
                    if let Err(e) = send_tg(
                        &ctx.bot,
                        tg_chat_id,
                        ctx.effective_thread_id,
                        "Claude needs authentication. Setup instructions incoming...",
                    )
                    .await
                    {
                        tracing::warn!(?chat_id, "failed to send auth error notification: {e:#}");
                    }
                    spawn_token_request(ctx, tg_chat_id, ctx.effective_thread_id);
                    return Ok(CcReply {
                        output: None,
                        session_uuid,
                        is_first_call,
                    });
                } else {
                    return Ok(CcReply {
                        output: None,
                        session_uuid,
                        is_first_call,
                    });
                }
            }
        }

        // If this was the first call, CC never created the session — deactivate
        // the DB record so the next message starts fresh instead of trying to
        // --resume a session that doesn't exist on the CC side.
        if is_first_call {
            deactivate_current(&conn, chat_id, eff_thread_id)
                .map_err(|e| {
                    tracing::error!(
                        ?chat_id,
                        "deactivate_current on first-call failure: {:#}",
                        e
                    )
                })
                .ok();
        }

        // Non-auth error: generic error reply.
        let error_detail = if stderr_str.trim().is_empty() && !stdout_str.trim().is_empty() {
            format!(
                "(stderr empty, stdout): {}",
                stdout_str.chars().take(500).collect::<String>()
            )
        } else {
            stderr_str.to_string()
        };
        let raw = format_error_reply(exit_code, &error_detail);
        return Err(InvokeCcFailure::Reflectable {
            kind: FailureKind::NonZeroExit { code: exit_code },
            ring_buffer_tail: ring_buffer.events().clone(),
            session_uuid: session_uuid.clone(),
            raw_message: raw,
            thinking_msg_id,
        });
    }

    // DIS-04: parse session_id for debug verification (D-15: mismatch only warns)
    match parse_reply_output(&stdout_str) {
        Ok((reply_output, session_id_from_cc)) => {
            // D-15: verify session_id at debug level only
            if let (Some(cc_sid), true) = (session_id_from_cc, is_first_call)
                && let Ok(Some(active)) = get_active_session(&conn, chat_id, eff_thread_id)
                && cc_sid != active.root_session_id
            {
                tracing::warn!(
                    ?chat_id,
                    cc_session_id = %cc_sid,
                    stored_session_id = %active.root_session_id,
                    "session_id mismatch between CC and stored — not blocking"
                );
            }
            // Update last_used_at (non-fatal: log error but do not fail the reply)
            if let Ok(Some(active)) = get_active_session(&conn, chat_id, eff_thread_id) {
                touch_session(&conn, active.id)
                    .map_err(|e| tracing::error!(?chat_id, "touch_session failed: {:#}", e))
                    .ok();
            }

            // Bootstrap completion is now detected by file presence after
            // reverse_sync in spawn_worker — no bootstrap_complete field needed.

            Ok(CcReply {
                output: Some(reply_output),
                session_uuid,
                is_first_call,
            })
        }
        Err(reason) => {
            // D-05: parse failure → error reply (HTML; html-escaped stdout in <pre>)
            tracing::warn!(?chat_id, reason, "CC response parse failed");
            let truncated: String = stdout_str.chars().take(200).collect();
            Err(format!(
                "\u{26a0}\u{fe0f} Agent error: {}\nRaw output (truncated):\n<pre>{}</pre>",
                html_escape(&reason),
                html_escape(&truncated),
            )
            .into())
        }
    }
}

async fn send_error_to_telegram(
    ctx: &WorkerContext,
    tg_chat_id: teloxide::types::ChatId,
    eff_thread_id: i64,
    message: &str,
) {
    use teloxide::types::{MessageId, ThreadId};
    let mut send = ctx
        .bot
        .send_message(tg_chat_id, message)
        .parse_mode(teloxide::types::ParseMode::Html);
    if eff_thread_id != 0 {
        send = send.message_thread_id(ThreadId(MessageId(eff_thread_id as i32)));
    }
    if let Err(e) = send.await {
        tracing::warn!(
            chat_id = ?tg_chat_id,
            eff_thread_id,
            "HTML error send failed, retrying plain text: {:#}",
            e
        );
        let plain = strip_html_tags(message);
        let mut fallback = ctx.bot.send_message(tg_chat_id, &plain);
        if eff_thread_id != 0 {
            fallback = fallback.message_thread_id(ThreadId(MessageId(eff_thread_id as i32)));
        }
        if let Err(e2) = fallback.await {
            tracing::error!(
                chat_id = ?tg_chat_id,
                eff_thread_id,
                "plain text fallback also failed: {:#}",
                e2
            );
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    // format_error_reply tests
    #[test]
    fn error_reply_contains_exit_code_and_stderr() {
        let reply = format_error_reply(1, "something failed");
        assert!(reply.contains("⚠️ Agent error (exit 1):"));
        assert!(reply.contains("something failed"));
        assert!(reply.contains("<pre>"));
        assert!(reply.contains("</pre>"));
    }

    #[test]
    fn error_reply_truncates_long_stderr() {
        let long_stderr = "y".repeat(500); // use 'y' — no collision with "exit" containing 'x'
        let reply = format_error_reply(2, &long_stderr);
        // The y-block in the reply should not exceed 300 chars of stderr
        let y_block: String = reply.chars().filter(|&c| c == 'y').collect();
        assert_eq!(y_block.len(), 300);
    }

    #[test]
    fn error_reply_escapes_html_special_chars() {
        let stderr = "status: <FailedPrecondition> & \"sandbox is not ready\"";
        let reply = format_error_reply(255, stderr);
        // raw special characters must not leak through as active HTML
        assert!(!reply.contains("<FailedPrecondition>"));
        assert!(reply.contains("&lt;FailedPrecondition&gt;"));
        assert!(reply.contains("&amp;"));
    }

    // parse_reply_output tests (new structured output format per D-03)
    #[test]
    fn parse_reply_output_content_string() {
        let json = r#"{"session_id":"abc","result":{"content":"hello","reply_to_message_id":null,"attachments":null}}"#;
        let (output, session_id) = parse_reply_output(json).unwrap();
        assert_eq!(output.content.as_deref(), Some("hello"));
        assert_eq!(session_id.as_deref(), Some("abc"));
    }

    #[test]
    fn parse_reply_output_content_null() {
        let json = r#"{"result":{"content":null}}"#;
        let (output, _) = parse_reply_output(json).unwrap();
        assert!(output.content.is_none());
    }

    #[test]
    fn parse_reply_output_missing_result_returns_error() {
        let json = r#"{"session_id":"x"}"#;
        let result = parse_reply_output(json);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("missing both"));
    }

    #[test]
    fn parse_reply_output_reply_to_message_id() {
        let json = r#"{"result":{"content":"hi","reply_to_message_id":42,"attachments":null}}"#;
        let (output, _) = parse_reply_output(json).unwrap();
        assert_eq!(output.reply_to_message_id, Some(42));
    }

    #[test]
    fn parse_reply_output_plain_string_result_wrapped_as_content() {
        // CC sometimes returns "result": "plain text" after MCP tool use instead of complying
        // with --json-schema. Must deliver the message rather than show an error.
        let json = r#"{"session_id":"abc","result":"hello from plain result"}"#;
        let (output, session_id) = parse_reply_output(json).unwrap();
        assert_eq!(output.content.as_deref(), Some("hello from plain result"));
        assert_eq!(session_id.as_deref(), Some("abc"));
    }

    #[test]
    fn parse_reply_output_empty_string_result_is_silent() {
        let json = r#"{"result":""}"#;
        let (output, _) = parse_reply_output(json).unwrap();
        assert!(output.content.is_none());
    }

    #[test]
    fn parse_reply_output_array_result_returns_error() {
        // Array instead of object should fail deserialization
        let json = r#"{"result":[{"type":"text"}]}"#;
        let result = parse_reply_output(json);
        assert!(result.is_err());
    }

    #[test]
    fn parse_reply_output_structured_output_field() {
        // When structured_output is present, it should be used instead of result
        let json = r#"{"session_id":"abc","result":"","structured_output":{"content":"Hello from structured!","reply_to_message_id":null,"attachments":null}}"#;
        let (output, session_id) = parse_reply_output(json).unwrap();
        assert_eq!(output.content.as_deref(), Some("Hello from structured!"));
        assert_eq!(session_id.as_deref(), Some("abc"));
    }

    #[test]
    fn parse_reply_output_falls_back_to_result_when_no_structured_output() {
        // When structured_output is absent, fall back to result field
        let json = r#"{"session_id":"xyz","result":{"content":"Fallback result","reply_to_message_id":null,"attachments":null}}"#;
        let (output, session_id) = parse_reply_output(json).unwrap();
        assert_eq!(output.content.as_deref(), Some("Fallback result"));
        assert_eq!(session_id.as_deref(), Some("xyz"));
    }

    #[test]
    fn parse_reply_output_missing_result_and_structured_output_returns_error() {
        let json = r#"{"session_id":"x"}"#;
        let result = parse_reply_output(json);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.contains("missing both"),
            "error should mention both fields: {err}"
        );
    }

    #[test]
    fn parse_reply_output_with_attachments() {
        let json = r#"{"session_id":"abc","result":{"content":"Here you go","attachments":[{"type":"document","path":"/sandbox/outbox/data.csv","filename":"results.csv","caption":"Exported data"}]}}"#;
        let (output, session_id) = parse_reply_output(json).unwrap();
        assert_eq!(output.content.as_deref(), Some("Here you go"));
        assert_eq!(session_id.as_deref(), Some("abc"));
        let atts = output.attachments.unwrap();
        assert_eq!(atts.len(), 1);
        assert_eq!(atts[0].path, "/sandbox/outbox/data.csv");
        assert_eq!(atts[0].filename.as_deref(), Some("results.csv"));
    }

    #[test]
    fn parse_reply_output_text_only() {
        let json =
            r#"{"result":{"content":"hello","reply_to_message_id":null,"attachments":null}}"#;
        let (output, _) = parse_reply_output(json).unwrap();
        assert_eq!(output.content.as_deref(), Some("hello"));
        assert!(output.attachments.is_none());
    }

    #[test]
    fn parse_reply_output_plain_string_fallback() {
        let json = r#"{"result":"plain text fallback"}"#;
        let (output, _) = parse_reply_output(json).unwrap();
        assert_eq!(output.content.as_deref(), Some("plain text fallback"));
        assert!(output.attachments.is_none());
    }

    // is_auth_error tests
    #[test]
    fn is_auth_error_detects_403() {
        let stdout = r#"{"type":"result","subtype":"success","is_error":true,"result":"Failed to authenticate. API Error: 403 status code (no body)"}"#;
        assert!(is_auth_error(stdout));
    }

    #[test]
    fn is_auth_error_detects_401() {
        let stdout = r#"{"type":"result","subtype":"success","is_error":true,"result":"Failed to authenticate. API Error: 401 Unauthorized"}"#;
        assert!(is_auth_error(stdout));
    }

    #[test]
    fn is_auth_error_detects_not_logged_in() {
        let stdout = r#"{"type":"result","subtype":"success","is_error":true,"result":"Not logged in · Please run /login"}"#;
        assert!(is_auth_error(stdout));
    }

    #[test]
    fn is_auth_error_detects_please_run_login() {
        let stdout = r#"{"type":"result","subtype":"success","is_error":true,"result":"Please run /login · API Error: 403"}"#;
        assert!(is_auth_error(stdout));
    }

    #[test]
    fn is_auth_error_false_for_normal_error() {
        let stdout = r#"{"type":"result","subtype":"success","is_error":true,"result":"Tool execution failed: timeout"}"#;
        assert!(!is_auth_error(stdout));
    }

    #[test]
    fn is_auth_error_false_for_success() {
        let stdout = r#"{"type":"result","subtype":"success","is_error":false,"result":{"content":"hello"}}"#;
        assert!(!is_auth_error(stdout));
    }

    #[test]
    fn is_auth_error_false_for_non_json() {
        assert!(!is_auth_error("Not logged in. Run claude auth login."));
    }

    #[test]
    fn is_auth_error_false_for_empty() {
        assert!(!is_auth_error(""));
    }

    // extract_auth_url tests
    #[test]
    fn extract_auth_url_finds_anthropic_url() {
        let lines = vec![
            "Initializing...".to_string(),
            "Open this URL to authenticate: https://console.anthropic.com/oauth/authorize?client_id=abc".to_string(),
            "Waiting for callback...".to_string(),
        ];
        let url = extract_auth_url(&lines);
        assert!(url.is_some());
        assert!(url.unwrap().contains("console.anthropic.com"));
    }

    #[test]
    fn extract_auth_url_finds_claude_ai_url() {
        let lines = vec!["Please visit: https://claude.ai/oauth/login?token=xyz".to_string()];
        let url = extract_auth_url(&lines);
        assert!(url.is_some());
        assert!(url.unwrap().contains("claude.ai"));
    }

    #[test]
    fn extract_auth_url_finds_claude_com_url() {
        // Real URL from `claude auth login --claudeai` inside sandbox.
        let lines = vec![
            "Opening browser to sign in…\r".to_string(),
            "If the browser didn't open, visit: https://claude.com/cai/oauth/authorize?code=true&client_id=abc".to_string(),
        ];
        let url = extract_auth_url(&lines);
        assert!(url.is_some());
        assert!(url.unwrap().contains("claude.com/cai/oauth/"));
    }

    #[test]
    fn extract_auth_url_returns_none_when_no_url() {
        let lines = vec![
            "Starting up...".to_string(),
            "Checking credentials...".to_string(),
        ];
        assert!(extract_auth_url(&lines).is_none());
    }

    #[test]
    fn extract_auth_url_ignores_non_auth_urls() {
        let lines = vec!["Connecting to https://api.example.com/v1".to_string()];
        assert!(extract_auth_url(&lines).is_none());
    }

    #[test]
    fn extract_auth_url_handles_empty() {
        let lines: Vec<String> = vec![];
        assert!(extract_auth_url(&lines).is_none());
    }

    #[test]
    fn extract_auth_url_ignores_non_oauth_anthropic_url() {
        // The "supported countries" link from error messages must not be picked up.
        let lines = vec![
            "Check supported countries at https://anthropic.com/supported-countries".to_string(),
        ];
        assert!(extract_auth_url(&lines).is_none());
    }

    #[test]
    fn extract_auth_url_extracts_just_url_from_line() {
        let lines = vec![
            "Go to https://console.anthropic.com/oauth/authorize?foo=bar to continue".to_string(),
        ];
        let url = extract_auth_url(&lines).unwrap();
        assert!(url.starts_with("https://"));
        assert!(!url.contains(" to continue"));
    }

    // bootstrap mode tests
    #[test]
    fn parse_reply_output_bootstrap_complete_true() {
        let json = r#"{"type":"result","result":{"content":"Done!","bootstrap_complete":true},"session_id":"abc-123"}"#;
        let (output, _sid) = parse_reply_output(json).unwrap();
        assert_eq!(output.content.as_deref(), Some("Done!"));
        assert_eq!(output.bootstrap_complete, Some(true));
    }

    #[test]
    fn parse_reply_output_bootstrap_complete_false() {
        let json = r#"{"type":"result","result":{"content":"What's your name?","bootstrap_complete":false},"session_id":"abc-123"}"#;
        let (output, _sid) = parse_reply_output(json).unwrap();
        assert_eq!(output.bootstrap_complete, Some(false));
    }

    #[test]
    fn parse_reply_output_no_bootstrap_field() {
        let json = r#"{"type":"result","result":{"content":"Hello!"},"session_id":"abc-123"}"#;
        let (output, _sid) = parse_reply_output(json).unwrap();
        assert_eq!(output.bootstrap_complete, None);
    }

    #[test]
    fn should_accept_bootstrap_all_files_present() {
        let dir = tempfile::tempdir().unwrap();
        for f in BOOTSTRAP_REQUIRED_FILES {
            std::fs::write(dir.path().join(f), "# test").unwrap();
        }
        assert!(should_accept_bootstrap(dir.path()));
    }

    #[test]
    fn should_accept_bootstrap_missing_files() {
        let dir = tempfile::tempdir().unwrap();
        // No identity files created
        assert!(!should_accept_bootstrap(dir.path()));
    }

    #[test]
    fn should_accept_bootstrap_partial_files() {
        let dir = tempfile::tempdir().unwrap();
        // Only IDENTITY.md exists
        std::fs::write(dir.path().join("IDENTITY.md"), "# test").unwrap();
        assert!(!should_accept_bootstrap(dir.path()));
    }

    #[test]
    fn stop_keyboard_format() {
        let kb = stop_keyboard(12345, 678);
        let buttons = &kb.inline_keyboard;
        assert_eq!(buttons.len(), 1);
        assert_eq!(buttons[0].len(), 1);
        assert_eq!(buttons[0][0].text, "\u{26d4} Stop");
        match &buttons[0][0].kind {
            teloxide::types::InlineKeyboardButtonKind::CallbackData(data) => {
                assert_eq!(data, "stop:12345:678");
            }
            other => panic!("expected CallbackData, got {other:?}"),
        }
    }

    #[test]
    fn truncate_to_chars_short_string() {
        assert_eq!(truncate_to_chars("hello", 800), "hello");
    }

    #[test]
    fn truncate_to_chars_exact_limit() {
        let s = "a".repeat(800);
        assert_eq!(truncate_to_chars(&s, 800).chars().count(), 800);
    }

    #[test]
    fn truncate_to_chars_over_limit() {
        let s = "a".repeat(1000);
        assert_eq!(truncate_to_chars(&s, 800).chars().count(), 800);
    }

    #[test]
    fn truncate_to_chars_multibyte() {
        let s = "é".repeat(1000);
        let truncated = truncate_to_chars(&s, 800);
        assert_eq!(truncated.chars().count(), 800);
        assert_eq!(truncated.len(), 1600);
    }

    #[test]
    fn truncate_to_chars_emoji() {
        let s = "🎯".repeat(1000);
        let truncated = truncate_to_chars(&s, 800);
        assert_eq!(truncated.chars().count(), 800);
        assert_eq!(truncated.len(), 3200);
    }

    #[test]
    fn truncate_to_chars_empty() {
        assert_eq!(truncate_to_chars("", 800), "");
    }

    #[test]
    fn truncate_to_chars_cyrillic() {
        let s = "я".repeat(500);
        let truncated = truncate_to_chars(&s, 800);
        assert_eq!(truncated.chars().count(), 500);
        assert_eq!(truncated, s);
    }

    // ── collect_batch / adaptive debounce window ──────────────────────────────
    //
    // These tests run under `#[tokio::test(start_paused = true)]`. Time is
    // virtual; `sleep` parks the test task and lets the paused runtime
    // auto-advance to the next pending timer when no task is ready, which
    // deterministically interleaves test main and the spawned `collect_batch`
    // task. We avoid `tokio::time::advance` because it bumps the clock without
    // running pending wakers — our `tx.send()` calls land in the channel before
    // the spawned task observes a freshly-elapsed timer, so the biased select
    // inside `collect_batch` would grab the message ahead of the timer.

    fn debug_msg(message_id: i32, media_group_id: Option<&str>) -> DebounceMsg {
        DebounceMsg {
            message_id,
            text: None,
            timestamp: Utc::now(),
            attachments: vec![],
            author: super::super::attachments::MessageAuthor {
                name: "u".into(),
                username: None,
                user_id: None,
            },
            forward_info: None,
            reply_to_id: None,
            address: None,
            group_open: true,
            chat: super::super::attachments::ChatContext::Group {
                id: -1001,
                title: None,
                topic_id: None,
            },
            reply_to_body: None,
            media_group_id: media_group_id.map(|s| s.to_string()),
        }
    }

    #[tokio::test(start_paused = true)]
    async fn fast_album_closes_after_idle_window() {
        let (tx, mut rx) = mpsc::channel::<DebounceMsg>(8);
        let first = debug_msg(1, Some("alb"));

        let task = tokio::spawn(async move { collect_batch(first, &mut rx).await });

        // Push siblings 2 and 3 with simulated 200 ms gaps.
        sleep(Duration::from_millis(200)).await;
        tx.send(debug_msg(2, Some("alb"))).await.unwrap();
        sleep(Duration::from_millis(200)).await;
        tx.send(debug_msg(3, Some("alb"))).await.unwrap();

        // No more arrivals — idle 1000 ms from msg 3 closes the window. The
        // batch returns once auto-advance reaches the deadline.
        let batch = task.await.unwrap();
        assert_eq!(batch.len(), 3);
        assert_eq!(
            batch.iter().map(|m| m.message_id).collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn slow_album_idle_reset_keeps_batch_open() {
        let (tx, mut rx) = mpsc::channel::<DebounceMsg>(8);
        let first = debug_msg(1, Some("alb"));

        let task = tokio::spawn(async move { collect_batch(first, &mut rx).await });

        // 600 ms — past the 500 ms non-media window, but in media-group mode the
        // idle window is 1000 ms from last arrival, so this still falls in.
        sleep(Duration::from_millis(600)).await;
        tx.send(debug_msg(2, Some("alb"))).await.unwrap();
        sleep(Duration::from_millis(900)).await;
        tx.send(debug_msg(3, Some("alb"))).await.unwrap();

        // Idle 1000 ms from msg 3 closes the batch via auto-advance.
        let batch = task.await.unwrap();
        assert_eq!(batch.len(), 3);
    }

    #[tokio::test(start_paused = true)]
    async fn album_hits_hard_cap_at_2500ms() {
        let (tx, mut rx) = mpsc::channel::<DebounceMsg>(8);
        let first = debug_msg(1, Some("alb"));

        let task = tokio::spawn(async move { collect_batch(first, &mut rx).await });

        // Drip-feed siblings every 700 ms. Idle alone never closes; hard cap at
        // 2500 ms from first arrival must terminate the batch. After msg4 the
        // deadline is min(last+1000=3100, first+2500=2500) = 2500. We then
        // sleep 600 ms — auto-advance fires the cap timer first, the task
        // closes and drops the receiver, so the follow-up send returns Err.
        sleep(Duration::from_millis(700)).await;
        tx.send(debug_msg(2, Some("alb"))).await.unwrap();
        sleep(Duration::from_millis(700)).await;
        tx.send(debug_msg(3, Some("alb"))).await.unwrap();
        sleep(Duration::from_millis(700)).await;
        tx.send(debug_msg(4, Some("alb"))).await.unwrap();
        sleep(Duration::from_millis(600)).await;
        let _ = tx.send(debug_msg(5, Some("alb"))).await;

        let batch = task.await.unwrap();
        assert_eq!(
            batch.iter().map(|m| m.message_id).collect::<Vec<_>>(),
            vec![1, 2, 3, 4],
            "hard cap must close at 2500 ms, leaving msg 5 outside the batch"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn non_album_keeps_500ms_window() {
        let (tx, mut rx) = mpsc::channel::<DebounceMsg>(8);
        let first = debug_msg(1, None);

        let task = tokio::spawn(async move { collect_batch(first, &mut rx).await });

        // sleep 600 ms — past the 500 ms idle window from first arrival.
        // Auto-advance fires the spawned task's 500 ms deadline first, so the
        // task closes and drops the receiver before main sends msg2. The
        // follow-up send returns Err; .ok() swallows.
        sleep(Duration::from_millis(600)).await;
        let _ = tx.send(debug_msg(2, None)).await;

        let batch = task.await.unwrap();
        assert_eq!(batch.len(), 1, "non-album message must use 500 ms window");
        assert_eq!(batch[0].message_id, 1);
    }

    #[tokio::test(start_paused = true)]
    async fn text_widens_window_when_album_joins() {
        let (tx, mut rx) = mpsc::channel::<DebounceMsg>(8);
        let first = debug_msg(1, None); // plain text

        let task = tokio::spawn(async move { collect_batch(first, &mut rx).await });

        // Album sibling joins at 200 ms — flips the batch into media-group mode.
        sleep(Duration::from_millis(200)).await;
        tx.send(debug_msg(2, Some("alb"))).await.unwrap();
        // Another sibling 700 ms later — still inside the new 1000 ms idle window.
        sleep(Duration::from_millis(700)).await;
        tx.send(debug_msg(3, Some("alb"))).await.unwrap();

        // No more arrivals — idle 1000 ms from msg 3 closes via auto-advance.
        let batch = task.await.unwrap();
        assert_eq!(batch.len(), 3);
    }

    #[test]
    fn batch_is_addressed_drops_all_none_group_batch() {
        let batch = vec![debug_msg(1, Some("alb")), debug_msg(2, Some("alb"))];
        assert!(!batch_is_addressed(&batch));
    }

    #[test]
    fn batch_is_addressed_passes_when_one_sibling_addressed() {
        let mut a = debug_msg(1, Some("alb"));
        a.address = Some(super::super::mention::AddressKind::GroupMentionText);
        let batch = vec![a, debug_msg(2, Some("alb"))];
        assert!(batch_is_addressed(&batch));
    }
}

#[cfg(test)]
mod tag_tests {
    use super::*;

    #[test]
    fn dm_tags_have_chat_only() {
        let t = retain_tags(42, Some(42), 0, false);
        assert_eq!(t, vec!["chat:42"]);
    }

    #[test]
    fn group_tags_have_user_and_topic() {
        let t = retain_tags(-1001, Some(100), 7, true);
        assert_eq!(t, vec!["chat:-1001", "user:100", "topic:7"]);
    }

    #[test]
    fn group_tags_no_topic_when_thread_zero() {
        let t = retain_tags(-1001, Some(100), 0, true);
        assert_eq!(t, vec!["chat:-1001", "user:100"]);
    }

    #[test]
    fn recall_tags_unchanged_by_group() {
        let t = recall_tags(-1001);
        assert_eq!(t, vec!["chat:-1001"]);
    }
}
