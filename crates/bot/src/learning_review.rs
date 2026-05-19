use std::io::{BufRead as _, Read as _};
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

pub(crate) const REVIEW_SCHEMA_JSON: &str = r#"{
  "type": "object",
  "properties": {
    "status": { "enum": ["nothing_to_learn", "create_candidate", "update_candidate", "failed"] },
    "confidence": { "enum": ["low", "medium", "high"] },
    "candidate_skill_name": { "type": ["string", "null"] },
    "candidate_summary": { "type": ["string", "null"] },
    "evidence_refs": { "type": "array", "items": { "type": "string" } },
    "user_notice": { "type": ["string", "null"] }
  },
  "required": ["status", "confidence", "candidate_skill_name", "candidate_summary", "evidence_refs", "user_notice"]
}"#;

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ReviewReportContext {
    pub(crate) agent_name: String,
    pub(crate) source_invocation_id: String,
    pub(crate) root_session_id: Option<String>,
    pub(crate) chat_id: Option<i64>,
    pub(crate) thread_id: Option<i64>,
    pub(crate) trigger_kind: right_agent::learned_skills::ReviewTriggerKind,
    pub(crate) telegram_notified: bool,
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

    pub(crate) fn to_report(
        &self,
        ctx: ReviewReportContext,
    ) -> right_agent::learned_skills::SkillReviewReport {
        right_agent::learned_skills::SkillReviewReport {
            agent_name: ctx.agent_name,
            source_invocation_id: ctx.source_invocation_id,
            root_session_id: ctx.root_session_id,
            chat_id: ctx.chat_id,
            thread_id: ctx.thread_id,
            trigger_kind: ctx.trigger_kind,
            status: self.status.as_domain(),
            confidence: self.confidence.as_domain(),
            candidate_skill_name: self.candidate_skill_name.clone(),
            candidate_summary: self.candidate_summary.clone(),
            evidence_refs: self.evidence_refs.clone(),
            review_output_json: self.raw.clone(),
            telegram_notified: ctx.telegram_notified,
        }
    }
}

