use std::path::PathBuf;

use crate::cloudflared::{CloudflaredCredentials, generate_cloudflared_config};

fn fixture_creds() -> CloudflaredCredentials {
    CloudflaredCredentials {
        tunnel_uuid: "test-uuid".to_string(),
        credentials_file: PathBuf::from("/tmp/creds.json"),
    }
}

#[test]
fn two_agents_produce_two_ingress_rules_plus_catch_all() {
    let agents = vec![
        (
            "alpha".to_string(),
            PathBuf::from("/home/user/.right/agents/alpha"),
        ),
        (
            "beta".to_string(),
            PathBuf::from("/home/user/.right/agents/beta"),
        ),
    ];
    let yaml =
        generate_cloudflared_config(&agents, "tunnel.example.com", &fixture_creds()).unwrap();
    assert!(
        yaml.contains("/oauth/alpha/callback"),
        "missing alpha ingress: {yaml}"
    );
    assert!(
        yaml.contains("/oauth/beta/callback"),
        "missing beta ingress: {yaml}"
    );
    assert!(
        yaml.contains("http_status:404"),
        "missing catch-all: {yaml}"
    );
}

#[test]
fn ingress_hostname_matches_tunnel_hostname() {
    let agents = vec![("myagent".to_string(), PathBuf::from("/tmp/agents/myagent"))];
    let yaml =
        generate_cloudflared_config(&agents, "my-tunnel.example.com", &fixture_creds()).unwrap();
    assert!(
        yaml.contains("my-tunnel.example.com"),
        "tunnel URL missing from ingress: {yaml}"
    );
}

#[test]
fn ingress_path_matches_oauth_callback_pattern() {
    let agents = vec![("bot-one".to_string(), PathBuf::from("/tmp/agents/bot-one"))];
    let yaml = generate_cloudflared_config(&agents, "t.example.com", &fixture_creds()).unwrap();
    assert!(
        yaml.contains("path: /oauth/bot-one/callback"),
        "wrong callback path: {yaml}"
    );
}

#[test]
fn ingress_service_is_unix_socket_in_agent_dir() {
    let agents = vec![(
        "myagent".to_string(),
        PathBuf::from("/home/user/.right/agents/myagent"),
    )];
    let yaml = generate_cloudflared_config(&agents, "t.example.com", &fixture_creds()).unwrap();
    assert!(
        yaml.contains("unix:/home/user/.right/agents/myagent/bot.sock"),
        "wrong socket service: {yaml}"
    );
}

#[test]
fn catch_all_is_always_last_entry() {
    let agents = vec![("agent1".to_string(), PathBuf::from("/tmp/agents/agent1"))];
    let yaml = generate_cloudflared_config(&agents, "t.example.com", &fixture_creds()).unwrap();
    let catch_all_pos = yaml.rfind("http_status:404").expect("catch-all not found");
    let agent_rule_pos = yaml.rfind("bot.sock").expect("agent rule not found");
    assert!(
        catch_all_pos > agent_rule_pos,
        "catch-all must be after all agent rules. yaml:\n{yaml}"
    );
}

#[test]
fn zero_agents_still_produces_catch_all() {
    let agents: Vec<(String, std::path::PathBuf)> = vec![];
    let yaml = generate_cloudflared_config(&agents, "t.example.com", &fixture_creds()).unwrap();
    assert!(
        yaml.contains("http_status:404"),
        "catch-all required even with 0 agents: {yaml}"
    );
}

#[test]
fn https_scheme_stripped_from_hostname() {
    let agents = vec![("agent".to_string(), PathBuf::from("/tmp/agents/agent"))];
    let yaml = generate_cloudflared_config(&agents, "https://right.example.com", &fixture_creds())
        .unwrap();
    assert!(
        yaml.contains("hostname: right.example.com"),
        "https:// scheme must be stripped: {yaml}"
    );
    assert!(
        !yaml.contains("https://"),
        "https:// must not appear in output: {yaml}"
    );
}

#[test]
fn http_scheme_stripped_from_hostname() {
    let agents = vec![("agent".to_string(), PathBuf::from("/tmp/agents/agent"))];
    let yaml =
        generate_cloudflared_config(&agents, "http://right.example.com", &fixture_creds()).unwrap();
    assert!(
        yaml.contains("hostname: right.example.com"),
        "http:// scheme must be stripped: {yaml}"
    );
}

// ---- Phase 38 credentials tests ----

#[test]
fn credentials_section_emitted_with_credentials() {
    let creds = CloudflaredCredentials {
        tunnel_uuid: "abc-uuid".to_string(),
        credentials_file: PathBuf::from("/etc/cf-creds.json"),
    };
    let agents = vec![("agent".to_string(), PathBuf::from("/tmp/agents/agent"))];
    let yaml = generate_cloudflared_config(&agents, "right.example.com", &creds).unwrap();
    assert!(yaml.contains("tunnel: abc-uuid"));
    assert!(yaml.contains("credentials-file: /etc/cf-creds.json"));
}

#[test]
fn webhook_ingress_rule_per_agent() {
    let creds = fixture_creds();
    let agents = vec![
        (
            "alpha".to_string(),
            PathBuf::from("/home/user/.right/agents/alpha"),
        ),
        (
            "beta".to_string(),
            PathBuf::from("/home/user/.right/agents/beta"),
        ),
    ];
    let yaml = generate_cloudflared_config(&agents, "t.example.com", &creds).unwrap();
    assert!(
        yaml.contains("path: ^/tg/alpha$"),
        "missing alpha webhook ingress: {yaml}"
    );
    assert!(
        yaml.contains("path: ^/tg/beta$"),
        "missing beta webhook ingress: {yaml}"
    );
}

#[test]
fn webhook_ingress_appears_before_oauth_for_same_agent() {
    let creds = fixture_creds();
    let agents = vec![("alpha".to_string(), PathBuf::from("/tmp/agents/alpha"))];
    let yaml = generate_cloudflared_config(&agents, "t.example.com", &creds).unwrap();
    let tg_pos = yaml.find("^/tg/alpha$").expect("missing /tg rule");
    let oauth_pos = yaml
        .find("/oauth/alpha/callback")
        .expect("missing /oauth rule");
    assert!(
        tg_pos < oauth_pos,
        "/tg rule must come before /oauth rule for same agent (first match wins). yaml:\n{yaml}"
    );
}
