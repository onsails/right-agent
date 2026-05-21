//! Shared prompt assembly for CC invocations (worker, cron, delivery).

/// Memory injection mode for prompt assembly.
pub(crate) enum MemoryMode {
    /// Inject MEMORY.md from agent directory.
    File,
    /// Inject composite memory file written by bot (Hindsight recall results).
    Hindsight { composite_memory_path: String },
}

/// Which composite prompt body to assemble.
///
/// `Bootstrap` swaps Operating Instructions for Bootstrap Instructions
/// and skips identity files (they're being created this turn).
/// `Cron` keeps Operating Instructions and adds the Cron Delivery
/// Contract; identity files are still emitted. `Normal` is the
/// everyday worker/delivery/reflection path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PromptMode {
    Normal,
    Bootstrap,
    Cron,
}

/// Shell-escape a string for safe inclusion in an SSH remote command.
pub(crate) fn shell_escape(s: &str) -> String {
    shlex::try_quote(s)
        .expect("shlex::try_quote cannot fail for valid UTF-8")
        .into_owned()
}

fn sandbox_user_local_env_prelude(workdir: &str) -> &'static str {
    if workdir == super::sandbox_env::SANDBOX_ROOT {
        super::sandbox_env::INLINE_FALLBACK_SCRIPT
    } else {
        ""
    }
}

/// Prompt section: a file from disk that gets a markdown header.
struct PromptSection {
    filename: &'static str,
    header: &'static str,
}

/// Identity and config files included in the system prompt (normal mode).
const PROMPT_SECTIONS: &[PromptSection] = &[
    PromptSection {
        filename: "IDENTITY.md",
        header: "## Your Identity",
    },
    PromptSection {
        filename: "SOUL.md",
        header: "## Your Personality and Values",
    },
    PromptSection {
        filename: "USER.md",
        header: "## Your User",
    },
    PromptSection {
        filename: "TOOLS.md",
        header: "## Environment and Tools",
    },
];

/// Generate a shell script that assembles a composite system prompt and runs `claude -p`.
///
/// Parameterized by `root_path` — the directory containing agent .md files:
/// - Sandbox: `/sandbox`
/// - No-sandbox: absolute path to `agent_dir`
///
/// The script reads files from `root_path`, assembles them into `prompt_file`,
/// then runs claude from `workdir`.
// internal helper; refactor to a config struct is out of scope for this cleanup pass
#[allow(clippy::too_many_arguments)]
pub(crate) fn build_prompt_assembly_script(
    base_prompt: &str,
    mode: PromptMode,
    root_path: &str,
    prompt_file: &str,
    workdir: &str,
    claude_args: &[String],
    mcp_instructions: Option<&str>,
    memory_mode: Option<&MemoryMode>,
) -> String {
    let escaped_base = base_prompt.replace('\'', "'\\''");
    let escaped_args: Vec<String> = claude_args.iter().map(|a| shell_escape(a)).collect();
    let claude_cmd = escaped_args.join(" ");
    let sandbox_env_prelude = sandbox_user_local_env_prelude(workdir);

    let file_sections = if matches!(mode, PromptMode::Bootstrap) {
        let escaped_bootstrap = right_codegen::BOOTSTRAP_INSTRUCTIONS.replace('\'', "'\\''");
        format!("\nprintf '\\n## Bootstrap Instructions\\n'\nprintf '%s\\n' '{escaped_bootstrap}'")
    } else {
        let escaped_ops = right_codegen::OPERATING_INSTRUCTIONS.replace('\'', "'\\''");
        let mut sections =
            format!("\nprintf '\\n## Operating Instructions\\n'\nprintf '%s\\n' '{escaped_ops}'");

        if matches!(mode, PromptMode::Cron) {
            let escaped_cron = right_codegen::CRON_INSTRUCTIONS.replace('\'', "'\\''");
            sections.push_str(&format!("\nprintf '\\n'\nprintf '%s\\n' '{escaped_cron}'"));
        }

        for s in PROMPT_SECTIONS {
            let filename = s.filename;
            let header = s.header;
            sections.push_str(&format!(
                r#"
if [ -f {root_path}/{filename} ]; then
  printf '\n{header}\n'
  cat {root_path}/{filename}
  printf '\n'
fi"#
            ));
        }
        sections
    };

    let mcp_section = match mcp_instructions {
        Some(instr) => {
            let escaped = instr.replace('\'', "'\\''");
            format!("\nprintf '\\n'\nprintf '%s\\n' '{escaped}'")
        }
        None => String::new(),
    };

    let memory_section = if matches!(mode, PromptMode::Bootstrap) {
        String::new()
    } else {
        match memory_mode {
            Some(MemoryMode::File) => {
                let prefix = right_prompt_safety::memory_wrap_prefix().replace('\'', "'\\''");
                let suffix = right_prompt_safety::memory_wrap_suffix().replace('\'', "'\\''");
                // `head` is gated by `[ -s ... ]`; a TOCTOU failure here would
                // produce empty content inside the wrap, which is harmless.
                // sed escape neutralizes any literal `--- END EXTERNAL CONTENT ---`
                // an agent may have written into MEMORY.md (boundary-injection defense).
                // Note: relies on GNU/BSD `\xHH` interpretation in sed; on a strict-POSIX
                // sed it would fail open to literal `\xHH` text — unlikely in our
                // sandbox base images but worth knowing if the image ever changes.
                format!(
                    r#"
if [ -s {root_path}/MEMORY.md ]; then
  printf '\n## Long-Term Memory\n\n'
  printf '%s\n' '{prefix}'
  head -200 {root_path}/MEMORY.md 2>/dev/null \
    | sed 's|--- END EXTERNAL CONTENT ---|---\xe2\x80\x8b END EXTERNAL CONTENT ---|g'
  printf '%s\n' '{suffix}'
fi"#
                )
            }
            Some(MemoryMode::Hindsight {
                composite_memory_path,
            }) => format!(
                r#"
if [ -s {composite_memory_path} ]; then
  cat {composite_memory_path}
fi"#
            ),
            None => String::new(),
        }
    };

    format!(
        "{sandbox_env_prelude}\n{{ printf '{escaped_base}'\n{file_sections}\n{mcp_section}\n{memory_section}\n}} > {prompt_file}\ncd {workdir} && {claude_cmd} --system-prompt-file {prompt_file}"
    )
}

