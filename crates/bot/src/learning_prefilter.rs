//! Haiku classifier deciding whether to spawn the probe-writer.
//!
//! Spec: docs/superpowers/specs/2026-05-22-skill-learning-writer-curator-design.md

use crate::cc::worker_reply::is_rightx_skill;
use crate::telegram::worker::ProbeAnchor;
use std::io::Read as _;
use std::path::{Path, PathBuf};

/// Decision returned by the prefilter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PrefilterDecision {
    Skip {
        reason: String,
    },
    PatchExisting {
        target_skill: String,
        reason: String,
    },
    CreateNew {
        topic_hint: String,
        reason: String,
    },
}

pub(crate) const PREFILTER_SCHEMA_JSON: &str = r#"{
  "type": "object",
  "properties": {
    "decision": {
      "type": "string",
      "enum": ["skip", "patch_existing", "create_new"]
    },
    "target_skill": {
      "type": "string",
      "pattern": "^rightx-[a-z0-9-]+$"
    },
    "topic_hint": {
      "type": "string",
      "maxLength": 120
    },
    "reason": {
      "type": "string",
      "maxLength": 400
    }
  },
  "required": ["decision", "reason"]
}"#;

/// Sum today's spend across learning sources from `usage_events`. Used by the
/// worker to gate the prefilter+probe-writer pipeline against the daily budget.
pub(crate) async fn today_spend_usd(
    conn: &right_db::Connection,
    now_utc: &str,
) -> Result<f64, right_db::DbError> {
    let date_part = now_utc.split_once('T').map(|(d, _)| d).unwrap_or(now_utc);
    let today_start = format!("{date_part}T00:00:00Z");
    let placeholders = std::iter::repeat_n("?", right_agent::usage::LEARNING_SOURCES.len())
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!(
        "SELECT COALESCE(SUM(total_cost_usd), 0.0) FROM usage_events \
         WHERE ts >= ?1 AND source IN ({placeholders})"
    );
    let mut params = right_db::params::ParamsBuilder::new();
    params.push(today_start.as_str())?;
    for s in right_agent::usage::LEARNING_SOURCES {
        params.push(s)?;
    }
    conn.query_row(&sql, params, |r| r.get::<_, f64>(0)).await
}

/// Compose the prompt that goes to Haiku.
pub(crate) fn build_prompt(
    anchor: &ProbeAnchor,
    baselines: &right_agent::usage::turn_baseline::TurnBaselines,
    skill_index_summary: &str,
) -> String {
    let user: String = anchor.user_msg_text.chars().take(2000).collect();
    let assistant: String = anchor.assistant_reply_text.chars().take(4000).collect();
    let stats = render_turn_stats(anchor, baselines);
    let receipts_section = if anchor.used_skill_receipts.is_empty() {
        "USED SKILLS: none".to_owned()
    } else {
        format!(
            "USED SKILLS:\n{}",
            anchor
                .used_skill_receipts
                .iter()
                .map(|s| format!("- {s}"))
                .collect::<Vec<_>>()
                .join("\n")
        )
    };
    let framing = if anchor.used_skill_receipts.is_empty() {
        "No existing skill was used in this turn. Decide whether the turn \
exposed a reusable procedure that warrants a *new* skill, or whether it \
was trivial (Skip)."
    } else {
        "One or more existing skills were used. Decide whether any of them \
needs a *patch* (because the turn exposed a gap or correction), whether a \
*new* skill should be created for a procedure beyond the cited skills' \
scope, or whether the turn was a clean application of existing skills \
(Skip)."
    };
    let skill_index_summary = skill_index_summary.trim_end();
    format!(
        "Decide whether the just-finished turn produced material worth \
spawning the probe-writer (Sonnet) over. Reply JSON per schema.

{stats}

{receipts_section}

EXISTING SKILLS (abbreviated):
{skill_index_summary}

USER: {user}
ASSISTANT: {assistant}

{framing}

Set:
- decision=\"skip\" if the turn was trivial chat/echo or already \
well-covered by existing skills.
- decision=\"patch_existing\" with target_skill=\"rightx-<name>\" if a \
used skill needs a focused update (gap, correction, missing edge case).
- decision=\"create_new\" with topic_hint=\"<short topic>\" if the turn \
exposes a reusable procedure not covered by any existing skill.

reason is a short justification (max 400 chars)."
    )
}

