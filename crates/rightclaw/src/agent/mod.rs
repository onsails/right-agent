pub mod discovery;
pub mod types;

pub use discovery::{discover_agents, parse_agent_config, validate_agent_name};
pub use types::{AgentConfig, AgentDef, RestartPolicy};
