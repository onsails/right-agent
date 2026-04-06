use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::agent::{AgentConfig, AgentDef, RestartPolicy, SandboxOverrides};
use crate::codegen::generate_settings;

fn make_test_agent(name: &str, config: Option<AgentConfig>) -> AgentDef {
    AgentDef {
        name: name.to_owned(),
        path: PathBuf::from(format!("/home/user/.rightclaw/agents/{name}")),
        identity_path: PathBuf::from(format!(
            "/home/user/.rightclaw/agents/{name}/IDENTITY.md"
        )),
        config,
        soul_path: None,
        user_path: None,
        agents_path: None,
        tools_path: None,
        bootstrap_path: None,
        heartbeat_path: None,
    }
}

#[test]
fn generates_sandbox_enabled_by_default() {
    let agent = make_test_agent("test-agent", None);
    let settings = generate_settings(&agent, false, Path::new("/home/user"), None).unwrap();
    assert_eq!(settings["sandbox"]["enabled"], true);
    assert_eq!(settings["sandbox"]["autoAllowBashIfSandboxed"], true);
    assert_eq!(settings["sandbox"]["allowUnsandboxedCommands"], false);
    assert_eq!(settings["skipDangerousModePermissionPrompt"], true);
    assert_eq!(settings["spinnerTipsEnabled"], false);
    assert_eq!(settings["prefersReducedMotion"], true);
    assert_eq!(settings["autoMemoryEnabled"], false);
}

#[test]
fn includes_default_allow_write() {
    let agent = make_test_agent("test-agent", None);
    let settings = generate_settings(&agent, false, Path::new("/home/user"), None).unwrap();

    let allow_write = settings["sandbox"]["filesystem"]["allowWrite"]
        .as_array()
        .expect("allowWrite should be an array");
    assert!(
        allow_write
            .iter()
            .any(|v| v == "/home/user/.rightclaw/agents/test-agent"),
        "allowWrite should contain agent path, got: {allow_write:?}"
    );
}

#[test]
fn includes_default_allowed_domains() {
    let agent = make_test_agent("test-agent", None);
    let settings = generate_settings(&agent, false, Path::new("/home/user"), None).unwrap();

    let domains = settings["sandbox"]["network"]["allowedDomains"]
        .as_array()
        .expect("allowedDomains should be an array");

    let expected = [
        "api.anthropic.com",
        "github.com",
        "npmjs.org",
        "crates.io",
        "agentskills.io",
        "api.telegram.org",
    ];
    for domain in &expected {
        assert!(
            domains.iter().any(|v| v == domain),
            "missing domain {domain} in {domains:?}"
        );
    }
    assert_eq!(domains.len(), expected.len(), "unexpected extra domains");
}

#[test]
fn no_sandbox_disables_sandbox_only() {
    let agent = make_test_agent("test-agent", None);
    let settings = generate_settings(&agent, true, Path::new("/home/user"), None).unwrap();

    assert_eq!(settings["sandbox"]["enabled"], false);
    // Other settings still present
    assert_eq!(settings["skipDangerousModePermissionPrompt"], true);
    assert_eq!(settings["spinnerTipsEnabled"], false);
    assert_eq!(settings["prefersReducedMotion"], true);
    assert_eq!(settings["sandbox"]["autoAllowBashIfSandboxed"], true);
    assert_eq!(settings["sandbox"]["allowUnsandboxedCommands"], false);
}

#[test]
fn merges_user_overrides_with_defaults() {
    let overrides = SandboxOverrides {
        allow_write: vec!["/tmp/custom".to_string()],
        allow_read: vec![],
        allowed_domains: vec!["custom.example.com".to_string()],
        excluded_commands: vec!["docker".to_string()],
    };
    let config = AgentConfig {
        restart: RestartPolicy::OnFailure,
        max_restarts: 3,
        backoff_seconds: 5,
        model: None,
        sandbox: Some(overrides),
        telegram_token_file: None,
        telegram_token: None,
        allowed_chat_ids: vec![],
        env: std::collections::HashMap::new(),
    };
    let agent = make_test_agent("test-agent", Some(config));
    let settings = generate_settings(&agent, false, Path::new("/home/user"), None).unwrap();

    let allow_write = settings["sandbox"]["filesystem"]["allowWrite"]
        .as_array()
        .unwrap();
    // Default (agent dir) + user override
    assert!(allow_write.len() >= 2);
    assert!(
        allow_write.iter().any(|v| v == "/tmp/custom"),
        "user override /tmp/custom missing from {allow_write:?}"
    );
    assert!(
        allow_write
            .iter()
            .any(|v| v == "/home/user/.rightclaw/agents/test-agent"),
        "default agent dir missing from {allow_write:?}"
    );

    let domains = settings["sandbox"]["network"]["allowedDomains"]
        .as_array()
        .unwrap();
    assert!(
        domains.iter().any(|v| v == "custom.example.com"),
        "user domain missing from {domains:?}"
    );
    assert!(
        domains.iter().any(|v| v == "api.anthropic.com"),
        "default domain missing from {domains:?}"
    );

    assert_eq!(settings["sandbox"]["excludedCommands"][0], "docker");
}

#[test]
fn excluded_commands_omitted_when_empty() {
    let agent = make_test_agent("test-agent", None);
    let settings = generate_settings(&agent, false, Path::new("/home/user"), None).unwrap();

    assert!(
        settings["sandbox"].get("excludedCommands").is_none(),
        "excludedCommands should be omitted when empty, got: {:?}",
        settings["sandbox"].get("excludedCommands")
    );
}

