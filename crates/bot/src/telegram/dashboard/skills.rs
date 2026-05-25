use std::{collections::BTreeMap, path::PathBuf, time::Duration};

use right_dashboard::api_types::{
    PinSkillResponse, SkillDetailResponse, SkillGroups, SkillSummary, SkillsResponse,
};
use right_dashboard::skill_inventory::{
    SkillInventoryError, classify_skill_group, parse_skill_description, read_host_skill_detail,
    scan_host_skills, sort_skill_groups, validate_skill_name,
};
use right_openshell::sandbox_exec::SandboxExec;

use super::DashboardState;

const SKILL_PREVIEW_LIMIT_BYTES: usize = 64 * 1024;
const SKILL_DESCRIPTION_PREVIEW_LIMIT_BYTES: usize = 8 * 1024;
const SANDBOX_SKILL_LIMIT_STR: &str = "200";
const SANDBOX_SKILLS_PATH: &str = "/sandbox/.claude/skills";
const SANDBOX_LIST_SKILLS_SCRIPT: &str = r#"cd "$1" 2>/dev/null || exit 0
limit="$2"
count=0
for d in *; do
  [ -d "$d" ] || continue
  [ -L "$d/SKILL.md" ] && continue
  [ -f "$d/SKILL.md" ] || continue
  printf '%s\n' "$d"
  count=$((count + 1))
  [ "$count" -ge "$limit" ] && break
done"#;
const SANDBOX_READ_SKILL_SCRIPT: &str = r#"cd "$1" 2>/dev/null || exit 3
file="$2/SKILL.md"
[ -e "$file" ] || exit 3
[ -L "$file" ] && exit 3
[ -f "$file" ] || exit 3
head -c "$3" "$file""#;

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
}

pub(super) async fn skills_response(
    state: &DashboardState,
) -> Result<SkillsResponse, SkillInventoryError> {
    if let Some(sandbox_exec) = state.sandbox_exec.as_ref() {
        match scan_sandbox_skills(&state.agent_name, sandbox_exec).await {
            Ok(mut response) => {
                try_enrich_skills_response(state, &mut response).await;
                return Ok(response);
            }
            Err(error) => {
                let mut response = scan_host_skills(
                    &state.agent_name,
                    &state.agent_dir,
                    right_codegen::BUILTIN_SKILL_NAMES,
                    "host",
                    SKILL_PREVIEW_LIMIT_BYTES,
                )?;
                response.warning = Some(format!(
                    "sandbox skill scan failed; showing host skills: {error:#}"
                ));
                try_enrich_skills_response(state, &mut response).await;
                return Ok(response);
            }
        }
    }

    let mut response = scan_host_skills(
        &state.agent_name,
        &state.agent_dir,
        right_codegen::BUILTIN_SKILL_NAMES,
        "host",
        SKILL_PREVIEW_LIMIT_BYTES,
    )?;
    try_enrich_skills_response(state, &mut response).await;
    Ok(response)
}

