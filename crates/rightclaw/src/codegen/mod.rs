pub mod agent_def;
pub mod claude_json;
pub mod mcp_config;
pub mod plugin;
pub mod process_compose;
pub mod settings;
pub mod skills;
pub mod telegram;

pub use agent_def::{generate_agent_definition, REPLY_SCHEMA_JSON};
pub use claude_json::{create_credential_symlink, create_plugins_symlink, generate_agent_claude_json};
pub use mcp_config::generate_mcp_config;
pub use process_compose::generate_process_compose;
pub use settings::generate_settings;
pub use skills::install_builtin_skills;
