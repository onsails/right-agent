use std::path::PathBuf;

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
        mcp_config_path: None,
        soul_path: None,
        user_path: None,
        memory_path: None,
        agents_path: None,
        tools_path: None,
        bootstrap_path: None,
        heartbeat_path: None,
    }
}

#[test]
fn generates_sandbox_enabled_by_default() {
    let agent = make_test_agent("test-agent", None);
    let settings = generate_settings(&agent, false).unwrap();
    assert_eq!(settings["sandbox"]["enabled"], true);
    assert_eq!(settings["sandbox"]["autoAllowBashIfSandboxed"], true);
    assert_eq!(settings["sandbox"]["allowUnsandboxedCommands"], false);
    assert_eq!(settings["skipDangerousModePermissionPrompt"], true);
    assert_eq!(settings["spinnerTipsEnabled"], false);
    assert_eq!(settings["prefersReducedMotion"], true);
}

#[test]
fn includes_default_allow_write() {
    let agent = make_test_agent("test-agent", None);
    let settings = generate_settings(&agent, false).unwrap();

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
    let settings = generate_settings(&agent, false).unwrap();

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
    let settings = generate_settings(&agent, true).unwrap();

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
        allowed_domains: vec!["custom.example.com".to_string()],
        excluded_commands: vec!["docker".to_string()],
    };
    let config = AgentConfig {
        restart: RestartPolicy::OnFailure,
        max_restarts: 3,
        backoff_seconds: 5,
        start_prompt: None,
        model: None,
        sandbox: Some(overrides),
    };
    let agent = make_test_agent("test-agent", Some(config));
    let settings = generate_settings(&agent, false).unwrap();

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
    let settings = generate_settings(&agent, false).unwrap();

    assert!(
        settings["sandbox"].get("excludedCommands").is_none(),
        "excludedCommands should be omitted when empty, got: {:?}",
        settings["sandbox"].get("excludedCommands")
    );
}

#[test]
fn includes_telegram_plugin_when_mcp_present() {
    let mut agent = make_test_agent("test-agent", None);
    agent.mcp_config_path = Some(PathBuf::from("/fake/.mcp.json"));
    let settings = generate_settings(&agent, false).unwrap();

    assert_eq!(
        settings["enabledPlugins"]["telegram@claude-plugins-official"],
        true,
        "expected telegram plugin enabled"
    );
}

#[test]
fn omits_telegram_plugin_when_no_mcp() {
    let agent = make_test_agent("test-agent", None);
    let settings = generate_settings(&agent, false).unwrap();

    assert!(
        settings.get("enabledPlugins").is_none(),
        "enabledPlugins should be omitted without mcp_config_path"
    );
}

#[test]
fn includes_deny_read_security_defaults() {
    let agent = make_test_agent("test-agent", None);
    let settings = generate_settings(&agent, false).unwrap();

    let deny_read = settings["sandbox"]["filesystem"]["denyRead"]
        .as_array()
        .expect("denyRead should be an array");

    let expected = ["~/.ssh", "~/.aws", "~/.gnupg"];
    for path in &expected {
        assert!(
            deny_read.iter().any(|v| v == path),
            "missing denyRead path {path} in {deny_read:?}"
        );
    }
}