pub(crate) fn parse_review_process_stdout(stdout: &str) -> Result<ReviewOutput, String> {
    let root: serde_json::Value =
        serde_json::from_str(stdout).map_err(|e| format!("parse review stdout JSON: {e}"))?;
    let selected = root
        .get("structured_output")
        .filter(|value| !value.is_null())
        .or_else(|| root.get("result").filter(|value| !value.is_null()))
        .unwrap_or(&root);
    let raw = match selected.as_str() {
        Some(json) => serde_json::from_str(json)
            .map_err(|e| format!("parse review stdout wrapper JSON string: {e}"))?,
        None => selected.clone(),
    };
    ReviewOutput::parse(raw)
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

const SKILL_INDEX_FIELD_SEPARATOR: char = '\0';
const SANDBOX_SKILL_PATH_PREFIX: &str = "/sandbox/.claude/skills/";
const SKILL_EXCERPT_MAX_BYTES: usize = 4_096;
const SKILL_EXCERPT_MAX_CHARS: usize = 4_096;
const SKILL_EXCERPT_MAX_LINES: usize = 120;

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

const REVIEW_WRAP_LABEL_ACCEPTED_SIGNAL: &str = "learning-review/accepted_signal_json";
const REVIEW_WRAP_LABEL_EVENT_TIMELINE: &str = "learning-review/event_timeline";
const REVIEW_WRAP_LABEL_LEARNING_EVENTS: &str = "learning-review/learning_events";
const REVIEW_WRAP_LABEL_SKILL_INDEX: &str = "learning-review/rightx_skill_index";

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
    prompt.push_str(
        "Decision rules:\n\
         - Candidates must be reusable across future sessions, not a summary of this one task.\n\
         - Do not preserve one-off task narrative in candidate summaries.\n\
         - Do not make persistent negative claims from transient tool failures.\n\
         - Prefer update candidates for existing rightx-* skills when the evidence refines an installed learned skill.\n\
         - Use create_candidate when repeated tool patterns or setup workflows are reusable and no existing rightx-* skill fits.\n\
         - Use nothing_to_learn when the evidence is only normal task progress, isolated facts, or one-time content.\n\n",
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
        let bounded = bounded_prompt_block(signal, REVIEW_PROMPT_ACCEPTED_SIGNAL_MAX_CHARS);
        prompt.push_str(&right_prompt_safety::wrap_external(
            REVIEW_WRAP_LABEL_ACCEPTED_SIGNAL,
            &bounded,
        ));
        prompt.push_str("\n\n");
    }

    push_bounded_list(
        &mut prompt,
        "event_timeline",
        REVIEW_WRAP_LABEL_EVENT_TIMELINE,
        &bundle.event_timeline,
        REVIEW_PROMPT_EVENT_TIMELINE_MAX_ITEMS,
        REVIEW_PROMPT_EVENT_TIMELINE_ITEM_MAX_CHARS,
    );
    push_bounded_list(
        &mut prompt,
        "learning_events",
        REVIEW_WRAP_LABEL_LEARNING_EVENTS,
        &bundle.learning_events,
        REVIEW_PROMPT_LEARNING_EVENTS_MAX_ITEMS,
        REVIEW_PROMPT_LEARNING_EVENT_ITEM_MAX_CHARS,
    );
    prompt.push_str("\nrightx_skill_index:\n");
    let mut skill_body = String::new();
    for skill in bundle
        .learned_skills
        .iter()
        .take(REVIEW_PROMPT_LEARNED_SKILLS_MAX_ITEMS)
    {
        skill_body.push_str("- ");
        skill_body.push_str(&bounded_prompt_line(
            &skill.name,
            REVIEW_PROMPT_SKILL_NAME_MAX_CHARS,
        ));
        skill_body.push_str(": ");
        skill_body.push_str(&bounded_prompt_line(
            &skill.excerpt,
            REVIEW_PROMPT_SKILL_EXCERPT_MAX_CHARS,
        ));
        skill_body.push('\n');
    }
    push_omitted_count(
        &mut skill_body,
        bundle.learned_skills.len(),
        REVIEW_PROMPT_LEARNED_SKILLS_MAX_ITEMS,
    );
    prompt.push_str(&right_prompt_safety::wrap_external(
        REVIEW_WRAP_LABEL_SKILL_INDEX,
        skill_body.trim_end_matches('\n'),
    ));
    prompt.push('\n');
    prompt.push_str(
        "\nReturn JSON with status, confidence, candidate_skill_name, candidate_summary, \
         evidence_refs, and user_notice. Use only rightx-* candidate skill names.\n",
    );
    prompt
}

#[cfg(test)]
pub(crate) async fn run_review_with_output<F, Fut>(
    bundle: ReviewBundle,
    run_json: F,
) -> Result<ReviewOutput, String>
where
    F: FnOnce(String) -> Fut,
    Fut: std::future::Future<Output = Result<serde_json::Value, String>>,
{
    let prompt = build_review_prompt(&bundle);
    let raw = run_json(prompt).await?;
    ReviewOutput::parse(raw)
}

pub(crate) fn select_review_trigger(
    has_learning_signal: bool,
    has_skill_issue_signal: bool,
) -> Option<right_agent::learned_skills::ReviewTriggerKind> {
    if has_skill_issue_signal {
        Some(right_agent::learned_skills::ReviewTriggerKind::SkillIssueSignal)
    } else if has_learning_signal {
        Some(right_agent::learned_skills::ReviewTriggerKind::LearningSignal)
    } else {
        None
    }
}