pub(super) async fn skill_detail_response(
    state: &DashboardState,
    skill_name: &str,
) -> Result<SkillDetailResponse, SkillDetailError> {
    validate_skill_name(skill_name)?;
    if let Some(sandbox_exec) = state.sandbox_exec.as_ref() {
        let mut response =
            read_sandbox_skill_detail(&state.agent_name, sandbox_exec, skill_name).await?;
        try_enrich_skill_summary(state, &mut response.skill).await;
        return Ok(response);
    }

    let mut response = read_host_skill_detail(
        &state.agent_name,
        &state.agent_dir,
        skill_name,
        right_codegen::BUILTIN_SKILL_NAMES,
        SKILL_PREVIEW_LIMIT_BYTES,
    )?;
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
    // Guard against pinning an orphan lifecycle row whose SKILL.md was
    // deleted out-of-band — surface as 404, not a silent pin. For
    // sandboxed agents the learned-skill package lives in
    // /sandbox/.claude/skills/<name>/SKILL.md, not under agent_dir, so
    // probe the sandbox there instead of the host filesystem.
    if let Some(sandbox_exec) = state.sandbox_exec.as_ref() {
        probe_sandbox_skill_package(sandbox_exec, skill_name).await?;
    } else {
        read_host_skill_detail(
            &state.agent_name,
            &state.agent_dir,
            skill_name,
            right_codegen::BUILTIN_SKILL_NAMES,
            SKILL_PREVIEW_LIMIT_BYTES,
        )?;
    }

    let agent_dir = state.agent_dir.clone();
    let skill_name = skill_name.to_owned();
    tokio::task::spawn_blocking(move || {
        let conn = right_db::open_connection(&agent_dir, false)?;
        let row =
            right_lifecycle::get(&conn, &skill_name)?.ok_or(PinSkillError::LifecycleMissing)?;
        if !matches!(
            row.created_by,
            right_lifecycle::CreatedBy::ProbeWriter | right_lifecycle::CreatedBy::Curator
        ) {
            return Err(PinSkillError::NotCuratorManaged);
        }
        right_lifecycle::set_pinned(&conn, &skill_name, pinned)?;
        Ok(PinSkillResponse { skill_name, pinned })
    })
    .await?
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
    sandbox_exec: &SandboxExec,
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
    let timeout = Duration::from_secs(super::DASHBOARD_SANDBOX_TIMEOUT_SECS);
    let (_, exit_code) =
        match tokio::time::timeout(timeout, sandbox_exec.exec(&probe_command)).await {
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

async fn scan_sandbox_skills(
    agent: &str,
    sandbox_exec: &SandboxExec,
) -> miette::Result<SkillsResponse> {
    let list_command = [
        "sh",
        "-c",
        SANDBOX_LIST_SKILLS_SCRIPT,
        "dashboard-skill-list",
        SANDBOX_SKILLS_PATH,
        SANDBOX_SKILL_LIMIT_STR,
    ];
    let timeout = Duration::from_secs(super::DASHBOARD_SANDBOX_TIMEOUT_SECS);
    let (stdout, exit_code) =
        match tokio::time::timeout(timeout, sandbox_exec.exec(&list_command)).await {
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
            match tokio::time::timeout(timeout, sandbox_exec.exec(&read_command)).await {
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
    sandbox_exec: &SandboxExec,
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
    let timeout = Duration::from_secs(super::DASHBOARD_SANDBOX_TIMEOUT_SECS);
    let (mut content_preview, exit_code) =
        match tokio::time::timeout(timeout, sandbox_exec.exec(&read_command)).await {
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
    match lifecycle_rows_by_name(state.agent_dir.clone()).await {
        Ok(lifecycle_rows) => {
            enrich_group(&mut response.groups.core, &lifecycle_rows);
            enrich_group(&mut response.groups.learned, &lifecycle_rows);
            enrich_group(&mut response.groups.other, &lifecycle_rows);
        }
        Err(error) => {
            tracing::warn!(
                agent = %state.agent_name,
                "dashboard skill lifecycle enrichment skipped: {error:#}",
            );
        }
    }
}

async fn try_enrich_skill_summary(state: &DashboardState, skill: &mut SkillSummary) {
    match lifecycle_rows_by_name(state.agent_dir.clone()).await {
        Ok(lifecycle_rows) => {
            if let Some(row) = lifecycle_rows.get(&skill.name) {
                skill.apply_lifecycle(row);
            }
        }
        Err(error) => {
            tracing::warn!(
                agent = %state.agent_name,
                skill = %skill.name,
                "dashboard skill lifecycle enrichment skipped: {error:#}",
            );
        }
    }
}

fn enrich_group(
    skills: &mut [SkillSummary],
    lifecycle_rows: &BTreeMap<String, right_lifecycle::SkillLifecycleRow>,
) {
    for skill in skills {
        if let Some(row) = lifecycle_rows.get(&skill.name) {
            skill.apply_lifecycle(row);
        }
    }
}

async fn lifecycle_rows_by_name(
    agent_dir: PathBuf,
) -> Result<BTreeMap<String, right_lifecycle::SkillLifecycleRow>, SkillLifecycleReadError> {
    tokio::task::spawn_blocking(move || {
        let conn = right_db::open_connection_readonly(agent_dir)?;
        let rows = right_lifecycle::list(&conn)?;
        Ok(rows
            .into_iter()
            .map(|row| (row.skill_name.clone(), row))
            .collect())
    })
    .await?
}
