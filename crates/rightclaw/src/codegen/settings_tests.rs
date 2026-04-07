use std::path::PathBuf;

use crate::agent::{AgentConfig, AgentDef, RestartPolicy};
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
fn generates_behavioral_flags() {
    let agent = make_test_agent("test-agent", None);
    let settings = generate_settings(&agent, None).unwrap();
    assert_eq!(settings["skipDangerousModePermissionPrompt"], true);
    assert_eq!(settings["spinnerTipsEnabled"], false);
    assert_eq!(settings["prefersReducedMotion"], true);
    assert_eq!(settings["autoMemoryEnabled"], false);
}

#[test]
fn no_sandbox_section() {
    let agent = make_test_agent("test-agent", None);
    let settings = generate_settings(&agent, None).unwrap();
    assert!(
        settings.get("sandbox").is_none(),
        "settings should not contain sandbox section, got: {:?}",
        settings.get("sandbox")
    );
}

#[test]
fn never_enables_telegram_plugin() {
    let config = AgentConfig {
        restart: RestartPolicy::OnFailure,
        max_restarts: 3,
        backoff_seconds: 5,
        model: None,
        sandbox: None,
        telegram_token: Some("tok".to_string()),
        allowed_chat_ids: vec![],
        env: std::collections::HashMap::new(),
    };
    let agent = make_test_agent("test-agent", Some(config));
    let settings = generate_settings(&agent, None).unwrap();
    assert!(settings.get("enabledPlugins").is_none());
}
