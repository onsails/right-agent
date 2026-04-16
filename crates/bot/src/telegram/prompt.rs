//! Shared prompt assembly for CC invocations (worker, cron, delivery).

/// Memory injection mode for prompt assembly.
pub(crate) enum MemoryMode {
    /// Inject MEMORY.md from agent directory.
    File,
    /// Inject composite memory file written by bot (Hindsight recall results).
    Hindsight { composite_memory_path: String },
}

/// Shell-escape a string for safe inclusion in an SSH remote command.
pub(crate) fn shell_escape(s: &str) -> String {
    shlex::try_quote(s).expect("shlex::try_quote cannot fail for valid UTF-8").into_owned()
}

/// Prompt section: a file from disk that gets a markdown header.
struct PromptSection {
    filename: &'static str,
    header: &'static str,
}

/// Identity and config files included in the system prompt (normal mode).
const PROMPT_SECTIONS: &[PromptSection] = &[
    PromptSection { filename: "IDENTITY.md", header: "## Your Identity" },
    PromptSection { filename: "SOUL.md", header: "## Your Personality and Values" },
    PromptSection { filename: "USER.md", header: "## Your User" },
    PromptSection { filename: "AGENTS.md", header: "## Agent Configuration" },
    PromptSection { filename: "TOOLS.md", header: "## Environment and Tools" },
];

