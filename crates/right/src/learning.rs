use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::Context;
use right_agent::learned_skills::{LearningAction, LearningStatus};
use right_mcp::LEARNED_SKILL_PREFIX;
use right_mcp::internal_client::{InternalClient, ProgressSendRequest};
use right_mcp::tool_error::tool_error;
use rmcp::model::CallToolResult;
use schemars::JsonSchema;
use serde::Deserialize;

const LEARNING_SEND_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum LearningActionParam {
    Create,
    Update,
}

impl LearningActionParam {
    pub(crate) fn as_domain(self) -> LearningAction {
        match self {
            Self::Create => LearningAction::Create,
            Self::Update => LearningAction::Update,
        }
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct SkillLearningStartParams {
    #[schemars(
        description = "Learning action: create a new rightx-* skill or update an existing rightx-* skill."
    )]
    pub(crate) action: LearningActionParam,
    #[schemars(description = "Skill package name only, never a path. Must start with rightx-.")]
    pub(crate) skill_name: String,
    #[schemars(description = "Short reason for learning or updating this skill.")]
    pub(crate) reason: Option<String>,
    #[schemars(
        description = "Evidence references from the current turn. Use one non-empty ref for explicit user requests, otherwise two or more."
    )]
    pub(crate) event_refs: Option<Vec<String>>,
    #[schemars(
        description = "Localized user-visible start message. Required for foreground learning starts."
    )]
    pub(crate) message: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum LearningFinishStatusParam {
    Created,
    Updated,
    Aborted,
    Failed,
}

