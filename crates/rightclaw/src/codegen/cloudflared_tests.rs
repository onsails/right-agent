use std::path::PathBuf;

use crate::codegen::cloudflared::generate_cloudflared_config;

#[test]
fn two_agents_produce_two_ingress_rules_plus_catch_all() {
    let agents = vec![
        (
            "alpha".to_string(),
            PathBuf::from("/home/user/.rightclaw/agents/alpha"),
        ),
        (
            "beta".to_string(),
            PathBuf::from("/home/user/.rightclaw/agents/beta"),
        ),
    ];
    let yaml = generate_cloudflared_config(&agents, "tunnel.example.com", None).unwrap();
    assert!(yaml.contains("/oauth/alpha/callback"), "missing alpha ingress: {yaml}");
    assert!(yaml.contains("/oauth/beta/callback"), "missing beta ingress: {yaml}");
    assert!(yaml.contains("http_status:404"), "missing catch-all: {yaml}");
}

#[test]
fn ingress_hostname_matches_tunnel_hostname() {
    let agents = vec![("myagent".to_string(), PathBuf::from("/tmp/agents/myagent"))];
    let yaml = generate_cloudflared_config(&agents, "my-tunnel.example.com", None).unwrap();
    assert!(yaml.contains("my-tunnel.example.com"), "tunnel URL missing from ingress: {yaml}");
}

#[test]
fn ingress_path_matches_oauth_callback_pattern() {
    let agents = vec![("bot-one".to_string(), PathBuf::from("/tmp/agents/bot-one"))];
    let yaml = generate_cloudflared_config(&agents, "t.example.com", None).unwrap();
    assert!(yaml.contains("path: /oauth/bot-one/callback"), "wrong callback path: {yaml}");
}

#[test]
fn ingress_service_is_unix_socket_in_agent_dir() {
    let agents = vec![(
        "myagent".to_string(),
        PathBuf::from("/home/user/.rightclaw/agents/myagent"),
    )];
    let yaml = generate_cloudflared_config(&agents, "t.example.com", None).unwrap();
    assert!(
        yaml.contains("unix:/home/user/.rightclaw/agents/myagent/oauth-callback.sock"),
        "wrong socket service: {yaml}"
    );
}

#[test]
fn catch_all_is_always_last_entry() {
    let agents = vec![("agent1".to_string(), PathBuf::from("/tmp/agents/agent1"))];
    let yaml = generate_cloudflared_config(&agents, "t.example.com", None).unwrap();
    let catch_all_pos = yaml.rfind("http_status:404").expect("catch-all not found");
    let agent_rule_pos = yaml.rfind("oauth-callback.sock").expect("agent rule not found");
    assert!(catch_all_pos > agent_rule_pos, "catch-all must be after all agent rules. yaml:\n{yaml}");
}

#[test]
fn zero_agents_still_produces_catch_all() {
    let agents: Vec<(String, std::path::PathBuf)> = vec![];
    let yaml = generate_cloudflared_config(&agents, "t.example.com", None).unwrap();
    assert!(yaml.contains("http_status:404"), "catch-all required even with 0 agents: {yaml}");
}

#[test]
fn https_scheme_stripped_from_hostname() {
    let agents = vec![("agent".to_string(), PathBuf::from("/tmp/agents/agent"))];
    let yaml = generate_cloudflared_config(&agents, "https://right.example.com", None).unwrap();
    assert!(yaml.contains("hostname: right.example.com"), "https:// scheme must be stripped: {yaml}");
    assert!(!yaml.contains("https://"), "https:// must not appear in output: {yaml}");
}

#[test]
fn http_scheme_stripped_from_hostname() {
    let agents = vec![("agent".to_string(), PathBuf::from("/tmp/agents/agent"))];
    let yaml = generate_cloudflared_config(&agents, "http://right.example.com", None).unwrap();
    assert!(yaml.contains("hostname: right.example.com"), "http:// scheme must be stripped: {yaml}");
}

// ---- Phase 38 credentials tests ----

#[test]
fn no_credentials_section_when_none() {
    let agents = vec![("agent".to_string(), PathBuf::from("/tmp/agents/agent"))];
    let yaml = generate_cloudflared_config(&agents, "right.example.com", None).unwrap();
    assert!(!yaml.contains("tunnel: "), "tunnel: field must be absent when no credentials: {yaml}");
    assert!(!yaml.contains("credentials-file:"), "credentials-file must be absent when no credentials: {yaml}");
}
