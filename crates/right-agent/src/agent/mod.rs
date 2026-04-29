pub mod allowlist;
pub mod destroy;
pub mod discovery;
pub mod register;
pub mod types;

pub use destroy::{DestroyOptions, DestroyResult, destroy_agent};
pub use discovery::{
    discover_agents, discover_single_agent, parse_agent_config, validate_agent_name,
};
pub use register::{RegisterOptions, RegisterResult, register_with_running_pc};
pub use types::{AgentConfig, AgentDef, RestartPolicy, SandboxConfig, SandboxMode};
