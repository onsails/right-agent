use crate::mcp::credentials::McpServerEntry;

/// Header emitted by [`generate_mcp_instructions_md`] even when no servers
/// have instructions. Used by callers to detect "header-only" responses.
pub const MCP_INSTRUCTIONS_HEADER: &str = "# MCP Server Instructions\n";

/// Generate MCP instructions markdown from registered MCP servers.
///
/// Only includes servers that have cached instructions (non-None).
/// Returns just the heading if no servers have instructions.
pub fn generate_mcp_instructions_md(servers: &[McpServerEntry]) -> String {
    let mut out = String::from(MCP_INSTRUCTIONS_HEADER);
    for server in servers {
        if let Some(ref instructions) = server.instructions {
            out.push_str(&format!("\n## {}\n\n{}\n", server.name, instructions));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_servers_returns_header_only() {
        let result = generate_mcp_instructions_md(&[]);
        assert_eq!(result, "# MCP Server Instructions\n");
    }

    #[test]
    fn servers_without_instructions_skipped() {
        let servers = vec![McpServerEntry {
            name: "notion".into(),
            url: "https://mcp.notion.com/mcp".into(),
            instructions: None,
            auth_type: None,
            auth_header: None,
            auth_token: None,
            refresh_token: None,
            token_endpoint: None,
            client_id: None,
            client_secret: None,
            expires_at: None,
        }];
        let result = generate_mcp_instructions_md(&servers);
        assert_eq!(result, "# MCP Server Instructions\n");
    }

    #[test]
    fn servers_with_instructions_included() {
        let servers = vec![McpServerEntry {
            name: "notion".into(),
            url: "https://mcp.notion.com/mcp".into(),
            instructions: Some("Search and update Notion pages.".into()),
            auth_type: None,
            auth_header: None,
            auth_token: None,
            refresh_token: None,
            token_endpoint: None,
            client_id: None,
            client_secret: None,
            expires_at: None,
        }];
        let result = generate_mcp_instructions_md(&servers);
        assert!(result.contains("## notion"));
        assert!(result.contains("Search and update Notion pages."));
    }

    #[test]
    fn mixed_servers_only_with_instructions() {
        let servers = vec![
            McpServerEntry {
                name: "composio".into(),
                url: "https://connect.composio.dev/mcp".into(),
                instructions: Some("Connect with 250+ apps.".into()),
                auth_type: None,
                auth_header: None,
                auth_token: None,
                refresh_token: None,
                token_endpoint: None,
                client_id: None,
                client_secret: None,
                expires_at: None,
            },
            McpServerEntry {
                name: "linear".into(),
                url: "https://mcp.linear.app/mcp".into(),
                instructions: None,
                auth_type: None,
                auth_header: None,
                auth_token: None,
                refresh_token: None,
                token_endpoint: None,
                client_id: None,
                client_secret: None,
                expires_at: None,
            },
            McpServerEntry {
                name: "notion".into(),
                url: "https://mcp.notion.com/mcp".into(),
                instructions: Some("Notion tools.".into()),
                auth_type: None,
                auth_header: None,
                auth_token: None,
                refresh_token: None,
                token_endpoint: None,
                client_id: None,
                client_secret: None,
                expires_at: None,
            },
        ];
        let result = generate_mcp_instructions_md(&servers);
        assert!(result.contains("## composio"));
        assert!(result.contains("## notion"));
        assert!(!result.contains("## linear"));
    }
}
