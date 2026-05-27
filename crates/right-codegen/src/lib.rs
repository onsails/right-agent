#![warn(unreachable_pub)]

pub(crate) mod agent_def;
pub(crate) mod claude_json;
pub(crate) mod cloudflared;
pub mod contract;
pub(crate) mod mcp_config;
pub mod mcp_instructions;
pub(crate) mod pipeline;
pub mod policy;
pub(crate) mod process_compose;

pub(crate) mod settings;
pub mod skills;
pub use agent_def::{
    BG_CONTINUATION_SCHEMA_JSON, BOOTSTRAP_INSTRUCTIONS, BOOTSTRAP_SCHEMA_JSON, CRON_INSTRUCTIONS,
    CRON_SCHEMA_JSON, CURATOR_SYSTEM_PROMPT, OPERATING_INSTRUCTIONS, PROBE_WRITER_ANCHOR_TEMPLATE,
    PROBE_WRITER_INSTRUCTIONS, REPLY_SCHEMA_JSON, generate_system_prompt,
};
pub use claude_json::{create_credential_symlink, generate_agent_claude_json};
pub use mcp_config::generate_mcp_config;
pub use mcp_config::generate_mcp_config_http;
pub use mcp_instructions::generate_mcp_instructions_md;
pub use pipeline::run_single_agent_codegen;
pub use pipeline::{CodegenOutcome, run_agent_codegen};
pub use process_compose::{ProcessComposeConfig, generate_process_compose};
pub use settings::generate_settings;
pub use skills::{BUILTIN_SKILL_LEGACY_NAMES, BUILTIN_SKILL_NAMES, install_builtin_skills};

#[cfg(test)]
mod policy_provider_tests;
