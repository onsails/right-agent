pub use right_core::agent_types::*;

/// Write `agent.yaml::model` via line-oriented MergedRMW.
///
/// `Some(value)` replaces or appends a `model: "<value>"` line.
/// `None` removes the existing `model:` line, leaving the key absent
/// (CC will use its default model).
///
/// Delegates to [`right_codegen::contract::write_merged_rmw`]. Preserves
/// all unknown fields, comments, and blank lines. The value is always
/// double-quoted to handle YAML special characters (e.g. the `[` in
/// `claude-sonnet-4-6[1m]`).
pub fn write_agent_yaml_model(
    path: &std::path::Path,
    new_value: Option<&str>,
) -> miette::Result<()> {
    right_codegen::contract::write_merged_rmw(path, |existing| {
        let original = existing.unwrap_or("");

        // Walk lines, replacing or removing the first `^model:` line.
        // Must match top-level only — indentation = nested key (e.g. memory.model).
        let mut found = false;
        let mut out = String::with_capacity(original.len() + 64);
        for line in original.split_inclusive('\n') {
            let is_top_level_model = line
                .strip_prefix("model:")
                .map(|rest| {
                    rest.starts_with(' ')
                        || rest.starts_with('\t')
                        || rest.is_empty()
                        || rest.starts_with('\n')
                        || rest.starts_with('\r')
                })
                .unwrap_or(false);
            if is_top_level_model {
                found = true;
                if let Some(v) = new_value {
                    let needs_newline = line.ends_with('\n');
                    out.push_str(&format!(
                        "model: \"{}\"{}",
                        v.replace('\\', "\\\\").replace('"', "\\\""),
                        if needs_newline { "\n" } else { "" }
                    ));
                }
                // else: skip this line entirely (removal)
            } else {
                out.push_str(line);
            }
        }

        // Append if the key was absent and we have a new value.
        if !found && let Some(v) = new_value {
            if !out.ends_with('\n') {
                out.push('\n');
            }
            out.push_str(&format!(
                "model: \"{}\"\n",
                v.replace('\\', "\\\\").replace('"', "\\\""),
            ));
        }

        Ok(out)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_agent_types_are_available_from_core_and_agent_paths() {
        let config: right_core::agent_types::AgentConfig = AgentConfig::default();
        let _: AgentConfig = config;
        let _: right_core::agent_types::AgentDef = AgentDef {
            name: "demo".to_owned(),
            path: std::path::PathBuf::from("/agents/demo"),
            identity_path: std::path::PathBuf::from("/agents/demo/IDENTITY.md"),
            config: None,
            soul_path: None,
            user_path: None,
            tools_path: None,
            bootstrap_path: None,
            heartbeat_path: None,
        };
        let _: right_core::agent_types::WhisperModel = WhisperModel::Small;
    }

    #[test]
    fn agent_config_telegram_token_field() {
        let yaml = r#"telegram_token: "123:abc""#;
        let config: AgentConfig = serde_saphyr::from_str(yaml).unwrap();
        assert_eq!(config.telegram_token.as_deref(), Some("123:abc"));
    }

    #[test]
    fn agent_config_without_telegram_defaults_to_none() {
        let yaml = "{}";
        let config: AgentConfig = serde_saphyr::from_str(yaml).unwrap();
        assert_eq!(config.telegram_token, None);
    }

    #[test]
    fn agent_config_deserializes_full_yaml() {
        let yaml = r#"
restart: always
max_restarts: 10
backoff_seconds: 30
"#;
        let config: AgentConfig = serde_saphyr::from_str(yaml).unwrap();
        assert_eq!(config.restart, RestartPolicy::Always);
        assert_eq!(config.max_restarts, 10);
        assert_eq!(config.backoff_seconds, 30);
    }

    #[test]
    fn agent_config_deserializes_minimal_yaml_with_defaults() {
        let yaml = "{}";
        let config: AgentConfig = serde_saphyr::from_str(yaml).unwrap();
        assert_eq!(config.restart, RestartPolicy::Always);
        assert_eq!(config.max_restarts, 3);
        assert_eq!(config.backoff_seconds, 3);
    }

    #[test]
    fn agent_config_rejects_unknown_fields() {
        let yaml = r#"
restart: never
unknown_field: "should fail"
"#;
        let result: Result<AgentConfig, _> = serde_saphyr::from_str(yaml);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("unknown field"),
            "expected 'unknown field' in error: {err}"
        );
    }

    #[test]
    fn restart_policy_deserializes_never() {
        let yaml = r#"restart: never"#;
        let config: AgentConfig = serde_saphyr::from_str(yaml).unwrap();
        assert_eq!(config.restart, RestartPolicy::Never);
    }

    #[test]
    fn restart_policy_deserializes_on_failure() {
        let yaml = r#"restart: on_failure"#;
        let config: AgentConfig = serde_saphyr::from_str(yaml).unwrap();
        assert_eq!(config.restart, RestartPolicy::OnFailure);
    }

    #[test]
    fn restart_policy_deserializes_always() {
        let yaml = r#"restart: always"#;
        let config: AgentConfig = serde_saphyr::from_str(yaml).unwrap();
        assert_eq!(config.restart, RestartPolicy::Always);
    }

    #[test]
    fn agent_config_without_sandbox_section() {
        let yaml = "restart: never";
        let config: AgentConfig = serde_saphyr::from_str(yaml).unwrap();
        assert!(config.sandbox.is_none());
    }

    #[test]
    fn agent_config_allowed_chat_ids_deserializes_list() {
        let yaml = "allowed_chat_ids:\n  - 123456789\n  - -1001234567890";
        let config: AgentConfig = serde_saphyr::from_str(yaml).unwrap();
        assert_eq!(
            config.allowed_chat_ids,
            vec![123456789_i64, -1001234567890_i64]
        );
    }

    #[test]
    fn agent_config_allowed_chat_ids_defaults_to_empty() {
        let yaml = "{}";
        let config: AgentConfig = serde_saphyr::from_str(yaml).unwrap();
        assert!(config.allowed_chat_ids.is_empty());
    }

    #[test]
    fn agent_config_allowed_chat_ids_absent_does_not_reject() {
        let yaml = "restart: never\nmax_restarts: 5";
        let result: Result<AgentConfig, _> = serde_saphyr::from_str(yaml);
        assert!(result.is_ok());
        assert!(result.unwrap().allowed_chat_ids.is_empty());
    }

    #[test]
    fn agent_config_allowed_chat_ids_negative_values() {
        let yaml = "allowed_chat_ids:\n  - -100";
        let config: AgentConfig = serde_saphyr::from_str(yaml).unwrap();
        assert_eq!(config.allowed_chat_ids, vec![-100_i64]);
    }

    #[test]
    fn allowed_chat_ids_still_parses_for_migration() {
        let yaml = "allowed_chat_ids:\n  - 42\n  - -100\n";
        let config: AgentConfig = serde_saphyr::from_str(yaml).unwrap();
        assert_eq!(config.allowed_chat_ids, vec![42, -100]);
    }

    #[test]
    fn network_policy_defaults_to_permissive() {
        let yaml = "{}";
        let config: AgentConfig = serde_saphyr::from_str(yaml).unwrap();
        assert_eq!(config.network_policy, NetworkPolicy::Permissive);
    }

    #[test]
    fn network_policy_deserializes_restrictive() {
        let yaml = "network_policy: restrictive";
        let config: AgentConfig = serde_saphyr::from_str(yaml).unwrap();
        assert_eq!(config.network_policy, NetworkPolicy::Restrictive);
    }

    #[test]
    fn network_policy_deserializes_permissive() {
        let yaml = "network_policy: permissive";
        let config: AgentConfig = serde_saphyr::from_str(yaml).unwrap();
        assert_eq!(config.network_policy, NetworkPolicy::Permissive);
    }

    #[test]
    fn agent_config_with_attachments_section() {
        let yaml = r#"
attachments:
  retention_days: 14
"#;
        let config: AgentConfig = serde_saphyr::from_str(yaml).unwrap();
        assert_eq!(config.attachments.retention_days, 14);
    }

    #[test]
    fn agent_config_default_attachments() {
        let yaml = "";
        let config: AgentConfig = serde_saphyr::from_str(yaml).unwrap();
        assert_eq!(config.attachments.retention_days, 7);
    }

    #[test]
    fn sandbox_config_mode_openshell_with_policy() {
        let yaml = r#"
sandbox:
  mode: openshell
  policy_file: policy.yaml
"#;
        let config: AgentConfig = serde_saphyr::from_str(yaml).unwrap();
        let sandbox = config.sandbox.unwrap();
        assert_eq!(sandbox.mode, SandboxMode::Openshell);
        assert_eq!(
            sandbox.policy_file.as_deref(),
            Some(std::path::Path::new("policy.yaml"))
        );
    }

    #[test]
    fn sandbox_config_mode_none() {
        let yaml = r#"
sandbox:
  mode: none
"#;
        let config: AgentConfig = serde_saphyr::from_str(yaml).unwrap();
        let sandbox = config.sandbox.unwrap();
        assert_eq!(sandbox.mode, SandboxMode::None);
        assert!(sandbox.policy_file.is_none());
    }

    #[test]
    fn sandbox_config_defaults_to_openshell() {
        let yaml = "sandbox: {}";
        let config: AgentConfig = serde_saphyr::from_str(yaml).unwrap();
        let sandbox = config.sandbox.unwrap();
        assert_eq!(sandbox.mode, SandboxMode::Openshell);
    }

    #[test]
    fn sandbox_config_rejects_unknown_mode() {
        let yaml = r#"
sandbox:
  mode: docker
"#;
        let result: Result<AgentConfig, _> = serde_saphyr::from_str(yaml);
        assert!(result.is_err());
    }

    #[test]
    fn sandbox_config_rejects_old_allow_write_field() {
        let yaml = r#"
sandbox:
  allow_write:
    - "/tmp"
"#;
        let result: Result<AgentConfig, _> = serde_saphyr::from_str(yaml);
        assert!(
            result.is_err(),
            "old SandboxOverrides fields must be rejected"
        );
    }

    #[test]
    fn agent_config_without_sandbox_defaults_mode_openshell() {
        let yaml = "{}";
        let config: AgentConfig = serde_saphyr::from_str(yaml).unwrap();
        // sandbox is None — effective mode should be openshell (tested via helper)
        assert!(config.sandbox.is_none());
    }

    #[test]
    fn sandbox_config_with_name() {
        let yaml = r#"
sandbox:
  mode: openshell
  policy_file: policy.yaml
  name: "rightclaw-brain-20260415-1430"
"#;
        let config: AgentConfig = serde_saphyr::from_str(yaml).unwrap();
        let sb = config.sandbox.unwrap();
        assert_eq!(sb.name.as_deref(), Some("rightclaw-brain-20260415-1430"));
    }

    #[test]
    fn sandbox_config_without_name_is_none() {
        let yaml = r#"
sandbox:
  mode: openshell
  policy_file: policy.yaml
"#;
        let config: AgentConfig = serde_saphyr::from_str(yaml).unwrap();
        let sb = config.sandbox.unwrap();
        assert!(sb.name.is_none());
    }

    #[test]
    fn write_agent_yaml_model_appends_when_absent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("agent.yaml");
        std::fs::write(&path, "restart: never\nmax_restarts: 5\n").unwrap();

        super::write_agent_yaml_model(&path, Some("claude-sonnet-4-6")).unwrap();

        let result = std::fs::read_to_string(&path).unwrap();
        assert!(
            result.contains("restart: never"),
            "preserve existing fields:\n{result}"
        );
        assert!(
            result.contains("max_restarts: 5"),
            "preserve existing fields:\n{result}"
        );
        assert!(
            result.contains("model: \"claude-sonnet-4-6\""),
            "append model when absent:\n{result}"
        );
        let parsed: AgentConfig = serde_saphyr::from_str(&result).unwrap();
        assert_eq!(parsed.model.as_deref(), Some("claude-sonnet-4-6"));
        assert_eq!(parsed.max_restarts, 5);
    }

    #[test]
    fn write_agent_yaml_model_replaces_existing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("agent.yaml");
        std::fs::write(&path, "restart: never\nmodel: sonnet\nmax_restarts: 5\n").unwrap();

        super::write_agent_yaml_model(&path, Some("claude-haiku-4-5")).unwrap();

        let result = std::fs::read_to_string(&path).unwrap();
        assert!(
            !result.contains("model: sonnet"),
            "old value must be gone:\n{result}"
        );
        assert!(
            result.contains("model: \"claude-haiku-4-5\""),
            "new value must be present:\n{result}"
        );
        let restart_pos = result.find("restart:").unwrap();
        let model_pos = result.find("model:").unwrap();
        assert!(restart_pos < model_pos, "field order preserved:\n{result}");
    }

    #[test]
    fn write_agent_yaml_model_removes_when_none() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("agent.yaml");
        std::fs::write(
            &path,
            "restart: never\nmodel: \"claude-sonnet-4-6\"\nmax_restarts: 5\n",
        )
        .unwrap();

        super::write_agent_yaml_model(&path, None).unwrap();

        let result = std::fs::read_to_string(&path).unwrap();
        assert!(!result.contains("model:"), "model line removed:\n{result}");
        assert!(result.contains("restart: never"));
        assert!(result.contains("max_restarts: 5"));
        let parsed: AgentConfig = serde_saphyr::from_str(&result).unwrap();
        assert!(parsed.model.is_none());
    }

    #[test]
    fn write_agent_yaml_model_none_when_already_absent_is_noop_safe() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("agent.yaml");
        let original = "restart: never\nmax_restarts: 5\n";
        std::fs::write(&path, original).unwrap();

        super::write_agent_yaml_model(&path, None).unwrap();

        let result = std::fs::read_to_string(&path).unwrap();
        assert!(result.contains("restart: never"));
        assert!(!result.contains("model:"));
    }

    #[test]
    fn write_agent_yaml_model_preserves_comments_and_blank_lines() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("agent.yaml");
        std::fs::write(
            &path,
            "# Agent config\nrestart: never\n\n# Restart policy bump\nmax_restarts: 5\n",
        )
        .unwrap();

        super::write_agent_yaml_model(&path, Some("claude-haiku-4-5")).unwrap();

        let result = std::fs::read_to_string(&path).unwrap();
        assert!(
            result.contains("# Agent config"),
            "leading comment preserved:\n{result}"
        );
        assert!(
            result.contains("# Restart policy bump"),
            "interior comment preserved:\n{result}"
        );
    }

    #[test]
    fn write_agent_yaml_model_value_with_brackets() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("agent.yaml");
        std::fs::write(&path, "restart: never\n").unwrap();

        super::write_agent_yaml_model(&path, Some("claude-sonnet-4-6[1m]")).unwrap();

        let result = std::fs::read_to_string(&path).unwrap();
        assert!(
            result.contains("model: \"claude-sonnet-4-6[1m]\""),
            "bracketed value double-quoted:\n{result}"
        );
        let parsed: AgentConfig = serde_saphyr::from_str(&result).unwrap();
        assert_eq!(parsed.model.as_deref(), Some("claude-sonnet-4-6[1m]"));
    }
}

