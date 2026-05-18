#![allow(dead_code)]

use std::io::BufRead as _;
use std::path::{Path, PathBuf};

use right_mcp::LEARNED_SKILL_PREFIX;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReviewOutputStatus {
    NothingToLearn,
    CreateCandidate,
    UpdateCandidate,
    Failed,
}

impl ReviewOutputStatus {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "nothing_to_learn" => Some(Self::NothingToLearn),
            "create_candidate" => Some(Self::CreateCandidate),
            "update_candidate" => Some(Self::UpdateCandidate),
            "failed" => Some(Self::Failed),
            _ => None,
        }
    }

    pub(crate) fn as_domain(self) -> right_agent::learned_skills::ReviewStatus {
        match self {
            Self::NothingToLearn => right_agent::learned_skills::ReviewStatus::NothingToLearn,
            Self::CreateCandidate => right_agent::learned_skills::ReviewStatus::CreateCandidate,
            Self::UpdateCandidate => right_agent::learned_skills::ReviewStatus::UpdateCandidate,
            Self::Failed => right_agent::learned_skills::ReviewStatus::Failed,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReviewOutputConfidence {
    Low,
    Medium,
    High,
}

impl ReviewOutputConfidence {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "low" => Some(Self::Low),
            "medium" => Some(Self::Medium),
            "high" => Some(Self::High),
            _ => None,
        }
    }

    pub(crate) fn as_domain(self) -> right_agent::learned_skills::ReviewConfidence {
        match self {
            Self::Low => right_agent::learned_skills::ReviewConfidence::Low,
            Self::Medium => right_agent::learned_skills::ReviewConfidence::Medium,
            Self::High => right_agent::learned_skills::ReviewConfidence::High,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ReviewOutput {
    pub(crate) status: ReviewOutputStatus,
    pub(crate) confidence: ReviewOutputConfidence,
    pub(crate) candidate_skill_name: Option<String>,
    pub(crate) candidate_summary: Option<String>,
    pub(crate) evidence_refs: Vec<String>,
    pub(crate) user_notice: Option<String>,
    pub(crate) raw: serde_json::Value,
}

impl ReviewOutput {
    pub(crate) fn parse(raw: serde_json::Value) -> Result<Self, String> {
        let status = raw
            .get("status")
            .and_then(serde_json::Value::as_str)
            .and_then(ReviewOutputStatus::parse)
            .ok_or_else(|| "review output status is invalid".to_owned())?;
        let confidence = raw
            .get("confidence")
            .and_then(serde_json::Value::as_str)
            .and_then(ReviewOutputConfidence::parse)
            .ok_or_else(|| "review output confidence is invalid".to_owned())?;
        let candidate_skill_name = optional_trimmed_string(&raw, "candidate_skill_name");
        if let Some(name) = &candidate_skill_name
            && !name.starts_with(LEARNED_SKILL_PREFIX)
        {
            return Err(format!(
                "candidate_skill_name must start with {LEARNED_SKILL_PREFIX}"
            ));
        }
        let candidate_summary = optional_trimmed_string(&raw, "candidate_summary");
        let user_notice = optional_trimmed_string(&raw, "user_notice");
        let evidence_refs = raw
            .get("evidence_refs")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| "review output evidence_refs must be an array".to_owned())?
            .iter()
            .map(parse_evidence_ref)
            .collect::<Result<Vec<_>, _>>()?;

        if matches!(
            status,
            ReviewOutputStatus::CreateCandidate | ReviewOutputStatus::UpdateCandidate
        ) {
            if candidate_skill_name.is_none() {
                return Err("candidate review output requires candidate_skill_name".to_owned());
            }
            if evidence_refs.is_empty() {
                return Err("candidate review output requires evidence_refs".to_owned());
            }
        }

        Ok(Self {
            status,
            confidence,
            candidate_skill_name,
            candidate_summary,
            evidence_refs,
            user_notice,
            raw,
        })
    }

    pub(crate) fn should_notify_user(&self) -> bool {
        matches!(
            self.status,
            ReviewOutputStatus::CreateCandidate | ReviewOutputStatus::UpdateCandidate
        ) && self.confidence == ReviewOutputConfidence::High
            && self.user_notice.is_some()
    }
}

fn optional_trimmed_string(raw: &serde_json::Value, field: &str) -> Option<String> {
    raw.get(field)
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn parse_evidence_ref(value: &serde_json::Value) -> Result<String, String> {
    value
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| "review output evidence_refs must contain non-empty strings".to_owned())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LearnedSkillSummary {
    pub(crate) name: String,
    pub(crate) excerpt: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReviewBundle {
    pub(crate) agent_name: String,
    pub(crate) source_invocation_id: String,
    pub(crate) root_session_id: Option<String>,
    pub(crate) trigger_kind: String,
    pub(crate) accepted_signal_json: Option<String>,
    pub(crate) tool_iters_since_review: i64,
    pub(crate) turns_since_review: i64,
    pub(crate) skill_issue_hints_since_review: i64,
    pub(crate) event_timeline: Vec<String>,
    pub(crate) learning_events: Vec<String>,
    pub(crate) learned_skills: Vec<LearnedSkillSummary>,
}

const REVIEW_PROMPT_ACCEPTED_SIGNAL_MAX_CHARS: usize = 2_048;
const REVIEW_PROMPT_EVENT_TIMELINE_MAX_ITEMS: usize = 24;
const REVIEW_PROMPT_EVENT_TIMELINE_ITEM_MAX_CHARS: usize = 200;
const REVIEW_PROMPT_LEARNING_EVENTS_MAX_ITEMS: usize = 24;
const REVIEW_PROMPT_LEARNING_EVENT_ITEM_MAX_CHARS: usize = 200;
const REVIEW_PROMPT_LEARNED_SKILLS_MAX_ITEMS: usize = 16;
const REVIEW_PROMPT_SKILL_NAME_MAX_CHARS: usize = 96;
const REVIEW_PROMPT_SKILL_EXCERPT_MAX_CHARS: usize = 240;

pub(crate) fn build_review_prompt(bundle: &ReviewBundle) -> String {
    let mut prompt = String::new();
    prompt.push_str("# Background Learned-Skill Review\n\n");
    prompt.push_str(
        "Report-only review. Do not write files. Do not call learning tools. \
         Do not ask the user questions. nothing_to_learn is normal when evidence is weak.\n\n",
    );
    prompt.push_str(&format!("agent_name: {}\n", bundle.agent_name));
    prompt.push_str(&format!(
        "source_invocation_id: {}\n",
        bundle.source_invocation_id
    ));
    if let Some(root_session_id) = &bundle.root_session_id {
        prompt.push_str(&format!("root_session_id: {root_session_id}\n"));
    }
    prompt.push_str(&format!("trigger_kind: {}\n", bundle.trigger_kind));
    prompt.push_str(&format!(
        "tool_iters_since_review: {}\n",
        bundle.tool_iters_since_review
    ));
    prompt.push_str(&format!(
        "turns_since_review: {}\n",
        bundle.turns_since_review
    ));
    prompt.push_str(&format!(
        "skill_issue_hints_since_review: {}\n\n",
        bundle.skill_issue_hints_since_review
    ));

    if let Some(signal) = &bundle.accepted_signal_json {
        prompt.push_str("accepted_signal_json:\n");
        prompt.push_str(&bounded_prompt_block(
            signal,
            REVIEW_PROMPT_ACCEPTED_SIGNAL_MAX_CHARS,
        ));
        prompt.push_str("\n\n");
    }

    push_bounded_list(
        &mut prompt,
        "event_timeline",
        &bundle.event_timeline,
        REVIEW_PROMPT_EVENT_TIMELINE_MAX_ITEMS,
        REVIEW_PROMPT_EVENT_TIMELINE_ITEM_MAX_CHARS,
    );
    push_bounded_list(
        &mut prompt,
        "learning_events",
        &bundle.learning_events,
        REVIEW_PROMPT_LEARNING_EVENTS_MAX_ITEMS,
        REVIEW_PROMPT_LEARNING_EVENT_ITEM_MAX_CHARS,
    );
    prompt.push_str("\nrightx_skill_index:\n");
    for skill in bundle
        .learned_skills
        .iter()
        .take(REVIEW_PROMPT_LEARNED_SKILLS_MAX_ITEMS)
    {
        prompt.push_str("- ");
        prompt.push_str(&bounded_prompt_line(
            &skill.name,
            REVIEW_PROMPT_SKILL_NAME_MAX_CHARS,
        ));
        prompt.push_str(": ");
        prompt.push_str(&bounded_prompt_line(
            &skill.excerpt,
            REVIEW_PROMPT_SKILL_EXCERPT_MAX_CHARS,
        ));
        prompt.push('\n');
    }
    push_omitted_count(
        &mut prompt,
        bundle.learned_skills.len(),
        REVIEW_PROMPT_LEARNED_SKILLS_MAX_ITEMS,
    );
    prompt.push_str(
        "\nReturn JSON with status, confidence, candidate_skill_name, candidate_summary, \
         evidence_refs, and user_notice. Use only rightx-* candidate skill names.\n",
    );
    prompt
}

fn push_bounded_list(
    prompt: &mut String,
    heading: &str,
    items: &[String],
    max_items: usize,
    max_item_chars: usize,
) {
    prompt.push_str(heading);
    prompt.push_str(":\n");
    for item in items.iter().take(max_items) {
        prompt.push_str("- ");
        prompt.push_str(&bounded_prompt_line(item, max_item_chars));
        prompt.push('\n');
    }
    push_omitted_count(prompt, items.len(), max_items);
    prompt.push('\n');
}

fn push_omitted_count(prompt: &mut String, item_count: usize, max_items: usize) {
    if item_count > max_items {
        prompt.push_str("- ... ");
        prompt.push_str(&(item_count - max_items).to_string());
        prompt.push_str(" additional items omitted\n");
    }
}

fn bounded_prompt_line(value: &str, max_chars: usize) -> String {
    bounded_prompt_text(&value.replace(['\r', '\n'], " "), max_chars)
}

fn bounded_prompt_block(value: &str, max_chars: usize) -> String {
    bounded_prompt_text(value, max_chars)
}

fn bounded_prompt_text(value: &str, max_chars: usize) -> String {
    let mut chars = value.chars();
    let mut out: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        out.push_str("... [truncated]");
    }
    out
}

pub(crate) fn review_stream_log_path(agent_dir: &Path, root_session_id: &str) -> PathBuf {
    agent_dir
        .parent()
        .and_then(Path::parent)
        .unwrap_or(agent_dir)
        .join("logs")
        .join("streams")
        .join(format!("{root_session_id}.ndjson"))
}

pub(crate) fn collect_stream_event_timeline(
    agent_dir: &Path,
    root_session_id: &str,
    max_events: usize,
) -> std::io::Result<Vec<String>> {
    let path = review_stream_log_path(agent_dir, root_session_id);
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err),
    };
    let reader = std::io::BufReader::new(file);
    let mut timeline = Vec::new();

    for line in reader.lines() {
        if timeline.len() >= max_events {
            break;
        }
        let line = line?;
        let Some(summary) = summarize_stream_event(&line) else {
            continue;
        };
        timeline.push(format!("event-{} {summary}", timeline.len() + 1));
    }

    Ok(timeline)
}

fn summarize_stream_event(line: &str) -> Option<String> {
    match crate::cc::stream::parse_stream_event(line) {
        crate::cc::stream::StreamEvent::Text(text) => {
            Some(format!("assistant_text: {}", bounded_event_text(&text)))
        }
        crate::cc::stream::StreamEvent::ToolUse {
            tool,
            input_summary,
        } => Some(format!(
            "tool_use {tool}: {}",
            bounded_event_text(&input_summary)
        )),
        crate::cc::stream::StreamEvent::Result(_) => {
            Some("result: foreground invocation completed".to_owned())
        }
        crate::cc::stream::StreamEvent::Thinking | crate::cc::stream::StreamEvent::Other => None,
    }
}

fn bounded_event_text(value: &str) -> String {
    const MAX_CHARS: usize = 280;
    let mut out = value.chars().take(MAX_CHARS).collect::<String>();
    if value.chars().count() > MAX_CHARS {
        out.push_str("...");
    }
    out
}

#[cfg(test)]
#[path = "learning_review_tests.rs"]
mod tests;
