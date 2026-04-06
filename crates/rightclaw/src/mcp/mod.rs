pub mod credentials;
pub mod detect;
pub mod oauth;
pub mod refresh;

/// Name of the built-in MCP server that rightclaw manages.
/// Protected from `/mcp remove` — required for core functionality (agent memory).
pub const PROTECTED_MCP_SERVER: &str = "rightmemory";
