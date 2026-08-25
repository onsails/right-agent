use std::{collections::BTreeMap, path::PathBuf, time::Duration};

use crate::sandbox::{Sandbox, exec_argv};
use right_dashboard::api_types::{
    PinSkillResponse, SkillDetailResponse, SkillGroups, SkillSummary, SkillsResponse,
};
use right_dashboard::skill_inventory::{
    SkillInventoryError, classify_skill_group, parse_skill_description, sort_skill_groups,
    validate_skill_name,
};

use super::DashboardState;

const SKILL_PREVIEW_LIMIT_BYTES: usize = 64 * 1024;
const SKILL_DESCRIPTION_PREVIEW_LIMIT_BYTES: usize = 8 * 1024;
const SANDBOX_SKILL_LIMIT_STR: &str = "200";
const SANDBOX_SKILLS_PATH: &str = "/sandbox/.claude/skills";
/// Detail reported by every skills route when the sandbox handle is absent.
/// Carries the same `sandbox_unreachable` label the identity routes use: an
/// unreachable sandbox is an error, never a host mirror served as truth (the
/// learned `rightx-*` packages exist only in the guest).
const SANDBOX_UNREACHABLE_DETAIL: &str = "sandbox_unreachable: no sandbox handle";
// These two scripts are kept single-line: each is passed as one
// `sh -c <script>` argv entry, and the `;`-joined form keeps the guest
// command line readable in exec traces. Rust's `\`-line-continuation strips
// the trailing newline AND the next line's leading whitespace, so the source
// indentation is cosmetic — the space before each `\` is the statement
// separator. `printf '%s\\n'` is a literal backslash+n for printf, not 0x0A.
const SANDBOX_LIST_SKILLS_SCRIPT: &str = "cd \"$1\" 2>/dev/null || exit 0; \
     limit=\"$2\"; \
     count=0; \
     for d in *; do \
     [ -d \"$d\" ] || continue; \
     [ -L \"$d/SKILL.md\" ] && continue; \
     [ -f \"$d/SKILL.md\" ] || continue; \
     printf '%s\\n' \"$d\"; \
     count=$((count + 1)); \
     [ \"$count\" -ge \"$limit\" ] && break; \
     done";
const SANDBOX_READ_SKILL_SCRIPT: &str = "cd \"$1\" 2>/dev/null || exit 3; \
     file=\"$2/SKILL.md\"; \
     [ -e \"$file\" ] || exit 3; \
     [ -L \"$file\" ] && exit 3; \
     [ -f \"$file\" ] || exit 3; \
     head -c \"$3\" \"$file\"";

