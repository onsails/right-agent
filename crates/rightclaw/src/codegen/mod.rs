pub mod agent_def;
pub mod claude_json;
pub mod cloudflared;
pub mod mcp_config;
pub mod pipeline;
pub mod plugin;
pub mod policy;
pub mod process_compose;

pub mod settings;
pub mod skills;
pub mod telegram;
pub mod tools;

pub use agent_def::{
    generate_agent_definition, generate_bootstrap_definition, generate_system_prompt,
    BOOTSTRAP_SCHEMA_JSON, CONTENT_MD_FILES, REPLY_SCHEMA_JSON,
};
pub use claude_json::{create_credential_symlink, generate_agent_claude_json};
pub use mcp_config::generate_mcp_config;
pub use mcp_config::generate_mcp_config_http;
pub use pipeline::run_agent_codegen;
pub use process_compose::{ProcessComposeConfig, generate_process_compose};
pub use settings::generate_settings;
pub use skills::install_builtin_skills;
pub use tools::generate_tools_md;