pub(crate) fn review_cooldown_cutoff(
    now: chrono::DateTime<chrono::Utc>,
    cooldown: chrono::Duration,
) -> Result<String, String> {
    if cooldown < chrono::Duration::zero() {
        return Err(format!(
            "skill review cooldown must not be negative: {cooldown:?}"
        ));
    }

    let cutoff = now.checked_sub_signed(cooldown).ok_or_else(|| {
        format!("skill review cooldown cutoff out of range for now {now} and cooldown {cooldown:?}")
    })?;
    Ok(cutoff.format("%Y-%m-%dT%H:%M:%SZ").to_string())
}

#[cfg(test)]
fn review_cooldown_elapsed(
    last_review_at: Option<&str>,
    now: chrono::DateTime<chrono::Utc>,
    cooldown: chrono::Duration,
) -> Result<bool, String> {
    let Some(last_review_at) = last_review_at else {
        return Ok(true);
    };
    let last_review_at_timestamp = chrono::DateTime::parse_from_rfc3339(last_review_at)
        .map_err(|err| format!("parse skill review last_review_at '{last_review_at}': {err}"))?
        .with_timezone(&chrono::Utc);

    Ok(now.signed_duration_since(last_review_at_timestamp) >= cooldown)
}

fn push_bounded_list(
    prompt: &mut String,
    heading: &str,
    wrap_label: &str,
    items: &[String],
    max_items: usize,
    max_item_chars: usize,
) {
    prompt.push_str(heading);
    prompt.push_str(":\n");
    let mut body = String::new();
    for item in items.iter().take(max_items) {
        body.push_str("- ");
        body.push_str(&bounded_prompt_line(item, max_item_chars));
        body.push('\n');
    }
    push_omitted_count(&mut body, items.len(), max_items);
    prompt.push_str(&right_prompt_safety::wrap_external(
        wrap_label,
        body.trim_end_matches('\n'),
    ));
    prompt.push_str("\n\n");
}

fn push_omitted_count(prompt: &mut String, item_count: usize, max_items: usize) {
    if item_count > max_items {
        prompt.push_str("- ... ");
        prompt.push_str(&(item_count - max_items).to_string());
        prompt.push_str(" additional items omitted\n");
    }
}

fn bounded_prompt_line(value: &str, max_chars: usize) -> String {
    bounded_text(
        &value.replace(['\r', '\n'], " "),
        max_chars,
        TRUNCATED_SUFFIX,
    )
}

fn bounded_prompt_block(value: &str, max_chars: usize) -> String {
    bounded_text(value, max_chars, TRUNCATED_SUFFIX)
}

pub(crate) const TRUNCATED_SUFFIX: &str = "... [truncated]";
pub(crate) const ELLIPSIS_SUFFIX: &str = "...";

pub(crate) fn bounded_text(value: &str, max_chars: usize, suffix: &str) -> String {
    let mut chars = value.chars().filter(|c| *c != '\0');
    let mut out: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        out.push_str(suffix);
    }
    out
}

pub(crate) fn collect_host_rightx_skill_index(
    agent_dir: &Path,
) -> std::io::Result<Vec<LearnedSkillSummary>> {
    let skills_dir = agent_dir.join(".claude/skills");
    let entries = match std::fs::read_dir(&skills_dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err),
    };
    let mut skills = Vec::new();

    for entry in entries {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }

        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.starts_with(LEARNED_SKILL_PREFIX) {
            continue;
        }

        let skill_path = entry.path().join("SKILL.md");
        let excerpt = match read_bounded_skill_excerpt(&skill_path) {
            Ok(Some(excerpt)) => excerpt,
            Ok(None) => continue,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => continue,
            Err(err) => return Err(err),
        };
        skills.push(LearnedSkillSummary { name, excerpt });
    }

    skills.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(skills)
}