fn render_turn_stats(
    anchor: &ProbeAnchor,
    b: &right_agent::usage::turn_baseline::TurnBaselines,
) -> String {
    use right_agent::usage::turn_baseline::BaselineMetric;
    let n = b.sample_size;
    let window = b.window_days;
    if matches!(b.cost_usd, BaselineMetric::Insufficient { .. }) {
        return format!(
            "TURN STATS (baseline insufficient, only n={n} prior turns):\n  \
num_turns: {turns}, cost: ${cost:.3}, elapsed: {elapsed_s}s",
            turns = anchor.num_turns,
            cost = anchor.total_cost_usd,
            elapsed_s = anchor.wall_elapsed_ms / 1000,
        );
    }
    let cost_line = match b.cost_usd {
        BaselineMetric::Available { p50, p90, p99 } => format!(
            "  cost:       ${cur:.3}   (P50=${p50:.3}, P90=${p90:.3}, P99=${p99:.3})",
            cur = anchor.total_cost_usd
        ),
        _ => format!("  cost:       ${:.3}", anchor.total_cost_usd),
    };
    let turns_line = match b.num_turns {
        BaselineMetric::Available { p50, p90, p99 } => format!(
            "  num_turns:  {cur}      (P50={p50}, P90={p90}, P99={p99})",
            cur = anchor.num_turns
        ),
        _ => format!("  num_turns:  {}", anchor.num_turns),
    };
    let elapsed_line = match b.wall_elapsed_ms {
        BaselineMetric::Available { p50, p90, p99 } => format!(
            "  elapsed:    {cur}s     (P50={p50}s, P90={p90}s, P99={p99}s)",
            cur = anchor.wall_elapsed_ms / 1000,
            p50 = p50 / 1000,
            p90 = p90 / 1000,
            p99 = p99 / 1000
        ),
        _ => format!("  elapsed:    {}s", anchor.wall_elapsed_ms / 1000),
    };
    format!(
        "TURN STATS (this turn vs agent's {window}d foreground baseline, n={n}):\n\
{turns_line}\n{cost_line}\n{elapsed_line}"
    )
}

/// Parse Haiku's JSON output into a decision. Returns Skip on any parse error.
pub(crate) fn parse_output(stdout: &str) -> PrefilterDecision {
    // CC --output-format json wraps assistant text. Strip the envelope first.
    let inner = match unwrap_structured_output_payload(stdout, "prefilter") {
        Ok(v) => v,
        Err(_) => {
            return PrefilterDecision::Skip {
                reason: "envelope parse failed".into(),
            };
        }
    };

    let decision = inner.get("decision").and_then(|v| v.as_str());
    let reason = inner
        .get("reason")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_owned();

    match decision {
        Some("skip") => PrefilterDecision::Skip { reason },
        Some("patch_existing") => {
            let target = inner
                .get("target_skill")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            if !is_rightx_skill(target) {
                tracing::warn!(
                    target = %target,
                    "prefilter patch_existing missing/invalid target_skill"
                );
                return PrefilterDecision::Skip {
                    reason: "patch_existing without valid target_skill".into(),
                };
            }
            PrefilterDecision::PatchExisting {
                target_skill: target.into(),
                reason,
            }
        }
        Some("create_new") => {
            let hint = inner
                .get("topic_hint")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            if hint.is_empty() {
                tracing::warn!("prefilter create_new missing topic_hint");
                return PrefilterDecision::Skip {
                    reason: "create_new without topic_hint".into(),
                };
            }
            PrefilterDecision::CreateNew {
                topic_hint: hint.into(),
                reason,
            }
        }
        other => {
            tracing::warn!(decision = ?other, "prefilter unknown decision");
            PrefilterDecision::Skip {
                reason: "unknown decision".into(),
            }
        }
    }
}

use std::time::Duration;