#[derive(Debug, thiserror::Error)]
pub(super) enum SkillsResponseError {
    #[error(transparent)]
    Inventory(#[from] SkillInventoryError),
    #[error("sandbox skill scan failed: {0}")]
    Sandbox(String),
}

#[derive(Debug, thiserror::Error)]
pub(super) enum SkillDetailError {
    #[error(transparent)]
    Inventory(#[from] SkillInventoryError),
    #[error("sandbox skill detail failed: {0}")]
    Sandbox(String),
}

#[derive(Debug, thiserror::Error)]
pub(super) enum PinSkillError {
    #[error(transparent)]
    Inventory(#[from] SkillInventoryError),
    #[error("skill name is not a rightx learned skill")]
    NonRightx,
    #[error("skill lifecycle row is missing")]
    LifecycleMissing,
    #[error("skill is not curator-managed")]
    NotCuratorManaged,
    #[error("database open failed: {0}")]
    DbOpen(#[from] right_db::DbError),
    #[error("lifecycle operation failed: {0}")]
    Lifecycle(#[from] right_lifecycle::LifecycleError),
    #[error("pinning task panicked: {0}")]
    Join(#[from] tokio::task::JoinError),
    #[error("sandbox skill probe failed: {0}")]
    Sandbox(String),
}

#[derive(Debug, thiserror::Error)]
pub(super) enum SkillLifecycleReadError {
    #[error("database open failed: {0}")]
    DbOpen(#[from] right_db::DbError),
    #[error("lifecycle read failed: {0}")]
    Lifecycle(#[from] right_lifecycle::LifecycleError),
    #[error("lifecycle read task panicked: {0}")]
    Join(#[from] tokio::task::JoinError),
    #[error("read model error: {0}")]
    ReadModel(#[from] right_dashboard::read_model::ReadModelError),
}

pub(super) async fn skills_response(
    state: &DashboardState,
) -> Result<SkillsResponse, SkillsResponseError> {
    // Learned (`rightx-*`) skills live only inside the sandbox at
    // `/sandbox/.claude/skills`. Falling back to the host filesystem — on a
    // scan failure or on a missing handle, which now means the sandbox never
    // came up rather than "unsandboxed agent" — would show "0 learned": a
    // wrong answer dressed as a valid one. Propagate instead, so the dashboard
    // renders an error and keeps any previously loaded skills.
    let sandbox = state
        .sandbox()
        .ok_or_else(|| SkillsResponseError::Sandbox(SANDBOX_UNREACHABLE_DETAIL.to_owned()))?;
    let mut response = scan_sandbox_skills(&state.agent_name, &sandbox)
        .await
        .map_err(|error| SkillsResponseError::Sandbox(format!("{error:#}")))?;
    try_enrich_skills_response(state, &mut response).await;
    Ok(response)
}

pub(super) async fn skill_detail_response(
    state: &DashboardState,
    skill_name: &str,
) -> Result<SkillDetailResponse, SkillDetailError> {
    validate_skill_name(skill_name)?;
    // Same rule as the list route: a learned skill's SKILL.md exists only in
    // the guest, so no handle means unreachable, not "read the host copy".
    let sandbox = state
        .sandbox()
        .ok_or_else(|| SkillDetailError::Sandbox(SANDBOX_UNREACHABLE_DETAIL.to_owned()))?;
    let mut response = read_sandbox_skill_detail(&state.agent_name, &sandbox, skill_name).await?;
    try_enrich_skill_summary(state, &mut response.skill).await;
    Ok(response)
}

pub(super) async fn pin_skill_response(
    state: &DashboardState,
    skill_name: &str,
    pinned: bool,
) -> Result<PinSkillResponse, PinSkillError> {
    validate_skill_name(skill_name)?;
    if !skill_name.starts_with("rightx-") {
        return Err(PinSkillError::NonRightx);
    }
    // Cheap local checks first, so a request that is wrong on its own terms
    // (unknown or non-curator-managed row) still answers precisely while the
    // sandbox is down.
    let skill_name = skill_name.to_owned();
    let row = state
        .internal_client
        .skill_lifecycle_get(&right_mcp::internal_db::SkillLifecycleGetRequest {
            agent: state.agent_name.clone(),
            skill_name: skill_name.clone(),
        })
        .await
        .map_err(|error| {
            PinSkillError::DbOpen(right_db::DbError::InvalidParameter(format!("{error:#}")))
        })?
        .row
        .ok_or(PinSkillError::LifecycleMissing)?;
    if !matches!(row.created_by.as_str(), "probe_writer" | "curator") {
        return Err(PinSkillError::NotCuratorManaged);
    }
    // Guard against pinning an orphan lifecycle row whose SKILL.md was deleted
    // out-of-band — surface as 404, not a silent pin. The learned-skill package
    // lives at /sandbox/.claude/skills/<name>/SKILL.md, so the probe runs in
    // the guest; with no sandbox handle there is nothing to verify against, and
    // the pin MUST NOT be written on the strength of a host read.
    let sandbox = state
        .sandbox()
        .ok_or_else(|| PinSkillError::Sandbox(SANDBOX_UNREACHABLE_DETAIL.to_owned()))?;
    probe_sandbox_skill_package(&sandbox, &skill_name).await?;

    state
        .internal_client
        .skill_pin(&right_mcp::internal_db::SkillPinRequest {
            agent: state.agent_name.clone(),
            skill_name: skill_name.clone(),
            pinned,
        })
        .await
        .map_err(|error| {
            PinSkillError::DbOpen(right_db::DbError::InvalidParameter(format!("{error:#}")))
        })?;
    Ok(PinSkillResponse { skill_name, pinned })
}

/// Cheap existence probe for a learned-skill package inside a sandbox.
/// Mirrors `read_sandbox_skill_detail`'s shell-script pattern: positional
/// arg passes the validated `skill_name` to `test -f` without string
/// interpolation into the script body. Non-zero exit maps to
/// `SkillInventoryError::NotFound` so the existing
/// `PinSkillError::Inventory` → 404 mapping continues to work for the
/// "SKILL.md was deleted out-of-band" case the host path also covers.
/// Timeout / gRPC failures propagate as `PinSkillError::Sandbox` (500).
async fn probe_sandbox_skill_package(
    sandbox: &Sandbox,
    skill_name: &str,
) -> Result<(), PinSkillError> {
    let skill_path = format!("{SANDBOX_SKILLS_PATH}/{skill_name}/SKILL.md");
    let probe_command = [
        "sh",
        "-c",
        r#"test -f "$1""#,
        "dashboard-skill-pin",
        skill_path.as_str(),
    ];
    // Interactive single-skill ops (pin/detail) only run after the list scan
    // already succeeded, so the sandbox is warm — use the short probe budget,
    // not the cold-start list-scan budget, so a transient failure fails fast.
    let timeout = Duration::from_secs(super::DASHBOARD_SANDBOX_TIMEOUT_SECS);
    let (_, exit_code) =
        match tokio::time::timeout(timeout, exec_argv(sandbox, &probe_command)).await {
            Ok(result) => result.map_err(|error| {
                PinSkillError::Sandbox(format!("sandbox skill probe failed: {error:#}"))
            })?,
            Err(_) => {
                return Err(PinSkillError::Sandbox(format!(
                    "sandbox skill probe timed out after {}s",
                    super::DASHBOARD_SANDBOX_TIMEOUT_SECS
                )));
            }
        };
    if exit_code != 0 {
        return Err(PinSkillError::Inventory(SkillInventoryError::NotFound(
            skill_name.to_owned(),
        )));
    }
    Ok(())
}

async fn scan_sandbox_skills(agent: &str, sandbox: &Sandbox) -> miette::Result<SkillsResponse> {
    let list_command = [
        "sh",
        "-c",
        SANDBOX_LIST_SKILLS_SCRIPT,
        "dashboard-skill-list",
        SANDBOX_SKILLS_PATH,
        SANDBOX_SKILL_LIMIT_STR,
    ];
    let timeout = Duration::from_secs(super::DASHBOARD_SANDBOX_SKILLS_TIMEOUT_SECS);
    let (stdout, exit_code) =
        match tokio::time::timeout(timeout, exec_argv(sandbox, &list_command)).await {
            Ok(result) => result?,
            Err(_) => {
                return Err(miette::miette!("sandbox skill list timed out"));
            }
        };
    if exit_code != 0 {
        return Err(miette::miette!(
            "sandbox skill list exited with code {exit_code}"
        ));
    }

    let mut groups = SkillGroups::default();
    for skill_name in stdout.lines() {
        if validate_skill_name(skill_name).is_err() {
            continue;
        }
        let limit = (SKILL_DESCRIPTION_PREVIEW_LIMIT_BYTES + 1).to_string();
        let read_command = [
            "sh",
            "-c",
            SANDBOX_READ_SKILL_SCRIPT,
            "dashboard-skill-read",
            SANDBOX_SKILLS_PATH,
            skill_name,
            limit.as_str(),
        ];
        let (preview, exit_code) =
            match tokio::time::timeout(timeout, exec_argv(sandbox, &read_command)).await {
                Ok(result) => result?,
                Err(_) => continue,
            };
        if exit_code != 0 {
            continue;
        }
        push_skill_summary(&mut groups, skill_name, "sandbox", &preview);
    }
    sort_skill_groups(&mut groups);

    Ok(SkillsResponse {
        agent: agent.to_owned(),
        source: "sandbox".to_owned(),
        warning: None,
        groups,
    })
}

async fn read_sandbox_skill_detail(
    agent: &str,
    sandbox: &Sandbox,
    skill_name: &str,
) -> Result<SkillDetailResponse, SkillDetailError> {
    validate_skill_name(skill_name)?;
    let limit = (SKILL_PREVIEW_LIMIT_BYTES + 1).to_string();
    let read_command = [
        "sh",
        "-c",
        SANDBOX_READ_SKILL_SCRIPT,
        "dashboard-skill-read",
        SANDBOX_SKILLS_PATH,
        skill_name,
        limit.as_str(),
    ];
    // Warm-path read (see probe_sandbox_skill_package): the user reaches a
    // skill detail only after the list scan loaded, so use the short budget.
    let timeout = Duration::from_secs(super::DASHBOARD_SANDBOX_TIMEOUT_SECS);
    let (mut content_preview, exit_code) =
        match tokio::time::timeout(timeout, exec_argv(sandbox, &read_command)).await {
            Ok(result) => result.map_err(sandbox_error)?,
            Err(_) => {
                return Err(SkillDetailError::Sandbox(format!(
                    "sandbox skill read timed out after {}s",
                    super::DASHBOARD_SANDBOX_TIMEOUT_SECS
                )));
            }
        };
    if exit_code == 3 {
        return Err(SkillInventoryError::NotFound(skill_name.to_owned()).into());
    }
    if exit_code != 0 {
        return Err(SkillDetailError::Sandbox(format!(
            "sandbox skill read exited with code {exit_code}"
        )));
    }
    let truncated = content_preview.len() > SKILL_PREVIEW_LIMIT_BYTES;
    if truncated {
        right_dashboard::fs_safety::truncate_to_char_boundary(
            &mut content_preview,
            SKILL_PREVIEW_LIMIT_BYTES,
        );
    }
    let group = classify_skill_group(skill_name, right_codegen::BUILTIN_SKILL_NAMES).to_owned();
    let skill = SkillSummary::new(
        skill_name.to_owned(),
        group,
        format!("{SANDBOX_SKILLS_PATH}/{skill_name}/SKILL.md"),
        parse_skill_description(&content_preview),
    );

    Ok(SkillDetailResponse {
        agent: agent.to_owned(),
        skill,
        content_preview,
        truncated,
    })
}

fn sandbox_error(error: miette::Report) -> SkillDetailError {
    SkillDetailError::Sandbox(format!("{error:#}"))
}

fn push_skill_summary(
    groups: &mut SkillGroups,
    skill_name: &str,
    source_root: &str,
    preview: &str,
) {
    let group = classify_skill_group(skill_name, right_codegen::BUILTIN_SKILL_NAMES);
    let summary = SkillSummary::new(
        skill_name.to_owned(),
        group.to_owned(),
        format!("{source_root}/.claude/skills/{skill_name}/SKILL.md"),
        parse_skill_description(preview),
    );
    match group {
        "core" => groups.core.push(summary),
        "learned" => groups.learned.push(summary),
        _ => groups.other.push(summary),
    }
}

async fn try_enrich_skills_response(state: &DashboardState, response: &mut SkillsResponse) {
    let lifecycle = state
        .internal_client
        .skill_lifecycle_list(&right_mcp::internal_db::SkillLifecycleListRequest {
            agent: state.agent_name.clone(),
        })
        .await;
    if let Ok(rows) = lifecycle {
        let rows: BTreeMap<_, _> = rows
            .rows
            .into_iter()
            .map(|row| (row.skill_name.clone(), row))
            .collect();
        enrich_group(&mut response.groups.core, &rows);
        enrich_group(&mut response.groups.learned, &rows);
        enrich_group(&mut response.groups.other, &rows);
    }
    if let Ok(spend) = state
        .internal_client
        .skill_spend_by_skill(&right_mcp::internal_db::SkillSpendBySkillRequest {
            agent: state.agent_name.clone(),
        })
        .await
    {
        enrich_group_spend(&mut response.groups.core, &spend.rows);
        enrich_group_spend(&mut response.groups.learned, &spend.rows);
        enrich_group_spend(&mut response.groups.other, &spend.rows);
    }
}

async fn try_enrich_skill_summary(state: &DashboardState, skill: &mut SkillSummary) {
    if let Ok(response) = state
        .internal_client
        .skill_lifecycle_get(&right_mcp::internal_db::SkillLifecycleGetRequest {
            agent: state.agent_name.clone(),
            skill_name: skill.name.clone(),
        })
        .await
        && let Some(row) = response.row
    {
        apply_lifecycle_dto(skill, &row);
    }
    if let Ok(spend) = state
        .internal_client
        .skill_spend_by_skill(&right_mcp::internal_db::SkillSpendBySkillRequest {
            agent: state.agent_name.clone(),
        })
        .await
        && let Some(agg) = spend.rows.get(&skill.name)
    {
        skill.apply_spend(agg);
    }
}

fn apply_lifecycle_dto(skill: &mut SkillSummary, row: &right_mcp::internal_db::SkillLifecycleDto) {
    skill.state = serde_json::from_value(serde_json::Value::String(row.state.clone())).ok();
    skill.pinned = row.pinned;
    skill.created_by =
        serde_json::from_value(serde_json::Value::String(row.created_by.clone())).ok();
    skill.use_count = i64::from(row.use_count);
    skill.patch_count = i64::from(row.patch_count);
    skill.created_at = row.created_at.clone();
    skill.last_used_at = row.last_used_at.clone();
    skill.last_patched_at = row.last_patched_at.clone();
}

fn enrich_group(
    skills: &mut [SkillSummary],
    lifecycle_rows: &BTreeMap<String, right_mcp::internal_db::SkillLifecycleDto>,
) {
    for skill in skills {
        if let Some(row) = lifecycle_rows.get(&skill.name) {
            apply_lifecycle_dto(skill, row);
        }
    }
}

fn enrich_group_spend(
    skills: &mut [SkillSummary],
    spend: &std::collections::HashMap<String, right_dashboard::api_types::SkillSpendAgg>,
) {
    for skill in skills {
        if let Some(agg) = spend.get(&skill.name) {
            skill.apply_spend(agg);
        }
    }
}

// The former `script_constants_tests` module asserted these scripts carried no
// real newline byte, because OpenShell's gRPC `ExecSandbox` rejected such
// arguments. Microsandbox passes argv through verbatim, so that constraint no
// longer exists and the guard asserted nothing about behavior.
