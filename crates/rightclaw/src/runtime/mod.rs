pub mod deps;
pub mod pc_client;
pub mod state;

pub use deps::verify_dependencies;
pub use pc_client::{MCP_HTTP_PORT, PC_PORT, PcClient, ProcessInfo};
pub use state::{AgentState, RuntimeState, read_state, write_state};

/// Generate a random 32-byte URL-safe base64 token for process-compose API auth.
pub fn generate_pc_api_token() -> String {
    use base64::Engine as _;
    use rand::Rng as _;
    let bytes: [u8; 32] = rand::rng().random();
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}
