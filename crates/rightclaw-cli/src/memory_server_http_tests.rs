use super::*;

#[test]
fn agent_token_map_insert_and_lookup() {
    let map: AgentTokenMap = Arc::new(RwLock::new(HashMap::new()));
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let mut w = map.write().await;
        w.insert(
            "test-token-123".to_string(),
            AgentInfo {
                name: "agent-a".to_string(),
                dir: PathBuf::from("/tmp/agents/agent-a"),
            },
        );
        drop(w);

        let r = map.read().await;
        let agent = r.get("test-token-123").unwrap();
        assert_eq!(agent.name, "agent-a");
        assert!(r.get("bad-token").is_none());
    });
}
