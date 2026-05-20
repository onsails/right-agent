use std::time::Duration;

use right_dashboard::api_types::{SkillDetailResponse, SkillGroups, SkillSummary, SkillsResponse};
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

pub(super) async fn skills_response(
    state: &DashboardState,
) -> Result<SkillsResponse, SkillInventoryError> {
    if let Some(sandbox_exec) = state.sandbox_exec.as_ref() {
        match scan_sandbox_skills(&state.agent_name, sandbox_exec).await {
            Ok(response) => return Ok(response),
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
                return Ok(response);
            }
        }
    }

    scan_host_skills(
        &state.agent_name,
        &state.agent_dir,
        right_codegen::BUILTIN_SKILL_NAMES,
        "host",
        SKILL_PREVIEW_LIMIT_BYTES,
    )
}

pub(super) async fn skill_detail_response(
    state: &DashboardState,
    skill_name: &str,
) -> Result<SkillDetailResponse, SkillDetailError> {
    validate_skill_name(skill_name)?;
    if let Some(sandbox_exec) = state.sandbox_exec.as_ref() {
        return read_sandbox_skill_detail(&state.agent_name, sandbox_exec, skill_name).await;
    }

    Ok(read_host_skill_detail(
        &state.agent_name,
        &state.agent_dir,
        skill_name,
        right_codegen::BUILTIN_SKILL_NAMES,
        SKILL_PREVIEW_LIMIT_BYTES,
    )?)
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
    let skill = SkillSummary {
        name: skill_name.to_owned(),
        group,
        path: format!("{SANDBOX_SKILLS_PATH}/{skill_name}/SKILL.md"),
        description: parse_skill_description(&content_preview),
    };

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
    let summary = SkillSummary {
        name: skill_name.to_owned(),
        group: group.to_owned(),
        path: format!("{source_root}/.claude/skills/{skill_name}/SKILL.md"),
        description: parse_skill_description(preview),
    };
    match group {
        "core" => groups.core.push(summary),
        "learned" => groups.learned.push(summary),
        _ => groups.other.push(summary),
    }
}