const PREFILTER_TIMEOUT: Duration = Duration::from_secs(30);
const PREFILTER_LOG_EXCERPT_MAX_CHARS: usize = 2048;
const TRUNCATED_SUFFIX: &str = "... [truncated]";
const SKILL_INDEX_DESC_MAX_CHARS: usize = 120;
const SKILL_EXCERPT_MAX_BYTES: usize = 4_096;
const SKILL_EXCERPT_MAX_CHARS: usize = 4_096;
const SKILL_EXCERPT_MAX_LINES: usize = 120;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LearnedSkillSummary {
    pub(crate) name: String,
    pub(crate) excerpt: String,
}

fn unwrap_structured_output_payload(
    stdout: &str,
    label: &str,
) -> Result<serde_json::Value, String> {
    let root: serde_json::Value =
        serde_json::from_str(stdout).map_err(|e| format!("parse {label} stdout JSON: {e}"))?;
    let selected = root
        .get("structured_output")
        .filter(|value| !value.is_null())
        .or_else(|| root.get("result").filter(|value| !value.is_null()))
        .unwrap_or(&root);
    match selected.as_str() {
        Some(json) => serde_json::from_str(json)
            .map_err(|e| format!("parse {label} stdout wrapper JSON string: {e}")),
        None => Ok(selected.clone()),
    }
}

fn bounded_text(value: &str, max_chars: usize, suffix: &str) -> String {
    let mut chars = value.chars().filter(|c| *c != '\0');
    let mut out: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        out.push_str(suffix);
    }
    out
}