/// Generate a shell script that assembles a composite system prompt and runs `claude -p`.
///
/// Parameterized by `root_path` — the directory containing agent .md files:
/// - Sandbox: `/sandbox`
/// - No-sandbox: absolute path to `agent_dir`
///
/// The script reads files from `root_path`, assembles them into `prompt_file`,
/// then runs claude from `workdir`.
pub(crate) fn build_prompt_assembly_script(
    base_prompt: &str,
    bootstrap_mode: bool,
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

    let file_sections = if bootstrap_mode {
        let escaped_bootstrap =
            rightclaw::codegen::BOOTSTRAP_INSTRUCTIONS.replace('\'', "'\\''");
        format!(
            "\nprintf '\\n## Bootstrap Instructions\\n'\nprintf '%s\\n' '{escaped_bootstrap}'"
        )
    } else {
        let escaped_ops =
            rightclaw::codegen::OPERATING_INSTRUCTIONS.replace('\'', "'\\''");
        let mut sections = format!(
            "\nprintf '\\n## Operating Instructions\\n'\nprintf '%s\\n' '{escaped_ops}'"
        );
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

    let memory_section = if bootstrap_mode {
        String::new()
    } else {
        match memory_mode {
            Some(MemoryMode::File) => format!(
                r#"
if [ -s {root_path}/MEMORY.md ]; then
  printf '\n## Long-Term Memory\n\n'
  head -200 {root_path}/MEMORY.md
fi"#
            ),
            Some(MemoryMode::Hindsight { composite_memory_path }) => format!(
                r#"
if [ -s {composite_memory_path} ]; then
  cat {composite_memory_path}
fi"#
            ),
            None => String::new(),
        }
    };

    format!(
        "{{ printf '{escaped_base}'\n{file_sections}\n{mcp_section}\n{memory_section}\n}} > {prompt_file}\ncd {workdir} && {claude_cmd} --system-prompt-file {prompt_file}"
    )
}

/// Deploy pre-recalled content as composite-memory.md to host (and sandbox if applicable).
///
/// Formats `content` into a `<memory-context>` fence with the given `label`,
/// writes to `agent_dir/.claude/composite-memory.md`, and uploads to sandbox.
pub(crate) async fn deploy_composite_memory(
    content: &str,
    label: &str,
    agent_dir: &std::path::Path,
    resolved_sandbox: Option<&str>,
) {
    let fenced = format!(
        "<memory-context>\n[System: recalled memory context, {label}.]\n\n{content}\n</memory-context>"
    );
    let host_path = agent_dir.join(".claude").join("composite-memory.md");
    if let Err(e) = tokio::fs::write(&host_path, &fenced).await {
        tracing::warn!("failed to write composite-memory.md: {e:#}");
    }
    if let Some(sandbox) = resolved_sandbox {
        if let Err(e) =
            rightclaw::openshell::upload_file(sandbox, &host_path, "/sandbox/.claude/").await
        {
            tracing::warn!("failed to upload composite-memory.md: {e:#}");
        }
    }
}

/// Remove composite-memory.md from host disk (best-effort).
pub(crate) async fn remove_composite_memory(agent_dir: &std::path::Path) {
    let host_path = agent_dir.join(".claude").join("composite-memory.md");
    let _ = tokio::fs::remove_file(&host_path).await;
}

/// Recall from Hindsight and deploy composite-memory.md to host (and sandbox if applicable).
///
/// Returns the recalled content string if successful, `None` otherwise.
/// On empty results, error, or timeout the host file is removed.
pub(crate) async fn recall_and_deploy_composite_memory(
    hs: &rightclaw::memory::hindsight::HindsightClient,
    query: &str,
    label: &str,
    agent_dir: &std::path::Path,
    resolved_sandbox: Option<&str>,
) -> Option<String> {
    match tokio::time::timeout(std::time::Duration::from_secs(5), hs.recall(query, None, None)).await {
        Ok(Ok(results)) if !results.is_empty() => {
            let content = rightclaw::memory::hindsight::join_recall_texts(&results);
            deploy_composite_memory(&content, label, agent_dir, resolved_sandbox).await;
            Some(content)
        }
        Ok(Ok(_)) => {
            remove_composite_memory(agent_dir).await;
            None
        }
        Ok(Err(e)) => {
            tracing::warn!("recall failed: {e:#}");
            remove_composite_memory(agent_dir).await;
            None
        }
        Err(_) => {
            tracing::warn!("recall timed out");
            remove_composite_memory(agent_dir).await;
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: build a script with sandbox-like paths for testing.
    fn test_script(base: &str, bootstrap: bool, args: &[String], mcp: Option<&str>) -> String {
        build_prompt_assembly_script(base, bootstrap, "/sandbox", "/tmp/rightclaw-system-prompt.md", "/sandbox", args, mcp, None)
    }

    #[test]
    fn script_bootstrap_includes_bootstrap_md() {
        let script = test_script("Base prompt", true, &["claude".into(), "-p".into()], None);
        assert!(script.contains("Bootstrap Instructions"), "must have Bootstrap Instructions header");
        assert!(script.contains("First-Time Setup"), "must contain compiled-in bootstrap content");
        assert!(!script.contains("cat /sandbox/IDENTITY.md"), "bootstrap must not cat IDENTITY.md");
        assert!(!script.contains("cat /sandbox/SOUL.md"), "bootstrap must not cat SOUL.md");
        assert!(script.contains("claude"), "must contain claude command");
        assert!(script.contains("--system-prompt-file"), "must pass --system-prompt-file");
    }

    #[test]
    fn script_normal_includes_all_identity_files() {
        let script = test_script("Base prompt", false, &["claude".into(), "-p".into()], None);
        assert!(script.contains("IDENTITY.md"));
        assert!(script.contains("SOUL.md"));
        assert!(script.contains("USER.md"));
        assert!(script.contains("AGENTS.md"));
        assert!(script.contains("TOOLS.md"));
        assert!(script.contains("Operating Instructions"), "must have compiled-in Operating Instructions");
        assert!(!script.contains("cat /sandbox/.claude/agents/BOOTSTRAP.md"), "normal must not cat BOOTSTRAP.md");
    }

    #[test]
    fn script_escapes_single_quotes_in_base() {
        let script = test_script("It's a test", true, &["claude".into()], None);
        // Single quote must be escaped for shell: ' → '\''
        assert!(!script.contains("It's"), "raw single quote must be escaped");
        assert!(script.contains("It"), "content must still be present");
    }

    #[test]
    fn script_shell_escapes_claude_args() {
        let script = test_script(
            "Base",
            false,
            &["claude".into(), "-p".into(), "--json-schema".into(), r#"{"type":"object"}"#.into()],
            None,
        );
        // JSON with braces and quotes must be shell-escaped
        assert!(script.contains("--json-schema"));
        assert!(script.contains("type"));
    }

    #[test]
    fn script_writes_to_prompt_file_and_uses_system_prompt_file() {
        let script = test_script("X", false, &["claude".into()], None);
        assert!(script.contains("/tmp/rightclaw-system-prompt.md"));
        assert!(script.contains("--system-prompt-file /tmp/rightclaw-system-prompt.md"));
    }

    #[test]
    fn script_custom_paths() {
        let script = build_prompt_assembly_script(
            "Base\n",
            false,
            "/home/agent",
            "/home/agent/.claude/composite-system-prompt.md",
            "/home/agent",
            &["claude".into(), "-p".into()],
            None,
            None,
        );
        assert!(script.contains("/home/agent/IDENTITY.md"), "must use custom root_path");
        assert!(script.contains("/home/agent/.claude/composite-system-prompt.md"), "must use custom prompt_file");
        assert!(script.contains("cd /home/agent"), "must cd to custom workdir");
    }

    #[test]
    fn script_bootstrap_mode_same_regardless_of_paths() {
        let script = build_prompt_assembly_script(
            "Base\n",
            true,
            "/home/agent",
            "/home/agent/.claude/composite-system-prompt.md",
            "/home/agent",
            &["claude".into()],
            None,
            None,
        );
        assert!(script.contains("## Bootstrap Instructions"));
        assert!(script.contains("First-Time Setup"), "must use compiled-in content");
        // Bootstrap never reads identity files regardless of path
        assert!(!script.contains("cat /home/agent/IDENTITY.md"), "bootstrap must not cat IDENTITY.md");
    }

    #[test]
    fn script_includes_mcp_instructions() {
        let script = test_script(
            "Base",
            false,
            &["claude".into()],
            Some("# MCP Server Instructions\n\n## composio\n\nConnect with 250+ apps.\n"),
        );
        assert!(script.contains("MCP Server Instructions"));
        assert!(script.contains("composio"));
        // Must use printf '%s' to prevent format string injection
        assert!(script.contains("printf '%s\\n'"));
    }

    #[test]
    fn script_none_mcp_instructions_omitted() {
        let script = test_script("Base", false, &["claude".into()], None);
        assert!(!script.contains("MCP Server Instructions"));
    }

    #[test]
    fn script_mcp_instructions_with_custom_paths() {
        let script = build_prompt_assembly_script(
            "Base\n",
            false,
            "/home/agent",
            "/home/agent/.claude/composite-system-prompt.md",
            "/home/agent",
            &["claude".into()],
            Some("# MCP Server Instructions\n\n## notion\n\nNotion tools.\n"),
            None,
        );
        assert!(script.contains("MCP Server Instructions"));
        assert!(script.contains("notion"));
        assert!(script.contains("Notion tools."));
    }

    #[test]
    fn script_bootstrap_uses_compiled_constant() {
        let script = test_script("Base prompt", true, &["claude".into(), "-p".into()], None);
        // Bootstrap uses compiled-in constant, NOT cat of file
        assert!(!script.contains("cat /sandbox"), "bootstrap must not cat any sandbox file");
        assert!(script.contains("First-Time Setup"), "must contain compiled-in bootstrap content");
        assert!(!script.contains("cat /sandbox/IDENTITY.md"), "bootstrap must not cat IDENTITY.md");
    }

    #[test]
    fn script_normal_has_operating_instructions_before_identity() {
        let script = test_script("Base prompt", false, &["claude".into()], None);
        let op_instr_pos = script.find("Operating Instructions").expect("must have Operating Instructions");
        let identity_pos = script.find("IDENTITY.md").expect("must have IDENTITY.md");
        assert!(op_instr_pos < identity_pos, "Operating Instructions must come before IDENTITY.md");
    }

    #[test]
    fn script_includes_memory_section_for_file_mode() {
        let script = build_prompt_assembly_script(
            "Base", false, "/sandbox", "/tmp/rightclaw-system-prompt.md", "/sandbox",
            &["claude".into()], None, Some(&MemoryMode::File),
        );
        assert!(script.contains("MEMORY.md"), "must reference MEMORY.md for file mode");
        assert!(script.contains("head -200"), "must truncate to 200 lines");
        assert!(script.contains("if [ -s"), "must check file exists and is non-empty");
    }

    #[test]
    fn script_includes_composite_memory_for_hindsight_mode() {
        let hs_mode = MemoryMode::Hindsight {
            composite_memory_path: "/sandbox/.claude/composite-memory.md".to_owned(),
        };
        let script = build_prompt_assembly_script(
            "Base", false, "/sandbox", "/tmp/rightclaw-system-prompt.md", "/sandbox",
            &["claude".into()], None, Some(&hs_mode),
        );
        assert!(script.contains("composite-memory.md"), "must reference composite-memory");
        assert!(script.contains("if [ -s"), "must check file exists");
    }

    #[test]
    fn script_no_memory_section_when_none() {
        let script = build_prompt_assembly_script(
            "Base", false, "/sandbox", "/tmp/rightclaw-system-prompt.md", "/sandbox",
            &["claude".into()], None, None,
        );
        assert!(!script.contains("MEMORY.md"));
        assert!(!script.contains("composite-memory"));
    }

    #[test]
    fn script_memory_section_is_last() {
        let script = build_prompt_assembly_script(
            "Base", false, "/sandbox", "/tmp/rightclaw-system-prompt.md", "/sandbox",
            &["claude".into()], Some("# MCP Instructions\n\n## composio\n"), Some(&MemoryMode::File),
        );
        let mcp_pos = script.rfind("MCP").unwrap();
        let memory_pos = script.rfind("MEMORY.md").unwrap();
        assert!(memory_pos > mcp_pos, "memory section must come after MCP instructions");
    }

    #[test]
    fn script_bootstrap_no_memory() {
        let script = build_prompt_assembly_script(
            "Base", true, "/sandbox", "/tmp/rightclaw-system-prompt.md", "/sandbox",
            &["claude".into()], None, Some(&MemoryMode::File),
        );
        assert!(!script.contains("MEMORY.md"), "bootstrap mode must not include memory");
    }
}