fn read_bounded_skill_excerpt(path: &Path) -> std::io::Result<Option<String>> {
    let metadata = std::fs::symlink_metadata(path)?;
    if !metadata.file_type().is_file() {
        return Ok(None);
    }

    let file = std::fs::File::open(path)?;
    let mut reader = std::io::BufReader::new(file).take(SKILL_EXCERPT_MAX_BYTES as u64);
    let mut bytes = Vec::with_capacity(SKILL_EXCERPT_MAX_BYTES);
    reader.read_to_end(&mut bytes)?;
    let content = String::from_utf8_lossy(&bytes);
    Ok(Some(bounded_skill_excerpt(&content)))
}

fn bounded_skill_excerpt(content: &str) -> String {
    bounded_skill_excerpt_from_lines(content.lines())
}

fn bounded_skill_excerpt_from_lines<'a>(lines: impl Iterator<Item = &'a str>) -> String {
    let mut out = String::new();
    let mut chars = 0;
    let mut first = true;

    for line in lines.take(SKILL_EXCERPT_MAX_LINES) {
        if !first && !push_bounded_skill_char(&mut out, '\n', &mut chars) {
            break;
        }
        first = false;

        for ch in line.chars() {
            if !push_bounded_skill_char(&mut out, ch, &mut chars) {
                return out.trim().to_owned();
            }
        }
    }

    out.trim().to_owned()
}

fn push_bounded_skill_char(out: &mut String, ch: char, chars: &mut usize) -> bool {
    if *chars >= SKILL_EXCERPT_MAX_CHARS || out.len() + ch.len_utf8() > SKILL_EXCERPT_MAX_BYTES {
        return false;
    }
    out.push(ch);
    *chars += 1;
    true
}

pub(crate) fn parse_sandbox_skill_index_stdout(stdout: &str) -> Vec<LearnedSkillSummary> {
    let mut skills = Vec::new();
    let mut fields = stdout.split(SKILL_INDEX_FIELD_SEPARATOR);

    while let Some(path) = fields.next() {
        let Some(content) = fields.next() else {
            break;
        };
        let Some(name) = sandbox_skill_name_from_path(path) else {
            continue;
        };
        let excerpt = bounded_skill_excerpt(content);
        skills.push(LearnedSkillSummary {
            name: name.to_owned(),
            excerpt,
        });
    }

    skills.sort_by(|a, b| a.name.cmp(&b.name));
    skills
}

fn sandbox_skill_name_from_path(path: &str) -> Option<&str> {
    let path = path.trim();
    let tail = path.strip_prefix(SANDBOX_SKILL_PATH_PREFIX)?;
    let name = tail.strip_suffix("/SKILL.md")?;
    (name.starts_with(LEARNED_SKILL_PREFIX) && !name.contains('/')).then_some(name)
}

pub(crate) fn sandbox_skill_index_command() -> [&'static str; 3] {
    [
        "sh",
        "-lc",
        "for f in /sandbox/.claude/skills/rightx-*/SKILL.md; do [ -f \"$f\" ] || continue; printf '%s\\0' \"$f\"; sed -n '1,120p' \"$f\" | head -c 4096 | tr '\\000' '\\n'; printf '\\0'; done",
    ]
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
    if max_events == 0 {
        return Ok(Vec::new());
    }
    let path = review_stream_log_path(agent_dir, root_session_id);
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err),
    };
    let reader = std::io::BufReader::new(file);
    let mut recent = std::collections::VecDeque::with_capacity(max_events);

    for line in reader.lines() {
        let line = line?;
        let Some(summary) = summarize_stream_event(&line) else {
            continue;
        };
        if recent.len() == max_events {
            recent.pop_front();
        }
        recent.push_back(summary);
    }

    Ok(recent
        .into_iter()
        .enumerate()
        .map(|(idx, summary)| format!("event-{} {summary}", idx + 1))
        .collect())
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
    bounded_text(value, MAX_CHARS, ELLIPSIS_SUFFIX)
}

#[cfg(test)]
#[path = "learning_review_tests.rs"]
mod tests;