fn collect_host_rightx_skill_index(agent_dir: &Path) -> std::io::Result<Vec<LearnedSkillSummary>> {
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
        if !is_rightx_skill(&name) {
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

/// Shell command run inside the sandbox to dump `rightx-*` skill frontmatter.
/// The `[ -f ... ]` guard makes a no-match glob emit nothing (no error), and
/// each skill is delimited by a `@@@SKILL <name>` marker on its own line.
const SANDBOX_SKILL_DUMP_CMD: &str = "for d in /sandbox/.claude/skills/rightx-*/; do \
       [ -f \"$d/SKILL.md\" ] || continue; \
       printf '\\n@@@SKILL %s\\n' \"$(basename \"$d\")\"; \
       head -c 4096 \"$d/SKILL.md\"; \
     done";

/// Parse the delimited dump from [`SANDBOX_SKILL_DUMP_CMD`] into per-skill
/// summaries. Non-`rightx-` chunks are dropped, and excerpts are bounded the
/// same way as the host path.
fn parse_sandbox_skill_dump(dump: &str) -> Vec<LearnedSkillSummary> {
    let mut out = Vec::new();
    for chunk in dump.split("\n@@@SKILL ") {
        let chunk = chunk.trim_start_matches("@@@SKILL ");
        let Some((name_line, body)) = chunk.split_once('\n') else {
            continue;
        };
        let name = name_line.trim();
        if !is_rightx_skill(name) {
            continue;
        }
        out.push(LearnedSkillSummary {
            name: name.to_owned(),
            excerpt: bounded_skill_excerpt(body),
        });
    }
    // `SANDBOX_SKILL_DUMP_CMD`'s shell glob (`rightx-*/`) already yields
    // lexicographic order, but sort explicitly so the guarantee matches the
    // host path and survives any future change to the dump command.
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// Read the `rightx-*` skill index from inside the sandbox via gRPC exec.
///
/// Learned skills live on the sandbox filesystem (`/sandbox/.claude/skills`),
/// never on the host, so the host directory walk in
/// [`collect_host_rightx_skill_index`] only applies to `sandbox: mode: none`
/// agents.
async fn collect_sandbox_rightx_skill_index(
    sandbox_name: &str,
) -> anyhow::Result<Vec<LearnedSkillSummary>> {
    use anyhow::Context as _;

    let mtls_dir = match right_openshell::openshell::preflight_check() {
        right_openshell::openshell::OpenShellStatus::Ready(dir) => dir,
        status => anyhow::bail!("openshell preflight not ready: {status:?}"),
    };
    let mut client = right_openshell::openshell::connect_grpc(&mtls_dir)
        .await
        .map_err(|e| anyhow::anyhow!("connect_grpc: {e:?}"))?;
    let sandbox_id = right_openshell::openshell::resolve_sandbox_id(&mut client, sandbox_name)
        .await
        .map_err(|e| anyhow::anyhow!("resolve_sandbox_id: {e:?}"))?;
    let (out, _exit) = right_openshell::openshell::exec_in_sandbox(
        &mut client,
        &sandbox_id,
        &["sh", "-c", SANDBOX_SKILL_DUMP_CMD],
        right_openshell::openshell::DEFAULT_EXEC_TIMEOUT_SECS,
    )
    .await
    .map_err(|e| anyhow::anyhow!("exec_in_sandbox: {e:?}"))
    .context("read sandbox skill index")?;
    Ok(parse_sandbox_skill_dump(&out))
}

/// Shared skill-index reader: from the sandbox via gRPC when a sandbox is
/// resolved, or the host directory walk for `sandbox: mode: none` agents.
pub(crate) async fn collect_rightx_skill_index(
    resolved_sandbox: Option<&str>,
    agent_dir: &Path,
) -> anyhow::Result<Vec<LearnedSkillSummary>> {
    match resolved_sandbox {
        Some(name) => collect_sandbox_rightx_skill_index(name).await,
        None => Ok(collect_host_rightx_skill_index(agent_dir)?),
    }
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
    let mut out = String::new();
    let mut chars = 0;
    let mut first = true;

    for line in content.lines().take(SKILL_EXCERPT_MAX_LINES) {
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

fn render_skill_index_summary(skills: &[LearnedSkillSummary]) -> String {
    use std::fmt::Write as _;
    let mut s = String::new();
    for skill in skills {
        let desc_line = extract_skill_description(&skill.excerpt)
            .chars()
            .take(SKILL_INDEX_DESC_MAX_CHARS)
            .collect::<String>();
        let _ = writeln!(s, "- {name}: {desc_line}", name = skill.name);
    }
    s
}

fn extract_skill_description(excerpt: &str) -> &str {
    const DESCRIPTION_PREFIX: &str = "description:";
    let mut lines = excerpt.lines();
    if lines.next().is_some_and(|line| line.trim() == "---") {
        for line in lines.by_ref() {
            if line.trim() == "---" {
                break;
            }
            if let Some(rest) = line.strip_prefix(DESCRIPTION_PREFIX) {
                return rest.trim();
            }
        }
    }

    for line in excerpt.lines() {
        let trimmed = line.trim();
        if !trimmed.is_empty() && trimmed != "---" && !trimmed.starts_with("```") {
            return trimmed;
        }
    }
    ""
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PrefilterFailureDiagnostics {
    argv: Vec<String>,
    stdout_bytes: usize,
    stderr_bytes: usize,
    stdout_excerpt: String,
    stderr_excerpt: String,
}

fn redact_prefilter_args(args: &[String]) -> Vec<String> {
    let mut redacted = Vec::with_capacity(args.len());
    let mut i = 0usize;
    while i < args.len() {
        match args[i].as_str() {
            "--json-schema" => {
                redacted.push(args[i].clone());
                if let Some(schema) = args.get(i + 1) {
                    redacted.push(format!("<json-schema chars={}>", schema.chars().count()));
                    i += 2;
                } else {
                    i += 1;
                }
            }
            "--" => {
                redacted.push(args[i].clone());
                let prompt = args[i + 1..].join(" ");
                redacted.push(format!("<prompt chars={}>", prompt.chars().count()));
                break;
            }
            _ => {
                redacted.push(args[i].clone());
                i += 1;
            }
        }
    }
    redacted
}

fn prefilter_log_excerpt(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    bounded_text(&text, PREFILTER_LOG_EXCERPT_MAX_CHARS, TRUNCATED_SUFFIX)
}

fn prefilter_failure_diagnostics(
    args: &[String],
    stdout: &[u8],
    stderr: &[u8],
) -> PrefilterFailureDiagnostics {
    PrefilterFailureDiagnostics {
        argv: redact_prefilter_args(args),
        stdout_bytes: stdout.len(),
        stderr_bytes: stderr.len(),
        stdout_excerpt: prefilter_log_excerpt(stdout),
        stderr_excerpt: prefilter_log_excerpt(stderr),
    }
}

/// Bundle of inputs needed to run one prefilter invocation.
#[derive(Debug, Clone)]
pub(crate) struct PrefilterContext {
    pub agent_dir: PathBuf,
    pub agent_db_dir: PathBuf,
    pub agent_name: String,
    pub ssh_config_path: Option<PathBuf>,
    pub resolved_sandbox: Option<String>,
    pub model: String,
    pub chat_id: i64,
    pub thread_id: i64,
    /// Window for baseline percentiles (passes through to `turn_baseline::compute`).
    pub baseline_window_days: u32,
    /// Minimum sample size for the baseline to be `Available`.
    pub baseline_min_sample: u32,
}

/// Run the Haiku prefilter on an anchor. Logs warns on any failure, returns Skip.
pub(crate) async fn run(ctx: PrefilterContext, anchor: ProbeAnchor) -> PrefilterDecision {
    use crate::cc::invocation::{ClaudeInvocation, OutputFormat, build_claude_command};

    let conn = match right_db::open_connection(&ctx.agent_db_dir, false).await {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(agent = %ctx.agent_name, "prefilter open_connection failed: {e:#}");
            return PrefilterDecision::Skip {
                reason: "db open failed".into(),
            };
        }
    };
    let baselines = match right_agent::usage::turn_baseline::compute(
        &conn,
        ctx.baseline_window_days,
        ctx.baseline_min_sample,
    )
    .await
    {
        Ok(b) => b,
        Err(e) => {
            tracing::warn!(agent = %ctx.agent_name, "prefilter baseline compute failed: {e:#}");
            return PrefilterDecision::Skip {
                reason: "baseline compute failed".into(),
            };
        }
    };

    let skills =
        match collect_rightx_skill_index(ctx.resolved_sandbox.as_deref(), &ctx.agent_dir).await {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(agent = %ctx.agent_name, "prefilter skill index failed: {e:#}");
                return PrefilterDecision::Skip {
                    reason: "skill index failed".into(),
                };
            }
        };
    let summary = render_skill_index_summary(&skills);

    let prompt = build_prompt(&anchor, &baselines, &summary);
    let invocation = ClaudeInvocation {
        mcp_config_path: None,
        json_schema: Some(PREFILTER_SCHEMA_JSON.into()),
        output_format: OutputFormat::Json,
        model: Some(ctx.model.clone()),
        max_budget_usd: None,
        max_turns: Some(5),
        resume_session_id: None,
        new_session_id: None,
        fork_session: false,
        allowed_tools: vec![],
        disallowed_tools: vec![],
        extra_args: crate::cc::invocation::disable_all_tools_args(),
        prompt: Some(prompt),
        debug_flag: None,
    };
    let args = invocation.into_args();
    let mut cmd = match build_claude_command(
        &args,
        &ctx.agent_dir,
        ctx.ssh_config_path.as_deref(),
        ctx.resolved_sandbox.as_deref(),
    )
    .await
    {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(agent = %ctx.agent_name, "skipping prefilter: {e:#}");
            return PrefilterDecision::Skip {
                reason: "sandboxed host-exec refused".into(),
            };
        }
    };
    cmd.stdin(std::process::Stdio::null());
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());

    let output = match tokio::time::timeout(PREFILTER_TIMEOUT, cmd.output()).await {
        Ok(Ok(o)) => o,
        Ok(Err(e)) => {
            let argv = redact_prefilter_args(&args);
            tracing::warn!(
                agent = %ctx.agent_name,
                model = %ctx.model,
                sandbox = ?ctx.resolved_sandbox,
                argv = ?argv,
                "prefilter spawn failed: {e:#}"
            );
            return PrefilterDecision::Skip {
                reason: "spawn failed".into(),
            };
        }
        Err(_) => {
            let argv = redact_prefilter_args(&args);
            tracing::warn!(
                agent = %ctx.agent_name,
                model = %ctx.model,
                sandbox = ?ctx.resolved_sandbox,
                argv = ?argv,
                "prefilter timed out after {}s",
                PREFILTER_TIMEOUT.as_secs()
            );
            return PrefilterDecision::Skip {
                reason: "timed out".into(),
            };
        }
    };

    if !output.status.success() {
        let diagnostics = prefilter_failure_diagnostics(&args, &output.stdout, &output.stderr);
        tracing::warn!(
            agent = %ctx.agent_name,
            model = %ctx.model,
            sandbox = ?ctx.resolved_sandbox,
            status = ?output.status,
            argv = ?diagnostics.argv,
            stdout_bytes = diagnostics.stdout_bytes,
            stderr_bytes = diagnostics.stderr_bytes,
            stdout_excerpt = %diagnostics.stdout_excerpt,
            stderr_excerpt = %diagnostics.stderr_excerpt,
            "prefilter non-zero exit"
        );
        return PrefilterDecision::Skip {
            reason: "non-zero exit".into(),
        };
    }

    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();

    // Record usage event. Zero-signal results still cost money and must be visible.
    if let Some(b) = crate::cc::stream::parse_usage_full(&stdout)
        && let Err(e) = right_agent::usage::insert::insert_learning_prefilter(
            &conn,
            &b,
            ctx.chat_id,
            ctx.thread_id,
        )
        .await
    {
        tracing::warn!(agent = %ctx.agent_name, "prefilter usage insert failed: {e:#}");
    }

    let decision = parse_output(&stdout);
    if matches!(
        decision,
        PrefilterDecision::Skip { ref reason } if reason == "envelope parse failed"
    ) {
        let diagnostics = prefilter_failure_diagnostics(&args, stdout.as_bytes(), &output.stderr);
        tracing::warn!(
            agent = %ctx.agent_name,
            model = %ctx.model,
            sandbox = ?ctx.resolved_sandbox,
            argv = ?diagnostics.argv,
            stdout_bytes = diagnostics.stdout_bytes,
            stderr_bytes = diagnostics.stderr_bytes,
            stdout_excerpt = %diagnostics.stdout_excerpt,
            stderr_excerpt = %diagnostics.stderr_excerpt,
            "prefilter output parse failed"
        );
    }
    decision
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_sandbox_skill_dump_extracts_name_and_excerpt() {
        let dump = "\n@@@SKILL rightx-foo\n---\nname: rightx-foo\ndescription: does foo\n---\n# body\n\
                    \n@@@SKILL rightx-bar\n---\nname: rightx-bar\ndescription: does bar\n---\n";
        let got = parse_sandbox_skill_dump(dump);
        // Dump order is foo-then-bar; output is sorted by name, so bar leads.
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].name, "rightx-bar");
        assert!(got[0].excerpt.contains("does bar"));
        assert_eq!(got[1].name, "rightx-foo");
        assert!(got[1].excerpt.contains("does foo"));
    }

    #[tokio::test]
    #[ignore = "ci-openshell: creates a live sandbox, writes a SKILL.md, reads the index"]
    async fn ci_openshell_sandbox_skill_index_reads_rightx() {
        let sb = right_openshell::test_support::TestSandbox::create("skill-index").await;
        // `printf -- ...` stops option parsing so the `---` frontmatter fence
        // isn't misread as printf flags (which would exit 2 and write nothing).
        let (_o, code) = sb
            .exec(&[
                "sh",
                "-c",
                "mkdir -p /sandbox/.claude/skills/rightx-demo && \
                 printf -- '---\\nname: rightx-demo\\ndescription: demo skill\\n---\\n' \
                 > /sandbox/.claude/skills/rightx-demo/SKILL.md",
            ])
            .await;
        assert_eq!(code, 0, "skill-file setup command failed");
        let idx = collect_sandbox_rightx_skill_index(sb.name()).await.unwrap();
        assert!(
            idx.iter()
                .any(|s| s.name == "rightx-demo" && s.excerpt.contains("demo skill"))
        );
    }

    fn anchor(user: &str, assistant: &str) -> ProbeAnchor {
        ProbeAnchor {
            user_msg_text: user.to_owned(),
            assistant_reply_text: assistant.to_owned(),
            main_session_uuid: "uuid-main".to_owned(),
            captured_at: chrono::Utc::now(),
            chat_id: 1,
            thread_id: 0,
            num_turns: 1,
            total_cost_usd: 0.0,
            wall_elapsed_ms: 0,
            used_skill_receipts: Vec::new(),
        }
    }

    fn baselines_insufficient(n: u32) -> right_agent::usage::turn_baseline::TurnBaselines {
        use right_agent::usage::turn_baseline::{BaselineMetric, TurnBaselines};
        TurnBaselines {
            sample_size: n,
            elapsed_sample_size: n,
            window_days: 14,
            cost_usd: BaselineMetric::Insufficient { sample_size: n },
            num_turns: BaselineMetric::Insufficient { sample_size: n },
            wall_elapsed_ms: BaselineMetric::Insufficient { sample_size: n },
        }
    }

    fn baselines_available() -> right_agent::usage::turn_baseline::TurnBaselines {
        use right_agent::usage::turn_baseline::{BaselineMetric, TurnBaselines};
        TurnBaselines {
            sample_size: 50,
            elapsed_sample_size: 50,
            window_days: 14,
            cost_usd: BaselineMetric::Available {
                p50: 0.03,
                p90: 0.18,
                p99: 0.95,
            },
            num_turns: BaselineMetric::Available {
                p50: 4,
                p90: 12,
                p99: 24,
            },
            wall_elapsed_ms: BaselineMetric::Available {
                p50: 6_000,
                p90: 22_000,
                p99: 58_000,
            },
        }
    }

    #[tokio::test]
    async fn build_prompt_embeds_anchor_texts() {
        let bs = baselines_insufficient(0);
        let p = build_prompt(&anchor("hello world", "hi back"), &bs, "");
        assert!(p.contains("hello world"));
        assert!(p.contains("hi back"));
    }

    #[tokio::test]
    async fn redacts_schema_and_prompt_from_prefilter_argv() {
        let args = vec![
            "claude".to_owned(),
            "-p".to_owned(),
            "--json-schema".to_owned(),
            "SECRET_SCHEMA".to_owned(),
            "--".to_owned(),
            "SECRET_PROMPT".to_owned(),
        ];

        let redacted = redact_prefilter_args(&args).join(" ");

        assert!(!redacted.contains("SECRET_SCHEMA"));
        assert!(!redacted.contains("SECRET_PROMPT"));
        assert!(redacted.contains("<json-schema chars=13>"));
        assert!(redacted.contains("<prompt chars=13>"));
    }

    #[tokio::test]
    async fn prefilter_failure_diagnostics_bound_output_excerpts() {
        let stdout = "s".repeat(PREFILTER_LOG_EXCERPT_MAX_CHARS + 100);
        let stderr = "e".repeat(PREFILTER_LOG_EXCERPT_MAX_CHARS + 200);

        let diagnostics = prefilter_failure_diagnostics(&[], stdout.as_bytes(), stderr.as_bytes());

        assert_eq!(diagnostics.stdout_bytes, stdout.len());
        assert_eq!(diagnostics.stderr_bytes, stderr.len());
        assert!(
            diagnostics.stdout_excerpt.chars().count()
                <= PREFILTER_LOG_EXCERPT_MAX_CHARS + TRUNCATED_SUFFIX.len()
        );
        assert!(
            diagnostics.stderr_excerpt.chars().count()
                <= PREFILTER_LOG_EXCERPT_MAX_CHARS + TRUNCATED_SUFFIX.len()
        );
    }

    #[tokio::test]
    async fn parses_skip_decision() {
        let stdout = wrap_cc_envelope(r#"{"decision":"skip","reason":"trivial echo"}"#);
        let d = parse_output(&stdout);
        assert!(matches!(d, PrefilterDecision::Skip { reason } if reason == "trivial echo"));
    }

    #[tokio::test]
    async fn parses_patch_existing_decision_with_target() {
        let stdout = wrap_cc_envelope(
            r#"{"decision":"patch_existing","target_skill":"rightx-foo","reason":"missed step"}"#,
        );
        let d = parse_output(&stdout);
        match d {
            PrefilterDecision::PatchExisting {
                target_skill,
                reason,
            } => {
                assert_eq!(target_skill, "rightx-foo");
                assert_eq!(reason, "missed step");
            }
            _ => panic!("expected PatchExisting"),
        }
    }

    #[tokio::test]
    async fn parses_create_new_decision_with_topic_hint() {
        let stdout = wrap_cc_envelope(
            r#"{"decision":"create_new","topic_hint":"git rebase recovery","reason":"new procedure"}"#,
        );
        let d = parse_output(&stdout);
        match d {
            PrefilterDecision::CreateNew { topic_hint, reason } => {
                assert_eq!(topic_hint, "git rebase recovery");
                assert_eq!(reason, "new procedure");
            }
            _ => panic!("expected CreateNew"),
        }
    }

    #[tokio::test]
    async fn patch_without_target_returns_skip() {
        let stdout = wrap_cc_envelope(r#"{"decision":"patch_existing","reason":"vague"}"#);
        let d = parse_output(&stdout);
        assert!(matches!(d, PrefilterDecision::Skip { .. }));
    }

    #[tokio::test]
    async fn create_without_topic_hint_returns_skip() {
        let stdout = wrap_cc_envelope(r#"{"decision":"create_new","reason":"vague"}"#);
        let d = parse_output(&stdout);
        assert!(matches!(d, PrefilterDecision::Skip { .. }));
    }

    #[tokio::test]
    async fn target_skill_not_rightx_returns_skip() {
        let stdout = wrap_cc_envelope(
            r#"{"decision":"patch_existing","target_skill":"foo-bar","reason":"x"}"#,
        );
        let d = parse_output(&stdout);
        assert!(matches!(d, PrefilterDecision::Skip { .. }));
    }

    #[tokio::test]
    async fn malformed_json_returns_skip() {
        let d = parse_output("not json");
        assert!(matches!(d, PrefilterDecision::Skip { .. }));
    }

    /// Wrap raw JSON in the CC `--output-format json` envelope the parser
    /// expects (`result` field).
    fn wrap_cc_envelope(inner_json: &str) -> String {
        serde_json::json!({
            "type": "result",
            "result": inner_json,
        })
        .to_string()
    }

    #[tokio::test]
    async fn build_prompt_truncates_long_inputs() {
        // Use non-ASCII markers absent from the template prose.
        let long_user = "ы".repeat(10_000);
        let long_asst = "ё".repeat(10_000);
        let bs = baselines_insufficient(0);
        let p = build_prompt(&anchor(&long_user, &long_asst), &bs, "");
        // User truncated to 2000 chars, assistant to 4000 chars.
        assert_eq!(p.matches('ы').count(), 2000);
        assert_eq!(p.matches('ё').count(), 4000);
    }

    #[tokio::test]
    async fn build_prompt_includes_create_new_framing_when_receipts_empty() {
        let mut a = anchor("hello", "hi");
        a.used_skill_receipts.clear();
        let bs = baselines_insufficient(8);
        let p = build_prompt(&a, &bs, "- rightx-foo: foo desc");
        assert!(p.contains("No existing skill was used"), "got: {p}");
        assert!(p.contains("USED SKILLS: none"), "got: {p}");
    }

    #[tokio::test]
    async fn build_prompt_includes_patch_framing_when_receipts_present() {
        let mut a = anchor("hello", "hi");
        a.used_skill_receipts = vec!["rightx-foo".into()];
        let bs = baselines_insufficient(8);
        let p = build_prompt(&a, &bs, "- rightx-foo: foo desc");
        assert!(
            p.contains("One or more existing skills were used"),
            "got: {p}"
        );
        assert!(p.contains("- rightx-foo"), "got: {p}");
    }

    #[tokio::test]
    async fn build_prompt_renders_percentiles_when_baseline_available() {
        let a = anchor("hello", "hi");
        let bs = baselines_available();
        let p = build_prompt(&a, &bs, "");
        assert!(p.contains("vs agent's"), "got: {p}");
        assert!(p.contains("P50="), "got: {p}");
        assert!(p.contains("P90="), "got: {p}");
        assert!(p.contains("P99="), "got: {p}");
    }

    #[tokio::test]
    async fn build_prompt_renders_insufficient_baseline_message() {
        let a = anchor("hello", "hi");
        let bs = baselines_insufficient(8);
        let p = build_prompt(&a, &bs, "");
        assert!(p.contains("baseline insufficient"), "got: {p}");
        assert!(p.contains("n=8"), "got: {p}");
    }
}
