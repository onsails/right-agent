//! Typed owner-backed bootstrap interview state machine.

use anyhow::Context as _;

pub(crate) async fn recorded_state(
    client: &right_mcp::internal_client::InternalClient,
    agent: &str,
    stage: &'static str,
    chat_id: i64,
    thread_id: i64,
) -> anyhow::Result<crate::cc::prompt::BootstrapPromptState> {
    let answers = client
        .bootstrap_recorded_answers(&right_mcp::internal_db::BootstrapStageScopeRequest {
            agent: agent.to_owned(),
            chat_id,
            thread_id,
        })
        .await
        .context("load bootstrap answers through owner")?
        .answers;
    let mut by_stage = answers
        .into_iter()
        .map(|answer| (answer.stage, answer.answer))
        .collect::<std::collections::BTreeMap<_, _>>();
    Ok(crate::cc::prompt::BootstrapPromptState {
        stage,
        user_name: by_stage.remove("user_name"),
        agent_name: by_stage.remove("agent_name"),
        nature: by_stage.remove("nature"),
        vibe: by_stage.remove("vibe"),
        emoji: by_stage.remove("emoji"),
    })
}
