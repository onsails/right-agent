pub mod agent_def;
pub mod claude_json;
pub mod cloudflared;
pub mod contract;
pub mod mcp_config;
pub mod mcp_instructions;
pub mod pipeline;
pub mod plugin;
pub mod policy;
pub mod process_compose;

pub mod settings;
pub mod skills;
pub mod telegram;
pub use agent_def::{
    BG_CONTINUATION_SCHEMA_JSON, BOOTSTRAP_INSTRUCTIONS, BOOTSTRAP_SCHEMA_JSON, CRON_SCHEMA_JSON,
    OPERATING_INSTRUCTIONS, REPLY_SCHEMA_JSON, generate_system_prompt,
};
pub use claude_json::{create_credential_symlink, generate_agent_claude_json};
pub use mcp_config::generate_mcp_config;
pub use mcp_config::generate_mcp_config_http;
pub use mcp_instructions::generate_mcp_instructions_md;
pub use pipeline::run_agent_codegen;
pub use pipeline::run_single_agent_codegen;
pub use process_compose::{ProcessComposeConfig, generate_process_compose};
pub use settings::generate_settings;
pub use skills::install_builtin_skills;
