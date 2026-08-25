//! Bot-side typed database IPC helpers.
//!
//! Production bot code never opens `data.db`. Call sites that already carry
//! the shared `InternalClient` use it directly; leaf helpers that historically
//! received only `agent_dir` call [`client_for_agent_dir`] to derive the same
//! `~/.right/run/internal.sock` endpoint and validated agent identity. There is
//! no direct-open fallback.

use std::path::Path;

pub(crate) fn client_for_agent_dir(
    agent_dir: &Path,
) -> anyhow::Result<(right_mcp::internal_client::InternalClient, String)> {
    let agent = agent_dir
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .ok_or_else(|| anyhow::anyhow!("invalid agent directory {}", agent_dir.display()))?
        .to_owned();
    let agents_dir = agent_dir
        .parent()
        .ok_or_else(|| anyhow::anyhow!("agent directory has no parent: {}", agent_dir.display()))?;
    let home = agents_dir.parent().ok_or_else(|| {
        anyhow::anyhow!("agents directory has no parent: {}", agents_dir.display())
    })?;
    Ok((
        right_mcp::internal_client::InternalClient::new(home.join("run/internal.sock")),
        agent,
    ))
}

pub(crate) fn usage_dto(
    usage: &right_agent::usage::UsageBreakdown,
) -> right_mcp::internal_db::UsageBreakdownDto {
    right_mcp::internal_db::UsageBreakdownDto {
        session_uuid: usage.session_uuid.clone(),
        total_cost_usd: usage.total_cost_usd,
        num_turns: usage.num_turns,
        input_tokens: usage.input_tokens,
        output_tokens: usage.output_tokens,
        cache_creation_tokens: usage.cache_creation_tokens,
        cache_read_tokens: usage.cache_read_tokens,
        web_search_requests: usage.web_search_requests,
        web_fetch_requests: usage.web_fetch_requests,
        model_usage_json: usage.model_usage_json.clone(),
        api_key_source: usage.api_key_source.clone(),
        wall_elapsed_ms: usage.wall_elapsed_ms,
    }
}
pub(crate) fn request_id() -> String {
    uuid::Uuid::new_v4().to_string()
}