/// Telegram plugin must NOT be enabled regardless of telegram config.
/// The native Rust bot (teloxide) owns Telegram; enabling CC's plugin creates a
/// getUpdates race on the same token, causing intermittent message drops.
#[test]
fn never_enables_telegram_plugin_in_settings() {
    // With telegram config
    let config = AgentConfig {
        restart: RestartPolicy::OnFailure,
        max_restarts: 3,
        backoff_seconds: 5,
        model: None,
        sandbox: None,
        telegram_token_file: None,
        telegram_token: Some("tok".to_string()),
        allowed_chat_ids: vec![],
        env: HashMap::new(),
    };
    let agent_with_tg = make_test_agent("test-agent", Some(config));
    let settings = generate_settings(&agent_with_tg, false, Path::new("/home/user"), None).unwrap();
    assert!(
        settings.get("enabledPlugins").is_none(),
        "enabledPlugins must be absent even when telegram token is configured — CC plugin races with native bot"
    );

    // Without telegram config
    let agent_no_tg = make_test_agent("test-agent", None);
    let settings = generate_settings(&agent_no_tg, false, Path::new("/home/user"), None).unwrap();
    assert!(
        settings.get("enabledPlugins").is_none(),
        "enabledPlugins should be omitted without telegram config"
    );
}

#[test]
fn includes_deny_read_security_defaults() {
    let agent = make_test_agent("test-agent", None);
    let settings = generate_settings(&agent, false, Path::new("/home/user"), None).unwrap();

    let deny_read = settings["sandbox"]["filesystem"]["denyRead"]
        .as_array()
        .expect("denyRead should be an array");

    // Must use absolute paths, not tilde-relative (HOME-05).
    let expected = [
        "/home/user/.ssh",
        "/home/user/.aws",
        "/home/user/.gnupg",
        "/home/user/",
    ];
    for path in &expected {
        assert!(
            deny_read.iter().any(|v| v == path),
            "missing denyRead path {path} in {deny_read:?}"
        );
    }
}

#[test]
fn deny_read_uses_absolute_paths_not_tilde() {
    let agent = make_test_agent("test-agent", None);
    let settings = generate_settings(&agent, false, Path::new("/home/user"), None).unwrap();
    let deny_read = settings["sandbox"]["filesystem"]["denyRead"]
        .as_array()
        .unwrap();
    for path in deny_read {
        let s = path.as_str().unwrap();
        assert!(!s.starts_with("~/"), "denyRead path should not use tilde: {s}");
        assert!(s.starts_with('/'), "denyRead path should be absolute: {s}");
    }
}

#[test]
fn includes_allow_read_with_agent_path() {
    let agent = make_test_agent("test-agent", None);
    let settings = generate_settings(&agent, false, Path::new("/home/user"), None).unwrap();
    let allow_read = settings["sandbox"]["filesystem"]["allowRead"]
        .as_array()
        .expect("allowRead should be an array");
    assert!(
        allow_read.iter().any(|v| v == "/home/user/.rightclaw/agents/test-agent"),
        "allowRead should contain agent path, got: {allow_read:?}"
    );
}

#[test]
fn merges_user_allow_read_overrides() {
    let overrides = SandboxOverrides {
        allow_write: vec![],
        allow_read: vec!["/data/shared".to_string()],
        allowed_domains: vec![],
        excluded_commands: vec![],
    };
    let config = AgentConfig {
        restart: RestartPolicy::OnFailure,
        max_restarts: 3,
        backoff_seconds: 5,
        model: None,
        sandbox: Some(overrides),
        telegram_token_file: None,
        telegram_token: None,
        allowed_chat_ids: vec![],
        env: std::collections::HashMap::new(),
    };
    let agent = make_test_agent("test-agent", Some(config));
    let settings = generate_settings(&agent, false, Path::new("/home/user"), None).unwrap();
    let allow_read = settings["sandbox"]["filesystem"]["allowRead"]
        .as_array()
        .unwrap();
    assert!(allow_read.iter().any(|v| v == "/data/shared"));
    assert!(allow_read.iter().any(|v| v == "/home/user/.rightclaw/agents/test-agent"));
}

#[test]
fn includes_fail_if_unavailable_unconditionally() {
    let agent = make_test_agent("test-agent", None);

    // With sandbox enabled
    let settings = generate_settings(&agent, false, Path::new("/home/user"), None).unwrap();
    assert_eq!(
        settings["sandbox"]["failIfUnavailable"], true,
        "failIfUnavailable must be true when sandbox is enabled"
    );

    // With sandbox disabled (--no-sandbox) — still present
    let settings = generate_settings(&agent, true, Path::new("/home/user"), None).unwrap();
    assert_eq!(
        settings["sandbox"]["failIfUnavailable"], true,
        "failIfUnavailable must be true even when sandbox is disabled"
    );
}

#[test]
fn injects_ripgrep_command_when_path_provided() {
    let agent = make_test_agent("test-agent", None);
    let rg = Some(PathBuf::from("/usr/bin/rg"));
    let settings = generate_settings(&agent, false, Path::new("/home/user"), rg).unwrap();

    assert_eq!(
        settings["sandbox"]["ripgrep"]["command"], "/usr/bin/rg",
        "ripgrep.command must be the absolute rg path"
    );
    assert_eq!(
        settings["sandbox"]["ripgrep"]["args"],
        serde_json::json!([]),
        "ripgrep.args must be an empty array"
    );
}

#[test]
fn omits_ripgrep_when_path_not_provided() {
    let agent = make_test_agent("test-agent", None);
    let settings = generate_settings(&agent, false, Path::new("/home/user"), None).unwrap();

    assert!(
        settings["sandbox"].get("ripgrep").is_none(),
        "ripgrep field must be absent when rg_path is None, got: {:?}",
        settings["sandbox"].get("ripgrep")
    );
}