impl LearningFinishStatusParam {
    pub(crate) fn is_success(self) -> bool {
        matches!(self, Self::Created | Self::Updated)
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::Updated => "updated",
            Self::Aborted => "aborted",
            Self::Failed => "failed",
        }
    }

    pub(crate) fn as_domain(self) -> LearningStatus {
        match self {
            Self::Created => LearningStatus::Created,
            Self::Updated => LearningStatus::Updated,
            Self::Aborted => LearningStatus::Aborted,
            Self::Failed => LearningStatus::Failed,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum LearningHintOutcomeParam {
    AppliedAsHinted,
    AppliedDifferently,
    Refused,
}

impl LearningHintOutcomeParam {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::AppliedAsHinted => "applied_as_hinted",
            Self::AppliedDifferently => "applied_differently",
            Self::Refused => "refused",
        }
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct SkillLearningFinishParams {
    #[schemars(
        description = "Learning action: create a new rightx-* skill or update an existing rightx-* skill."
    )]
    pub(crate) action: LearningActionParam,
    #[schemars(description = "Skill package name only, never a path. Must start with rightx-.")]
    pub(crate) skill_name: String,
    #[schemars(
        description = "created/updated only after .claude/skills/<skill_name>/SKILL.md exists; otherwise aborted or failed."
    )]
    pub(crate) status: LearningFinishStatusParam,
    #[schemars(
        description = "LLM-authored localized receipt message. Required for successful created/updated finishes."
    )]
    pub(crate) message: Option<String>,
    #[schemars(
        description = "Optional short summary of what was learned, updated, aborted, or failed."
    )]
    pub(crate) summary: Option<String>,
    #[schemars(
        description = "Evidence references from the current turn. Use one non-empty ref for explicit user requests, otherwise two or more."
    )]
    pub(crate) event_refs: Option<Vec<String>>,
    #[schemars(
        description = "Optional. Probe-writer reports back whether the prefilter hint matched. One of: applied_as_hinted, applied_differently, refused."
    )]
    #[serde(default)]
    pub(crate) hint_outcome: Option<LearningHintOutcomeParam>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SkillPackageExpectation {
    MustExist,
    MustNotExist,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LearningMessagePhase {
    Start,
    FinishSuccess,
}

fn skill_name_error(message: impl Into<String>) -> CallToolResult {
    tool_error("invalid_argument", message.into(), None)
}

fn validate_skill_name_value(skill_name: &str) -> Result<(), String> {
    let len = skill_name.chars().count();
    if !(3..=80).contains(&len) {
        return Err("skill_name must be between 3 and 80 characters".to_owned());
    }
    if skill_name.starts_with('-') || skill_name.ends_with('-') {
        return Err("skill_name must not start or end with '-'".to_owned());
    }
    if !skill_name
        .bytes()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    {
        return Err(
            "skill_name must contain only lowercase ASCII letters, digits, and hyphens".to_owned(),
        );
    }
    Ok(())
}

pub(crate) fn validate_skill_name(skill_name: &str) -> Result<(), CallToolResult> {
    validate_skill_name_value(skill_name).map_err(skill_name_error)
}

pub(crate) fn skill_package_dir(
    agent_dir: &Path,
    skill_name: &str,
) -> Result<PathBuf, CallToolResult> {
    validate_skill_name(skill_name)?;
    let base = agent_dir.join(".claude").join("skills");
    let dir = base.join(skill_name);
    if !dir.starts_with(&base) {
        return Err(skill_name_error(
            "derived skill package path escaped .claude/skills",
        ));
    }
    Ok(dir)
}

pub(crate) async fn skill_package_exists(
    agent_name: &str,
    mtls_dir: Option<&Path>,
    agent_dir: &Path,
    skill_name: &str,
) -> anyhow::Result<bool> {
    validate_skill_name_value(skill_name).map_err(|e| anyhow::anyhow!("{e}"))?;

    let config = right_agent::agent::parse_agent_config(agent_dir)
        .map_err(|e| anyhow::anyhow!("{e:#}"))
        .with_context(|| format!("failed to parse agent config for {agent_name}"))?;
    let is_sandboxed = config.as_ref().map(|c| c.is_sandboxed()).unwrap_or(true);

    if is_sandboxed {
        let mtls_dir = mtls_dir.filter(|path| path.exists()).ok_or_else(|| {
            anyhow::anyhow!("sandboxed agent requires OpenShell mTLS for skill package checks")
        })?;
        let explicit_sandbox_name = config
            .as_ref()
            .and_then(|c| c.sandbox.as_ref())
            .and_then(|s| s.name.as_deref());
        let sandbox_name =
            right_openshell::openshell::resolve_sandbox_name(agent_name, explicit_sandbox_name);
        let mut client = right_openshell::openshell::connect_grpc(mtls_dir)
            .await
            .map_err(|e| anyhow::anyhow!("{e:#}"))
            .context("skill package check: failed to connect to OpenShell gRPC")?;
        let sandbox_id = right_openshell::openshell::resolve_sandbox_id(&mut client, &sandbox_name)
            .await
            .map_err(|e| anyhow::anyhow!("{e:#}"))
            .with_context(|| {
                format!("skill package check: failed to resolve sandbox ID for {sandbox_name}")
            })?;
        let sandbox_path = format!("/sandbox/.claude/skills/{skill_name}/SKILL.md");
        let (_, exit_code) = right_openshell::openshell::exec_in_sandbox(
            &mut client,
            &sandbox_id,
            &["test", "-f", &sandbox_path],
            right_openshell::openshell::DEFAULT_EXEC_TIMEOUT_SECS,
        )
        .await
        .map_err(|e| anyhow::anyhow!("{e:#}"))
        .with_context(|| format!("skill package check: exec test -f {sandbox_path} failed"))?;
        return Ok(exit_code == 0);
    }

    let host_dir = skill_package_dir(agent_dir, skill_name)
        .map_err(|_| anyhow::anyhow!("invalid derived skill package path"))?;
    Ok(host_dir.join("SKILL.md").is_file())
}

pub(crate) fn is_known_core_skill(skill_name: &str) -> bool {
    right_codegen::BUILTIN_SKILL_NAMES.contains(&skill_name)
        || right_codegen::BUILTIN_SKILL_LEGACY_NAMES.contains(&skill_name)
}

pub(crate) fn installed_json_marks_core(
    agent_dir: &Path,
    skill_name: &str,
) -> Result<bool, CallToolResult> {
    let path = agent_dir
        .join(".claude")
        .join("skills")
        .join("installed.json");
    let content = match std::fs::read_to_string(&path) {
        Ok(content) => content,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => {
            return Err(tool_error(
                "skill_registry_invalid",
                format!("failed to read skill registry {}: {e:#}", path.display()),
                None,
            ));
        }
    };
    let value = serde_json::from_str::<serde_json::Value>(&content).map_err(|e| {
        tool_error(
            "skill_registry_invalid",
            format!("failed to parse skill registry {}: {e:#}", path.display()),
            None,
        )
    })?;
    let Some(entry) = value.get(skill_name) else {
        return Ok(false);
    };
    Ok(source_marks_core(entry))
}

fn source_marks_core(value: &serde_json::Value) -> bool {
    let source = value.as_str().or_else(|| {
        value
            .as_object()
            .and_then(|object| object.get("source"))
            .and_then(|source| source.as_str())
    });
    matches!(
        source,
        Some("builtin" | "platform" | "core" | "codegen" | "bundled" | "codegen-owned")
    )
}

pub(crate) fn validate_learning_target(
    agent_dir: &Path,
    _action: LearningActionParam,
    skill_name: &str,
) -> Result<(), CallToolResult> {
    validate_skill_name(skill_name)?;
    let registry_marks_core = installed_json_marks_core(agent_dir, skill_name)?;
    if is_known_core_skill(skill_name) || registry_marks_core {
        return Err(tool_error(
            "skill_core_readonly",
            "core/platform/codegen skill packages are read-only",
            None,
        ));
    }
    if !skill_name.starts_with(LEARNED_SKILL_PREFIX) {
        return Err(tool_error(
            "invalid_argument",
            format!("skill learning requires skill_name to start with '{LEARNED_SKILL_PREFIX}'"),
            None,
        ));
    }
    Ok(())
}

pub(crate) fn validate_start_message(
    kind: crate::progress::ProgressInvocationKind,
    message: Option<&str>,
) -> Result<(), CallToolResult> {
    if !kind.sends_learning_messages() {
        return Ok(());
    }
    validate_nonempty_message("learning start message", message)
}

pub(crate) async fn validate_skill_package_state(
    agent_name: &str,
    mtls_dir: Option<&Path>,
    agent_dir: &Path,
    skill_name: &str,
    expectation: SkillPackageExpectation,
) -> Result<(), CallToolResult> {
    let exists = match skill_package_exists(agent_name, mtls_dir, agent_dir, skill_name).await {
        Ok(exists) => exists,
        Err(e) => {
            return Err(tool_error(
                "skill_package_check_failed",
                format!("{e:#}"),
                None,
            ));
        }
    };
    match (expectation, exists) {
        (SkillPackageExpectation::MustExist, false) => Err(tool_error(
            "skill_package_missing",
            format!("skill package not found at .claude/skills/{skill_name}/SKILL.md"),
            None,
        )),
        (SkillPackageExpectation::MustNotExist, true) => Err(tool_error(
            "skill_already_exists",
            format!("skill package already exists at .claude/skills/{skill_name}/SKILL.md"),
            None,
        )),
        _ => Ok(()),
    }
}

pub(crate) fn validate_finish_receipt_message(
    status: LearningFinishStatusParam,
    message: Option<&str>,
) -> Result<(), CallToolResult> {
    if !status.is_success() {
        return Ok(());
    }
    validate_nonempty_message("successful skill_learning_finish message", message)
}

fn validate_nonempty_message(label: &str, message: Option<&str>) -> Result<(), CallToolResult> {
    let Some(message) = message.map(str::trim).filter(|m| !m.is_empty()) else {
        return Err(tool_error(
            "invalid_argument",
            format!("{label} must not be empty"),
            None,
        ));
    };
    if message.chars().count() > crate::progress::PROGRESS_MESSAGE_MAX_CHARS {
        return Err(tool_error(
            "invalid_argument",
            format!(
                "learning message must be at most {} characters",
                crate::progress::PROGRESS_MESSAGE_MAX_CHARS
            ),
            None,
        ));
    }
    Ok(())
}

pub(crate) fn should_send_learning_message(
    kind: crate::progress::ProgressInvocationKind,
    phase: LearningMessagePhase,
) -> bool {
    let phase_sends = match phase {
        LearningMessagePhase::Start | LearningMessagePhase::FinishSuccess => true,
    };
    kind.sends_learning_messages() && phase_sends
}

pub(crate) async fn send_learning_message(
    progress: &crate::progress::ProgressRegistry,
    invocation_id: &str,
    phase: LearningMessagePhase,
    message: Option<&str>,
) -> Result<(), CallToolResult> {
    // Single mutex acquisition: looking up kind and target separately would let
    // an unregister between the two `.await`s flip the result from "send" to a
    // confusing `Unavailable` error after the phase gate already approved.
    let (kind, target) = match progress
        .learning_invocation_kind_and_target(invocation_id)
        .await
    {
        Ok(pair) => pair,
        Err(crate::progress::ProgressError::Unavailable) => {
            return Err(tool_error(
                "learning_unavailable",
                "learning messages are available only for a registered invocation",
                None,
            ));
        }
        Err(crate::progress::ProgressError::Forbidden) => {
            return Err(tool_error(
                "learning_unavailable",
                "learning messages are unavailable for this invocation kind",
                None,
            ));
        }
        Err(crate::progress::ProgressError::RateLimited { .. }) => {
            return Err(tool_error(
                "learning_send_failed",
                "internal error: learning target was rate limited",
                None,
            ));
        }
    };

    if !should_send_learning_message(kind, phase) {
        return Ok(());
    }

    let Some(message) = message.map(str::trim).filter(|m| !m.is_empty()) else {
        return Err(tool_error(
            "invalid_argument",
            "learning message must not be empty",
            None,
        ));
    };
    if message.chars().count() > crate::progress::PROGRESS_MESSAGE_MAX_CHARS {
        return Err(tool_error(
            "invalid_argument",
            format!(
                "learning message must be at most {} characters",
                crate::progress::PROGRESS_MESSAGE_MAX_CHARS
            ),
            None,
        ));
    }

    let client = InternalClient::new(target.bot_socket_path);
    let request = ProgressSendRequest {
        invocation_id: invocation_id.to_owned(),
        token: target.bot_send_token,
        message: message.to_owned(),
    };
    match tokio::time::timeout(LEARNING_SEND_TIMEOUT, client.progress_send(&request)).await {
        Ok(Ok(_)) => Ok(()),
        Ok(Err(e)) => Err(tool_error("learning_send_failed", format!("{e:#}"), None)),
        Err(_) => Err(tool_error(
            "learning_send_failed",
            format!(
                "learning message send timed out after {}s",
                LEARNING_SEND_TIMEOUT.as_secs()
            ),
            None,
        )),
    }
}

pub(crate) fn success_json(status: &str, skill_name: &str) -> String {
    serde_json::json!({
        "status": status,
        "skill_name": skill_name,
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skill_learning_finish_accepts_hint_outcome_field() {
        let json = r#"{"action":"create","status":"created","skill_name":"rightx-foo","hint_outcome":"applied_as_hinted"}"#;
        let params: SkillLearningFinishParams = serde_json::from_str(json).unwrap();
        assert_eq!(
            params.hint_outcome.map(LearningHintOutcomeParam::as_str),
            Some("applied_as_hinted")
        );
    }

    #[test]
    fn skill_learning_finish_accepts_missing_hint_outcome() {
        let json = r#"{"action":"create","status":"created","skill_name":"rightx-foo"}"#;
        let params: SkillLearningFinishParams = serde_json::from_str(json).unwrap();
        assert_eq!(params.hint_outcome, None);
    }

    #[test]
    fn skill_learning_finish_rejects_invalid_hint_outcome_field() {
        let json = r#"{"action":"create","status":"aborted","skill_name":"rightx-foo","hint_outcome":"bogus"}"#;
        let err = serde_json::from_str::<SkillLearningFinishParams>(json).unwrap_err();
        assert!(
            err.to_string().contains("unknown variant"),
            "unexpected error: {err}"
        );
    }
}