#[cfg(test)]
mod stt_config_tests {
    use super::*;

    #[test]
    fn stt_config_defaults_when_missing() {
        let yaml = "";
        let cfg: AgentConfig = serde_saphyr::from_str(yaml).unwrap();
        assert!(
            !cfg.stt.enabled,
            "default must be false to grandfather existing agents"
        );
        assert_eq!(cfg.stt.model, WhisperModel::Small);
    }

    #[test]
    fn pre_existing_yaml_without_stt_block_defaults_to_disabled() {
        // Simulates an agent.yaml from before the STT feature shipped:
        // it has other fields but no stt: block.
        let yaml = "telegram_token: \"x\"\nmodel: sonnet\n";
        let cfg: AgentConfig = serde_saphyr::from_str(yaml).unwrap();
        assert!(
            !cfg.stt.enabled,
            "existing agents without stt: block must NOT be silently enabled"
        );
    }

    #[test]
    fn stt_config_explicit_yaml_roundtrip() {
        let yaml = "\
stt:
  enabled: false
  model: tiny
";
        let cfg: AgentConfig = serde_saphyr::from_str(yaml).unwrap();
        assert!(!cfg.stt.enabled);
        assert_eq!(cfg.stt.model, WhisperModel::Tiny);
    }

    #[test]
    fn stt_config_large_v3_kebab_case() {
        let yaml = "\
stt:
  model: large-v3
";
        let cfg: AgentConfig = serde_saphyr::from_str(yaml).unwrap();
        assert_eq!(cfg.stt.model, WhisperModel::LargeV3);
    }

