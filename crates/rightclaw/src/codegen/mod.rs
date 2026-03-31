pub mod claude_json;
pub mod mcp_config;
pub mod plugin;
pub mod process_compose;
pub mod settings;
pub mod skills;
pub mod system_prompt;
pub mod telegram;

pub use claude_json::{create_credential_symlink, create_plugins_symlink, generate_agent_claude_json};
pub use mcp_config::generate_mcp_config;
pub use plugin::{ensure_bun_installed, ensure_telegram_plugin_installed};
pub use process_compose::generate_process_compose;
pub use settings::generate_settings;
pub use skills::install_builtin_skills;
pub use system_prompt::generate_system_prompt;
pub use telegram::generate_telegram_channel_config;