/// Format a composite-memory file body: bot-trusted label note,
/// ironclaw-wrapped content (untrusted), then bot-trusted status / bg
/// markers. Pure function — extracted for unit testing.
pub(crate) fn format_composite_memory(
    content: &str,
    label: &str,
    status_marker: Option<&str>,
    bg_marker: Option<&str>,
) -> String {
    let label_line = format!("[System: recalled memory context, {label}.]\n\n");
    let wrapped = right_prompt_safety::wrap_memory_for_prompt(content);
    let status_tail = status_marker
        .map(|m| format!("\n\n{m}"))
        .unwrap_or_default();
    let bg_tail = bg_marker.map(|m| format!("\n\n{m}")).unwrap_or_default();
    format!("{label_line}{wrapped}{status_tail}{bg_tail}")
}

/// Deploy pre-recalled content as composite-memory.md to host (and
/// sandbox if applicable).
pub(crate) async fn deploy_composite_memory(
    content: &str,
    label: &str,
    agent_dir: &std::path::Path,
    resolved_sandbox: Option<&str>,
    status_marker: Option<&str>,
    bg_marker: Option<&str>,
) -> Result<(), DeployError> {
    let body = format_composite_memory(content, label, status_marker, bg_marker);
    let host_path = agent_dir.join(".claude").join("composite-memory.md");
    tokio::fs::write(&host_path, &body)
        .await
        .map_err(DeployError::Write)?;
    if let Some(sandbox) = resolved_sandbox {
        right_openshell::openshell::upload_file(sandbox, &host_path, "/sandbox/.claude/")
            .await
            .map_err(|e| DeployError::Upload(format!("{e:#}")))?;
    }
    Ok(())
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum DeployError {
    #[error("write composite-memory.md: {0}")]
    Write(std::io::Error),
    #[error("upload composite-memory.md: {0}")]
    Upload(String),
}

/// Sandbox reference for best-effort remote cleanup in `remove_composite_memory`.
///
/// In sandbox mode the host copy of `composite-memory.md` is a staging file —
/// the live copy lives at `/sandbox/.claude/composite-memory.md` inside the
/// sandbox and is what the prompt-assembly script `cat`s. Removing only the
/// host file leaves stale recall content (and a stale `<memory-status>`
/// marker) in the sandbox that leaks into future system prompts.
pub(crate) struct SandboxRef<'a> {
    pub ssh_config: &'a std::path::Path,
    pub sandbox_name: &'a str,
}

/// Remove composite-memory.md from host disk and (if sandboxed) from the
/// sandbox. Best-effort: failures on the sandbox side are logged but not
/// propagated — blocking the bot on cleanup would be worse than a stale file.
pub(crate) async fn remove_composite_memory(
    agent_dir: &std::path::Path,
    sandbox: Option<SandboxRef<'_>>,
) {
    let host_path = agent_dir.join(".claude").join("composite-memory.md");
    let _ = tokio::fs::remove_file(&host_path).await;

    // In no-sandbox mode the host path IS the effective path the prompt
    // script reads — nothing else to clean up.
    let Some(sb) = sandbox else {
        return;
    };

    let ssh_host = right_openshell::openshell::ssh_host_for_sandbox(sb.sandbox_name);
    let mut cmd = tokio::process::Command::new("ssh");
    cmd.arg("-F").arg(sb.ssh_config);
    cmd.arg(&ssh_host);
    cmd.arg("--");
    cmd.arg("rm -f /sandbox/.claude/composite-memory.md");
    cmd.stdin(std::process::Stdio::null());
    cmd.stdout(std::process::Stdio::null());
    cmd.stderr(std::process::Stdio::piped());

    match cmd.output().await {
        Ok(out) if out.status.success() => {}
        Ok(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            tracing::warn!(
                sandbox = sb.sandbox_name,
                status = ?out.status,
                stderr = %stderr,
                "remove_composite_memory: sandbox rm failed (best-effort, continuing)"
            );
        }
        Err(e) => {
            tracing::warn!(
                sandbox = sb.sandbox_name,
                error = %e,
                "remove_composite_memory: ssh spawn failed (best-effort, continuing)"
            );
        }
    }
}

#[cfg(test)]
#[path = "prompt_tests.rs"]
mod tests;