    #[test]
    fn stt_config_invalid_model_errors() {
        let yaml = "\
stt:
  model: huge
";
        let result: Result<AgentConfig, _> = serde_saphyr::from_str(yaml);
        assert!(result.is_err());
    }

    #[test]
    fn whisper_model_filename() {
        assert_eq!(WhisperModel::Tiny.filename(), "ggml-tiny.bin");
        assert_eq!(WhisperModel::Base.filename(), "ggml-base.bin");
        assert_eq!(WhisperModel::Small.filename(), "ggml-small.bin");
        assert_eq!(WhisperModel::Medium.filename(), "ggml-medium.bin");
        assert_eq!(WhisperModel::LargeV3.filename(), "ggml-large-v3.bin");
    }

    #[test]
    fn whisper_model_download_url_is_huggingface() {
        let url = WhisperModel::Small.download_url();
        assert!(url.starts_with("https://huggingface.co/ggerganov/whisper.cpp/"));
        assert!(url.ends_with("ggml-small.bin"));
    }

    #[test]
    fn whisper_model_approx_size_mb_is_sane() {
        assert!(WhisperModel::Tiny.approx_size_mb() < 100);
        assert!(WhisperModel::Small.approx_size_mb() < 600);
        assert!(WhisperModel::LargeV3.approx_size_mb() > 2000);
    }
}
