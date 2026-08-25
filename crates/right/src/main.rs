use std::path::{Path, PathBuf};
use std::process::Stdio;

use clap::{Parser, Subcommand};

pub(crate) mod aggregator;
pub(crate) mod db_owner;
pub(crate) mod db_repair;
pub(crate) mod internal_api;
pub(crate) mod internal_api_db;
#[cfg(test)]
mod internal_api_db_tests;
pub(crate) mod internal_api_providers;
pub(crate) mod learning;
pub(crate) mod mcp_persistence;
pub(crate) mod migrate_sandbox;
pub(crate) mod progress;
mod restore;
pub(crate) mod retain_owner;
pub(crate) mod right_backend;
pub(crate) mod runtime_builder;
pub(crate) mod runtime_quiescence;
mod wizard;

/// Source-of-truth list for every interactive prompt label rendered from
/// `crates/right/src/main.rs`. Mirrors `wizard::PROMPT_LABELS` /
/// `right_agent::init::PROMPT_LABELS` — the brand voice regression test
/// (`voice_pass_main`) walks this list and the `voice_pass.rs` integration
/// test on `right-agent` does the same for those crates. When you add or
/// edit an `inquire` prompt in this file, update this array — failure to do
/// so is caught by the test.
#[cfg(test)]
pub(crate) const MAIN_PROMPT_LABELS: &[&str] = &[
    // cmd_agent_init: existing-agent override path
    "how to initialize this agent?",
    "create fresh",
    "restore from backup",
    "backup directory path:",
    "restore binding mode:",
    "preserve source bindings",
    "rebind to target",
    "set memory bank id",
    "memory bank id:",
    // resolve_claude_setup_token
    "claude setup token:",
    // cmd_agent_destroy
    "create backup before destroying?",
    // cmd_agent_destroy: dynamic confirm — agent_name varies, prefix is the static portion
    "permanently destroy agent '",
    // cmd_agent_rebootstrap: dynamic confirm — agent_name varies, prefix is the static portion
    "rebootstrap agent '",
];

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod voice_pass_main {
    //! Brand voice regression for `MAIN_PROMPT_LABELS`. Mirrors the inline
    //! `wizard::voice_pass` block and the integration test
    //! `crates/right-agent/tests/voice_pass.rs`.

    use super::MAIN_PROMPT_LABELS;

    const ALLOWED_PROPER_NOUNS: &[&str] = &[
        "HINDSIGHT_API_KEY",
        "RIGHT_TG_TOKEN",
        "MEMORY.md",
        "@BotFather",
        "@userinfobot",
    ];

    #[test]
    fn main_prompt_labels_are_lowercase_first() {
        for label in MAIN_PROMPT_LABELS {
            let trimmed = label.trim_start();
            if trimmed.is_empty() {
                continue;
            }
            let first = trimmed.chars().next().unwrap();
            if !first.is_alphabetic() {
                continue;
            }
            let starts_with_proper = ALLOWED_PROPER_NOUNS
                .iter()
                .any(|noun| trimmed.starts_with(noun));
            if starts_with_proper {
                continue;
            }
            assert!(
                first.is_lowercase(),
                "prompt label must be lowercase-first (or start with an allowed proper noun): {label:?}"
            );
        }
    }

    #[test]
    fn main_prompt_labels_have_no_exclamation_marks() {
        for label in MAIN_PROMPT_LABELS {
            assert!(
                !label.contains('!'),
                "prompt label must not contain '!': {label:?}"
            );
        }
    }
}

#[cfg(test)]
mod cli_parse_tests {
    use clap::{CommandFactory, Parser};
    use serde_json::json;

    use super::{
        AgentCommands, AgentProvidersCommands, Cli, Commands, build_provider_create_generic_arg,
    };

    #[test]
    fn restore_binding_flags_require_from_backup() {
        let err =
            match Cli::try_parse_from(["right", "agent", "init", "clone", "--rebind-to-target"]) {
                Ok(_) => panic!("restore binding flags must require --from-backup"),
                Err(err) => err,
            };

        assert_eq!(err.kind(), clap::error::ErrorKind::MissingRequiredArgument);
    }

    /// The config parser refuses to start an unmigrated agent and names this
    /// command verbatim, so the command must exist under exactly that name.
    #[test]
    fn migrate_sandbox_is_spelled_the_way_the_config_error_promises() {
        let cli = Cli::try_parse_from(["right", "agent", "migrate-sandbox", "finance"])
            .expect("`right agent migrate-sandbox <agent>` must parse");
        match cli.command {
            Commands::Agent {
                command: AgentCommands::MigrateSandbox { name },
            } => assert_eq!(name, "finance"),
            _ => panic!("migrate-sandbox did not parse into its own subcommand"),
        }
        assert!(
            right_agent_config::OPENSHELL_UNMIGRATED.contains("right agent migrate-sandbox"),
            "the rejection message must point at the command that fixes it"
        );
    }

    #[test]
    fn db_repair_requires_one_or_more_agent_names() {
        let cli = Cli::try_parse_from(["right", "agent", "db-repair", "right", "riskoff"])
            .expect("`right agent db-repair <name>...` must parse");
        match cli.command {
            Commands::Agent {
                command: AgentCommands::DbRepair { names },
            } => assert_eq!(names, ["right", "riskoff"]),
            _ => panic!("db-repair did not parse into its own subcommand"),
        }

        let err = Cli::try_parse_from(["right", "agent", "db-repair"])
            .err()
            .expect("db-repair without names must be rejected by clap");
        assert_eq!(err.kind(), clap::error::ErrorKind::MissingRequiredArgument);
    }

    #[test]
    fn restore_binding_flags_conflict() {
        let mut command = Cli::command();
        let err = command
            .try_get_matches_from_mut([
                "right",
                "agent",
                "init",
                "clone",
                "--from-backup",
                "/tmp/backup",
                "--preserve-source-bindings",
                "--rebind-to-target",
            ])
            .expect_err("restore binding flags must be mutually exclusive");

        assert_eq!(err.kind(), clap::error::ErrorKind::ArgumentConflict);
    }

    #[test]
    fn claude_setup_token_is_not_an_init_argument() {
        for args in [
            vec!["right", "init", "--claude-setup-token", "secret"],
            vec![
                "right",
                "agent",
                "init",
                "agent-a",
                "--claude-setup-token",
                "secret",
            ],
        ] {
            let err = match Cli::try_parse_from(args) {
                Ok(_) => panic!("Claude setup token must never be accepted on argv"),
                Err(error) => error,
            };
            assert_eq!(err.kind(), clap::error::ErrorKind::UnknownArgument);
        }
    }

    #[test]
    fn claude_setup_token_is_absent_from_init_help() {
        let mut command = Cli::command();
        for args in [
            ["right", "init", "--help"].as_slice(),
            ["right", "agent", "init", "agent-a", "--help"].as_slice(),
        ] {
            let err = command
                .try_get_matches_from_mut(args)
                .expect_err("help must exit through clap");
            assert_eq!(err.kind(), clap::error::ErrorKind::DisplayHelp);
            assert!(!err.to_string().contains("claude-setup-token"));
        }
    }

    #[test]
    fn memory_bank_id_rejects_empty_value() {
        let err = match Cli::try_parse_from([
            "right",
            "agent",
            "init",
            "clone",
            "--from-backup",
            "/tmp/backup",
            "--memory-bank-id",
            "   ",
        ]) {
            Ok(_) => panic!("--memory-bank-id must reject whitespace-only values"),
            Err(err) => err,
        };

        assert_eq!(err.kind(), clap::error::ErrorKind::ValueValidation);
    }

    #[test]
    fn agent_skill_list_still_parses() {
        Cli::try_parse_from(["right", "agent", "skill", "list"])
            .expect("agent skill list must remain available");
    }

    #[test]
    fn providers_add_accepts_repeated_upstream_host_values() {
        let cli = Cli::try_parse_from([
            "right",
            "agent",
            "providers",
            "add",
            "fal-agent",
            "--credential",
            "secret",
            "--env-var",
            "FAL_KEY",
            "--upstream-host",
            "fal.run",
            "--upstream-host",
            "queue.fal.run",
        ])
        .expect("providers add must parse repeated upstream hosts");

        let upstream_host = match cli.command {
            Commands::Agent {
                command:
                    AgentCommands::Providers {
                        command: AgentProvidersCommands::Add { upstream_host, .. },
                    },
            } => upstream_host,
            _ => panic!("expected agent providers add command"),
        };

        assert_eq!(
            upstream_host,
            vec!["fal.run".to_string(), "queue.fal.run".to_string()]
        );
    }

    #[test]
    fn providers_add_accepts_hidden_legacy_header_name_but_omits_it_from_request() {
        let cli = Cli::try_parse_from([
            "right",
            "agent",
            "providers",
            "add",
            "fal-agent",
            "--credential",
            "secret",
            "--env-var",
            "FAL_KEY",
            "--upstream-host",
            "fal.run",
            "--upstream-path-prefix",
            "/v1",
            "--header-name",
            "X-Fal-Key",
        ])
        .expect("legacy header-name flag must keep parsing");

        let (upstream_host, upstream_path_prefix, header_name, env_var) = match cli.command {
            Commands::Agent {
                command:
                    AgentCommands::Providers {
                        command:
                            AgentProvidersCommands::Add {
                                upstream_host,
                                upstream_path_prefix,
                                header_name,
                                env_var,
                                ..
                            },
                    },
            } => (upstream_host, upstream_path_prefix, header_name, env_var),
            _ => panic!("expected agent providers add command"),
        };

        assert_eq!(header_name.as_deref(), Some("X-Fal-Key"));

        let generic = build_provider_create_generic_arg(
            "generic",
            &upstream_host,
            upstream_path_prefix.as_deref(),
            env_var.as_deref(),
        )
        .expect("generic arg should build")
        .expect("generic type should produce generic arg");
        let req = right_mcp::internal_client::ProviderCreateRequest {
            agent: "fal-agent",
            type_: "generic",
            label: None,
            credential: "secret",
            generic: Some(generic),
        };
        let body = serde_json::to_value(req).expect("request should serialize");

        assert_eq!(body["generic"]["upstream_hosts"], json!(["fal.run"]));
        assert!(
            body["generic"].get("header_name").is_none(),
            "legacy header_name must not be sent to the internal API: {body}"
        );
    }

    #[test]
    fn agent_mode_accepts_topic_and_group_forms() {
        Cli::try_parse_from([
            "right",
            "agent",
            "mode",
            "clone",
            "-100123",
            "--thread-id",
            "8",
            "all",
        ])
        .expect("agent mode must accept topic form with negative group chat ID");

        Cli::try_parse_from([
            "right",
            "agent",
            "mode",
            "clone",
            "-100123",
            "--group",
            "addressed",
        ])
        .expect("agent mode must accept group form with negative group chat ID");
    }

    #[test]
    fn agent_mode_accepts_value_for_runtime_validation() {
        Cli::try_parse_from(["right", "agent", "mode", "clone", "-100123", "bogus"])
            .expect("agent mode value validation is handled by runtime code");
    }

    #[test]
    fn agent_mode_help_lists_topic_group_and_value() {
        let mut command = Cli::command();
        let err = command
            .try_get_matches_from_mut(["right", "agent", "mode", "--help"])
            .expect_err("agent mode --help must render clap help");

        assert_eq!(err.kind(), clap::error::ErrorKind::DisplayHelp);
        assert_eq!(err.exit_code(), 0);

        let help = err.to_string();
        assert!(help.contains("--thread-id"));
        assert!(help.contains("--group"));
        assert!(help.contains("<VALUE>"));
        assert!(help.contains("One of: addressed | all | clear"));
    }

    #[test]
    fn up_accepts_non_interactive_flag() {
        let cli = Cli::try_parse_from(["right", "up", "--non-interactive", "--agents", "a,b"])
            .expect("up readiness flags must parse");
        let Commands::Up {
            agents,
            non_interactive,
            ..
        } = cli.command
        else {
            panic!("expected up command");
        };
        assert!(non_interactive);
        assert_eq!(agents, Some(vec!["a".to_owned(), "b".to_owned()]));
    }
}

#[derive(Parser)]
#[command(name = "right", version, about = "Multi-agent runtime for Claude Code")]
pub struct Cli {
    /// Path to Right Agent home directory
    #[arg(long, env = "RIGHT_HOME")]
    pub home: Option<String>,

    /// Enable verbose logging
    #[arg(short, long)]
    pub verbose: bool,

    /// Disable color output. Equivalent to setting NO_COLOR=1 for this run.
    #[arg(long, global = true)]
    pub no_color: bool,

    #[command(subcommand)]
    pub command: Commands,
}

/// Subcommands for `right config`.
#[derive(Subcommand)]
pub enum ConfigCommands {
    /// Enable machine-wide domain blocking via managed settings (requires sudo)
    StrictSandbox,
    /// Read a config value by key (e.g. tunnel.hostname)
    Get {
        /// Config key (e.g. tunnel.hostname, tunnel.uuid, tunnel.credentials-file)
        key: String,
    },
    /// Set a config value by key
    Set {
        /// Config key
        key: String,
        /// New value
        value: String,
    },
}

/// Subcommands for `right agent`.
#[derive(Subcommand)]
pub enum AgentCommands {
    /// Initialize a new agent
    Init {
        /// Agent name (alphanumeric + hyphens)
        name: String,
        /// Non-interactive mode
        #[arg(short = 'y', long)]
        yes: bool,
        /// If agent exists, wipe and re-create (confirms unless -y)
        #[arg(long)]
        force_recreate: bool,
        /// With --force-recreate: re-run wizard instead of reusing existing config
        #[arg(long, requires = "force_recreate")]
        fresh: bool,
        /// Network policy: restrictive or permissive
        #[arg(long)]
        network_policy: Option<right_agent::agent::types::NetworkPolicy>,
        /// Restore agent from a backup directory
        #[arg(long, conflicts_with_all = ["fresh", "network_policy"])]
        from_backup: Option<std::path::PathBuf>,
        /// Preserve the source backup's implicit Hindsight memory binding
        #[arg(
            long,
            requires = "from_backup",
            conflicts_with_all = ["rebind_to_target", "memory_bank_id"]
        )]
        preserve_source_bindings: bool,
        /// Rebind implicit Hindsight memory to the target agent name
        #[arg(
            long,
            requires = "from_backup",
            conflicts_with_all = ["preserve_source_bindings", "memory_bank_id"]
        )]
        rebind_to_target: bool,
        /// Set the restored Hindsight memory bank id explicitly
        #[arg(
            long,
            requires = "from_backup",
            value_parser = non_empty_arg,
            conflicts_with_all = ["preserve_source_bindings", "rebind_to_target"]
        )]
        memory_bank_id: Option<String>,
        /// Telegram bot token. Also accepted via RIGHT_TELEGRAM_TOKEN env var,
        /// which avoids leaking the token into shell history, `ps`, and journald.
        #[arg(long, env = "RIGHT_TELEGRAM_TOKEN", hide_env_values = true)]
        telegram_token: Option<String>,
        /// Comma-separated list of Telegram chat IDs allowed to use this bot
        /// (e.g. --telegram-allowed-chat-ids 12345678,100200300)
        #[arg(long, value_delimiter = ',')]
        telegram_allowed_chat_ids: Vec<i64>,
    },
    /// Configure an agent interactively (or get/set a specific setting)
    Config {
        /// Agent name (interactive selection if omitted)
        name: Option<String>,
        /// Setting key (e.g. telegram-token)
        key: Option<String>,
        /// New value (omit to print current)
        value: Option<String>,
    },
    /// List discovered agents
    List,
    /// Back up an agent's sandbox and configuration
    Backup {
        /// Agent name
        name: String,
        /// Only back up sandbox files (skip agent.yaml, data.db, allowlist.yaml)
        #[arg(long)]
        sandbox_only: bool,
        /// Include rebuildable sandbox dependency/cache directories (.cache, .venv, .npm, .uv)
        #[arg(long)]
        include_rebuildable: bool,
    },
    /// Destroy an agent (stop, optionally backup, delete sandbox and files)
    Destroy {
        /// Agent name
        name: String,
        /// Create backup before destroying
        #[arg(long)]
        backup: bool,
        /// Skip interactive prompts
        #[arg(long)]
        force: bool,
    },
    /// Re-enter bootstrap mode (debug only). Backs up identity files,
    /// deletes them from host and sandbox, recreates BOOTSTRAP.md, and
    /// deactivates active sessions. Sandbox, credentials, memory bank,
    /// and data.db rows are preserved.
    Rebootstrap {
        /// Agent name
        name: String,
        /// Skip the confirmation prompt
        #[arg(short = 'y', long = "yes")]
        yes: bool,
    },
    /// Add a trusted user to this agent's allowlist
    Allow {
        /// Agent name
        name: String,
        /// Telegram user ID (positive integer)
        user_id: i64,
        /// Optional label (first_name or username)
        #[arg(long)]
        label: Option<String>,
    },
    /// Remove a trusted user from this agent's allowlist
    Deny {
        /// Agent name
        name: String,
        /// Telegram user ID
        user_id: i64,
    },
    /// Open a group for all members (non-trusted senders may address the bot)
    #[command(name = "allow_all")]
    AllowAll {
        /// Agent name
        name: String,
        /// Telegram group chat ID (negative integer for regular groups)
        #[arg(allow_hyphen_values = true)]
        chat_id: i64,
        /// Optional label (group title)
        #[arg(long)]
        label: Option<String>,
    },
    /// Set the response mode for a topic (or the group default with --group)
    #[command(name = "mode")]
    Mode {
        /// Agent name
        name: String,
        /// Telegram group chat ID
        #[arg(allow_hyphen_values = true)]
        chat_id: i64,
        /// Effective thread id (0 = General). Ignored with --group.
        #[arg(long, default_value_t = 0)]
        thread_id: i64,
        /// Set the group-level default instead of a topic
        #[arg(long)]
        group: bool,
        /// One of: addressed | all | clear (clear is topic-only)
        value: String,
    },
    /// Close an opened group
    #[command(name = "deny_all")]
    DenyAll {
        /// Agent name
        name: String,
        /// Telegram group chat ID
        #[arg(allow_hyphen_values = true)]
        chat_id: i64,
    },
    /// Dump the current allowlist
    Allowed {
        /// Agent name
        name: String,
        /// Emit as JSON instead of a table
        #[arg(long)]
        json: bool,
    },
    /// Read learned skill lifecycle state
    Skill {
        #[command(subcommand)]
        command: AgentSkillCommands,
    },
    /// Programmatic provider management (mirrors the Telegram Mini App
    /// dashboard's create/list flows). Intended for fleet automation
    /// where the dashboard isn't reachable (e.g. multi-tenant
    /// provisioning by an upstream registrar). Requires `right up` to
    /// be running so the internal API socket exists.
    Providers {
        #[command(subcommand)]
        command: AgentProvidersCommands,
    },
    /// Move an agent out of its OpenShell sandbox into a microsandbox VM.
    /// The old sandbox is deleted only after the restore is verified; any
    /// earlier failure leaves the agent exactly as it was, so a failed
    /// migration can simply be re-run.
    #[command(name = "migrate-sandbox")]
    MigrateSandbox {
        /// Agent name
        name: String,
    },
    /// Offline repair of legacy multiprocess-WAL databases. Stops the entire
    /// runtime, preserves forensic artifacts, and does not restart.
    #[command(name = "db-repair")]
    DbRepair {
        /// One or more agent names (database paths and SQL are never accepted).
        #[arg(required = true, num_args = 1..)]
        names: Vec<String>,
    },
}

/// Skill lifecycle subcommands.
#[derive(Subcommand)]
pub enum AgentSkillCommands {
    /// List learned skill lifecycle rows across agents
    List,
}

/// Provider management subcommands. Same surface the dashboard exposes
/// via internal-socket REST; this is a non-interactive entry point for
/// automation. All actions go through the internal API so existing
/// validation, locking, and store-side rollback are reused.
#[derive(Subcommand)]
pub enum AgentProvidersCommands {
    /// Add a provider to an agent. Generic providers carry their own
    /// spec (hosts + env-var). Built-in providers (e.g. `anthropic`,
    /// `github`) reuse the built-in catalog entry. The credential
    /// value is taken from
    /// `RIGHT_PROVIDER_CREDENTIAL` env if not given via
    /// `--credential` — passing secrets on argv leaks them to
    /// `ps`, shell history, and journald.
    Add {
        /// Agent name
        agent: String,
        /// Provider type slug. `generic` requires at least one `--upstream-host`
        /// and `--env-var`; built-in types (e.g. `anthropic`,
        /// `github`) pull these from the built-in provider catalog.
        #[arg(long, default_value = "generic")]
        type_: String,
        /// Optional label, used in the resulting provider name
        /// (`<agent>-<label>`). Defaults to the type slug.
        #[arg(long)]
        label: Option<String>,
        /// Credential value (the API token / key). Prefer
        /// `RIGHT_PROVIDER_CREDENTIAL` env to avoid argv leak.
        #[arg(long, env = "RIGHT_PROVIDER_CREDENTIAL", hide_env_values = true)]
        credential: String,
        /// Upstream host (e.g. `api.openai.com`). Repeat for multi-host providers.
        /// Generic only.
        #[arg(long)]
        upstream_host: Vec<String>,
        /// Upstream path prefix (e.g. `/v1`). Generic only.
        #[arg(long)]
        upstream_path_prefix: Option<String>,
        /// Legacy generic header name. Accepted for compatibility and ignored.
        #[arg(long, hide = true)]
        header_name: Option<String>,
        /// Env var name the sandbox sees as the placeholder.
        /// Generic only.
        #[arg(long)]
        env_var: Option<String>,
    },
}

/// Subcommands for `right memory`.
#[derive(Subcommand)]
pub enum MemoryCommands {
    /// Show paginated memory table (newest first)
    List {
        /// Agent name
        agent: String,
        /// Max entries to show (default: 10)
        #[arg(long, default_value = "10")]
        limit: i64,
        /// Skip first N entries (for pagination)
        #[arg(long, default_value = "0")]
        offset: i64,
        /// Emit newline-delimited JSON instead of table
        #[arg(long)]
        json: bool,
    },
    /// Full-text search memories
    Search {
        /// Agent name
        agent: String,
        /// Full-text search query
        query: String,
        /// Max entries to show (default: 10)
        #[arg(long, default_value = "10")]
        limit: i64,
        /// Skip first N entries (for pagination)
        #[arg(long, default_value = "0")]
        offset: i64,
        /// Emit newline-delimited JSON instead of table
        #[arg(long)]
        json: bool,
    },
    /// Hard-delete a memory entry (operator bypass of soft-delete)
    Delete {
        /// Agent name
        agent: String,
        /// Memory entry ID to delete
        id: i64,
    },
    /// Show memory database statistics
    Stats {
        /// Agent name
        agent: String,
        /// Emit JSON instead of text
        #[arg(long)]
        json: bool,
    },
}

/// Subcommands for `right mcp`.
#[derive(Subcommand)]
pub enum McpCommands {
    /// Show MCP OAuth auth status for all agents (or a single agent)
    Status {
        /// Filter to a single agent by name
        #[arg(long)]
        agent: Option<String>,
    },
}

#[derive(Subcommand)]
pub enum Commands {
    /// Initialize Right Agent home directory with default agent
    Init {
        /// Telegram bot token for channel setup (skip with Enter if interactive)
        #[arg(long)]
        telegram_token: Option<String>,
        /// Comma-separated list of Telegram chat IDs allowed to use this bot
        /// (e.g. --telegram-allowed-chat-ids 12345678,100200300)
        #[arg(long, value_delimiter = ',')]
        telegram_allowed_chat_ids: Vec<i64>,
        /// Cloudflare Named Tunnel name (created if not exists; requires cloudflared login)
        #[arg(long, default_value = "right")]
        tunnel_name: String,
        /// Public hostname for the tunnel (e.g. right.example.com)
        #[arg(long)]
        tunnel_hostname: Option<String>,
        /// Tunnel provider: `cloudflared` (default — `right` runs cloudflared
        /// as part of process-compose) or `external` (operator runs their own
        /// reverse proxy on the public hostname).
        #[arg(long, default_value = "cloudflared")]
        tunnel_provider: String,
        /// Non-interactive mode — skip all prompts (requires --tunnel-hostname when cloudflared login detected)
        #[arg(short = 'y', long)]
        yes: bool,
        /// Network policy: restrictive (Anthropic/Claude only) or permissive (all HTTPS)
        #[arg(long)]
        network_policy: Option<right_agent::agent::types::NetworkPolicy>,
    },
    /// List discovered agents and their status
    List,
    /// Validate dependencies and agent configuration
    Doctor,
    /// Ensure the install directory is on your shell PATH
    SetupPath,
    /// Launch agents with process-compose
    Up {
        /// Only launch specific agents (comma-separated)
        #[arg(long, value_delimiter = ',')]
        agents: Option<Vec<String>>,
        /// Launch in background with TUI server
        #[arg(short, long)]
        detach: bool,
        /// Enable debug logging (writes to $RIGHT_HOME/run/<agent>-debug.log)
        #[arg(long)]
        debug: bool,
        /// Validate readiness without prompting or changing configuration
        #[arg(long)]
        non_interactive: bool,
    },
    /// Stop all agents
    Down,
    /// Speech-to-text model management
    Stt {
        #[command(subcommand)]
        command: SttCommands,
    },
    /// Re-sync agent codegen and hot-update running process-compose
    Reload {
        /// Only re-run codegen for specific agents (comma-separated).
        /// process-compose.yaml always includes all agents.
        #[arg(long, value_delimiter = ',')]
        agents: Option<Vec<String>>,
    },
    /// Show running agent status
    Status,
    /// Restart a single agent
    Restart {
        /// Agent name to restart
        agent: String,
    },
    /// Attach to running process-compose TUI
    Attach,
    /// Launch an agent interactively for setup (Telegram pairing, onboarding)
    Pair {
        /// Agent name (defaults to "right")
        agent: Option<String>,
    },
    /// Manage Right Agent configuration (interactive wizard if no subcommand)
    Config {
        #[command(subcommand)]
        command: Option<ConfigCommands>,
    },
    /// Manage agents
    Agent {
        #[command(subcommand)]
        command: AgentCommands,
    },
    /// Inspect and manage agent memory databases
    Memory {
        #[command(subcommand)]
        command: MemoryCommands,
    },
    /// Run MCP Aggregator HTTP server (multi-agent, Bearer token auth)
    McpServer {
        /// Port to listen on
        #[arg(long, default_value = "8100")]
        port: u16,
        /// Path to agent-tokens.json (agent name → Bearer token map)
        #[arg(long)]
        token_map: PathBuf,
    },
    /// Inspect MCP OAuth token status
    Mcp {
        #[command(subcommand)]
        command: McpCommands,
    },
    /// Run the per-agent Telegram bot (webhook)
    Bot {
        /// Agent name (resolves to $RIGHT_HOME/agents/<name>/)
        #[arg(long)]
        agent: String,
        /// Pass --verbose to CC subprocess and log CC stderr at debug level
        #[arg(long)]
        debug: bool,
    },
}

#[derive(Subcommand)]
pub enum SttCommands {
    /// Pre-download whisper model(s) into the home cache. Idempotent: skips
    /// any model already present. Bypasses the ffmpeg check (downloading the
    /// model and running transcription are separate concerns) and fails loudly
    /// on download error. Useful for warming a CI or host cache before the STT
    /// integration tests, so they don't each download the model concurrently.
    Preload {
        /// Models to download (comma-separated): tiny, base, small, medium,
        /// large-v3. Defaults to `tiny`.
        #[arg(
            long,
            value_delimiter = ',',
            default_value = "tiny",
            value_parser = parse_whisper_model,
        )]
        model: Vec<right_agent_config::WhisperModel>,
    },
}

fn parse_whisper_model(s: &str) -> Result<right_agent_config::WhisperModel, String> {
    use right_agent_config::WhisperModel;
    match s.trim() {
        "tiny" => Ok(WhisperModel::Tiny),
        "base" => Ok(WhisperModel::Base),
        "small" => Ok(WhisperModel::Small),
        "medium" => Ok(WhisperModel::Medium),
        "large-v3" => Ok(WhisperModel::LargeV3),
        other => Err(format!(
            "unknown whisper model '{other}' (expected: tiny, base, small, medium, large-v3)"
        )),
    }
}

fn non_empty_arg(value: &str) -> Result<String, String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        Err("value cannot be empty".to_string())
    } else {
        Ok(trimmed.to_string())
    }
}

fn restore_binding_mode_from_flags(
    preserve_source_bindings: bool,
    rebind_to_target: bool,
    memory_bank_id: Option<String>,
) -> restore::RestoreBindingMode {
    if preserve_source_bindings {
        restore::RestoreBindingMode::PreserveSource
    } else if rebind_to_target {
        restore::RestoreBindingMode::RebindToTarget
    } else if let Some(memory_bank_id) = memory_bank_id {
        restore::RestoreBindingMode::MemoryBankId(memory_bank_id)
    } else {
        restore::RestoreBindingMode::DirectUnspecified
    }
}

/// Intercept `BlockAlreadyRendered`: exit code 1, no miette formatting.
/// Used when a command has already rendered a brand-conformant rail block
/// explaining the failure.
fn handle_dispatch(result: miette::Result<()>) -> miette::Result<()> {
    if let Err(ref e) = result
        && e.downcast_ref::<right_ui::BlockAlreadyRendered>().is_some()
    {
        std::process::exit(1);
    }
    result
}

#[tokio::main]
async fn main() -> miette::Result<()> {
    rustls::crypto::ring::default_provider()
        .install_default()
        .map_err(|_| miette::miette!("rustls ring crypto provider already installed"))?;

    miette::set_hook(Box::new(|_| {
        Box::new(miette::MietteHandlerOpts::new().build())
    }))?;

    let cli = Cli::parse();

    if cli.no_color {
        // SAFETY: main is still single-threaded at this point — no readers of NO_COLOR yet.
        unsafe {
            std::env::set_var("NO_COLOR", "1");
        }
    }

    // Brand-conformant inquire prompt chrome — replaces the default
    // LightGreen `?` and LightCyan answers/highlighted-options with subtle
    // DarkGrey (or no styling at all on Mono/Ascii themes).
    right_ui::install_prompt_render_config();

    let filter = if cli.verbose {
        "right=debug,right_agent=debug,right_bot=debug"
    } else {
        "right=info,right_agent=info,right_bot=info"
    };
    // Set up tracing with console + per-process file log.
    // Bot writes console to stderr (stdout reserved for JSON), aggregator to stdout (colored).
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;

    let setup_file_log = |name: &str| {
        let log_dir = dirs::home_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("/tmp"))
            .join(".right")
            .join("logs");
        let _ = std::fs::create_dir_all(&log_dir);
        let file_appender = tracing_appender::rolling::daily(&log_dir, format!("{name}.log"));
        tracing_appender::non_blocking(file_appender)
    };

    let _log_guard = match &cli.command {
        Commands::Bot { agent, .. } => {
            let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(filter));
            let (non_blocking, guard) = setup_file_log(agent);
            tracing_subscriber::registry()
                .with(env_filter)
                .with(tracing_subscriber::fmt::layer().with_writer(std::io::stderr))
                .with(
                    tracing_subscriber::fmt::layer()
                        .with_writer(non_blocking)
                        .with_ansi(false),
                )
                .init();
            Some(guard)
        }
        Commands::McpServer { .. } => {
            let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(filter));
            let (non_blocking, guard) = setup_file_log("mcp-aggregator");
            tracing_subscriber::registry()
                .with(env_filter)
                .with(tracing_subscriber::fmt::layer())
                .with(
                    tracing_subscriber::fmt::layer()
                        .with_writer(non_blocking)
                        .with_ansi(false),
                )
                .init();
            Some(guard)
        }
        _ => {
            let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(filter));
            tracing_subscriber::fmt()
                .with_writer(std::io::stderr)
                .with_env_filter(env_filter)
                .init();
            None
        }
    };

    let home = right_config::resolve_home(
        cli.home.as_deref(),
        std::env::var("RIGHT_HOME").ok().as_deref(),
    )?;

    let result = match cli.command {
        Commands::Init {
            telegram_token,
            telegram_allowed_chat_ids,
            tunnel_name,
            tunnel_hostname,
            tunnel_provider,
            yes,
            network_policy,
        } => {
            let claude_setup_token = std::env::var("RIGHT_CLAUDE_SETUP_TOKEN").ok();
            let quiescence_guard = runtime_quiescence::require_runtime_quiesced(&home).await?;
            cmd_init(
                &home,
                telegram_token.as_deref(),
                claude_setup_token.as_deref(),
                &telegram_allowed_chat_ids,
                &tunnel_name,
                tunnel_hostname.as_deref(),
                &tunnel_provider,
                yes,
                network_policy,
                &quiescence_guard,
            )
            .await
        }
        Commands::List => cmd_list(&home),
        Commands::Doctor => cmd_doctor(&home).await,
        Commands::SetupPath => cmd_setup_path(),
        Commands::Up {
            agents,
            detach,
            debug,
            non_interactive,
        } => cmd_up(&home, agents, detach, debug, non_interactive).await,
        Commands::Down => cmd_down(&home).await,
        Commands::Stt { command } => match command {
            SttCommands::Preload { model } => cmd_stt_preload(&home, &model).await,
        },
        Commands::Reload { agents } => cmd_reload(&home, agents).await,
        Commands::Status => cmd_status(&home).await,
        Commands::Restart { agent } => cmd_restart(&home, &agent).await,
        Commands::Attach => cmd_attach(&home),
        Commands::Pair { agent } => cmd_pair(&home, agent.as_deref()),
        Commands::Config { command } => match command {
            None => {
                crate::wizard::combined_setting_menu(&home).await?;
                Ok(())
            }
            Some(ConfigCommands::StrictSandbox) => cmd_config_strict_sandbox(),
            Some(ConfigCommands::Get { key }) => {
                let config = right_config::read_global_config(&home)?;
                match key.as_str() {
                    "tunnel.hostname" => println!("{}", config.tunnel.hostname),
                    "tunnel.provider" => match &config.tunnel.provider {
                        right_config::TunnelProvider::Cloudflared { .. } => {
                            println!("cloudflared")
                        }
                        right_config::TunnelProvider::External => println!("external"),
                    },
                    "tunnel.uuid" => match &config.tunnel.provider {
                        right_config::TunnelProvider::Cloudflared { tunnel_uuid, .. } => {
                            println!("{tunnel_uuid}")
                        }
                        right_config::TunnelProvider::External => {
                            return Err(miette::miette!(
                                "tunnel.uuid is not set: tunnel.provider is `external`"
                            ));
                        }
                    },
                    "tunnel.credentials-file" => match &config.tunnel.provider {
                        right_config::TunnelProvider::Cloudflared {
                            credentials_file, ..
                        } => println!("{}", credentials_file.display()),
                        right_config::TunnelProvider::External => {
                            return Err(miette::miette!(
                                "tunnel.credentials-file is not set: tunnel.provider is `external`"
                            ));
                        }
                    },
                    other => return Err(miette::miette!("Unknown config key: {other}")),
                }
                Ok(())
            }
            Some(ConfigCommands::Set { key, value }) => Err(miette::miette!(
                "Direct set not yet implemented for key '{key}' with value '{value}'. Use `right config` for interactive mode."
            )),
        },
        Commands::Agent { command } => match command {
            AgentCommands::Init {
                name,
                yes,
                force_recreate,
                fresh,
                network_policy,
                from_backup,
                preserve_source_bindings,
                rebind_to_target,
                memory_bank_id,
                telegram_token,
                telegram_allowed_chat_ids,
            } => {
                let supplied_claude_setup_token = std::env::var("RIGHT_CLAUDE_SETUP_TOKEN").ok();
                let claude_setup_token =
                    resolve_claude_setup_token(supplied_claude_setup_token.as_deref(), !yes)?;
                let quiescence_guard = runtime_quiescence::require_runtime_quiesced(&home).await?;
                if let Some(backup_path) = from_backup {
                    let restore_binding_mode = restore_binding_mode_from_flags(
                        preserve_source_bindings,
                        rebind_to_target,
                        memory_bank_id,
                    );
                    cmd_agent_restore(
                        &home,
                        &name,
                        &backup_path,
                        restore_binding_mode,
                        &claude_setup_token,
                        &quiescence_guard,
                    )
                    .await
                } else {
                    preflight_agent_init_tunnel(&home)?;
                    cmd_agent_init(
                        &home,
                        &name,
                        yes,
                        force_recreate,
                        fresh,
                        network_policy,
                        telegram_token.as_deref(),
                        &claude_setup_token,
                        &telegram_allowed_chat_ids,
                        &quiescence_guard,
                    )
                    .await
                }
            }
            AgentCommands::List => cmd_list(&home),
            AgentCommands::Config { name, key, value } => {
                match (key, value) {
                    (None, None) => {
                        // Egress and secret structure are create-time only, so
                        // a config change that needs a different sandbox is
                        // handled by an explicit recreate, never implicitly
                        // from the settings menu.
                        crate::wizard::agent_setting_menu(&home, name.as_deref()).await?;
                    }
                    (Some(_key), _) => {
                        return Err(miette::miette!(
                            "Direct get/set not yet implemented. Use `right agent config` for interactive mode."
                        ));
                    }
                    (None, Some(_)) => {
                        return Err(miette::miette!("Cannot set a value without a key"));
                    }
                }
                Ok(())
            }
            AgentCommands::Backup {
                name,
                sandbox_only,
                include_rebuildable,
            } => cmd_agent_backup(&home, &name, sandbox_only, include_rebuildable).await,
            AgentCommands::Destroy {
                name,
                backup,
                force,
            } => cmd_agent_destroy(&home, &name, backup, force).await,
            AgentCommands::Rebootstrap { name, yes } => {
                cmd_agent_rebootstrap(&home, &name, yes).await
            }
            AgentCommands::Allow {
                name,
                user_id,
                label,
            } => {
                if user_id < 0 {
                    miette::bail!(
                        "user_id cannot be negative (groups/channels use `right agent allow_all`)"
                    );
                }
                let dir = right_config::agents_dir(&home).join(&name);
                if !dir.exists() {
                    return Err(miette::miette!("agent not found: {}", dir.display()));
                }
                use right_agent::agent::allowlist::{
                    self, AddOutcome, AllowedUser, AllowlistState,
                };
                let outcome = allowlist::with_lock(&dir, |d| -> Result<AddOutcome, String> {
                    let file = allowlist::read_file(d)?.unwrap_or_default();
                    let mut state = AllowlistState::from_file(file);
                    let outcome = state.add_user(AllowedUser {
                        id: user_id,
                        label: label.clone(),
                        added_by: None,
                        added_at: chrono::Utc::now(),
                    });
                    allowlist::write_file_inner(d, &state.to_file())?;
                    Ok(outcome)
                })
                .map_err(|e| miette::miette!("{e}"))?;
                match outcome {
                    AddOutcome::Inserted => println!("added user {user_id}"),
                    AddOutcome::AlreadyPresent => println!("user {user_id} already allowed"),
                }
                Ok(())
            }
            AgentCommands::Deny { name, user_id } => {
                let dir = right_config::agents_dir(&home).join(&name);
                if !dir.exists() {
                    return Err(miette::miette!("agent not found: {}", dir.display()));
                }
                use right_agent::agent::allowlist::{self, AllowlistState, RemoveOutcome};
                let outcome = allowlist::with_lock(&dir, |d| -> Result<RemoveOutcome, String> {
                    let file = allowlist::read_file(d)?.unwrap_or_default();
                    let mut state = AllowlistState::from_file(file);
                    let outcome = state.remove_user(user_id);
                    allowlist::write_file_inner(d, &state.to_file())?;
                    Ok(outcome)
                })
                .map_err(|e| miette::miette!("{e}"))?;
                match outcome {
                    RemoveOutcome::Removed => println!("removed user {user_id}"),
                    RemoveOutcome::NotFound => println!("user {user_id} not in allowlist"),
                }
                Ok(())
            }
            AgentCommands::AllowAll {
                name,
                chat_id,
                label,
            } => {
                let dir = right_config::agents_dir(&home).join(&name);
                if !dir.exists() {
                    return Err(miette::miette!("agent not found: {}", dir.display()));
                }
                use right_agent::agent::allowlist::{
                    self, AddOutcome, AllowedGroup, AllowlistState, GroupKind, ResponseMode,
                };
                let outcome = allowlist::with_lock(&dir, |d| -> Result<AddOutcome, String> {
                    let file = allowlist::read_file(d)?.unwrap_or_default();
                    let mut state = AllowlistState::from_file(file);
                    let outcome = state.add_group(AllowedGroup {
                        id: chat_id,
                        label: label.clone(),
                        opened_by: None,
                        opened_at: chrono::Utc::now(),
                        mode: ResponseMode::Addressed,
                        topics: Vec::new(),
                        kind: GroupKind::Group,
                    });
                    allowlist::write_file_inner(d, &state.to_file())?;
                    Ok(outcome)
                })
                .map_err(|e| miette::miette!("{e}"))?;
                match outcome {
                    AddOutcome::Inserted => println!("opened group {chat_id}"),
                    AddOutcome::AlreadyPresent => println!("group {chat_id} already opened"),
                }
                Ok(())
            }
            AgentCommands::Mode {
                name,
                chat_id,
                thread_id,
                group,
                value,
            } => {
                let dir = right_config::agents_dir(&home).join(&name);
                if !dir.exists() {
                    return Err(miette::miette!("agent not found: {}", dir.display()));
                }
                use right_agent::agent::allowlist::{self, AllowlistState, ResponseMode};
                let mode = match value.as_str() {
                    "addressed" => Some(ResponseMode::Addressed),
                    "all" => Some(ResponseMode::All),
                    "clear" => None,
                    other => {
                        return Err(miette::miette!(
                            "invalid mode '{other}' (addressed|all|clear)"
                        ));
                    }
                };
                if group && mode.is_none() {
                    return Err(miette::miette!(
                        "`clear` is topic-only; --group needs addressed|all"
                    ));
                }
                let applied = allowlist::with_lock(&dir, |d| -> Result<bool, String> {
                    let file = allowlist::read_file(d)?.unwrap_or_default();
                    let mut state = AllowlistState::from_file(file);
                    let ok = if group {
                        state.set_group_mode(chat_id, mode.expect("checked above"))
                    } else if let Some(m) = mode {
                        state.set_topic_mode(chat_id, thread_id, m)
                    } else {
                        state.clear_topic_mode(chat_id, thread_id) || state.is_group_open(chat_id)
                    };
                    if ok {
                        allowlist::write_file_inner(d, &state.to_file())?;
                    }
                    Ok(ok)
                })
                .map_err(|e| miette::miette!("{e}"))?;
                if applied {
                    println!("mode updated for {chat_id}");
                } else {
                    println!("group {chat_id} is not opened; run `right agent allow_all` first");
                }
                Ok(())
            }
            AgentCommands::DenyAll { name, chat_id } => {
                let dir = right_config::agents_dir(&home).join(&name);
                if !dir.exists() {
                    return Err(miette::miette!("agent not found: {}", dir.display()));
                }
                use right_agent::agent::allowlist::{self, AllowlistState, RemoveOutcome};
                let outcome = allowlist::with_lock(&dir, |d| -> Result<RemoveOutcome, String> {
                    let file = allowlist::read_file(d)?.unwrap_or_default();
                    let mut state = AllowlistState::from_file(file);
                    let outcome = state.remove_group(chat_id);
                    allowlist::write_file_inner(d, &state.to_file())?;
                    Ok(outcome)
                })
                .map_err(|e| miette::miette!("{e}"))?;
                match outcome {
                    RemoveOutcome::Removed => println!("closed group {chat_id}"),
                    RemoveOutcome::NotFound => println!("group {chat_id} was not opened"),
                }
                Ok(())
            }
            AgentCommands::Allowed { name, json } => {
                let dir = right_config::agents_dir(&home).join(&name);
                if !dir.exists() {
                    return Err(miette::miette!("agent not found: {}", dir.display()));
                }
                use right_agent::agent::allowlist;
                let file = allowlist::read_file(&dir)
                    .map_err(|e| miette::miette!("{e}"))?
                    .unwrap_or_default();
                if json {
                    let out = serde_json::json!({
                        "users": file.users,
                        "groups": file.groups,
                    });
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&out).map_err(|e| miette::miette!("{e:#}"))?
                    );
                } else {
                    println!("Trusted users:");
                    if file.users.is_empty() {
                        println!("  (none)");
                    }
                    for u in &file.users {
                        println!(
                            "  - {} {} (added {})",
                            u.id,
                            u.label.as_deref().unwrap_or(""),
                            u.added_at.format("%Y-%m-%d")
                        );
                    }
                    println!("Opened groups:");
                    if file.groups.is_empty() {
                        println!("  (none)");
                    }
                    for g in &file.groups {
                        println!(
                            "  - {} {} (opened {})",
                            g.id,
                            g.label.as_deref().unwrap_or(""),
                            g.opened_at.format("%Y-%m-%d")
                        );
                    }
                }
                Ok(())
            }
            AgentCommands::Skill { command } => cmd_agent_skill(&home, command).await,
            AgentCommands::Providers { command } => cmd_agent_providers(&home, command).await,
            AgentCommands::MigrateSandbox { name } => {
                migrate_sandbox::cmd_agent_migrate_sandbox(&home, &name).await
            }
            AgentCommands::DbRepair { names } => {
                db_repair::cmd_agent_db_repair(&home, &names).await
            }
        },
        Commands::Memory { command } => match command {
            MemoryCommands::List {
                agent,
                limit,
                offset,
                json,
            } => cmd_memory_list(&home, &agent, limit, offset, json).await,
            MemoryCommands::Search {
                agent,
                query,
                limit,
                offset,
                json,
            } => cmd_memory_search(&home, &agent, &query, limit, offset, json).await,
            MemoryCommands::Delete { agent, id } => cmd_memory_delete(&home, &agent, id).await,
            MemoryCommands::Stats { agent, json } => cmd_memory_stats(&home, &agent, json).await,
        },
        Commands::Mcp { command } => match command {
            McpCommands::Status { agent } => cmd_mcp_status(&home, agent.as_deref()).await,
        },
        Commands::McpServer {
            port,
            ref token_map,
        } => {
            let agents_dir = right_config::agents_dir(&home);
            let token_map_path = token_map.clone();
            let allowed_hosts = right_config::read_global_config(&home)?
                .aggregator
                .allowed_hosts;
            let token_map_content = std::fs::read_to_string(token_map)
                .map_err(|e| miette::miette!("failed to read token map: {e:#}"))?;
            let token_entries: std::collections::HashMap<String, String> =
                serde_json::from_str(&token_map_content)
                    .map_err(|e| miette::miette!("failed to parse token map: {e:#}"))?;

            // Construct every complete per-agent runtime before publishing any
            // routing. A single broken database or restore aborts startup.
            let db_owners = db_owner::DbOwnerRegistry::new();

            // One store handle for the whole server: the per-agent backends
            // and the internal API answer from the same authority.
            let providers = internal_api::open_provider_store(&home).await?;

            let token_map = {
                let mut map = std::collections::HashMap::new();
                for (agent_name, token) in &token_entries {
                    let agent_dir = agents_dir.join(agent_name);
                    map.insert(
                        token.clone(),
                        aggregator::AgentInfo {
                            name: agent_name.clone(),
                            dir: agent_dir,
                        },
                    );
                }
                std::sync::Arc::new(tokio::sync::RwLock::new(map))
            };

            let dispatcher = std::sync::Arc::new(aggregator::ToolDispatcher {
                agents: dashmap::DashMap::new(),
            });
            let refresh_senders: aggregator::RefreshSenders =
                std::sync::Arc::new(dashmap::DashMap::new());
            let reconnect_managers: aggregator::ReconnectManagers =
                std::sync::Arc::new(dashmap::DashMap::new());
            let http_client = runtime_builder::build_http_client()
                .map_err(|error| miette::miette!("{error:#}"))?;

            for agent_name in token_entries.keys() {
                let runtime = runtime_builder::build_agent_runtime(
                    agent_name,
                    agents_dir.join(agent_name),
                    &agents_dir,
                    std::sync::Arc::clone(&providers),
                    http_client.clone(),
                )
                .await
                .map_err(|error| {
                    miette::miette!("failed to initialize runtime for {agent_name}: {error:#}")
                })?;
                db_owners
                    .insert_bundle(std::sync::Arc::clone(&runtime.bundle))
                    .await
                    .map_err(|error| {
                        miette::miette!("failed to register runtime for {agent_name}: {error:#}")
                    })?;
                refresh_senders.insert(agent_name.clone(), runtime.refresh_sender);
                reconnect_managers.insert(
                    agent_name.clone(),
                    tokio::sync::Mutex::new(runtime.reconnect_manager),
                );
                dispatcher
                    .agents
                    .insert(agent_name.clone(), runtime.registry);
                runtime.bundle.publish();
            }

            aggregator::run_aggregator_http(
                port,
                token_map,
                token_map_path,
                dispatcher,
                agents_dir,
                home,
                providers,
                refresh_senders,
                reconnect_managers,
                db_owners,
                allowed_hosts,
            )
            .await
        }
        Commands::Bot { agent, debug } => {
            let needs_restart = right_bot::run(right_bot::BotArgs {
                agent,
                home: cli.home,
                debug,
            })
            .await?;
            if needs_restart {
                std::process::exit(right_bot::CONFIG_RESTART_EXIT_CODE);
            }
            Ok(())
        }
    };
    handle_dispatch(result)
}

fn resolve_claude_setup_token(supplied: Option<&str>, interactive: bool) -> miette::Result<String> {
    if let Some(token) = supplied.filter(|token| !token.trim().is_empty()) {
        return Ok(token.to_string());
    }
    if !interactive {
        return Err(miette::miette!(
            help = "Run `claude setup-token`, then set RIGHT_CLAUDE_SETUP_TOKEN to its output",
            "Claude setup token is required in non-interactive mode"
        ));
    }

    println!("Run `claude setup-token` in another terminal, then paste the token below.");
    inquire::Password::new("claude setup token:")
        .without_confirmation()
        .with_display_mode(inquire::PasswordDisplayMode::Hidden)
        .with_validator(|token: &str| {
            if token.trim().is_empty() {
                Ok(inquire::validator::Validation::Invalid(
                    "token cannot be empty".into(),
                ))
            } else {
                Ok(inquire::validator::Validation::Valid)
            }
        })
        .prompt()
        .map_err(|error| miette::miette!("Claude setup-token prompt failed: {error:#}"))
}

#[derive(Debug, thiserror::Error, miette::Diagnostic)]
enum ConfiguredTunnelError {
    #[error("cannot read Cloudflare tunnel credentials at {path}")]
    CredentialsRead {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("Cloudflare tunnel credentials at {path} are not valid JSON")]
    CredentialsJson {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error(
        "Cloudflare tunnel credentials identify {actual:?}, but config.yaml selects {configured}"
    )]
    TunnelIdMismatch {
        configured: String,
        actual: Option<String>,
    },
    #[error("cannot run cloudflared to inspect configured tunnel {tunnel_uuid}")]
    InfoCommand {
        tunnel_uuid: String,
        #[source]
        source: std::io::Error,
    },
    #[error(
        "Cloudflare account cannot access configured tunnel {tunnel_uuid} (cloudflared exited {status})"
    )]
    InfoFailed {
        tunnel_uuid: String,
        status: std::process::ExitStatus,
    },
}

#[derive(Debug)]
struct ConfiguredTunnelFailure {
    config: right_config::GlobalConfig,
    error: ConfiguredTunnelError,
}

fn validate_configured_tunnel_with(
    config: &right_config::GlobalConfig,
    cloudflared: &Path,
) -> Result<(), ConfiguredTunnelError> {
    let right_config::TunnelProvider::Cloudflared {
        tunnel_uuid,
        credentials_file,
    } = &config.tunnel.provider
    else {
        return Ok(());
    };

    let credentials_json = std::fs::read(credentials_file).map_err(|source| {
        ConfiguredTunnelError::CredentialsRead {
            path: credentials_file.clone(),
            source,
        }
    })?;
    let credentials: serde_json::Value =
        serde_json::from_slice(&credentials_json).map_err(|source| {
            ConfiguredTunnelError::CredentialsJson {
                path: credentials_file.clone(),
                source,
            }
        })?;
    let actual = credentials
        .get("TunnelID")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned);
    if actual.as_deref() != Some(tunnel_uuid.as_str()) {
        return Err(ConfiguredTunnelError::TunnelIdMismatch {
            configured: tunnel_uuid.clone(),
            actual,
        });
    }

    let status = std::process::Command::new(cloudflared)
        .args(["tunnel", "--loglevel", "error", "info", "--output", "json"])
        .arg(tunnel_uuid)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|source| ConfiguredTunnelError::InfoCommand {
            tunnel_uuid: tunnel_uuid.clone(),
            source,
        })?;
    if !status.success() {
        return Err(ConfiguredTunnelError::InfoFailed {
            tunnel_uuid: tunnel_uuid.clone(),
            status,
        });
    }
    Ok(())
}

fn validate_configured_tunnel(home: &Path) -> miette::Result<right_config::GlobalConfig> {
    let config = right_config::read_global_config(home)?;
    validate_configured_tunnel_with(&config, Path::new("cloudflared"))
        .map_err(miette::Report::new)?;
    Ok(config)
}

fn configured_tunnel_failure(home: &Path) -> miette::Result<Option<ConfiguredTunnelFailure>> {
    let config = right_config::read_global_config(home)?;
    Ok(
        validate_configured_tunnel_with(&config, Path::new("cloudflared"))
            .err()
            .map(|error| ConfiguredTunnelFailure { config, error }),
    )
}

fn preflight_agent_init_tunnel(home: &Path) -> miette::Result<()> {
    validate_configured_tunnel(home).map(|_| ())
}

fn validate_host_claude() -> miette::Result<PathBuf> {
    for binary_name in ["claude", "claude-bun"] {
        let Ok(binary) = which::which(binary_name) else {
            continue;
        };
        let Ok(output) = std::process::Command::new(&binary)
            .arg("--version")
            .stdin(Stdio::null())
            .output()
        else {
            continue;
        };
        let identifies_claude = String::from_utf8_lossy(&output.stdout).contains("Claude Code")
            || String::from_utf8_lossy(&output.stderr).contains("Claude Code");
        if output.status.success() && identifies_claude {
            return Ok(binary);
        }
    }

    Err(miette::miette!(
        help = "Install Claude Code and ensure a working `claude` or `claude-bun` is in PATH: https://docs.anthropic.com/en/docs/claude-code",
        "Claude Code is required on the host for `right init`, but no usable executable was found"
    ))
}

async fn persist_claude_setup_token(
    agent_dir: &Path,
    token: &str,
    _guard: &right_agent::runtime::RuntimeExclusionGuard,
) -> miette::Result<()> {
    let conn = right_db::open_connection(agent_dir, false)
        .await
        .map_err(|error| {
            miette::miette!("failed to open data.db for Claude authentication: {error:#}")
        })?;
    right_mcp::credentials::save_auth_token(&conn, token)
        .await
        .map_err(|error| miette::miette!("failed to save Claude setup token: {error:#}"))?;
    Ok(())
}

#[allow(clippy::needless_borrow, clippy::too_many_arguments)]
async fn cmd_init(
    home: &Path,
    telegram_token: Option<&str>,
    claude_setup_token: Option<&str>,
    telegram_allowed_chat_ids: &[i64],
    tunnel_name: &str,
    tunnel_hostname: Option<&str>,
    tunnel_provider: &str,
    yes: bool,
    network_policy: Option<right_agent::agent::types::NetworkPolicy>,
    quiescence_guard: &right_agent::runtime::RuntimeExclusionGuard,
) -> miette::Result<()> {
    let interactive = !yes;
    let claude_setup_token = resolve_claude_setup_token(claude_setup_token, interactive)?;

    // Brand: splash + dependency probe.
    {
        let theme = right_ui::detect();
        let version = env!("CARGO_PKG_VERSION");
        println!(
            "{}",
            right_ui::splash(theme, version, "sandboxed multi-agent runtime")
        );
        println!("{}", right_ui::section(theme, "dependencies"));
        println!("{}", right_ui::Rail::blank(theme));

        let mut block = right_ui::Block::new();
        let mut fatal = false;

        // process-compose (fatal)
        match which::which("process-compose") {
            Ok(_) => block.push(
                right_ui::status(right_ui::Glyph::Ok)
                    .noun("process-compose")
                    .verb("in PATH"),
            ),
            Err(_) => {
                fatal = true;
                block.push(
                    right_ui::status(right_ui::Glyph::Err)
                        .noun("process-compose")
                        .verb("not in PATH")
                        .fix("https://f1bonacc1.github.io/process-compose/installation/"),
                );
            }
        }

        match validate_host_claude() {
            Ok(path) => block.push(
                right_ui::status(right_ui::Glyph::Ok)
                    .noun("claude")
                    .verb("executable")
                    .detail(path.display().to_string()),
            ),
            Err(_) => {
                fatal = true;
                block.push(
                    right_ui::status(right_ui::Glyph::Err)
                        .noun("claude")
                        .verb("not usable (tried claude, claude-bun)")
                        .fix("https://docs.anthropic.com/en/docs/claude-code"),
                );
            }
        }

        // cloudflared (warn)
        match which::which("cloudflared") {
            Ok(_) => block.push(
                right_ui::status(right_ui::Glyph::Ok)
                    .noun("cloudflared")
                    .verb("in PATH"),
            ),
            Err(_) => block.push(
                right_ui::status(right_ui::Glyph::Warn)
                    .noun("cloudflared")
                    .verb("not in PATH (optional, tunnel)"),
            ),
        }

        println!("{}", block.render(theme));
        println!("{}", right_ui::Rail::blank(theme));

        if fatal {
            return Err(right_ui::BlockAlreadyRendered.into());
        }
    }

    // Validate CLI-passed token up front so we fail fast before any prompt —
    // both in interactive and non-interactive mode.
    if let Some(t) = telegram_token {
        right_agent::init::validate_telegram_token(t)?;
    }

    // Non-interactive: use CLI flags or defaults.
    // Interactive: wizard with Esc-to-go-back between steps.
    let (
        network_policy_val,
        token,
        chat_ids,
        memory_provider,
        memory_api_key,
        memory_bank_id,
        memory_recall_budget,
        memory_recall_max_tokens,
    );

    if !interactive {
        network_policy_val =
            network_policy.unwrap_or(right_agent::agent::types::NetworkPolicy::Permissive);
        token = telegram_token.map(|t| t.to_string());
        chat_ids = telegram_allowed_chat_ids.to_vec();
        memory_provider = right_agent::agent::types::MemoryProvider::Hindsight;
        memory_api_key = None;
        memory_bank_id = None;
        memory_recall_budget = right_agent::init::DEFAULT_RECALL_BUDGET;
        memory_recall_max_tokens = right_agent::init::DEFAULT_RECALL_MAX_TOKENS;
    } else {
        // Wizard state machine: Esc goes back to previous step.
        #[derive(Clone, Copy)]
        enum Step {
            Network,
            Telegram,
            ChatIds,
            Memory,
            Done,
        }

        let theme = right_ui::detect();
        println!("{}", right_ui::section(theme, "agent"));
        println!("{}", right_ui::Rail::blank(theme));

        let mut step = Step::Network;
        let mut w_network =
            network_policy.unwrap_or(right_agent::agent::types::NetworkPolicy::Permissive);
        let mut w_token: Option<String> = telegram_token.map(|t| t.to_string());
        let mut w_chat_ids: Vec<i64> = telegram_allowed_chat_ids.to_vec();
        let mut w_mem = (
            right_agent::agent::types::MemoryProvider::Hindsight,
            None::<String>,
            None::<String>,
            right_agent::init::DEFAULT_RECALL_BUDGET,
            right_agent::init::DEFAULT_RECALL_MAX_TOKENS,
        );

        loop {
            match step {
                Step::Network => {
                    if let Some(p) = network_policy {
                        w_network = p;
                        step = Step::Telegram;
                    } else if let Some(p) = right_agent::init::prompt_network_policy()? {
                        w_network = p;
                        step = Step::Telegram;
                    } else {
                        // Esc on the first step — abort.
                        return Err(miette::miette!("cancelled"));
                    }
                }
                Step::Telegram => {
                    if telegram_token.is_some() {
                        w_token = telegram_token.map(|t| t.to_string());
                        step = if w_token.is_some() {
                            Step::ChatIds
                        } else {
                            Step::Memory
                        };
                    } else {
                        use crate::wizard::TelegramSetupOutcome;
                        match crate::wizard::telegram_setup(None, true, false)? {
                            TelegramSetupOutcome::Token(t) => {
                                w_token = Some(t);
                                step = Step::ChatIds;
                            }
                            TelegramSetupOutcome::Skipped => {
                                w_token = None;
                                step = Step::Memory;
                            }
                            TelegramSetupOutcome::Back => {
                                step = Step::Network;
                            }
                        }
                    }
                }
                Step::ChatIds => {
                    if !telegram_allowed_chat_ids.is_empty() {
                        w_chat_ids = telegram_allowed_chat_ids.to_vec();
                        step = Step::Memory;
                    } else {
                        match crate::wizard::chat_ids_setup(false)? {
                            Some(ids) => {
                                w_chat_ids = ids;
                                step = Step::Memory;
                            }
                            None => {
                                step = Step::Telegram;
                            } // back
                        }
                    }
                }
                Step::Memory => match right_agent::init::prompt_memory_config("right")? {
                    Some((p, k, b, rb, rt)) => {
                        w_mem = (p, k, b, rb, rt);
                        step = Step::Done;
                    }
                    None => {
                        step = if w_token.is_some() {
                            Step::ChatIds
                        } else {
                            Step::Telegram
                        };
                    }
                },
                Step::Done => break,
            }
        }

        network_policy_val = w_network;
        token = w_token;
        chat_ids = w_chat_ids;
        memory_provider = w_mem.0;
        memory_api_key = w_mem.1;
        memory_bank_id = w_mem.2;
        memory_recall_budget = w_mem.3;
        memory_recall_max_tokens = w_mem.4;
    }

    // Compute memory_detail before init_right_home consumes memory_provider.
    let memory_detail = match memory_provider {
        right_agent::agent::types::MemoryProvider::Hindsight => "hindsight".to_string(),
        right_agent::agent::types::MemoryProvider::File => "file".to_string(),
    };

    if home.join("agents/right").exists() {
        return Err(miette::miette!(
            "Right Agent home already initialized at {}. Use `right config` to change settings.",
            home.join("agents/right").display()
        ));
    }

    // Tunnel setup and global config must succeed before the default agent is
    // created. A failed pre-creation tunnel boundary therefore leaves no
    // `agents/right` state behind.
    {
        let theme = right_ui::detect();
        println!("{}", right_ui::section(theme, "tunnel"));
        println!("{}", right_ui::Rail::blank(theme));
    }
    let tunnel_cfg = match tunnel_provider {
        "cloudflared" => crate::wizard::tunnel_setup(tunnel_name, tunnel_hostname, interactive)?,
        "external" => crate::wizard::external_tunnel_setup(tunnel_hostname, interactive)?,
        other => {
            return Err(miette::miette!(
                help = "supported values: `cloudflared`, `external`",
                "unknown tunnel provider `{other}` (--tunnel-provider)"
            ));
        }
    };
    let aggregator = if home.join("config.yaml").exists() {
        right_config::read_global_config(home)?.aggregator
    } else {
        right_config::AggregatorConfig::default()
    };
    let global_config = right_config::GlobalConfig {
        tunnel: tunnel_cfg,
        aggregator,
    };
    right_config::write_global_config(home, &global_config)?;

    right_agent::init::init_right_home(
        home,
        token.as_deref(),
        &chat_ids,
        &network_policy_val,
        memory_provider,
        memory_api_key,
        memory_bank_id,
        memory_recall_budget,
        memory_recall_max_tokens,
    )?;

    // Run codegen for the default "right" agent.
    // Per-agent codegen was moved to bot startup (59243d0) but init needs it
    // for schemas and settings before sandbox staging upload.
    {
        let agent_dir = home.join("agents/right");
        let self_exe =
            std::env::current_exe().unwrap_or_else(|_| std::path::PathBuf::from("right"));
        let agent_def = right_agent::agent::AgentDef {
            name: "right".to_string(),
            path: agent_dir.clone(),
            identity_path: agent_dir.join("IDENTITY.md"),
            config: right_agent::agent::discovery::parse_agent_config(&agent_dir)?,
            soul_path: None,
            user_path: None,
            tools_path: None,
            bootstrap_path: if agent_dir.join("BOOTSTRAP.md").exists() {
                Some(agent_dir.join("BOOTSTRAP.md"))
            } else {
                None
            },
            heartbeat_path: None,
        };
        right_codegen::run_agent_codegen_for_init(
            home,
            std::slice::from_ref(&agent_def),
            &self_exe,
            false,
        )?;
        right_codegen::run_single_agent_codegen(home, &agent_def, &self_exe, false).await?;
        // The command already owns runtime exclusion for direct database work.
        right_db::open_db(&agent_dir, true)
            .await
            .map_err(|error| miette::miette!("failed to migrate data.db: {error:#}"))?;
        persist_claude_setup_token(&agent_dir, &claude_setup_token, &quiescence_guard).await?;

        // The Agent Sandbox is deliberately not created here. The bot's
        // sandbox supervisor is the sole owner of sandbox lifecycle: it
        // create-or-attaches on first `right up` from the spec it also uses
        // for recovery, so a second creator in the CLI could only drift from
        // it (egress and secret structure are create-time only). The stored
        // setup token is likewise validated by the first in-guest probe, not
        // here — there is no host transport to validate it over.
    }

    let theme = right_ui::detect();
    let mode = network_policy_val.to_string();
    let chat_ids_detail = if chat_ids.is_empty() {
        "0 allowed (blocks all)".to_string()
    } else {
        format!("{} allowed", chat_ids.len())
    };
    let telegram_detail = if token.is_some() {
        "configured".to_string()
    } else {
        "not configured".to_string()
    };

    let mut recap = right_ui::Recap::new("ready")
        .ok("agent", &format!("right ({mode})"))
        .ok("tunnel", &global_config.tunnel.hostname)
        .ok("claude", "credential stored")
        .warn("sandbox", "created and checked on first `right up`");
    recap = if token.is_some() {
        recap.ok("telegram", &telegram_detail)
    } else {
        recap.warn("telegram", &telegram_detail)
    };
    recap = recap
        .ok("chat ids", &chat_ids_detail)
        .ok("memory", &memory_detail)
        .next("right up");
    println!("{}", recap.render(theme));

    Ok(())
}

#[allow(clippy::needless_borrow, clippy::too_many_arguments)]
async fn cmd_agent_init(
    home: &Path,
    name: &str,
    yes: bool,
    force_recreate: bool,
    fresh: bool,
    network_policy: Option<right_agent::agent::types::NetworkPolicy>,
    telegram_token: Option<&str>,
    claude_setup_token: &str,
    telegram_allowed_chat_ids: &[i64],
    quiescence_guard: &right_agent::runtime::RuntimeExclusionGuard,
) -> miette::Result<()> {
    if let Some(t) = telegram_token {
        right_agent::init::validate_telegram_token(t)?;
    }
    let interactive = !yes;
    let agents_parent = right_config::agents_dir(home);
    let agent_dir = agents_parent.join(name);
    let agent_existed = agent_dir.exists();

    // Reject if exists and --force-recreate not given.
    if agent_dir.exists() && !force_recreate {
        return Err(miette::miette!(
            help = "Use --force-recreate to wipe and re-create, or `right agent config` to change settings",
            "Agent directory already exists at {}",
            agent_dir.display()
        ));
    }

    // --- Force wipe logic ---
    let saved_overrides = if force_recreate && agent_dir.exists() {
        // Read existing config before deletion (unless --fresh).
        let saved = if fresh {
            None
        } else {
            let yaml_path = agent_dir.join("agent.yaml");
            let yaml_str = std::fs::read_to_string(&yaml_path).map_err(|e| {
                miette::miette!(
                    help = "Use --fresh to reconfigure from scratch",
                    "Could not read existing agent.yaml: {e:#}"
                )
            })?;
            let config: right_agent::agent::types::AgentConfig = serde_saphyr::from_str(&yaml_str)
                .map_err(|e| {
                    miette::miette!(
                        help = "Use --fresh to reconfigure from scratch",
                        "Could not parse existing agent.yaml: {e:#}"
                    )
                })?;
            Some(config)
        };

        if let Some(pc_client) = right_agent::runtime::PcClient::from_home(home)? {
            let process_name = format!("{name}-bot");
            let processes = pc_client.list_processes().await?;
            if let Some(process) = processes
                .iter()
                .find(|process| process.name == process_name)
                && !matches!(
                    process.status.to_ascii_lowercase().as_str(),
                    "stopped" | "completed" | "disabled"
                )
            {
                return Err(miette::miette!(
                    help = "Run `right down` first",
                    "Agent '{name}' is currently {}; it cannot be recreated",
                    process.status
                ));
            }
        }

        // Confirm with user.
        if interactive {
            use std::io::{self, Write};
            println!("Agent \"{name}\" already exists at {}", agent_dir.display());
            println!("This will permanently delete:");
            println!("  - All agent files (identity, memory, skills, config)");
            let explicit_sandbox_name = saved
                .as_ref()
                .and_then(|c| c.sandbox.as_ref())
                .and_then(|s| s.name.as_deref());
            let display_sb = right_sandbox::resolve_sandbox_name(name, explicit_sandbox_name);
            println!("  - Agent Sandbox \"{}\" (if it exists)", display_sb);
            print!("Continue? [y/N] ");
            io::stdout()
                .flush()
                .map_err(|e| miette::miette!("stdout flush: {e}"))?;
            let mut input = String::new();
            io::stdin()
                .read_line(&mut input)
                .map_err(|e| miette::miette!("failed to read input: {e}"))?;
            if !matches!(input.trim().to_lowercase().as_str(), "y" | "yes") {
                return Err(miette::miette!("Aborted"));
            }
        }

        // Delete the old sandbox before its agent directory: a surviving
        // sandbox under the same name would be re-attached by the bot and
        // silently keep the state the user asked to destroy.
        let explicit_sandbox_name = saved
            .as_ref()
            .and_then(|c| c.sandbox.as_ref())
            .and_then(|s| s.name.as_deref());
        let sb_name = right_sandbox::resolve_sandbox_name(name, explicit_sandbox_name);
        right_sandbox::SandboxHandle::delete(&sb_name)
            .await
            .map_err(|error| {
                miette::miette!("delete the existing sandbox '{sb_name}': {error:#}")
            })?;

        // Delete agent directory.
        std::fs::remove_dir_all(&agent_dir).map_err(|e| {
            miette::miette!(
                "Failed to delete agent directory {}: {e:#}",
                agent_dir.display()
            )
        })?;

        tracing::info!(agent = name, "wiped agent directory and sandbox");

        saved
    } else {
        None
    };

    let theme = right_ui::detect();
    println!(
        "{}",
        right_ui::section(theme, &format!("agent init: {name}"))
    );
    println!("{}", right_ui::Rail::blank(theme));

    // --- Build overrides ---
    let overrides = if let Some(config) = saved_overrides {
        // Reuse saved config from old agent.yaml. CLI-supplied
        // --telegram-token / --telegram-allowed-chat-ids take precedence
        // over saved values so the operator can rotate them at re-init
        // time without dropping into the wizard.
        let merged_token = telegram_token
            .map(|t| t.to_string())
            .or(config.telegram_token);
        let merged_chat_ids = if !telegram_allowed_chat_ids.is_empty() {
            telegram_allowed_chat_ids.to_vec()
        } else {
            config.allowed_chat_ids
        };
        right_agent::init::InitOverrides {
            network_policy: config.network_policy,
            telegram_token: merged_token,
            allowed_chat_ids: merged_chat_ids,
            model: config.model,
            learning: config.learning,
            env: config.env,
            memory_provider: config
                .memory
                .as_ref()
                .map(|m| m.provider.clone())
                .unwrap_or_default(),
            memory_api_key: config.memory.as_ref().and_then(|m| m.api_key.clone()),
            memory_bank_id: config.memory.as_ref().and_then(|m| m.bank_id.clone()),
            memory_recall_budget: config
                .memory
                .as_ref()
                .map(|m| m.recall_budget.clone())
                .unwrap_or(right_agent::init::DEFAULT_RECALL_BUDGET),
            memory_recall_max_tokens: config
                .memory
                .as_ref()
                .map(|m| m.recall_max_tokens)
                .unwrap_or(right_agent::init::DEFAULT_RECALL_MAX_TOKENS),
            stt: config.stt,
        }
    } else {
        // Fresh init: optionally restore from backup or run wizard.
        if interactive && !force_recreate {
            let options = vec!["create fresh", "restore from backup"];
            let choice = inquire::Select::new("how to initialize this agent?", options)
                .prompt()
                .map_err(|e| miette::miette!("prompt failed: {e:#}"))?;

            if choice == "restore from backup" {
                let backup_path_str = inquire::Text::new("backup directory path:")
                    .prompt()
                    .map_err(|e| miette::miette!("prompt failed: {e:#}"))?;
                let backup_path = std::path::PathBuf::from(backup_path_str.trim());
                if !backup_path.exists() {
                    return Err(miette::miette!(
                        "Backup directory does not exist: {}",
                        backup_path.display()
                    ));
                }
                return tokio::task::block_in_place(|| {
                    tokio::runtime::Handle::current().block_on(cmd_agent_restore(
                        home,
                        name,
                        &backup_path,
                        restore::RestoreBindingMode::Interactive,
                        claude_setup_token,
                        &quiescence_guard,
                    ))
                });
            }
        }

        // Run wizard or use CLI flags. Esc goes back to previous step.
        if !interactive {
            let ffmpeg_ok = right_stt::ffmpeg_available();
            let stt = right_agent::agent::types::SttConfig {
                enabled: ffmpeg_ok,
                model: right_agent_config::WhisperModel::Small,
            };
            if !ffmpeg_ok {
                eprintln!(
                    "warning: STT disabled — ffmpeg not in PATH. \
                     Install (macOS): brew install ffmpeg, then enable via \
                     `right agent config {name}`."
                );
            }
            right_agent::init::InitOverrides {
                network_policy: network_policy
                    .unwrap_or(right_agent::agent::types::NetworkPolicy::Permissive),
                telegram_token: telegram_token.map(|t| t.to_string()),
                allowed_chat_ids: telegram_allowed_chat_ids.to_vec(),
                model: None,
                learning: right_agent::agent::types::LearningConfig::default(),
                env: std::collections::HashMap::new(),
                memory_provider: right_agent::agent::types::MemoryProvider::Hindsight,
                memory_api_key: None,
                memory_bank_id: None,
                memory_recall_budget: right_agent::init::DEFAULT_RECALL_BUDGET,
                memory_recall_max_tokens: right_agent::init::DEFAULT_RECALL_MAX_TOKENS,
                stt,
            }
        } else {
            #[derive(Clone, Copy)]
            enum Step {
                Network,
                Telegram,
                ChatIds,
                Stt,
                Memory,
                Done,
            }

            let mut step = Step::Network;
            let mut w_network =
                network_policy.unwrap_or(right_agent::agent::types::NetworkPolicy::Permissive);
            let mut w_token: Option<String> = telegram_token.map(|t| t.to_string());
            let mut w_chat_ids: Vec<i64> = telegram_allowed_chat_ids.to_vec();
            let mut w_stt: right_agent::agent::types::SttConfig =
                right_agent::agent::types::SttConfig::default();
            let mut w_mem = (
                right_agent::agent::types::MemoryProvider::Hindsight,
                None::<String>,
                None::<String>,
                right_agent::init::DEFAULT_RECALL_BUDGET,
                right_agent::init::DEFAULT_RECALL_MAX_TOKENS,
            );

            loop {
                match step {
                    Step::Network => {
                        if let Some(p) = network_policy {
                            w_network = p;
                            step = Step::Telegram;
                        } else if let Some(p) = right_agent::init::prompt_network_policy()? {
                            w_network = p;
                            step = Step::Telegram;
                        } else {
                            return Err(miette::miette!("Setup cancelled."));
                        }
                    }
                    Step::Telegram => {
                        // Honour CLI/env-supplied token: skip the wizard step.
                        if telegram_token.is_some() {
                            step = Step::ChatIds;
                            continue;
                        }
                        use crate::wizard::TelegramSetupOutcome;
                        match crate::wizard::telegram_setup(None, true, true)? {
                            TelegramSetupOutcome::Token(t) => {
                                w_token = Some(t);
                                step = Step::ChatIds;
                            }
                            TelegramSetupOutcome::Skipped => {
                                unreachable!("required=true, telegram_setup never skips")
                            }
                            TelegramSetupOutcome::Back => {
                                step = Step::Network;
                            }
                        }
                    }
                    Step::ChatIds => {
                        // Honour CLI-supplied --telegram-allowed-chat-ids: skip prompt.
                        if !telegram_allowed_chat_ids.is_empty() {
                            step = Step::Stt;
                            continue;
                        }
                        match crate::wizard::chat_ids_setup(true)? {
                            Some(ids) => {
                                w_chat_ids = ids;
                                step = Step::Stt;
                            }
                            None => {
                                // The Telegram step may be CLI/env-pinned (and
                                // thus auto-skipped); back-navigate past it so
                                // "back" reaches the previous interactive prompt
                                // instead of bouncing straight back here.
                                step = if telegram_token.is_some() {
                                    Step::Network
                                } else {
                                    Step::Telegram
                                };
                            }
                        }
                    }
                    Step::Stt => match crate::wizard::stt_setup() {
                        Ok(Some((enabled, model))) => {
                            w_stt = right_agent::agent::types::SttConfig { enabled, model };
                            step = Step::Memory;
                        }
                        Ok(None) => {
                            // ChatIds and Telegram may be CLI/env-pinned (and
                            // thus auto-skipped); back-navigate to the nearest
                            // interactive step rather than a pinned one that
                            // would immediately skip forward again.
                            step = if telegram_allowed_chat_ids.is_empty() {
                                Step::ChatIds
                            } else if telegram_token.is_none() {
                                Step::Telegram
                            } else {
                                Step::Network
                            };
                        }
                        Err(e) => return Err(e),
                    },
                    Step::Memory => match right_agent::init::prompt_memory_config(name)? {
                        Some((p, k, b, rb, rt)) => {
                            w_mem = (p, k, b, rb, rt);
                            step = Step::Done;
                        }
                        None => {
                            step = Step::Stt;
                        }
                    },
                    Step::Done => break,
                }
            }

            right_agent::init::InitOverrides {
                network_policy: w_network,
                telegram_token: w_token,
                allowed_chat_ids: w_chat_ids,
                model: None,
                learning: right_agent::agent::types::LearningConfig::default(),
                env: std::collections::HashMap::new(),
                memory_provider: w_mem.0,
                memory_api_key: w_mem.1,
                memory_bank_id: w_mem.2,
                memory_recall_budget: w_mem.3,
                memory_recall_max_tokens: w_mem.4,
                stt: w_stt,
            }
        }
    };

    let agent_dir = right_agent::init::init_agent(&agents_parent, name, Some(&overrides))?;

    // Run codegen so settings, schemas, skills are generated.
    // Per-agent codegen was moved to bot startup (59243d0) but init/agent-init
    // need it for schemas and settings before sandbox staging upload.
    {
        let self_exe =
            std::env::current_exe().unwrap_or_else(|_| std::path::PathBuf::from("right"));
        let agent_def = right_agent::agent::AgentDef {
            name: name.to_string(),
            path: agent_dir.clone(),
            identity_path: agent_dir.join("IDENTITY.md"),
            config: right_agent::agent::discovery::parse_agent_config(&agent_dir)?,
            soul_path: None,
            user_path: None,
            tools_path: None,
            bootstrap_path: if agent_dir.join("BOOTSTRAP.md").exists() {
                Some(agent_dir.join("BOOTSTRAP.md"))
            } else {
                None
            },
            heartbeat_path: None,
        };
        right_codegen::run_agent_codegen_for_init(
            home,
            std::slice::from_ref(&agent_def),
            &self_exe,
            false,
        )?;
        right_codegen::run_single_agent_codegen(home, &agent_def, &self_exe, false).await?;
        // Nested database work reuses the outer exclusion capability.
        right_db::open_db(&agent_dir, true)
            .await
            .map_err(|error| miette::miette!("failed to migrate data.db: {error:#}"))?;
        persist_claude_setup_token(&agent_dir, claude_setup_token, &quiescence_guard).await?;
    }

    // No sandbox is created here: the bot's supervisor create-or-attaches on
    // first `right up` from the spec it also uses for recovery, and it is the
    // sole owner of sandbox lifecycle. Creating one here would be a second
    // creator that can only drift from it — egress and secret structure are
    // create-time only, so drift needs a recreate to undo. The setup token is
    // validated by that first in-guest probe for the same reason: the probe
    // runs like a turn, and there is no host transport to run it over.

    let cfg = right_agent::agent::discovery::parse_agent_config(&agent_dir)?
        .ok_or_else(|| miette::miette!("agent.yaml missing after init"))?;

    let sandbox_detail = format!(
        "{} — created and checked on first `right up`",
        cfg.network_policy
    );

    let chat_ids_detail = if cfg.allowed_chat_ids.is_empty() {
        "0 allowed (blocks all)".to_string()
    } else {
        format!("{} allowed", cfg.allowed_chat_ids.len())
    };

    let stt_detail = if cfg.stt.enabled {
        cfg.stt.model.yaml_str().to_string()
    } else {
        "off".to_string()
    };

    let memory_detail = match cfg.memory.as_ref().map(|m| &m.provider) {
        Some(right_agent::agent::types::MemoryProvider::Hindsight) => "hindsight",
        _ => "file",
    };

    // If PC is already running, hot-add the new agent's bot via reload.
    // No PC ⇒ pc_running: false ⇒ recap ends with `next: right up`.
    let register_outcome = tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current().block_on(async {
            right_agent::agent::register_with_running_pc(
                home,
                right_agent::agent::RegisterOptions {
                    agent_name: name.to_string(),
                    recreated: agent_existed && force_recreate,
                },
            )
            .await
        })
    });

    let mut recap = right_ui::Recap::new("ready")
        .ok("agent", &format!("{name} created"))
        .warn("sandbox", &sandbox_detail)
        .ok("claude", "credential stored")
        .ok(
            "telegram",
            if cfg.telegram_token.is_some() {
                "configured"
            } else {
                "not configured"
            },
        )
        .ok("chat ids", &chat_ids_detail)
        .ok("stt", &stt_detail)
        .ok("memory", memory_detail);

    recap = match register_outcome {
        Ok(right_agent::agent::RegisterResult { pc_running: false }) => recap.next("right up"),
        Ok(right_agent::agent::RegisterResult { pc_running: true }) => {
            recap.next("send /start to your bot in Telegram")
        }
        Err(e) => {
            tracing::warn!(
                error = format!("{e:#}"),
                "PC reload failed during agent init"
            );
            recap
                .warn("reload", "failed to add to running right")
                .next("right restart")
        }
    };
    println!("{}", recap.render(theme));

    Ok(())
}

/// Exit status from `right setup-path` when the shell rc file could not be
/// written. `install.sh` checks for this exact code to re-surface the
/// failure in its closing summary — keep the two in sync.
const PATH_SETUP_RC_WRITE_FAILED: i32 = 10;

/// `right setup-path` — ensure the install dir is on the user's shell PATH.
///
/// Never fails the installer: exits `0` when PATH is ensured, `10` when the
/// rc could not be written (after printing a warning the user can act on).
fn cmd_setup_path() -> miette::Result<()> {
    let theme = right_ui::detect();

    let exe = std::env::current_exe()
        .map_err(|e| miette::miette!("cannot resolve current executable: {e}"))?;
    let bindir = right_hostpath::bin_dir(&exe);
    let manual_fix = format!("add manually: export PATH=\"{}:$PATH\"", bindir.display());

    let Some(home) = dirs::home_dir() else {
        let line = right_ui::status(right_ui::Glyph::Warn)
            .noun("PATH")
            .verb("couldn't determine your home directory")
            .fix(manual_fix);
        println!("{}", line.render(theme));
        std::process::exit(PATH_SETUP_RC_WRITE_FAILED);
    };
    let shell = std::env::var("SHELL").ok();

    let (line, code) = match right_hostpath::ensure_on_path(&bindir, &home, shell.as_deref()) {
        Ok(right_hostpath::EnsureOutcome::AlreadyOnPath) => (
            right_ui::status(right_ui::Glyph::Ok)
                .noun("PATH")
                .verb("ready"),
            0,
        ),
        Ok(right_hostpath::EnsureOutcome::Wrote { file }) => (
            right_ui::status(right_ui::Glyph::Ok)
                .noun("PATH")
                .verb(format!("added to {}", file.display()))
                .fix(format!(
                    "open a new shell, or run: source {}",
                    file.display()
                )),
            0,
        ),
        Ok(right_hostpath::EnsureOutcome::CouldNotWrite { file, reason }) => (
            right_ui::status(right_ui::Glyph::Warn)
                .noun("PATH")
                .verb(format!("couldn't update {}", file.display()))
                .detail(reason)
                .fix(manual_fix),
            PATH_SETUP_RC_WRITE_FAILED,
        ),
        Err(e) => (
            right_ui::status(right_ui::Glyph::Warn)
                .noun("PATH")
                .verb("couldn't set up PATH")
                .detail(format!("{e:#}"))
                .fix(manual_fix),
            PATH_SETUP_RC_WRITE_FAILED,
        ),
    };

    println!("{}", line.render(theme));
    std::process::exit(code);
}

async fn cmd_doctor(home: &Path) -> miette::Result<()> {
    let theme = right_ui::detect();
    let checks = right_agent::doctor::run_doctor(home).await;

    println!("{}", right_ui::section(theme, "diagnostics"));
    println!("{}", right_ui::Rail::blank(theme));

    let mut block = right_ui::Block::new();
    for check in &checks {
        block.push(check.to_ui_line());
    }
    println!("{}", block.render(theme));
    println!("{}", right_ui::Rail::blank(theme));

    let pass = checks
        .iter()
        .filter(|c| matches!(c.status, right_agent::doctor::CheckStatus::Pass))
        .count();
    let warn = checks
        .iter()
        .filter(|c| matches!(c.status, right_agent::doctor::CheckStatus::Warn))
        .count();
    let fail = checks
        .iter()
        .filter(|c| matches!(c.status, right_agent::doctor::CheckStatus::Fail))
        .count();
    let total = checks.len();

    let summary = if warn == 0 && fail == 0 {
        format!("{pass}/{total} checks passed")
    } else {
        let mut parts = Vec::new();
        if warn > 0 {
            parts.push(format!("{warn} warn"));
        }
        if fail > 0 {
            parts.push(format!("{fail} fail"));
        }
        format!("{pass}/{total} checks passed ({})", parts.join(", "))
    };
    println!("{}{}", right_ui::Rail::prefix(theme), summary);

    if fail > 0 {
        return Err(miette::miette!("checks failed — see above for fixes"));
    }
    Ok(())
}

fn cmd_list(home: &Path) -> miette::Result<()> {
    let agents_dir = right_config::agents_dir(home);
    if !agents_dir.exists() {
        println!("No agents directory found. Run `right init` first.");
        return Ok(());
    }

    let agents = right_agent::agent::discover_agents(&agents_dir)?;
    if agents.is_empty() {
        println!("No agents found in {}", agents_dir.display());
    } else {
        println!("Discovered {} agent(s):", agents.len());
        for agent in &agents {
            let config_status = if agent.config.is_some() { "yes" } else { "no" };
            let mcp_status = if agent.path.join("mcp.json").exists() {
                "yes"
            } else {
                "no"
            };
            println!(
                "  {:<20} {}    config: {}    mcp: {}",
                agent.name,
                agent.path.display(),
                config_status,
                mcp_status,
            );
        }
    }
    Ok(())
}

struct UpAgentDiscovery {
    agents: Vec<right_agent::agent::AgentDef>,
    issues: Vec<String>,
    /// Agents whose config is well-formed but still points at an OpenShell
    /// sandbox. Kept apart from `issues` because this is a transitional state
    /// with a known fix, not a broken config: it must not stop the agents that
    /// *can* run from starting.
    unmigrated: Vec<String>,
}

fn discover_up_agents(
    agents_dir: &Path,
    filter: Option<&[String]>,
) -> miette::Result<UpAgentDiscovery> {
    let paths = if let Some(names) = filter {
        names
            .iter()
            .map(|name| agents_dir.join(name))
            .collect::<Vec<_>>()
    } else {
        let entries = std::fs::read_dir(agents_dir)
            .map_err(|error| miette::miette!("cannot read {}: {error:#}", agents_dir.display()))?;
        let mut paths = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|error| {
                miette::miette!(
                    "cannot read an entry in {}: {error:#}",
                    agents_dir.display()
                )
            })?;
            if entry
                .file_type()
                .map_err(|error| {
                    miette::miette!("cannot inspect {}: {error:#}", entry.path().display())
                })?
                .is_dir()
            {
                paths.push(entry.path());
            }
        }
        paths.sort();
        paths
    };

    let mut agents = Vec::new();
    let mut issues = Vec::new();
    let mut unmigrated = Vec::new();
    for path in paths {
        let name = path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("<invalid>");
        match right_agent::agent::discover_single_agent(&path) {
            Ok(agent) => agents.push(agent),
            Err(error) => {
                // The strict parser rejects `sandbox.mode: openshell`, which is
                // exactly how an unmigrated agent is kept from starting. Read
                // the raw yaml to tell that expected state apart from a config
                // that is actually malformed.
                if is_unmigrated_agent(&path) {
                    unmigrated.push(name.to_owned());
                } else {
                    issues.push(format!("{name}: configuration failed: {error:#}"));
                }
            }
        }
    }
    Ok(UpAgentDiscovery {
        agents,
        issues,
        unmigrated,
    })
}

/// Whether `agent.yaml` is a well-formed config that simply has not been
/// migrated yet.
fn is_unmigrated_agent(agent_dir: &Path) -> bool {
    let Ok(yaml) = std::fs::read_to_string(agent_dir.join("agent.yaml")) else {
        return false;
    };
    matches!(
        crate::migrate_sandbox::migration_source(&yaml),
        Ok(crate::migrate_sandbox::MigrationSource::OpenShell { .. })
    )
}

fn readiness_error(issues: &[String]) -> miette::Report {
    miette::miette!(
        help = "Fix every item below, then rerun `right up --non-interactive`; run `right up` without the flag for targeted interactive repair",
        "right up readiness failed:\n  - {}",
        issues.join("\n  - ")
    )
}

async fn run_up_preflight<G, GuardFuture, R, ReadinessFuture>(
    guard: G,
    readiness: R,
) -> miette::Result<()>
where
    G: FnOnce() -> GuardFuture,
    GuardFuture: Future<Output = miette::Result<()>>,
    R: FnOnce() -> ReadinessFuture,
    ReadinessFuture: Future<Output = miette::Result<()>>,
{
    guard().await?;
    readiness().await
}

async fn ensure_up_runtime_available(home: &Path) -> miette::Result<()> {
    if let Some(client) = right_agent::runtime::PcClient::from_home(home)?
        && client.health_check().await.is_ok()
    {
        return Err(miette::miette!(
            "right is already running. Use `right down` first or `right attach` to connect."
        ));
    }
    check_port_available(right_runtime_state::MCP_HTTP_PORT).await
}

fn non_interactive_readiness_result(issues: Vec<String>) -> miette::Result<()> {
    if issues.is_empty() {
        Ok(())
    } else {
        Err(readiness_error(&issues))
    }
}

async fn repair_telegram_token(agent: &right_agent::agent::AgentDef) -> miette::Result<()> {
    let existing = agent
        .config
        .as_ref()
        .and_then(|config| config.telegram_token.as_deref());
    loop {
        let token = match crate::wizard::telegram_setup(existing, true, true)? {
            crate::wizard::TelegramSetupOutcome::Token(token) => token,
            crate::wizard::TelegramSetupOutcome::Skipped
            | crate::wizard::TelegramSetupOutcome::Back => {
                return Err(miette::miette!("Telegram token repair cancelled"));
            }
        };
        match right_bot::validate_telegram_token_live(&token).await {
            Ok(()) => {
                crate::wizard::set_agent_telegram_token(&agent.path.join("agent.yaml"), &token)?;
                return Ok(());
            }
            Err(error) => eprintln!("Telegram rejected the replacement token: {error:#}"),
        }
    }
}

/// Readiness that `right up` can check from the host, without a sandbox.
///
/// Claude authentication is deliberately absent: the credential is only
/// usable from inside the agent's sandbox, which the bot's supervisor creates
/// during bring-up. Probing it here would mean attaching to a sandbox that
/// does not exist yet and reporting a missing microVM as a bad setup token —
/// in interactive mode, an endless setup-token prompt for a problem no token
/// can fix. Bring-up and the keepalive probe report auth failures instead, so
/// there is exactly one place that validates the credential.
trait AgentReadinessBackend {
    async fn validate_telegram(
        &mut self,
        agent: &right_agent::agent::AgentDef,
    ) -> miette::Result<()>;
    async fn repair_telegram(&mut self, agent: &right_agent::agent::AgentDef)
    -> miette::Result<()>;
}

struct LiveAgentReadiness;

impl AgentReadinessBackend for LiveAgentReadiness {
    async fn validate_telegram(
        &mut self,
        agent: &right_agent::agent::AgentDef,
    ) -> miette::Result<()> {
        let token = agent
            .config
            .as_ref()
            .and_then(|config| config.telegram_token.as_deref())
            .ok_or_else(|| miette::miette!("Telegram token is missing"))?;
        right_bot::validate_telegram_token_live(token)
            .await
            .map_err(|error| miette::miette!("{error:#}"))
    }

    async fn repair_telegram(
        &mut self,
        agent: &right_agent::agent::AgentDef,
    ) -> miette::Result<()> {
        repair_telegram_token(agent).await
    }
}

async fn validate_agent_readiness_with<B: AgentReadinessBackend>(
    agents: &[right_agent::agent::AgentDef],
    interactive: bool,
    backend: &mut B,
    issues: &mut Vec<String>,
) -> miette::Result<()> {
    for agent in agents {
        if agent.config.is_none() {
            issues.push(format!("{}: agent.yaml is missing", agent.name));
            continue;
        }

        if let Err(error) = backend.validate_telegram(agent).await {
            if interactive {
                eprintln!(
                    "Agent `{}` Telegram readiness failed: {error:#}",
                    agent.name
                );
                backend.repair_telegram(agent).await?;
            } else {
                issues.push(format!(
                    "{}: Telegram validation failed: {error:#}. Repair: run `right up` interactively or set telegram-token through `right config {}`",
                    agent.name, agent.name
                ));
            }
        }
    }
    Ok(())
}

async fn validate_up_readiness(
    home: &Path,
    agents: &[right_agent::agent::AgentDef],
    non_interactive: bool,
    mut issues: Vec<String>,
) -> miette::Result<()> {
    let interactive = !non_interactive;
    if interactive && let Some(issue) = issues.first() {
        return Err(miette::miette!(
            help = "Repair the selected agent configuration, then rerun `right up`",
            "selected agent configuration is invalid: {issue}"
        ));
    }

    if let Some(failure) = configured_tunnel_failure(home)? {
        if interactive {
            eprintln!("Configured tunnel is not ready: {}", failure.error);
            crate::wizard::repair_configured_tunnel(home, &failure.config)?;
            validate_configured_tunnel(home)?;
        } else {
            issues.push(format!(
                "global tunnel: {}. Repair: run `right up` interactively or `right config`",
                failure.error
            ));
        }
    }

    let mut backend = LiveAgentReadiness;
    validate_agent_readiness_with(agents, interactive, &mut backend, &mut issues).await?;

    non_interactive_readiness_result(issues)
}

async fn cmd_up(
    home: &Path,
    agents_filter: Option<Vec<String>>,
    detach: bool,
    debug: bool,
    non_interactive: bool,
) -> miette::Result<()> {
    let t_total = std::time::Instant::now();
    let mut t_phase = std::time::Instant::now();

    // Fail fast if required tools are missing.
    right_agent::runtime::verify_dependencies()?;
    tracing::info!(
        elapsed_ms = t_phase.elapsed().as_millis() as u64,
        "up: verify_dependencies"
    );
    t_phase = std::time::Instant::now();
    // Exclude every offline direct database command from the entire startup
    // publication window. The guard is released only after process-compose is
    // reachable; offline commands recheck runtime state while holding this same lock.
    let startup_guard = right_agent::runtime::acquire_runtime_exclusion(home).await?;

    let run_dir = home.join("run");

    // Enumerate the selected raw directories before parsing so invalid entries
    // become readiness issues instead of being silently skipped or preventing
    // checks of other selected agents.
    let agents_dir = right_config::agents_dir(home);
    let discovery = discover_up_agents(&agents_dir, agents_filter.as_deref())?;
    if discovery.agents.is_empty() && discovery.issues.is_empty() && discovery.unmigrated.is_empty()
    {
        return Err(miette::miette!(
            "no agents found. Run `right agent init <name>` to create one."
        ));
    }
    // An unmigrated agent cannot start, but it must not hold back the ones
    // that can: say plainly which are sitting out and how to bring them over.
    if !discovery.unmigrated.is_empty() {
        for name in &discovery.unmigrated {
            eprintln!(
                "Agent `{name}` still lives in an OpenShell sandbox and is not being started. \
                 Move it over with: right agent migrate-sandbox {name}"
            );
        }
        if discovery.agents.is_empty() {
            return Err(miette::miette!(
                help = "Run `right agent migrate-sandbox <name>` for each, then rerun `right up`",
                "every selected agent still lives in an OpenShell sandbox"
            ));
        }
    }
    let mut agents = discovery.agents;

    // Validate the mandatory global configuration before the fixed-port probe
    // so an isolated, misconfigured home reports its own actionable error even
    // if another Right home is already running. Runtime availability still
    // precedes network-backed agent readiness and interactive repair.
    right_config::read_global_config(home)?;
    run_up_preflight(
        || ensure_up_runtime_available(home),
        || validate_up_readiness(home, &agents, non_interactive, discovery.issues),
    )
    .await?;
    // Reload before codegen so the launched configuration is the validated one.
    if !non_interactive {
        let refreshed = discover_up_agents(&agents_dir, agents_filter.as_deref())?;
        if let Some(issue) = refreshed.issues.first() {
            return Err(miette::miette!(
                "selected agent configuration is invalid: {issue}"
            ));
        }
        agents = refreshed.agents;
    }

    tracing::info!(
        elapsed_ms = t_phase.elapsed().as_millis() as u64,
        agents = agents.len(),
        "up: readiness"
    );
    t_phase = std::time::Instant::now();

    // Download any whisper models needed by STT-enabled agents.
    {
        use right_agent_config::WhisperModel;
        use std::collections::HashSet;

        let mut models: HashSet<WhisperModel> = HashSet::new();
        for agent in &agents {
            if let Some(cfg) = agent.config.as_ref()
                && cfg.stt.enabled
            {
                models.insert(cfg.stt.model);
            }
        }
        if !models.is_empty() {
            println!("Ensuring whisper models are cached...");
            if let Err(e) = right_stt::ensure_models_cached(home, &models).await {
                eprintln!("warning: model cache step failed: {e:#}");
            }
        }
    }
    tracing::info!(
        elapsed_ms = t_phase.elapsed().as_millis() as u64,
        "up: whisper_models_cache"
    );
    t_phase = std::time::Instant::now();

    // Clear rightcron init locks so the bootstrap hook fires on this session.
    for agent in &agents {
        let lock = agent.path.join(".rightcron-init-done");
        let _ = std::fs::remove_file(&lock);
    }

    // Resolve current executable path once — written into each agent's mcp.json so the
    // right MCP server can be found even when right is not on PATH (process-compose).
    let self_exe = std::env::current_exe()
        .map_err(|e| miette::miette!("failed to resolve current executable path: {e:#}"))?;

    // Run cross-agent codegen: token map, cloudflared config,
    // process-compose.yaml, and runtime state.
    right_codegen::run_agent_codegen(home, &agents, &self_exe, debug)?;
    tracing::info!(
        elapsed_ms = t_phase.elapsed().as_millis() as u64,
        "up: cross_agent_codegen"
    );

    // Check that at least one agent has a Telegram token configured.
    let has_bot_agents = agents.iter().any(|a| {
        a.config
            .as_ref()
            .map(|c| c.telegram_token.is_some())
            .unwrap_or(false)
    });
    if !has_bot_agents {
        eprintln!("right: no agents have Telegram tokens configured — nothing to start");
        return Err(miette::miette!("no agents have Telegram tokens configured"));
    }

    // Build process-compose command.
    let config_path = run_dir.join("process-compose.yaml");
    let mut cmd = tokio::process::Command::new("process-compose");
    // Use TCP API (avoids --use-uds which crashes TUI).
    let pc_port = right_runtime_state::PC_PORT.to_string();
    cmd.args([
        "up",
        "-f",
        config_path.to_str().unwrap_or_default(),
        "--port",
        &pc_port,
    ]);

    // Read the API token from state.json (just written by codegen) and inject
    // as PC_API_TOKEN env var. process-compose then rejects any unauthenticated
    // REST API request — prevents stray HTTP callers from stopping production bots.
    let state_path = run_dir.join("state.json");
    if let Ok(state) = right_runtime_state::read_state(&state_path)
        && let Some(token) = &state.pc_api_token
    {
        cmd.env("PC_API_TOKEN", token);
    }

    tracing::info!(
        total_pre_pc_ms = t_total.elapsed().as_millis() as u64,
        detach,
        "up: spawning process-compose"
    );

    if detach {
        cmd.arg("--detached");
        let child = cmd
            .spawn()
            .map_err(|e| miette::miette!("failed to spawn process-compose: {e:#}"))?;

        // Wait briefly for process-compose to start.
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;

        // Verify it's alive. `run_agent_codegen` above wrote state.json, so
        // `from_home` resolves to Some(_); missing state here would be a bug.
        let client = right_agent::runtime::PcClient::from_home(home)?.ok_or_else(|| {
            miette::miette!("runtime state missing after codegen — refusing to health-check")
        })?;
        client.health_check().await.map_err(|e| {
            miette::miette!("process-compose started but health check failed: {e:#}")
        })?;
        drop(startup_guard);

        println!(
            "right started in background ({} agent(s)). Use `right attach` to view TUI.",
            agents.len()
        );

        // Drop child handle without killing -- it's detached.
        drop(child);
    } else {
        let mut child = cmd
            .spawn()
            .map_err(|e| miette::miette!("failed to spawn process-compose: {e:#}"))?;
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        let client = right_agent::runtime::PcClient::from_home(home)?.ok_or_else(|| {
            miette::miette!("runtime state missing after codegen — refusing to health-check")
        })?;
        client.health_check().await.map_err(|e| {
            miette::miette!("process-compose started but health check failed: {e:#}")
        })?;
        drop(startup_guard);
        let status = child
            .wait()
            .await
            .map_err(|e| miette::miette!("failed to wait for process-compose: {e:#}"))?;

        if !status.success() {
            return Err(miette::miette!(
                "process-compose exited with status: {}",
                status
            ));
        }
    }

    Ok(())
}

/// Fail fast if a required port is already occupied by a stale process.
async fn check_port_available(port: u16) -> miette::Result<()> {
    match tokio::net::TcpListener::bind(("127.0.0.1", port)).await {
        Ok(_listener) => Ok(()), // bound successfully → port is free
        Err(_) => Err(miette::miette!(
            help = "A previous right session may still be running. Kill it first:\n  \
                    killall right  # or: right down",
            "port {port} is already in use"
        )),
    }
}

async fn cmd_stt_preload(
    home: &Path,
    models: &[right_agent_config::WhisperModel],
) -> miette::Result<()> {
    use std::collections::HashSet;

    let theme = right_ui::detect();
    println!("{}", right_ui::section(theme, "whisper preload"));
    println!("{}", right_ui::Rail::blank(theme));

    // Dedup while preserving the order given on the command line.
    let mut seen: HashSet<right_agent_config::WhisperModel> = HashSet::new();
    let unique: Vec<right_agent_config::WhisperModel> =
        models.iter().copied().filter(|m| seen.insert(*m)).collect();

    let mut block = right_ui::Block::new();
    for model in unique {
        let dest = right_stt::model_cache_path(home, model);
        if right_stt::is_model_cached(&dest) {
            block.push(
                right_ui::status(right_ui::Glyph::Ok)
                    .noun(model.filename())
                    .verb("already cached"),
            );
            continue;
        }
        right_stt::download_model(model, &dest)
            .await
            .map_err(|e| miette::miette!("download {} failed: {e:#}", model.filename()))?;
        block.push(
            right_ui::status(right_ui::Glyph::Ok)
                .noun(model.filename())
                .verb("downloaded"),
        );
    }

    println!("{}", block.render(theme));
    println!("{}", right_ui::Rail::blank(theme));
    Ok(())
}

async fn cmd_down(home: &Path) -> miette::Result<()> {
    let client = right_agent::runtime::PcClient::from_home(home)?.ok_or_else(|| {
        miette::miette!(
            help = "Start right first with `right up`",
            "No running instance found. Is right running?"
        )
    })?;

    client
        .health_check()
        .await
        .map_err(|_| miette::miette!("No running instance found. Is right running?"))?;

    client.shutdown().await.map_err(|e| {
        miette::miette!("Shutdown request failed (process-compose may already be stopped): {e:#}")
    })?;

    println!("All agents stopped.");
    Ok(())
}

async fn cmd_reload(home: &Path, _agents_filter: Option<Vec<String>>) -> miette::Result<()> {
    let client = right_agent::runtime::PcClient::from_home(home)?.ok_or_else(|| {
        miette::miette!(
            help = "Start right first with `right up`",
            "nothing running — cannot reload"
        )
    })?;
    client.health_check().await.map_err(|_| {
        miette::miette!(
            help = "Start right first with `right up`",
            "nothing running — cannot reload"
        )
    })?;

    let agents_dir = right_config::agents_dir(home);
    let all_agents = right_agent::agent::discover_agents(&agents_dir)?;

    if all_agents.is_empty() {
        return Err(miette::miette!(
            "no agents found. Run `right agent init <name>` to create one."
        ));
    }

    let self_exe = std::env::current_exe()
        .map_err(|e| miette::miette!("failed to resolve current executable path: {e:#}"))?;

    let codegen_outcome = right_codegen::run_agent_codegen(home, &all_agents, &self_exe, false)?;

    client.reload_configuration().await?;
    client
        .restart_cloudflared_or_warn(codegen_outcome.cloudflared_config_changed)
        .await;

    // Notify aggregator to pick up new agents from updated token map
    let socket_path = home.join("run/internal.sock");
    let internal = right_mcp::internal_client::InternalClient::new(&socket_path);
    match internal.reload().await {
        Ok(resp) => {
            if !resp.added.is_empty() {
                println!(
                    "Registered {} new agent(s) in aggregator: {}",
                    resp.added.len(),
                    resp.added.join(", "),
                );
            }
            if !resp.removed.is_empty() {
                println!(
                    "Removed {} agent(s) from aggregator: {}",
                    resp.removed.len(),
                    resp.removed.join(", "),
                );
            }
        }
        Err(e) => {
            eprintln!("warning: failed to reload aggregator: {e:#}");
        }
    }

    let has_bot = all_agents.iter().any(|a| {
        a.config
            .as_ref()
            .map(|c| c.telegram_token.is_some())
            .unwrap_or(false)
    });
    if !has_bot {
        eprintln!("right: warning: no agents have Telegram tokens — nothing will run");
    }

    println!("Reloaded. Active agents:");
    for agent in &all_agents {
        let has_token = agent
            .config
            .as_ref()
            .map(|c| c.telegram_token.is_some())
            .unwrap_or(false);
        let status = if has_token {
            "bot"
        } else {
            "no token (skipped)"
        };
        println!("  {:<20} {}", agent.name, status);
    }

    Ok(())
}

async fn cmd_status(home: &Path) -> miette::Result<()> {
    let theme = right_ui::detect();

    println!("{}", right_ui::section(theme, "status"));
    println!("{}", right_ui::Rail::blank(theme));

    let Some(client) = right_agent::runtime::PcClient::from_home(home)? else {
        let line = right_ui::status(right_ui::Glyph::Err)
            .noun("right agent")
            .verb("not running")
            .fix("right up")
            .render(theme);
        println!("{line}");
        return Err(right_ui::BlockAlreadyRendered.into());
    };

    if client.health_check().await.is_err() {
        let line = right_ui::status(right_ui::Glyph::Err)
            .noun("right agent")
            .verb("not running")
            .fix("right up")
            .render(theme);
        println!("{line}");
        return Err(right_ui::BlockAlreadyRendered.into());
    }

    let processes = client.list_processes().await?;

    if processes.is_empty() {
        let line = right_ui::status(right_ui::Glyph::Err)
            .noun("right agent")
            .verb("no processes")
            .fix("right up")
            .render(theme);
        println!("{line}");
        return Err(right_ui::BlockAlreadyRendered.into());
    }

    let mut block = right_ui::Block::new();
    for p in &processes {
        let glyph = match p.status.as_str() {
            "Running" => right_ui::Glyph::Ok,
            "Restarting" | "Pending" => right_ui::Glyph::Warn,
            _ => right_ui::Glyph::Err,
        };
        let verb = format!("{:<6} {}", p.pid, p.system_time);
        block.push(right_ui::status(glyph).noun(&p.name).verb(verb));
    }
    println!("{}", block.render(theme));
    println!("{}", right_ui::Rail::blank(theme));

    let warn = processes
        .iter()
        .filter(|p| matches!(p.status.as_str(), "Restarting" | "Pending"))
        .count();
    let fail = processes
        .iter()
        .filter(|p| !matches!(p.status.as_str(), "Running" | "Restarting" | "Pending"))
        .count();
    let total = processes.len();
    let summary = if warn == 0 && fail == 0 {
        format!("{total} processes")
    } else {
        let mut parts = Vec::new();
        if warn > 0 {
            parts.push(format!("{warn} warn"));
        }
        if fail > 0 {
            parts.push(format!("{fail} fail"));
        }
        format!("{total} processes ({})", parts.join(", "))
    };
    println!("{}{}", right_ui::Rail::prefix(theme), summary);

    Ok(())
}

async fn cmd_restart(_home: &Path, _agent: &str) -> miette::Result<()> {
    // process-compose crashes on programmatic restart (both REST API and CLI client).
    // This is a known process-compose bug. Direct users to the TUI instead.
    Err(miette::miette!(
        help = "Use the process-compose TUI: select the agent and press Ctrl+R to restart",
        "Programmatic restart is not supported (process-compose bug). Use `right attach` and Ctrl+R instead."
    ))
}

fn cmd_attach(home: &Path) -> miette::Result<()> {
    use std::os::unix::process::CommandExt;

    // Read the recorded PC port for this home. With an isolated --home and no
    // prior `right up`, there is nothing to attach to — fail loudly.
    let state_path = home.join("run").join("state.json");
    let state = right_runtime_state::read_state(&state_path).map_err(|e| {
        miette::miette!(
            help = "Start right first with `right up`",
            "No running instance recorded at {} ({e:#})",
            state_path.display(),
        )
    })?;

    let err = std::process::Command::new("process-compose")
        .arg("attach")
        .arg("--port")
        .arg(state.pc_port.to_string())
        .exec();

    Err(miette::miette!("Failed to attach: {err}"))
}
/// Guest path the backup tar is staged at before extraction. Removed again
/// as soon as `tar` has read it, so a restored sandbox carries no copy of
/// its own backup.
const RESTORE_TAR_GUEST_PATH: &str = "/tmp/right-restore.tar.gz";

/// How long the in-guest extraction may take before it is killed.
const RESTORE_EXTRACT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(600);

/// Push a backup tar into a freshly created sandbox and unpack it over the
/// agent's guest home.
///
/// Mirrors what the backup side wrote: the archive holds a `sandbox/` prefix,
/// so extraction strips one component into [`right_sandbox::GUEST_HOME`].
/// `-p` preserves the modes the backup captured, which is what keeps the
/// read-only `.platform` tree read-only.
async fn upload_restore_tar(
    sandbox: &right_sandbox::SandboxHandle,
    tar_path: &Path,
) -> miette::Result<()> {
    sandbox
        .fs_copy_from_host(tar_path, RESTORE_TAR_GUEST_PATH)
        .await
        .map_err(|error| {
            miette::miette!("upload {} into the sandbox: {error:#}", tar_path.display())
        })?;

    let request = right_sandbox::ExecRequest {
        cmd: "tar".to_owned(),
        args: vec![
            "xzpf".to_owned(),
            RESTORE_TAR_GUEST_PATH.to_owned(),
            "-C".to_owned(),
            right_sandbox::GUEST_HOME.to_owned(),
            "--strip-components=1".to_owned(),
            "sandbox".to_owned(),
        ],
        user: Some("0".to_owned()),
        timeout: Some(RESTORE_EXTRACT_TIMEOUT),
        ..right_sandbox::ExecRequest::default()
    };
    let outcome = sandbox
        .exec(&request)
        .await
        .map_err(|error| miette::miette!("extract the backup in the sandbox: {error:#}"))?;
    if !outcome.success() {
        return Err(miette::miette!(
            "extracting the backup in the sandbox exited with {}: {}",
            outcome.code,
            String::from_utf8_lossy(&outcome.stderr).trim(),
        ));
    }

    sandbox
        .fs_remove(RESTORE_TAR_GUEST_PATH)
        .await
        .map_err(|error| miette::miette!("remove the staged backup from the sandbox: {error:#}"))
}

#[derive(Debug)]
struct RestoreCleanupPlan {
    agent_dir: PathBuf,
    sandbox_name: Option<String>,
}

impl RestoreCleanupPlan {
    fn new(agent_dir: PathBuf) -> Self {
        Self {
            agent_dir,
            sandbox_name: None,
        }
    }

    fn track_sandbox(&mut self, sandbox_name: String) {
        self.sandbox_name = Some(sandbox_name);
    }
}

/// Roll back a failed restore.
///
/// Deletes the sandbox the restore created, then the half-populated agent
/// directory. A sandbox that cannot be deleted keeps the agent directory:
/// that directory is the operator's only handle on the state still out there,
/// so removing it would strand a live microVM with no record of its owner.
async fn cleanup_failed_restore(plan: &RestoreCleanupPlan) -> miette::Result<()> {
    cleanup_failed_restore_with(plan, |sandbox_name| async move {
        right_sandbox::SandboxHandle::delete(&sandbox_name)
            .await
            .map(|_| ())
            .map_err(|error| miette::miette!("{error:#}"))
    })
    .await
}

async fn cleanup_failed_restore_with<D, DeleteFuture>(
    plan: &RestoreCleanupPlan,
    delete_sandbox: D,
) -> miette::Result<()>
where
    D: FnOnce(String) -> DeleteFuture,
    DeleteFuture: Future<Output = miette::Result<()>>,
{
    if let Some(sandbox_name) = &plan.sandbox_name {
        delete_sandbox(sandbox_name.clone()).await.map_err(|error| {
            miette::miette!(
                "failed to delete restore sandbox '{sandbox_name}': {error:#}; retaining recovery state at {}",
                plan.agent_dir.display()
            )
        })?;
    }
    cleanup_failed_restore_agent_dir(&plan.agent_dir)
}

async fn open_provider_store_for_restore(
    home: &Path,
    _quiescence_guard: &right_agent::runtime::RuntimeExclusionGuard,
) -> miette::Result<right_providers::ProviderStore> {
    right_providers::ProviderStore::open(home)
        .await
        .map_err(|error| miette::miette!("open provider store: {error:#}"))
}

async fn migrate_restored_agent_db(
    agent_dir: &Path,
    _quiescence_guard: &right_agent::runtime::RuntimeExclusionGuard,
) -> miette::Result<()> {
    right_db::open_db(agent_dir, true)
        .await
        .map_err(|error| miette::miette!("failed to migrate restored data.db: {error:#}"))?;
    Ok(())
}

#[allow(clippy::needless_borrow)]
async fn cmd_agent_restore(
    home: &Path,
    agent_name: &str,
    backup_path: &Path,
    restore_binding_mode: restore::RestoreBindingMode,
    claude_setup_token: &str,
    quiescence_guard: &right_agent::runtime::RuntimeExclusionGuard,
) -> miette::Result<()> {
    use miette::IntoDiagnostic;

    // 1. Validate preconditions.
    let agents_dir = right_config::agents_dir(home);
    let agent_dir = agents_dir.join(agent_name);

    if agent_dir.exists() {
        return Err(miette::miette!(
            help = "Remove the existing agent first, or choose a different name",
            "Agent '{}' already exists at {}",
            agent_name,
            agent_dir.display()
        ));
    }

    let tar_path = backup_path.join("sandbox.tar.gz");
    if !tar_path.exists() {
        return Err(miette::miette!(
            "sandbox.tar.gz not found in backup directory {}",
            backup_path.display()
        ));
    }

    let agent_yaml_src = backup_path.join("agent.yaml");
    if !agent_yaml_src.exists() {
        return Err(miette::miette!(
            help = "Full backups (not --sandbox-only) include agent.yaml",
            "agent.yaml not found in backup directory {}",
            backup_path.display()
        ));
    }

    // 2. Parse backup config and resolve binding semantics before creating
    // partial target state. Direct restores with ambiguous implicit Hindsight
    // bindings must fail without leaving home/agents/<target> behind.
    let backup_config = right_agent::agent::discovery::parse_agent_config(backup_path)?;
    let backup_config = backup_config.ok_or_else(|| {
        miette::miette!(
            "agent.yaml exists but parsed config is unavailable at {}",
            backup_path.display()
        )
    })?;

    let effective_restore_binding_mode = if matches!(
        restore_binding_mode,
        restore::RestoreBindingMode::Interactive
    ) {
        if restore::restore_binding_choice_required(home, agent_name, backup_path, &backup_config)?
        {
            prompt_restore_binding_mode()?
        } else {
            restore::RestoreBindingMode::DirectUnspecified
        }
    } else {
        restore_binding_mode
    };

    let restore_decision = restore::decide_restore(
        home,
        agent_name,
        backup_path,
        &backup_config,
        effective_restore_binding_mode,
    )
    .await?;

    // Tunnel configuration is the last pre-creation boundary after validating
    // the restore request itself. External providers receive shape validation
    // only; Right cannot probe operator-owned ingress reachability here.
    preflight_agent_init_tunnel(home)?;

    let theme = right_ui::detect();
    println!(
        "{}",
        right_ui::section(theme, &format!("agent restore: {agent_name}"))
    );
    println!("{}", right_ui::Rail::blank(theme));

    // Emit warnings before any persistent side effects so the operator sees
    // them even if a later step fails or leaves partial state behind.
    if !restore_decision.warnings.is_empty() {
        let mut warn_block = right_ui::Block::new();
        for warning in &restore_decision.warnings {
            warn_block.push(
                right_ui::status(right_ui::Glyph::Warn)
                    .noun("clone restore")
                    .detail(warning),
            );
        }
        right_ui::stderr(theme, &warn_block.render(theme));
    }

    // 3. Create agent dir and restore config files.
    std::fs::create_dir_all(&agent_dir)
        .into_diagnostic()
        .map_err(|e| {
            miette::miette!("failed to create agent dir {}: {e:#}", agent_dir.display())
        })?;
    let mut cleanup_plan = RestoreCleanupPlan::new(agent_dir.clone());

    // Wrap the rest of the function in an inner async block so any intermediate
    // failure between here and the success return unifies cleanup of the
    // half-populated agent dir, instead of relying on ad-hoc per-callsite rollback.
    let result: miette::Result<()> = async {
        copy_agent_restore_config_files(backup_path, &agent_dir)?;
        remove_database_sidecars(&agent_dir)?;

        // 4. Sandboxed restore: normalize restored agent.yaml before codegen
        // or sandbox creation can use it, then create new sandbox and upload
        // tar contents.
        restore::apply_memory_action(
            &agent_dir.join("agent.yaml"),
            backup_config.clone(),
            restore_decision.memory_action.clone(),
        )?;

        // Parsing the restored agent.yaml validates it before the sandbox is
        // created; the values themselves come from `backup_config`.
        right_agent::agent::discovery::parse_agent_config(&agent_dir)?;

        let timestamp = chrono::Local::now().format("%Y%m%d-%H%M").to_string();
        let new_sandbox_name =
            right_sandbox::fit_sandbox_name(&format!("right-{agent_name}-{timestamp}"));

        // Codegen needs a discoverable agent. Create a minimal IDENTITY.md
        // placeholder so discover_single_agent succeeds (the real one is
        // inside the tar).
        let identity_path = agent_dir.join("IDENTITY.md");
        if !identity_path.exists() {
            std::fs::write(&identity_path, "# Placeholder (restoring from backup)\n")
                .into_diagnostic()
                .map_err(|e| miette::miette!("failed to write placeholder IDENTITY.md: {e:#}"))?;
        }

        let agent_def = right_agent::agent::discover_single_agent(&agent_dir)?;
        let self_exe = std::env::current_exe()
            .into_diagnostic()
            .map_err(|e| miette::miette!("failed to resolve self exe: {e:#}"))?;

        right_codegen::run_single_agent_codegen(home, &agent_def, &self_exe, false).await?;

        // Create the restore target through the same spec builder the bot's
        // supervisor uses, so the sandbox this restore hands over is
        // indistinguishable from one the bot created: egress and secret
        // structure are create-time only, and a mismatch here would need
        // another recreate to undo.
        let restored_config = agent_def.config.as_ref().ok_or_else(|| {
            miette::miette!("restored agent.yaml is missing before sandbox create")
        })?;
        let providers = open_provider_store_for_restore(home, &quiescence_guard).await?;
        // Serialize restore bring-up against the bot's sandbox supervisor,
        // which reads credentials and applies them under this same per-agent
        // lock. The spec build below resolves every declared provider's
        // credential, and `create_or_attach` installs them, so both must run
        // while holding the authoritative lock.
        let _provider_guard = providers.agent_lock(agent_name).await.map_err(|error| {
            miette::miette!("lock provider store for agent {agent_name}: {error:#}")
        })?;
        let spec = right_bot::agent_sandbox_spec_for_offline(
            agent_name,
            &new_sandbox_name,
            restored_config,
            &providers,
        )
        .await?;

        right_sandbox::ensure_runtime_installed()
            .await
            .map_err(|error| miette::miette!("install the sandbox runtime: {error:#}"))?;
        right_sandbox::diagnose_host()
            .map_err(|error| miette::miette!("this host cannot run microVMs: {error:#}"))?;

        println!(
            "{}",
            right_ui::status(right_ui::Glyph::Info)
                .noun("sandbox")
                .verb("creating")
                .detail(&new_sandbox_name)
                .render(theme)
        );
        let sandbox = right_sandbox::SandboxHandle::create_or_attach(&spec)
            .await
            .map_err(|error| {
                miette::miette!("create restore sandbox '{new_sandbox_name}': {error:#}")
            })?;
        drop(_provider_guard);
        cleanup_plan.track_sandbox(new_sandbox_name.clone());
        sandbox
            .wait_ready(right_sandbox::DEFAULT_READY_TIMEOUT)
            .await
            .map_err(|error| {
                miette::miette!(
                    "restore sandbox '{new_sandbox_name}' never became ready: {error:#}"
                )
            })?;

        println!(
            "{}",
            right_ui::status(right_ui::Glyph::Info)
                .noun("sandbox backup")
                .verb("uploading")
                .render(theme)
        );
        upload_restore_tar(&sandbox, &tar_path)
            .await
            .map_err(|error| {
                miette::miette!(
                    "Sandbox restore failed for agent '{}' in new sandbox '{}': {error:#}",
                    agent_name,
                    new_sandbox_name,
                )
            })?;
        println!(
            "{}",
            right_ui::status(right_ui::Glyph::Ok)
                .noun("sandbox files")
                .verb("restored")
                .render(theme)
        );

        // Write sandbox.name into agent.yaml.
        crate::wizard::update_agent_yaml_sandbox_name(&agent_dir, &new_sandbox_name)?;
        println!(
            "{}",
            right_ui::status(right_ui::Glyph::Ok)
                .noun("sandbox.name")
                .verb("written to agent.yaml")
                .detail(&new_sandbox_name)
                .render(theme)
        );

        right_agent::identity_mirror::sync_identity_mirror_from_sandbox(&agent_dir, &sandbox)
            .await
            .map_err(|e| {
                miette::miette!(
                    "sandbox restored but identity mirror sync failed for '{}': {e:#}",
                    new_sandbox_name
                )
            })?;
        println!(
            "{}",
            right_ui::status(right_ui::Glyph::Ok)
                .noun("identity mirror")
                .verb("restored from sandbox")
                .render(theme)
        );

        migrate_restored_agent_db(&agent_dir, &quiescence_guard).await?;
        persist_claude_setup_token(&agent_dir, claude_setup_token, &quiescence_guard).await?;
        let restored_agent = right_agent::agent::discover_single_agent(&agent_dir)?;
        let self_exe = std::env::current_exe()
            .map_err(|error| miette::miette!("failed to resolve self exe: {error:#}"))?;
        right_codegen::run_agent_codegen_for_init(
            home,
            std::slice::from_ref(&restored_agent),
            &self_exe,
            false,
        )?;
        right_codegen::run_single_agent_codegen(home, &restored_agent, &self_exe, false).await?;
        validate_agent_init_auth(&restored_agent).await?;

        Ok(())
    }
    .await;

    if let Err(error) = result {
        cleanup_failed_restore(&cleanup_plan)
            .await
            .map_err(|cleanup_error| {
                miette::miette!("restore failed: {error:#}; cleanup also failed: {cleanup_error:#}")
            })?;
        return Err(error);
    }

    let register_outcome = right_agent::agent::register_with_running_pc(
        home,
        right_agent::agent::RegisterOptions {
            agent_name: agent_name.to_string(),
            recreated: false,
        },
    )
    .await;
    let recap = restore_recap(agent_name, &agent_dir, register_outcome);
    println!("{}", recap.render(theme));
    Ok(())
}
fn restore_recap(
    agent_name: &str,
    agent_dir: &Path,
    register_outcome: miette::Result<right_agent::agent::RegisterResult>,
) -> right_ui::Recap {
    let recap = right_ui::Recap::new("restored")
        .ok("agent", agent_name)
        .ok("path", &agent_dir.display().to_string());
    match register_outcome {
        Ok(right_agent::agent::RegisterResult { pc_running: true }) => {
            recap.next("send /start to your bot in Telegram")
        }
        Ok(right_agent::agent::RegisterResult { pc_running: false }) => recap.next("right up"),
        Err(error) => {
            tracing::warn!(
                error = format!("{error:#}"),
                "PC reload failed after agent restore"
            );
            recap
                .warn("reload", "failed; restored state is retained")
                .next("run `right reload`, or restart right")
        }
    }
}

/// Validate the agent's stored Claude credential with a real one-turn call
/// inside its sandbox.
///
/// Only `agent restore` calls this: it is the one CLI path that has a live
/// sandbox of its own (it just created one to unpack the backup into). `init`
/// and `agent init` create no sandbox, so their credential is validated by
/// the bot's first bring-up instead.
async fn validate_agent_init_auth(agent: &right_agent::agent::AgentDef) -> miette::Result<()> {
    let config = agent.config.as_ref().ok_or_else(|| {
        miette::miette!("agent.yaml missing before Claude authentication validation")
    })?;
    // The probe runs in-guest, exactly like a bot turn: attach here and fail
    // fast, because there is no host transport to fall back to.
    let sandbox_name = match config.sandbox.as_ref().and_then(|s| s.name.as_deref()) {
        Some(explicit) => right_sandbox::fit_sandbox_name(explicit),
        None => right_sandbox::sandbox_name(&agent.name),
    };
    let sandbox = std::sync::Arc::new(
        right_sandbox::SandboxHandle::attach(&sandbox_name)
            .await
            .map_err(|error| {
                miette::miette!("attach to sandbox `{sandbox_name}` for Claude auth: {error:#}")
            })?,
    );
    let probe = right_bot::InitAuthProbe::new(agent.path.clone(), sandbox, config.model.clone());
    right_bot::validate_init_auth(probe)
        .await
        .map_err(|error| miette::miette!("{error:#}"))
}

fn prompt_restore_binding_mode() -> miette::Result<restore::RestoreBindingMode> {
    let options = vec![
        "preserve source bindings",
        "rebind to target",
        "set memory bank id",
    ];
    let choice = inquire::Select::new("restore binding mode:", options)
        .prompt()
        .map_err(|e| miette::miette!("prompt failed: {e:#}"))?;

    match choice {
        "preserve source bindings" => Ok(restore::RestoreBindingMode::PreserveSource),
        "rebind to target" => Ok(restore::RestoreBindingMode::RebindToTarget),
        "set memory bank id" => {
            let memory_bank_id = inquire::Text::new("memory bank id:")
                .prompt()
                .map_err(|e| miette::miette!("prompt failed: {e:#}"))?;
            let memory_bank_id =
                non_empty_arg(&memory_bank_id).map_err(|e| miette::miette!("{e}"))?;
            Ok(restore::RestoreBindingMode::MemoryBankId(memory_bank_id))
        }
        _ => Err(miette::miette!(
            "unexpected restore binding mode selection: {choice}"
        )),
    }
}

fn cleanup_failed_restore_agent_dir(agent_dir: &Path) -> miette::Result<()> {
    use miette::IntoDiagnostic;

    if !agent_dir.exists() {
        return Ok(());
    }

    std::fs::remove_dir_all(agent_dir)
        .into_diagnostic()
        .map_err(|e| {
            miette::miette!(
                "failed to remove partial restored agent dir {}: {e:#}",
                agent_dir.display()
            )
        })
}

fn remove_database_sidecars(agent_dir: &Path) -> miette::Result<usize> {
    use miette::IntoDiagnostic;

    if !agent_dir.exists() {
        return Ok(0);
    }

    let entries = std::fs::read_dir(agent_dir)
        .into_diagnostic()
        .map_err(|e| {
            miette::miette!(
                "failed to read agent dir {} for database sidecar cleanup: {e:#}",
                agent_dir.display()
            )
        })?;

    let mut removed = 0usize;
    for entry in entries {
        let entry = entry.into_diagnostic().map_err(|e| {
            miette::miette!(
                "failed to inspect agent dir {} during database sidecar cleanup: {e:#}",
                agent_dir.display()
            )
        })?;
        let file_name = entry.file_name();
        let Some(file_name) = file_name.to_str() else {
            continue;
        };
        if !file_name.starts_with("data.db-") {
            continue;
        }

        let file_type = entry.file_type().into_diagnostic().map_err(|e| {
            miette::miette!(
                "failed to read file type for {} during database sidecar cleanup: {e:#}",
                entry.path().display()
            )
        })?;
        if file_type.is_dir() {
            continue;
        }

        std::fs::remove_file(entry.path())
            .into_diagnostic()
            .map_err(|e| {
                miette::miette!(
                    "failed to remove database sidecar {}: {e:#}",
                    entry.path().display()
                )
            })?;
        removed += 1;
    }

    Ok(removed)
}

fn copy_database_snapshot_for_restore(backup_dir: &Path, agent_dir: &Path) -> miette::Result<bool> {
    use miette::IntoDiagnostic;

    let rel = Path::new("data.db");
    let dest = agent_dir.join(rel);
    match std::fs::symlink_metadata(&dest) {
        Ok(meta) if meta.is_dir() && !meta.file_type().is_symlink() => {
            std::fs::remove_dir_all(&dest)
                .into_diagnostic()
                .map_err(|e| {
                    miette::miette!(
                        "failed to remove restored database directory {}: {e:#}",
                        dest.display()
                    )
                })?;
        }
        Ok(_) => {
            std::fs::remove_file(&dest).into_diagnostic().map_err(|e| {
                miette::miette!(
                    "failed to remove restored database file {}: {e:#}",
                    dest.display()
                )
            })?;
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            return Err(miette::miette!("failed to stat {}: {e:#}", dest.display()));
        }
    }

    let src = backup_dir.join(rel);
    match std::fs::symlink_metadata(&src) {
        Ok(meta) => {
            if meta.file_type().is_symlink() {
                return Err(miette::miette!(
                    "agent file {} is a symlink; symlinks are rejected",
                    src.display()
                ));
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => {
            return Err(miette::miette!("failed to stat {}: {e:#}", src.display()));
        }
    }

    copy_required_agent_file(backup_dir, agent_dir, rel)?;
    Ok(true)
}

fn copy_agent_backup_config_files(agent_dir: &Path, backup_dir: &Path) -> miette::Result<()> {
    for filename in ["agent.yaml", "allowlist.yaml"] {
        let rel = Path::new(filename);
        if copy_agent_file_if_exists(agent_dir, backup_dir, rel)? {
            println!("{filename} copied");
        }
    }
    Ok(())
}

fn copy_agent_restore_config_files(backup_dir: &Path, agent_dir: &Path) -> miette::Result<()> {
    for filename in ["agent.yaml", "allowlist.yaml"] {
        let rel = Path::new(filename);
        if copy_agent_file_if_exists(backup_dir, agent_dir, rel)? {
            println!("{filename} restored");
        }
    }
    if copy_database_snapshot_for_restore(backup_dir, agent_dir)? {
        println!("data.db restored");
    }
    Ok(())
}

fn copy_agent_file_if_exists(
    src_root: &Path,
    dest_root: &Path,
    rel: &Path,
) -> miette::Result<bool> {
    validate_relative_agent_file(rel, "agent file")?;
    let src = src_root.join(rel);
    match std::fs::symlink_metadata(&src) {
        Ok(meta) => {
            if meta.file_type().is_symlink() {
                return Err(miette::miette!(
                    "agent file {} is a symlink; symlinks are rejected",
                    src.display()
                ));
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => {
            return Err(miette::miette!("failed to stat {}: {e:#}", src.display()));
        }
    }
    copy_required_agent_file(src_root, dest_root, rel)?;
    Ok(true)
}

fn copy_required_agent_file(src_root: &Path, dest_root: &Path, rel: &Path) -> miette::Result<()> {
    use miette::IntoDiagnostic;

    validate_relative_agent_file(rel, "agent file")?;
    let src = src_root.join(rel);
    match std::fs::symlink_metadata(&src) {
        Ok(meta) => {
            if meta.file_type().is_symlink() {
                return Err(miette::miette!(
                    "agent file {} is a symlink; symlinks are rejected",
                    src.display()
                ));
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(miette::miette!(
                "required agent file not found at {}",
                src.display()
            ));
        }
        Err(e) => {
            return Err(miette::miette!("failed to stat {}: {e:#}", src.display()));
        }
    }

    let dest = dest_root.join(rel);
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)
            .into_diagnostic()
            .map_err(|e| miette::miette!("failed to create {}: {e:#}", parent.display()))?;
    }
    std::fs::copy(&src, &dest).into_diagnostic().map_err(|e| {
        miette::miette!(
            "failed to copy {} to {}: {e:#}",
            src.display(),
            dest.display()
        )
    })?;
    Ok(())
}

fn validate_relative_agent_file(path: &Path, label: &str) -> miette::Result<()> {
    if path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return Err(miette::miette!(
            "{label} must be relative and must not contain '..': {}",
            path.display()
        ));
    }
    Ok(())
}

async fn cmd_agent_backup(
    home: &Path,
    agent_name: &str,
    sandbox_only: bool,
    include_rebuildable: bool,
) -> miette::Result<()> {
    use miette::IntoDiagnostic;

    // 1. Discover agent and parse config
    let agents_dir = right_config::agents_dir(home);
    let agents = right_agent::agent::discover_agents(&agents_dir)?;
    let _agent = agents
        .iter()
        .find(|a| a.name == agent_name)
        .ok_or_else(|| {
            let available: Vec<&str> = agents.iter().map(|a| a.name.as_str()).collect();
            miette::miette!(
                "Agent '{}' not found. Available: {}",
                agent_name,
                available.join(", ")
            )
        })?;

    let agent_dir = agents_dir.join(agent_name);
    let config = right_agent::agent::discovery::parse_agent_config(&agent_dir)?;

    // 2. Create backup directory: ~/.right/backups/<agent>/<YYYYMMDD-HHMM>/
    let timestamp = chrono::Local::now().format("%Y%m%d-%H%M").to_string();
    let backup_base = right_config::backups_dir(home, agent_name);
    let backup_dir = backup_base.join(&timestamp);
    std::fs::create_dir_all(&backup_dir)
        .into_diagnostic()
        .map_err(|e| {
            miette::miette!(
                "failed to create backup dir {}: {e:#}",
                backup_dir.display()
            )
        })?;

    tracing::info!(agent = agent_name, backup_dir = %backup_dir.display(), "starting backup");

    // 3. Archive the guest home. The agent's authoritative state lives in the
    // sandbox, so a backup that cannot reach it is a failure, never a
    // host-only archive that looks like a backup.
    let explicit_sandbox_name = config
        .as_ref()
        .and_then(|c| c.sandbox.as_ref())
        .and_then(|s| s.name.as_deref());
    let sb_name = right_sandbox::resolve_sandbox_name(agent_name, explicit_sandbox_name);
    let sandbox = right_sandbox::SandboxHandle::attach(&sb_name)
        .await
        .map_err(|error| {
            miette::miette!(
                help = "Start the agent with: right up",
                "cannot reach sandbox '{sb_name}' to back it up: {error:#}"
            )
        })?;

    let dest_tar = backup_dir.join("sandbox.tar.gz");
    tracing::info!(sandbox = %sb_name, dest = %dest_tar.display(), "archiving sandbox guest home");
    right_agent::sandbox_backup::archive_guest_home(&sandbox, &dest_tar, include_rebuildable)
        .await?;
    println!(
        "sandbox.tar.gz written ({} bytes)",
        std::fs::metadata(&dest_tar).map(|m| m.len()).unwrap_or(0)
    );

    // 4. Config files (unless --sandbox-only)
    if !sandbox_only {
        copy_agent_backup_config_files(&agent_dir, &backup_dir)?;

        let _quiescence_guard = runtime_quiescence::require_runtime_quiesced(home).await?;
        let db_path = agent_dir.join("data.db");
        if db_path.exists() {
            let backup_db = backup_dir.join("data.db");
            // `open_connection(.., migrate=false)` returns a writable handle.
            // Turso's `VACUUM INTO` needs writability on the source DB.
            let conn = right_db::open_connection(&agent_dir, false)
                .await
                .into_diagnostic()
                .map_err(|e| miette::miette!("failed to open data.db: {e:#}"))?;
            conn.execute(
                &format!(
                    "VACUUM INTO '{}'",
                    backup_db.display().to_string().replace('\'', "''")
                ),
                [],
            )
            .await
            .into_diagnostic()
            .map_err(|e| miette::miette!("VACUUM INTO failed: {e:#}"))?;
            println!(
                "data.db vacuumed ({} bytes)",
                std::fs::metadata(&backup_db).map(|m| m.len()).unwrap_or(0)
            );
        }

        let manifest = restore::build_backup_manifest(
            agent_name,
            config.as_ref(),
            Some(&backup_dir.join("data.db")),
        )
        .await?;
        restore::write_backup_manifest(&backup_dir, &manifest)?;
        println!("backup.json written");
    }

    println!("Backup complete: {}", backup_dir.display());
    Ok(())
}

async fn cmd_agent_destroy(
    home: &Path,
    agent_name: &str,
    backup_flag: bool,
    force: bool,
) -> miette::Result<()> {
    use inquire::ui::{Color, RenderConfig, Styled};

    // Validate agent exists
    let agents_dir = right_config::agents_dir(home);
    let agent_dir = agents_dir.join(agent_name);
    if !agent_dir.exists() {
        return Err(miette::miette!("Agent '{}' not found", agent_name));
    }

    let config = right_agent::agent::parse_agent_config(&agent_dir)?;

    let do_backup = if force {
        backup_flag
    } else {
        // Show summary of what will be destroyed
        println!("Agent: {agent_name}");
        println!("  Directory: {}", agent_dir.display());
        if let Ok(size) = dir_size(&agent_dir) {
            println!("  Size: {}", format_bytes(size));
        }
        let explicit_sandbox_name = config
            .as_ref()
            .and_then(|c| c.sandbox.as_ref())
            .and_then(|s| s.name.as_deref());
        let sb_name = right_sandbox::resolve_sandbox_name(agent_name, explicit_sandbox_name);
        println!("  Sandbox: {sb_name}");
        let db_path = agent_dir.join("data.db");
        if db_path.exists()
            && let Ok(meta) = std::fs::metadata(&db_path)
        {
            println!("  data.db: {}", format_bytes(meta.len()));
        }

        // Check if PC is running and agent is active. `from_home` returns
        // None when this --home has no recorded runtime state, in which case
        // there is no PC to contact. See ARCHITECTURE.md "Runtime isolation".
        let pc_running = match right_agent::runtime::PcClient::from_home(home)? {
            Some(pc_client) => pc_client.health_check().await.is_ok(),
            None => false,
        };
        if pc_running {
            println!("  Process: running (will be stopped)");
        } else {
            println!("  Process: not running");
        }

        println!();

        // Backup prompt
        let do_backup = if backup_flag {
            true
        } else {
            inquire::Confirm::new("create backup before destroying?")
                .with_default(false)
                .prompt()
                .map_err(|e| miette::miette!("prompt failed: {e:#}"))?
        };

        // Final confirmation — red styled
        let red_config =
            RenderConfig::default().with_prompt_prefix(Styled::new("⚠").with_fg(Color::LightRed));

        let confirmed = inquire::Confirm::new(&format!(
            "permanently destroy agent '{agent_name}'? this cannot be undone."
        ))
        .with_default(false)
        .with_render_config(red_config)
        .prompt()
        .map_err(|e| miette::miette!("prompt failed: {e:#}"))?;

        if !confirmed {
            println!("Aborted.");
            return Ok(());
        }

        do_backup
    };

    let options = right_agent::agent::DestroyOptions {
        agent_name: agent_name.to_string(),
        backup: do_backup,
    };

    let result = right_agent::agent::destroy_agent(home, &options).await?;

    // Print summary
    println!();
    println!("Destroyed agent '{agent_name}':");
    if result.agent_stopped {
        println!("  ✓ Stopped process");
    }
    if let Some(ref path) = result.backup_path {
        println!("  ✓ Backup saved to {}", path.display());
    }
    if result.sandbox_deleted {
        println!("  ✓ Deleted sandbox");
    }
    if result.dir_removed {
        println!("  ✓ Removed agent directory");
    }
    if result.pc_reloaded {
        println!("  ✓ Reloaded process-compose");
    }

    Ok(())
}

/// Read-only skill lifecycle helper.
async fn cmd_agent_skill(home: &Path, command: AgentSkillCommands) -> miette::Result<()> {
    match command {
        AgentSkillCommands::List => cmd_agent_skill_list(home).await,
    }
}

async fn cmd_agent_providers(home: &Path, command: AgentProvidersCommands) -> miette::Result<()> {
    match command {
        AgentProvidersCommands::Add {
            agent,
            type_,
            label,
            credential,
            upstream_host,
            upstream_path_prefix,
            header_name,
            env_var,
        } => {
            if header_name.is_some() {
                eprintln!("warning: --header-name is deprecated and ignored");
            }
            cmd_agent_providers_add(
                home,
                &agent,
                &type_,
                label.as_deref(),
                &credential,
                &upstream_host,
                upstream_path_prefix.as_deref(),
                env_var.as_deref(),
            )
            .await
        }
    }
}

// CLI subcommand handler: the arguments mirror the parsed CLI flags, so a
// parameter object would just shuffle the same fields without clarifying.
#[allow(clippy::too_many_arguments)]
async fn cmd_agent_providers_add(
    home: &Path,
    agent: &str,
    type_: &str,
    label: Option<&str>,
    credential: &str,
    upstream_hosts: &[String],
    upstream_path_prefix: Option<&str>,
    env_var: Option<&str>,
) -> miette::Result<()> {
    // Validate the static argument shape before probing runtime state, so a
    // misconfigured command surfaces the actionable arg error even when
    // `right up` isn't running yet.
    let generic =
        build_provider_create_generic_arg(type_, upstream_hosts, upstream_path_prefix, env_var)?;

    let socket_path = home.join("run/internal.sock");
    if !socket_path.exists() {
        return Err(miette::miette!(
            help = "Start right first with `right up`",
            "Internal API socket not found at {} — `right up` must be running",
            socket_path.display()
        ));
    }

    let req = right_mcp::internal_client::ProviderCreateRequest {
        agent,
        type_,
        label,
        credential,
        generic,
    };

    let client = right_mcp::internal_client::InternalClient::new(&socket_path);
    let view = client
        .provider_create(&req)
        .await
        .map_err(|e| miette::miette!("provider_create failed: {e:#}"))?;
    println!(
        "{}",
        serde_json::to_string_pretty(&view).map_err(|e| miette::miette!("{e:#}"))?
    );
    Ok(())
}

fn build_provider_create_generic_arg<'a>(
    type_: &str,
    upstream_hosts: &'a [String],
    upstream_path_prefix: Option<&'a str>,
    env_var: Option<&'a str>,
) -> miette::Result<Option<right_mcp::internal_client::ProviderCreateGenericArg<'a>>> {
    if type_ != "generic" {
        return Ok(None);
    }

    if upstream_hosts.is_empty() {
        return Err(miette::miette!(
            "--upstream-host is required for `--type generic`"
        ));
    }
    let env =
        env_var.ok_or_else(|| miette::miette!("--env-var is required for `--type generic`"))?;

    Ok(Some(right_mcp::internal_client::ProviderCreateGenericArg {
        env_var: env,
        upstream_hosts,
        upstream_path_prefix,
    }))
}

async fn cmd_agent_skill_list(home: &Path) -> miette::Result<()> {
    let agents_dir = right_config::agents_dir(home);
    if !agents_dir.is_dir() {
        println!("No learned skills.");
        return Ok(());
    }
    let socket_path = home.join("run/internal.sock");
    let client = right_mcp::internal_client::InternalClient::new(&socket_path);
    let mut agent_names = std::fs::read_dir(&agents_dir)
        .map_err(|e| miette::miette!("cannot read agents dir: {e:#}"))?
        .filter_map(Result::ok)
        .filter(|entry| entry.path().is_dir())
        .filter_map(|entry| entry.file_name().into_string().ok())
        .collect::<Vec<_>>();
    agent_names.sort();

    let mut any = false;
    for agent_name in agent_names {
        let response = client
            .skill_lifecycle_list(&right_mcp::internal_db::SkillLifecycleListRequest {
                agent: agent_name.clone(),
            })
            .await
            .map_err(|e| miette::miette!("list lifecycle rows for {agent_name}: {e:#}"))?;
        for row in response.rows {
            println!(
                "{agent_name}\t{}\tstate={}\tpinned={}\tcreated_by={}\tuses={}\tpatches={}",
                row.skill_name,
                row.state,
                row.pinned,
                row.created_by,
                row.use_count,
                row.patch_count
            );
            any = true;
        }
    }

    if !any {
        println!("No learned skills.");
    }
    Ok(())
}

async fn cmd_agent_rebootstrap(home: &Path, agent_name: &str, yes: bool) -> miette::Result<()> {
    use right_ui::{Block, Glyph, Rail, detect, section, status};

    let plan = right_agent::rebootstrap::plan(home, agent_name)?;
    let theme = detect();
    let pc_process = format!("{agent_name}-bot");

    if !yes {
        println!("{}", section(theme, &format!("rebootstrap: {agent_name}")));
        println!("{}", Rail::blank(theme));

        let sandbox_detail = plan.sandbox_name.clone();
        let mut plan_block = Block::new();
        plan_block.push(
            status(Glyph::Info)
                .noun("directory")
                .verb(plan.agent_dir.display().to_string()),
        );
        plan_block.push(status(Glyph::Info).noun("sandbox").verb(sandbox_detail));
        plan_block.push(
            status(Glyph::Info)
                .noun("backup")
                .verb(plan.backup_dir.display().to_string()),
        );
        println!("{}", plan_block.render(theme));
        println!("{}", Rail::blank(theme));

        println!("{}", section(theme, "effects"));
        println!("{}", Rail::blank(theme));
        let mut effects = Block::new();
        effects.push(
            status(Glyph::Info)
                .noun("back up")
                .verb("IDENTITY.md, SOUL.md, USER.md (host + sandbox)"),
        );
        effects.push(
            status(Glyph::Info)
                .noun("remove")
                .verb("same files from host and sandbox"),
        );
        effects.push(
            status(Glyph::Info)
                .noun("recreate")
                .verb("BOOTSTRAP.md on host"),
        );
        effects.push(
            status(Glyph::Info)
                .noun("deactivate")
                .verb("active sessions in data.db"),
        );
        effects.push(
            status(Glyph::Info)
                .noun("bounce")
                .verb(format!("{pc_process} if running")),
        );
        println!("{}", effects.render(theme));
        println!("{}", Rail::blank(theme));
        println!(
            "{}preserved: sandbox, credentials, hindsight memory, data.db rows",
            Rail::prefix(theme)
        );
        println!("{}", Rail::blank(theme));

        let confirmed = inquire::Confirm::new(&format!(
            "rebootstrap '{agent_name}'? this rewinds onboarding state"
        ))
        .with_default(false)
        .prompt()
        .map_err(|e| miette::miette!("prompt failed: {e:#}"))?;

        if !confirmed {
            println!(
                "{}",
                status(Glyph::Warn)
                    .noun("aborted")
                    .verb("no changes made")
                    .render(theme)
            );
            return Ok(());
        }
    }

    // Three states, NOT two — we must not silently skip the bot bounce when
    // state.json is present but PC's API is unreachable. That's how the
    // 2026-04-29 incident ("rebootstrap ran but my bot kept serving the old
    // persona") happened: the previous code treated 401-on-/live as
    // equivalent to "PC not running", continued with the file-side rewind,
    // and left the still-running bot serving stale identity.
    //
    //   None              — no state.json: PC was never started from this
    //                       home. No live bot to bounce; file ops are safe.
    //   Some(Some(pc))    — state.json + healthy PC. Stop now, restart later.
    //   error             — state.json present but PC unreachable. We REFUSE
    //                       to do file ops because the bot would keep serving
    //                       the old persona.
    let stopped_pc = match right_agent::runtime::PcClient::from_home(home)? {
        None => {
            println!(
                "{}",
                status(Glyph::Info)
                    .noun(pc_process.as_str())
                    .verb("not running, skipping bot stop")
                    .render(theme)
            );
            None
        }
        Some(pc) => {
            pc.health_check().await.map_err(|e| {
                miette::miette!(
                    "process-compose API unreachable: {e:#}\n\
                     Refusing to rebootstrap: cannot bounce {pc_process}, and proceeding \
                     would leave the running bot serving the old identity.\n\
                     Verify `right up` is healthy (or stop it cleanly) and retry."
                )
            })?;
            pc.stop_process(&pc_process).await.map_err(|e| {
                miette::miette!(
                    "failed to stop {pc_process} (not safe to proceed with bot up): {e:#}"
                )
            })?;
            println!(
                "{}",
                status(Glyph::Ok)
                    .noun(pc_process.as_str())
                    .verb("stopped")
                    .render(theme)
            );
            Some(pc)
        }
    };

    let report = right_agent::rebootstrap::execute(&plan).await?;

    if let Some(pc) = &stopped_pc {
        pc.start_process(&pc_process).await?;
        println!(
            "{}",
            status(Glyph::Ok)
                .noun(pc_process.as_str())
                .verb("started")
                .render(theme)
        );
    }

    println!(
        "{}",
        section(theme, &format!("rebootstrapped: {agent_name}"))
    );
    println!("{}", Rail::blank(theme));
    let mut recap_block = Block::new();
    recap_block.push(
        status(Glyph::Ok)
            .noun("backup")
            .verb(report.backup_dir.display().to_string()),
    );
    let host_detail = if report.host_backed_up.is_empty() {
        "none (agent had not bootstrapped)".to_string()
    } else {
        report.host_backed_up.join(", ")
    };
    recap_block.push(status(Glyph::Ok).noun("host").verb(host_detail));
    if !report.sandbox_backed_up.is_empty() {
        recap_block.push(
            status(Glyph::Ok)
                .noun("sandbox")
                .verb(report.sandbox_backed_up.join(", ")),
        );
    }
    recap_block.push(
        status(Glyph::Ok)
            .noun("sessions")
            .verb(format!("{} deactivated", report.sessions_deactivated)),
    );
    println!("{}", recap_block.render(theme));
    println!("{}", Rail::blank(theme));

    if stopped_pc.is_none() {
        println!("{}next: right up", Rail::prefix(theme));
    }

    Ok(())
}

fn dir_size(path: &Path) -> std::io::Result<u64> {
    let mut total = 0;
    for entry in walkdir::WalkDir::new(path)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        if entry.file_type().is_file() {
            total += entry.metadata().map(|m| m.len()).unwrap_or(0);
        }
    }
    Ok(total)
}

fn format_bytes(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

// Tests are placed mid-file historically; moving them is a structural
// change out of scope for this cleanup pass.
#[allow(clippy::items_after_test_module)]
#[cfg(test)]
mod tests {
    use super::{
        AgentReadinessBackend, ConfigCommands, MemoryCommands, RestoreCleanupPlan,
        cleanup_failed_restore_agent_dir, cleanup_failed_restore_with,
        copy_agent_backup_config_files, copy_agent_restore_config_files,
        copy_database_snapshot_for_restore, discover_up_agents, non_interactive_readiness_result,
        remove_database_sidecars, resolve_agent_db, restore_recap, run_up_preflight,
        truncate_content, validate_agent_readiness_with, validate_configured_tunnel_with,
        write_managed_settings,
    };
    use crate::runtime_builder;

    use right_agent_config::{AgentConfig, SandboxConfig};
    use std::fs;
    use std::path::{Path, PathBuf};
    use tempfile::TempDir;

    fn readiness_agent(name: &str) -> right_agent::agent::AgentDef {
        let path = PathBuf::from(name);
        right_agent::agent::AgentDef {
            name: name.to_owned(),
            path: path.clone(),
            identity_path: path.join("IDENTITY.md"),
            config: Some(AgentConfig {
                sandbox: Some(SandboxConfig {
                    name: None,
                    providers: Vec::new(),
                }),
                telegram_token: Some("redacted-test-token".to_owned()),
                ..AgentConfig::default()
            }),
            soul_path: None,
            user_path: None,
            tools_path: None,
            bootstrap_path: None,
            heartbeat_path: None,
        }
    }

    #[derive(Default)]
    struct RecordingReadinessBackend {
        telegram_checks: Vec<String>,
        repairs: Vec<String>,
    }

    impl AgentReadinessBackend for RecordingReadinessBackend {
        async fn validate_telegram(
            &mut self,
            agent: &right_agent::agent::AgentDef,
        ) -> miette::Result<()> {
            self.telegram_checks.push(agent.name.clone());
            Err(miette::miette!("telegram unavailable"))
        }

        async fn repair_telegram(
            &mut self,
            agent: &right_agent::agent::AgentDef,
        ) -> miette::Result<()> {
            self.repairs.push(agent.name.clone());
            Ok(())
        }
    }

    #[tokio::test]
    async fn up_runtime_guard_precedes_readiness() {
        let events = std::cell::RefCell::new(Vec::new());
        run_up_preflight(
            || async {
                events.borrow_mut().push("guard");
                Ok(())
            },
            || async {
                events.borrow_mut().push("readiness");
                Ok(())
            },
        )
        .await
        .unwrap();

        assert_eq!(*events.borrow(), ["guard", "readiness"]);
    }

    #[tokio::test]
    async fn up_runtime_guard_failure_skips_readiness() {
        let readiness_calls = std::cell::Cell::new(0_u8);
        let error = run_up_preflight(
            || async { Err(miette::miette!("already running")) },
            || async {
                readiness_calls.set(readiness_calls.get() + 1);
                Ok(())
            },
        )
        .await
        .unwrap_err();

        assert!(format!("{error:#}").contains("already running"));
        assert_eq!(readiness_calls.get(), 0);
    }

    #[tokio::test]
    async fn noninteractive_readiness_checks_all_selected_agents_and_never_repairs() {
        let agents = [readiness_agent("alpha"), readiness_agent("beta")];
        let mut backend = RecordingReadinessBackend::default();
        let mut issues = Vec::new();
        validate_agent_readiness_with(&agents, false, &mut backend, &mut issues)
            .await
            .unwrap();

        assert_eq!(backend.telegram_checks, ["alpha", "beta"]);
        assert!(backend.repairs.is_empty());
        let error = non_interactive_readiness_result(issues).unwrap_err();
        let message = format!("{error:#}");
        assert!(message.contains("alpha: Telegram validation failed"));
        assert!(message.contains("beta: Telegram validation failed"));
        // Claude authentication is not a `right up` concern any more: the
        // credential is only usable inside a sandbox the bot has not created
        // yet, so bring-up reports it.
        assert!(!message.contains("Claude"));
    }

    #[tokio::test]
    async fn noninteractive_discovery_and_live_failures_aggregate_without_downstream_work() {
        let tmp = TempDir::new().unwrap();
        let agents_dir = tmp.path().join("agents");
        fs::create_dir_all(agents_dir.join("valid")).unwrap();
        fs::create_dir_all(agents_dir.join("missing")).unwrap();
        fs::create_dir_all(agents_dir.join("malformed")).unwrap();
        fs::write(
            agents_dir.join("valid/agent.yaml"),
            "telegram_token: \"123:test\"\nsandbox:\n  name: valid\n",
        )
        .unwrap();
        fs::write(agents_dir.join("malformed/agent.yaml"), "sandbox: [").unwrap();
        let discovery = discover_up_agents(&agents_dir, None).unwrap();
        let mut backend = RecordingReadinessBackend::default();
        let mut issues = discovery.issues;
        validate_agent_readiness_with(&discovery.agents, false, &mut backend, &mut issues)
            .await
            .unwrap();
        let downstream_calls = std::cell::Cell::new(0_u8);
        let result: miette::Result<()> = async {
            non_interactive_readiness_result(issues)?;
            downstream_calls.set(downstream_calls.get() + 1);
            Ok(())
        }
        .await;

        let message = format!("{:#}", result.unwrap_err());
        assert!(message.contains("missing: configuration failed"));
        assert!(message.contains("malformed: configuration failed"));
        assert!(message.contains("valid: Telegram validation failed"));
        assert_eq!(backend.telegram_checks, ["valid"]);
        assert!(backend.repairs.is_empty());
        assert_eq!(downstream_calls.get(), 0);
    }

    /// An agent still on OpenShell cannot start, but it is a transitional
    /// state with a known fix — not a broken config. It must be reported
    /// separately so it never blocks the agents that are already migrated,
    /// which is what made `right up` refuse the whole host after one agent
    /// was migrated.
    #[test]
    fn an_unmigrated_agent_does_not_block_the_migrated_ones() {
        let tmp = tempfile::tempdir().unwrap();
        let agents_dir = tmp.path().join("agents");
        std::fs::create_dir_all(agents_dir.join("migrated")).unwrap();
        std::fs::create_dir_all(agents_dir.join("stillopenshell")).unwrap();
        std::fs::create_dir_all(agents_dir.join("broken")).unwrap();
        std::fs::write(
            agents_dir.join("migrated/agent.yaml"),
            "telegram_token: \"123:test\"\nsandbox:\n  name: migrated\n",
        )
        .unwrap();
        std::fs::write(
            agents_dir.join("stillopenshell/agent.yaml"),
            "telegram_token: \"123:test\"\nsandbox:\n  mode: openshell\n  name: right-old\n",
        )
        .unwrap();
        std::fs::write(agents_dir.join("broken/agent.yaml"), "sandbox: [").unwrap();

        let discovery = discover_up_agents(&agents_dir, None).unwrap();

        let started: Vec<&str> = discovery.agents.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(
            started,
            ["migrated"],
            "a migrated agent must still start when a sibling is unmigrated"
        );
        assert_eq!(discovery.unmigrated, ["stillopenshell"]);
        assert_eq!(
            discovery.issues.len(),
            1,
            "only the genuinely malformed config is an issue: {:?}",
            discovery.issues
        );
        assert!(discovery.issues[0].starts_with("broken:"));
    }

    #[tokio::test]
    async fn readiness_failure_prevents_downstream_up_work() {
        let downstream_calls = std::cell::Cell::new(0_u8);
        let result: miette::Result<()> = async {
            run_up_preflight(
                || async { Ok(()) },
                || async { Err(miette::miette!("readiness failed")) },
            )
            .await?;
            downstream_calls.set(downstream_calls.get() + 1);
            Ok(())
        }
        .await;

        assert!(result.is_err());
        assert_eq!(downstream_calls.get(), 0);
    }

    #[tokio::test]
    async fn interactive_readiness_repairs_telegram_in_place() {
        let agents = [readiness_agent("alpha")];
        let mut backend = RecordingReadinessBackend::default();
        let mut issues = Vec::new();
        validate_agent_readiness_with(&agents, true, &mut backend, &mut issues)
            .await
            .unwrap();

        assert_eq!(backend.repairs, ["alpha"]);
        assert!(
            issues.is_empty(),
            "an interactively repaired agent leaves no issue behind: {issues:?}"
        );
    }

    #[test]
    fn up_selection_aggregates_invalid_selected_agents_and_keeps_valid_ones() {
        let tmp = TempDir::new().unwrap();
        let agents_dir = tmp.path().join("agents");
        fs::create_dir_all(agents_dir.join("valid")).unwrap();
        fs::create_dir_all(agents_dir.join("missing")).unwrap();
        fs::create_dir_all(agents_dir.join("malformed")).unwrap();
        fs::write(
            agents_dir.join("valid/agent.yaml"),
            "sandbox:\n  name: valid\n",
        )
        .unwrap();
        fs::write(agents_dir.join("malformed/agent.yaml"), "sandbox: [").unwrap();

        let selected = discover_up_agents(
            &agents_dir,
            Some(&[
                "valid".to_owned(),
                "missing".to_owned(),
                "malformed".to_owned(),
            ]),
        )
        .unwrap();
        assert_eq!(selected.agents.len(), 1);
        assert_eq!(selected.agents[0].name, "valid");
        assert_eq!(selected.issues.len(), 2);
        assert!(
            selected
                .issues
                .iter()
                .any(|issue| issue.starts_with("missing:"))
        );
        assert!(
            selected
                .issues
                .iter()
                .any(|issue| issue.starts_with("malformed:"))
        );

        let all = discover_up_agents(&agents_dir, None).unwrap();
        assert_eq!(all.agents.len(), 1);
        assert_eq!(all.issues.len(), 2);
    }

    #[cfg(unix)]
    #[test]
    fn configured_tunnel_validator_checks_credentials_and_account() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = TempDir::new().unwrap();
        let credentials = tmp.path().join("credentials.json");
        fs::write(&credentials, r#"{"TunnelID":"expected"}"#).unwrap();
        let cloudflared = tmp.path().join("cloudflared");
        fs::write(&cloudflared, "#!/bin/sh\nexit 0\n").unwrap();
        let mut permissions = fs::metadata(&cloudflared).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&cloudflared, permissions).unwrap();
        let config = right_config::GlobalConfig {
            tunnel: right_config::TunnelConfig {
                hostname: "right.example.com".to_owned(),
                provider: right_config::TunnelProvider::Cloudflared {
                    tunnel_uuid: "expected".to_owned(),
                    credentials_file: credentials.clone(),
                },
            },
            aggregator: right_config::AggregatorConfig::default(),
        };

        validate_configured_tunnel_with(&config, &cloudflared).unwrap();
        fs::write(&credentials, r#"{"TunnelID":"wrong"}"#).unwrap();
        let error = validate_configured_tunnel_with(&config, &cloudflared).unwrap_err();
        assert!(format!("{error}").contains("wrong"));
    }

    #[cfg(unix)]
    #[test]
    fn configured_tunnel_validator_rejects_inaccessible_account_tunnel() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = TempDir::new().unwrap();
        let credentials = tmp.path().join("credentials.json");
        fs::write(&credentials, r#"{"TunnelID":"expected"}"#).unwrap();
        let cloudflared = tmp.path().join("cloudflared");
        fs::write(&cloudflared, "#!/bin/sh\nexit 7\n").unwrap();
        let mut permissions = fs::metadata(&cloudflared).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&cloudflared, permissions).unwrap();
        let config = right_config::GlobalConfig {
            tunnel: right_config::TunnelConfig {
                hostname: "right.example.com".to_owned(),
                provider: right_config::TunnelProvider::Cloudflared {
                    tunnel_uuid: "expected".to_owned(),
                    credentials_file: credentials,
                },
            },
            aggregator: right_config::AggregatorConfig::default(),
        };

        let error = validate_configured_tunnel_with(&config, &cloudflared).unwrap_err();
        assert!(format!("{error}").contains("cannot access"));
    }

    #[test]
    fn external_tunnel_validation_is_shape_only() {
        let config = right_config::GlobalConfig {
            tunnel: right_config::TunnelConfig {
                hostname: "right.example.com".to_owned(),
                provider: right_config::TunnelProvider::External,
            },
            aggregator: right_config::AggregatorConfig::default(),
        };

        validate_configured_tunnel_with(&config, Path::new("definitely-absent")).unwrap();
    }

    #[test]
    fn restored_mcp_auth_method_fails_closed_for_missing_header_secrets() {
        assert!(
            runtime_builder::restored_mcp_auth_method(Some("headers"), None, None).is_none(),
            "headers auth must not restore as an unauthenticated backend when secrets fail to load"
        );
    }

    #[test]
    fn restored_mcp_auth_method_fails_closed_for_empty_header_secrets() {
        assert!(
            runtime_builder::restored_mcp_auth_method(Some("headers"), None, Some(Vec::new()))
                .is_none(),
            "headers auth must not restore as an unauthenticated backend with no stored headers"
        );
    }

    #[test]
    fn restored_mcp_auth_method_preserves_non_empty_header_secrets() {
        let header =
            right_mcp::credentials::HttpHeaderSecret::new("Authorization", "Bearer secret")
                .unwrap();

        assert_eq!(
            runtime_builder::restored_mcp_auth_method(
                Some("headers"),
                None,
                Some(vec![header.clone()]),
            ),
            Some(right_mcp::proxy::AuthMethod::Headers(vec![header]))
        );
    }

    // ---- memory commands variant existence (compile-time) ----

    #[test]
    fn memory_commands_list_variant_exists() {
        let _ = MemoryCommands::List {
            agent: "x".to_string(),
            limit: 10,
            offset: 0,
            json: false,
        };
    }

    #[test]
    fn memory_commands_stats_variant_exists() {
        let _ = MemoryCommands::Stats {
            agent: "x".to_string(),
            json: false,
        };
    }

    // ---- resolve_agent_db error paths ----

    #[tokio::test]
    async fn resolve_agent_db_errors_on_missing_agent_dir() {
        let tmp = TempDir::new().unwrap();
        // home exists but agents/nonexistent does not
        let result = resolve_agent_db(tmp.path(), "nonexistent").await;
        let err = result.expect_err("should fail when agent dir missing");
        let msg = format!("{err:?}");
        assert!(
            msg.contains("not found at"),
            "error must mention 'not found at', got: {msg}"
        );
    }

    #[tokio::test]
    async fn resolve_agent_db_errors_on_missing_memory_db() {
        let tmp = TempDir::new().unwrap();
        // create agent dir but no data.db
        let agent_dir = tmp.path().join("agents").join("testagent");
        fs::create_dir_all(&agent_dir).unwrap();
        let result = resolve_agent_db(tmp.path(), "testagent").await;
        let err = result.expect_err("should fail when data.db missing");
        let msg = format!("{err:?}");
        assert!(
            msg.contains("no memory database"),
            "error must mention 'no memory database', got: {msg}"
        );
    }

    #[test]
    fn cleanup_failed_restore_agent_dir_removes_partial_agent_state() {
        let tmp = TempDir::new().unwrap();
        let agent_dir = tmp.path().join("agents").join("right-drill");
        fs::create_dir_all(agent_dir.join("staging")).unwrap();
        fs::write(
            agent_dir.join("agent.yaml"),
            "sandbox:\n  name: test-agent\n",
        )
        .unwrap();
        fs::write(agent_dir.join("data.db"), "partial").unwrap();

        cleanup_failed_restore_agent_dir(&agent_dir).unwrap();

        assert!(
            !agent_dir.exists(),
            "failed restore cleanup must remove the partial agent directory"
        );
    }

    #[tokio::test]
    async fn cleanup_failed_restore_deletes_the_sandbox_it_created() {
        let tmp = TempDir::new().unwrap();
        let agent_dir = tmp.path().join("agents").join("right-drill");
        fs::create_dir_all(&agent_dir).unwrap();
        fs::write(
            agent_dir.join("agent.yaml"),
            "sandbox:\n  name: right-drill\n",
        )
        .unwrap();
        let mut plan = RestoreCleanupPlan::new(agent_dir.clone());
        plan.track_sandbox("right-drill".to_string());

        let deleted = std::cell::RefCell::new(Vec::new());
        cleanup_failed_restore_with(&plan, |name| async {
            deleted.borrow_mut().push(name);
            Ok(())
        })
        .await
        .unwrap();

        assert_eq!(*deleted.borrow(), ["right-drill"]);
        assert!(!agent_dir.exists());
    }

    #[tokio::test]
    async fn cleanup_failed_restore_retains_agent_state_when_the_sandbox_survives() {
        let tmp = TempDir::new().unwrap();
        let agent_dir = tmp.path().join("agents").join("right-drill");
        fs::create_dir_all(&agent_dir).unwrap();
        fs::write(
            agent_dir.join("agent.yaml"),
            "sandbox:\n  name: test-agent\n",
        )
        .unwrap();
        let mut plan = RestoreCleanupPlan::new(agent_dir.clone());
        plan.track_sandbox("right-drill".to_string());

        let error = cleanup_failed_restore_with(&plan, |_| async {
            Err(miette::miette!("sandbox backend unreachable"))
        })
        .await
        .expect_err("an undeletable sandbox must keep the recovery state");

        assert!(format!("{error:#}").contains("retaining recovery state"));
        assert!(
            agent_dir.exists(),
            "the agent dir is the only record of the surviving sandbox"
        );
    }

    #[test]
    fn restore_recap_warns_and_directs_reload_on_registration_error() {
        let recap = restore_recap(
            "restored-agent",
            Path::new("/tmp/restored-agent"),
            Err(miette::miette!("reload failed")),
        )
        .render(right_ui::Theme::Ascii);

        assert!(recap.contains("restored"));
        assert!(recap.contains("failed; restored state is retained"));
        assert!(recap.contains("right reload"));
    }

    #[test]
    fn remove_database_sidecars_deletes_runtime_files_only() {
        let tmp = TempDir::new().unwrap();
        let agent_dir = tmp.path().join("agents").join("right-drill");
        fs::create_dir_all(agent_dir.join("data.db-dir")).unwrap();
        fs::write(agent_dir.join("data.db"), "canonical").unwrap();
        fs::write(agent_dir.join("data.db-wal"), "wal").unwrap();
        fs::write(agent_dir.join("data.db-shm"), "shm").unwrap();
        fs::write(agent_dir.join("data.db-tshm"), "tshm").unwrap();
        fs::write(agent_dir.join("data.db-future"), "future").unwrap();
        fs::write(agent_dir.join("notes.txt"), "notes").unwrap();

        let removed = remove_database_sidecars(&agent_dir).unwrap();

        assert_eq!(removed, 4);
        assert!(agent_dir.join("data.db").exists());
        assert!(agent_dir.join("data.db-dir").exists());
        assert!(agent_dir.join("notes.txt").exists());
        assert!(!agent_dir.join("data.db-wal").exists());
        assert!(!agent_dir.join("data.db-shm").exists());
        assert!(!agent_dir.join("data.db-tshm").exists());
        assert!(!agent_dir.join("data.db-future").exists());
    }

    #[cfg(unix)]
    #[test]
    fn remove_database_sidecars_unlinks_symlink_without_touching_target() {
        let tmp = TempDir::new().unwrap();
        let agent_dir = tmp.path().join("agents").join("right-drill");
        fs::create_dir_all(&agent_dir).unwrap();
        fs::write(agent_dir.join("target.txt"), "target").unwrap();
        std::os::unix::fs::symlink(agent_dir.join("target.txt"), agent_dir.join("data.db-link"))
            .unwrap();

        let removed = remove_database_sidecars(&agent_dir).unwrap();

        assert_eq!(removed, 1);
        assert!(agent_dir.join("target.txt").exists());
        assert!(!agent_dir.join("data.db-link").exists());
    }

    #[test]
    fn copy_database_snapshot_for_restore_replaces_stale_data_db() {
        let tmp = TempDir::new().unwrap();
        let backup_dir = tmp.path().join("backup");
        let agent_dir = tmp.path().join("agents").join("right-drill");
        fs::create_dir_all(&backup_dir).unwrap();
        fs::create_dir_all(&agent_dir).unwrap();
        fs::write(backup_dir.join("data.db"), "canonical").unwrap();
        fs::write(agent_dir.join("data.db"), "stale").unwrap();

        let copied = copy_database_snapshot_for_restore(&backup_dir, &agent_dir).unwrap();

        assert!(copied);
        assert_eq!(
            fs::read_to_string(agent_dir.join("data.db")).unwrap(),
            "canonical"
        );
    }

    #[test]
    fn copy_database_snapshot_for_restore_removes_stale_data_db_without_canonical_snapshot() {
        let tmp = TempDir::new().unwrap();
        let backup_dir = tmp.path().join("backup");
        let agent_dir = tmp.path().join("agents").join("right-drill");
        fs::create_dir_all(&backup_dir).unwrap();
        fs::create_dir_all(&agent_dir).unwrap();
        fs::write(agent_dir.join("data.db"), "stale").unwrap();

        let copied = copy_database_snapshot_for_restore(&backup_dir, &agent_dir).unwrap();

        assert!(!copied);
        assert!(
            !agent_dir.join("data.db").exists(),
            "stale restored data.db must be removed when no canonical snapshot exists"
        );
    }

    #[cfg(unix)]
    #[test]
    fn copy_database_snapshot_for_restore_unlinks_symlink_destination() {
        let tmp = TempDir::new().unwrap();
        let backup_dir = tmp.path().join("backup");
        let agent_dir = tmp.path().join("agents").join("right-drill");
        fs::create_dir_all(&backup_dir).unwrap();
        fs::create_dir_all(&agent_dir).unwrap();
        fs::write(backup_dir.join("data.db"), "canonical").unwrap();
        fs::write(agent_dir.join("target.txt"), "target").unwrap();
        std::os::unix::fs::symlink(agent_dir.join("target.txt"), agent_dir.join("data.db"))
            .unwrap();

        let copied = copy_database_snapshot_for_restore(&backup_dir, &agent_dir).unwrap();

        assert!(copied);
        assert_eq!(
            fs::read_to_string(agent_dir.join("target.txt")).unwrap(),
            "target"
        );
        assert_eq!(
            fs::read_to_string(agent_dir.join("data.db")).unwrap(),
            "canonical"
        );
        assert!(
            !fs::symlink_metadata(agent_dir.join("data.db"))
                .unwrap()
                .file_type()
                .is_symlink(),
            "restored data.db must be a regular copied file, not the tar symlink"
        );
    }

    #[test]
    fn backup_config_files_carry_only_the_live_agent_config() {
        let tmp = TempDir::new().unwrap();
        let agent_dir = tmp.path().join("agents").join("legacy-policy-agent");
        let backup_dir = tmp.path().join("backups").join("legacy-policy-agent");
        fs::create_dir_all(agent_dir.join("policies")).unwrap();
        fs::create_dir_all(&backup_dir).unwrap();
        fs::write(
            agent_dir.join("agent.yaml"),
            "sandbox:\n  policy_file: policies/custom-policy.yaml\n",
        )
        .unwrap();
        fs::write(
            agent_dir.join("policies/custom-policy.yaml"),
            "version: 1\nnetwork_policies: {}\n",
        )
        .unwrap();
        // A leftover from the OpenShell era: no writer produces it any more,
        // and nothing reads it, so it must not be carried into a backup.
        fs::write(agent_dir.join("policy.yaml"), "version: 1\n").unwrap();

        copy_agent_backup_config_files(&agent_dir, &backup_dir).unwrap();

        assert!(backup_dir.join("agent.yaml").exists());
        assert!(
            !backup_dir.join("policy.yaml").exists(),
            "sandbox policy is create-time SDK state now, not a backed-up file"
        );
        assert!(
            !backup_dir.join("policies/custom-policy.yaml").exists(),
            "the retired sandbox.policy_file key must not pull extra files into the backup"
        );
    }

    #[test]
    fn backup_config_files_include_allowlist_yaml() {
        let tmp = TempDir::new().unwrap();
        let agent_dir = tmp.path().join("agents").join("allowlisted-agent");
        let backup_dir = tmp.path().join("backups").join("allowlisted-agent");
        fs::create_dir_all(&agent_dir).unwrap();
        fs::create_dir_all(&backup_dir).unwrap();
        fs::write(
            agent_dir.join("agent.yaml"),
            "sandbox:\n  name: test-agent\n",
        )
        .unwrap();
        let allowlist = "\
version: 1
users:
  - id: 111
    label: alice
    added_by: null
    added_at: 2026-05-16T12:00:00Z
groups:
  - id: -222
    label: ops
    opened_by: null
    opened_at: 2026-05-16T12:00:00Z
";
        fs::write(agent_dir.join("allowlist.yaml"), allowlist).unwrap();

        copy_agent_backup_config_files(&agent_dir, &backup_dir).unwrap();

        assert_eq!(
            fs::read_to_string(backup_dir.join("allowlist.yaml")).unwrap(),
            allowlist,
            "full backup must include bot-managed allowlist.yaml"
        );
    }

    #[test]
    fn restore_config_files_ignore_retired_policy_file_key() {
        let tmp = TempDir::new().unwrap();
        let backup_dir = tmp.path().join("backup");
        let agent_dir = tmp.path().join("agents").join("restored-agent");
        fs::create_dir_all(backup_dir.join("policies")).unwrap();
        fs::create_dir_all(&agent_dir).unwrap();
        fs::write(
            backup_dir.join("agent.yaml"),
            "sandbox:\n  policy_file: policies/custom-policy.yaml\n",
        )
        .unwrap();
        fs::write(
            backup_dir.join("policies/custom-policy.yaml"),
            "version: 1\nnetwork_policies: {}\n",
        )
        .unwrap();
        fs::write(backup_dir.join("data.db"), "db").unwrap();

        copy_agent_restore_config_files(&backup_dir, &agent_dir).unwrap();

        assert!(agent_dir.join("agent.yaml").exists());
        assert!(agent_dir.join("data.db").exists());
        assert!(
            !agent_dir.join("policies/custom-policy.yaml").exists(),
            "the retired sandbox.policy_file key must not restore extra files"
        );
    }

    #[test]
    fn restore_config_files_copy_allowlist_yaml() {
        let tmp = TempDir::new().unwrap();
        let backup_dir = tmp.path().join("backup");
        let agent_dir = tmp.path().join("agents").join("restored-agent");
        fs::create_dir_all(&backup_dir).unwrap();
        fs::create_dir_all(&agent_dir).unwrap();
        fs::write(
            backup_dir.join("agent.yaml"),
            "sandbox:\n  name: test-agent\n",
        )
        .unwrap();
        fs::write(backup_dir.join("data.db"), "db").unwrap();
        let allowlist = "\
version: 1
users:
  - id: 333
    label: bob
    added_by: null
    added_at: 2026-05-16T12:00:00Z
groups:
  - id: -444
    label: product
    opened_by: null
    opened_at: 2026-05-16T12:00:00Z
";
        fs::write(backup_dir.join("allowlist.yaml"), allowlist).unwrap();

        copy_agent_restore_config_files(&backup_dir, &agent_dir).unwrap();

        assert_eq!(
            fs::read_to_string(agent_dir.join("allowlist.yaml")).unwrap(),
            allowlist,
            "restore must materialize allowlist.yaml before bot startup"
        );
    }

    #[test]
    fn identity_mirror_files_are_not_treated_as_restore_config_files() {
        let tmp = TempDir::new().unwrap();
        let backup_dir = tmp.path().join("backup");
        let agent_dir = tmp.path().join("agent");
        fs::create_dir_all(&backup_dir).unwrap();
        fs::create_dir_all(&agent_dir).unwrap();

        fs::write(
            backup_dir.join("agent.yaml"),
            "sandbox:\n  name: test-agent\n",
        )
        .unwrap();
        fs::write(backup_dir.join("IDENTITY.md"), "# wrong source\n").unwrap();
        fs::write(backup_dir.join("SOUL.md"), "# wrong source\n").unwrap();
        fs::write(backup_dir.join("USER.md"), "# wrong source\n").unwrap();

        copy_agent_restore_config_files(&backup_dir, &agent_dir).unwrap();

        assert!(
            !agent_dir.join("IDENTITY.md").exists(),
            "restore config copy must not treat host identity files as authoritative for sandboxed agents"
        );
        assert!(
            !agent_dir.join("SOUL.md").exists(),
            "SOUL.md must come from sandbox identity mirror reconciliation"
        );
        assert!(
            !agent_dir.join("USER.md").exists(),
            "USER.md must come from sandbox identity mirror reconciliation"
        );
    }

    // ---- Task 2: search/delete variant existence (compile-time) ----

    #[test]
    fn memory_commands_search_variant_exists() {
        let _ = MemoryCommands::Search {
            agent: "x".to_string(),
            query: "q".to_string(),
            limit: 10,
            offset: 0,
            json: false,
        };
    }

    #[test]
    fn memory_commands_delete_variant_exists() {
        let _ = MemoryCommands::Delete {
            agent: "x".to_string(),
            id: 1,
        };
    }

    // ---- truncate_content tests ----

    #[test]
    fn truncate_content_truncates_long_string() {
        let s = "a".repeat(65);
        let result = truncate_content(&s, 60);
        let char_count: usize = result.chars().count();
        assert_eq!(
            char_count, 61,
            "truncated string should be 61 chars (60 + ellipsis), got {char_count}"
        );
        assert!(
            result.ends_with('…'),
            "truncated string should end with ellipsis"
        );
    }

    #[test]
    fn truncate_content_preserves_short_string() {
        let result = truncate_content("hello", 60);
        assert_eq!(result, "hello");
    }

    #[test]
    fn truncate_content_handles_multibyte() {
        // "你好世界test" = 4 CJK + 4 ASCII = 8 chars total
        let result = truncate_content("你好世界test", 4);
        // should not panic; 4 chars taken + ellipsis = 5 chars
        let char_count: usize = result.chars().count();
        assert_eq!(
            char_count, 5,
            "should be 5 chars (4 + ellipsis), got {char_count}"
        );
        assert!(result.ends_with('…'));
    }

    // ---- format_size tests ----

    #[test]
    fn format_size_bytes() {
        use super::format_size;
        assert_eq!(format_size(512), "512 B");
    }

    #[test]
    fn format_size_kb() {
        use super::format_size;
        assert_eq!(format_size(2048), "2.0 KB");
    }

    #[test]
    fn format_size_mb() {
        use super::format_size;
        assert_eq!(format_size(2_097_152), "2.0 MB");
    }

    // ---- config strict-sandbox tests ----

    #[test]
    fn config_commands_strict_sandbox_variant_exists() {
        // Compile-time check: ConfigCommands::StrictSandbox must exist.
        // If it doesn't compile, the test fails.
        let _cmd = ConfigCommands::StrictSandbox;
    }

    #[test]
    fn write_managed_settings_writes_correct_json_to_writable_dir() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("etc").join("claude-code");
        let path = dir.join("managed-settings.json");

        write_managed_settings(dir.to_str().unwrap(), path.to_str().unwrap())
            .expect("should succeed in writable temp dir");

        let content = fs::read_to_string(&path).unwrap();
        assert!(
            content.contains("\"allowManagedDomainsOnly\": true"),
            "file must contain allowManagedDomainsOnly:true, got: {content}"
        );
    }

    #[test]
    fn write_managed_settings_returns_error_with_sudo_hint_on_nonexistent_path() {
        // /nonexistent cannot be created without root.
        let result = write_managed_settings(
            "/nonexistent-rightclaw-test-dir",
            "/nonexistent-rightclaw-test-dir/managed-settings.json",
        );
        let err = result.expect_err("should fail on unwritable path");
        let msg = format!("{err:?}");
        assert!(msg.contains("sudo"), "error must mention sudo, got: {msg}");
    }

    #[test]
    fn write_managed_settings_is_idempotent() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("etc").join("claude-code");
        let path = dir.join("managed-settings.json");

        write_managed_settings(dir.to_str().unwrap(), path.to_str().unwrap())
            .expect("first call should succeed");

        write_managed_settings(dir.to_str().unwrap(), path.to_str().unwrap())
            .expect("second call should also succeed (idempotent)");

        let content = fs::read_to_string(&path).unwrap();
        assert!(content.contains("\"allowManagedDomainsOnly\": true"));
    }

    /// Creates a minimal agent directory with IDENTITY.md so discover_agents accepts it.
    fn make_agent_dir(base: &TempDir, name: &str) -> PathBuf {
        let agent_dir = base.path().join(name);
        fs::create_dir_all(&agent_dir).unwrap();
        fs::write(agent_dir.join("IDENTITY.md"), format!("# {name}\n")).unwrap();
        agent_dir
    }

    // ---- git init tests ----

    #[test]
    fn git_init_creates_dot_git_when_absent() {
        let tmp = TempDir::new().unwrap();
        let agent_dir = make_agent_dir(&tmp, "agent-git-test");

        assert!(
            !agent_dir.join(".git").exists(),
            "pre-condition: no .git yet"
        );

        // Run git init logic (same block as in cmd_up).
        if !agent_dir.join(".git").exists() {
            let status = std::process::Command::new("git")
                .arg("init")
                .current_dir(&agent_dir)
                .status();
            match status {
                Ok(s) if s.success() => {}
                Ok(s) => panic!("git init failed with status {s}"),
                Err(e) => panic!("git not found: {e}"),
            }
        }

        assert!(
            agent_dir.join(".git").exists(),
            ".git/ should exist after init"
        );
    }

    #[test]
    fn git_init_is_idempotent_when_dot_git_exists() {
        let tmp = TempDir::new().unwrap();
        let agent_dir = make_agent_dir(&tmp, "agent-idempotent");

        // First init.
        std::process::Command::new("git")
            .arg("init")
            .current_dir(&agent_dir)
            .status()
            .expect("first git init should succeed");

        assert!(agent_dir.join(".git").exists());

        // Second run of the conditional block — should NOT re-init.
        let was_skipped = agent_dir.join(".git").exists();
        if was_skipped {
            // Condition false — nothing happens.
        } else {
            std::process::Command::new("git")
                .arg("init")
                .current_dir(&agent_dir)
                .status()
                .unwrap();
        }

        assert!(
            agent_dir.join(".git").exists(),
            ".git/ still present after idempotent run"
        );
    }

    // ---- settings.local.json tests ----

    #[test]
    fn settings_local_json_created_when_absent() {
        let tmp = TempDir::new().unwrap();
        let agent_dir = make_agent_dir(&tmp, "agent-settings");
        let claude_dir = agent_dir.join(".claude");
        fs::create_dir_all(&claude_dir).unwrap();

        let settings_local = claude_dir.join("settings.local.json");
        assert!(
            !settings_local.exists(),
            "pre-condition: no settings.local.json"
        );

        if !settings_local.exists() {
            fs::write(&settings_local, "{}").unwrap();
        }

        assert!(
            settings_local.exists(),
            "settings.local.json should be created"
        );
        assert_eq!(fs::read_to_string(&settings_local).unwrap(), "{}");
    }

    #[test]
    fn settings_local_json_not_overwritten_when_exists() {
        let tmp = TempDir::new().unwrap();
        let agent_dir = make_agent_dir(&tmp, "agent-settings-preserve");
        let claude_dir = agent_dir.join(".claude");
        fs::create_dir_all(&claude_dir).unwrap();

        let settings_local = claude_dir.join("settings.local.json");
        let original_content = r#"{"theme":"dark","customKey":42}"#;
        fs::write(&settings_local, original_content).unwrap();

        // cmd_up conditional: only write if absent.
        if !settings_local.exists() {
            fs::write(&settings_local, "{}").unwrap();
        }

        let after = fs::read_to_string(&settings_local).unwrap();
        assert_eq!(
            after, original_content,
            "pre-existing content must not be overwritten"
        );
    }

    // ---- skills install tests ----

    #[test]
    fn skills_install_creates_builtin_skill_dirs() {
        let tmp = TempDir::new().unwrap();
        let agent_dir = make_agent_dir(&tmp, "agent-skills");

        right_codegen::install_builtin_skills(
            &agent_dir,
            &right_agent::agent::types::MemoryProvider::File,
        )
        .expect("install_builtin_skills should succeed");

        let skills_dir = agent_dir.join(".claude").join("skills");
        let skills_skill = skills_dir.join("right-skills").join("SKILL.md");
        assert!(
            skills_skill.exists(),
            "right-skills/SKILL.md should be installed"
        );
    }

    #[test]
    fn cmd_up_removes_stale_clawhub_skill_dir() {
        let tmp = TempDir::new().unwrap();
        let agent_dir = make_agent_dir(&tmp, "agent-stale");

        // Simulate pre-v2.2 state: clawhub dir exists
        let stale = agent_dir.join(".claude").join("skills").join("clawhub");
        std::fs::create_dir_all(&stale).unwrap();
        std::fs::write(stale.join("SKILL.md"), "old content").unwrap();
        assert!(stale.exists(), "stale dir should exist before cleanup");

        // Run cleanup (same logic as cmd_up inserts)
        let _ = std::fs::remove_dir_all(agent_dir.join(".claude/skills/clawhub"));

        assert!(
            !stale.exists(),
            "stale clawhub dir should be removed after cleanup"
        );
    }

    #[test]
    fn stale_cleanup_is_idempotent_when_dir_absent() {
        let tmp = TempDir::new().unwrap();
        let agent_dir = make_agent_dir(&tmp, "agent-no-stale");
        // No clawhub dir — cleanup should not error
        let result = std::fs::remove_dir_all(agent_dir.join(".claude/skills/clawhub"));
        // Either Ok or NotFound error — never panics
        assert!(result.is_ok() || result.unwrap_err().kind() == std::io::ErrorKind::NotFound);
    }

    #[test]
    fn cmd_up_removes_stale_skills_skill_dir() {
        let tmp = TempDir::new().unwrap();
        let agent_dir = make_agent_dir(&tmp, "agent-stale-skills");

        // Simulate Phase 12 intermediate state: skills/ dir exists
        let stale = agent_dir.join(".claude").join("skills").join("skills");
        std::fs::create_dir_all(&stale).unwrap();
        std::fs::write(stale.join("SKILL.md"), "old content").unwrap();
        assert!(stale.exists(), "stale dir should exist before cleanup");

        // Run cleanup (same logic as cmd_up inserts)
        let _ = std::fs::remove_dir_all(agent_dir.join(".claude/skills/skills"));

        assert!(
            !stale.exists(),
            "stale skills dir should be removed after cleanup"
        );
    }

    #[test]
    fn stale_skills_cleanup_is_idempotent_when_dir_absent() {
        let tmp = TempDir::new().unwrap();
        let agent_dir = make_agent_dir(&tmp, "agent-no-stale-skills");
        // No skills/ dir — cleanup should not error
        let result = std::fs::remove_dir_all(agent_dir.join(".claude/skills/skills"));
        // Either Ok or NotFound error — never panics
        assert!(result.is_ok() || result.unwrap_err().kind() == std::io::ErrorKind::NotFound);
    }

    // ---- McpCommands variant existence (compile-time) ----

    #[test]
    fn mcp_commands_status_variant_exists() {
        use super::McpCommands;
        let _ = McpCommands::Status { agent: None };
        let _ = McpCommands::Status {
            agent: Some("right".to_string()),
        };
    }

    // ---- cmd_mcp_status error paths ----

    #[tokio::test]
    async fn cmd_mcp_status_errors_on_nonexistent_agent() {
        use super::cmd_mcp_status;
        let tmp = TempDir::new().unwrap();
        let agents_dir = tmp.path().join("agents");
        fs::create_dir_all(&agents_dir).unwrap();

        let result = cmd_mcp_status(tmp.path(), Some("nonexistent")).await;
        let err = result.expect_err("should fail when agent not found");
        let msg = format!("{err:?}");
        assert!(
            msg.contains("agent not found"),
            "error must mention 'agent not found', got: {msg}"
        );
    }

    #[tokio::test]
    async fn cmd_mcp_status_fails_when_runtime_owner_is_unavailable() {
        use super::cmd_mcp_status;
        let tmp = TempDir::new().unwrap();
        let agent_dir = tmp.path().join("agents").join("myagent");
        fs::create_dir_all(&agent_dir).unwrap();

        let error = cmd_mcp_status(tmp.path(), Some("myagent"))
            .await
            .expect_err("MCP status must not fall back to a direct database open");
        let message = format!("{error:#}");
        assert!(
            message.contains("list MCP servers") || message.contains("internal"),
            "error must identify unavailable owner IPC: {message}"
        );
        assert!(
            !agent_dir.join("data.db").exists(),
            "MCP status must not create data.db when the owner is unavailable"
        );
    }
}

const MANAGED_SETTINGS_DIR: &str = "/etc/claude-code";
const MANAGED_SETTINGS_PATH: &str = "/etc/claude-code/managed-settings.json";

/// Write managed-settings.json to the given dir/path (extracted for testability).
fn write_managed_settings(dir: &str, path: &str) -> miette::Result<()> {
    std::fs::create_dir_all(dir).map_err(|e| {
        miette::miette!(
            help = "Run with elevated privileges: sudo right config strict-sandbox",
            "Permission denied creating {dir}: {e:#}"
        )
    })?;
    std::fs::write(path, "{\"allowManagedDomainsOnly\": true}\n").map_err(|e| {
        miette::miette!(
            help = "Run with elevated privileges: sudo right config strict-sandbox",
            "Permission denied writing {path}: {e:#}"
        )
    })?;
    Ok(())
}

fn cmd_config_strict_sandbox() -> miette::Result<()> {
    write_managed_settings(MANAGED_SETTINGS_DIR, MANAGED_SETTINGS_PATH)?;
    println!("Wrote {MANAGED_SETTINGS_PATH} — machine-wide domain blocking enabled.");
    Ok(())
}

/// Truncate content to at most `max_chars` characters, appending '…' if truncated.
/// Uses char-safe slicing (avoids byte-boundary panic on multi-byte UTF-8).
fn truncate_content(s: &str, max_chars: usize) -> String {
    let mut chars = s.chars();
    let prefix: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        format!("{prefix}…")
    } else {
        prefix
    }
}

/// Auto-scale byte count to human-readable size string.
fn format_size(bytes: u64) -> String {
    if bytes < 1_024 {
        format!("{bytes} B")
    } else if bytes < 1_048_576 {
        format!("{:.1} KB", bytes as f64 / 1_024.0)
    } else {
        format!("{:.1} MB", bytes as f64 / 1_048_576.0)
    }
}

/// Resolve agent directory and open its memory database.
///
/// Returns a live `Connection` or a fatal miette error.
struct OfflineAgentDb {
    _quiescence_guard: right_agent::runtime::RuntimeExclusionGuard,
    connection: right_db::Connection,
}
impl std::fmt::Debug for OfflineAgentDb {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("OfflineAgentDb { .. }")
    }
}

/// Resolve an agent database for commands that are explicitly offline-only.
async fn resolve_agent_db(home: &Path, agent: &str) -> miette::Result<OfflineAgentDb> {
    let agent_path = right_config::agents_dir(home).join(agent);
    if !agent_path.exists() {
        return Err(miette::miette!(
            "agent '{}' not found at {}",
            agent,
            agent_path.display()
        ));
    }
    let db_path = agent_path.join("data.db");
    if !db_path.exists() {
        return Err(miette::miette!(
            "no memory database for agent '{}' — run `right up` first",
            agent
        ));
    }
    let quiescence_guard = runtime_quiescence::require_runtime_quiesced(home).await?;
    let connection = right_db::open_connection(&agent_path, false)
        .await
        .map_err(|e| miette::miette!("failed to open data.db for '{}': {e:#}", agent))?;
    Ok(OfflineAgentDb {
        _quiescence_guard: quiescence_guard,
        connection,
    })
}

async fn cmd_memory_list(
    home: &Path,
    agent: &str,
    limit: i64,
    offset: i64,
    json: bool,
) -> miette::Result<()> {
    let db = resolve_agent_db(home, agent).await?;
    let conn = &db.connection;
    let mut stmt = conn
        .prepare(
            "SELECT id, content, tags, stored_by, created_at \
             FROM memories \
             WHERE deleted_at IS NULL \
             ORDER BY created_at DESC, id DESC \
             LIMIT ?1 OFFSET ?2",
        )
        .map_err(|e| miette::miette!("failed to list memories: {e:#}"))?;
    // Local SQLite row projection; extracting a named alias is out of scope.
    #[allow(clippy::type_complexity)]
    let entries: Vec<(i64, String, Option<String>, Option<String>, String)> = stmt
        .query_map(right_db::params![limit, offset], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
            ))
        })
        .await
        .map_err(|e| miette::miette!("failed to list memories: {e:#}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| miette::miette!("failed to list memories: {e:#}"))?;

    if json {
        for (id, content, tags, stored_by, created_at) in &entries {
            let obj = serde_json::json!({
                "id": id,
                "content": content,
                "tags": tags,
                "stored_by": stored_by,
                "created_at": created_at,
            });
            println!(
                "{}",
                serde_json::to_string(&obj)
                    .map_err(|e| miette::miette!("JSON serialization failed: {e:#}"))?
            );
        }
        return Ok(());
    }

    if entries.is_empty() {
        println!("No memories for agent '{agent}'.");
        return Ok(());
    }

    println!(
        "{:<6} {:<61} {:<20} CREATED_AT",
        "ID", "CONTENT", "STORED_BY"
    );
    for (id, content, _tags, stored_by, created_at) in &entries {
        let truncated = truncate_content(content, 60);
        let stored_by = stored_by.as_deref().unwrap_or("(unknown)");
        println!(
            "{:<6} {:<61} {:<20} {}",
            id, truncated, stored_by, created_at
        );
    }

    // Pagination footer (text mode only, when result count == limit)
    if entries.len() as i64 == limit {
        let total: i64 = conn
            .query_row(
                "SELECT count(*) FROM memories WHERE deleted_at IS NULL",
                [],
                |r| r.get(0),
            )
            .await
            .map_err(|e| miette::miette!("failed to count memories: {e:#}"))?;
        println!(
            "\n{} of {} entries shown  (--offset {} for next page)",
            limit,
            total,
            offset + limit
        );
    }

    Ok(())
}

async fn cmd_memory_stats(home: &Path, agent: &str, json: bool) -> miette::Result<()> {
    // resolve_agent_db validates agent dir and data.db existence before opening.
    let db = resolve_agent_db(home, agent).await?;
    let conn = &db.connection;

    // db_path needed only for fs metadata (file size) — derive from home, not conn.
    let db_path = right_config::agents_dir(home).join(agent).join("data.db");
    let db_size = std::fs::metadata(&db_path)
        .map_err(|e| miette::miette!("failed to stat data.db: {e:#}"))?
        .len();

    let (total_entries, oldest, newest): (i64, Option<String>, Option<String>) = conn
        .query_row(
            "SELECT count(*), min(created_at), max(created_at) \
             FROM memories WHERE deleted_at IS NULL",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .await
        .map_err(|e| miette::miette!("failed to query stats: {e:#}"))?;

    if json {
        let obj = serde_json::json!({
            "agent": agent,
            "db_size_bytes": db_size,
            "total_entries": total_entries,
            "oldest": oldest,
            "newest": newest,
        });
        println!("{obj}");
        return Ok(());
    }

    println!("Agent:         {agent}");
    println!("DB size:       {}", format_size(db_size));
    println!("Total entries: {total_entries}");
    println!("Oldest:        {}", oldest.as_deref().unwrap_or("(none)"));
    println!("Newest:        {}", newest.as_deref().unwrap_or("(none)"));

    Ok(())
}

async fn cmd_memory_search(
    home: &Path,
    agent: &str,
    query: &str,
    limit: i64,
    offset: i64,
    json: bool,
) -> miette::Result<()> {
    let db = resolve_agent_db(home, agent).await?;
    let conn = &db.connection;
    let search_err = |e: right_db::DbError| {
        miette::miette!(
            help = "Full-text search uses Turso MATCH syntax: use simple words or phrases.",
            "search failed: {e:#}"
        )
    };
    let mut stmt = conn
        .prepare(
            "SELECT m.id, m.content, m.tags, m.stored_by, m.created_at \
             FROM memories m \
             WHERE m.content MATCH ?1 \
               AND m.deleted_at IS NULL \
             ORDER BY m.created_at DESC, m.id DESC \
             LIMIT ?2 OFFSET ?3",
        )
        .map_err(search_err)?;
    // Local SQLite row projection; extracting a named alias is out of scope.
    #[allow(clippy::type_complexity)]
    let entries: Vec<(i64, String, Option<String>, Option<String>, String)> = stmt
        .query_map(right_db::params![query, limit, offset], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
            ))
        })
        .await
        .map_err(search_err)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(search_err)?;

    if json {
        for (id, content, tags, stored_by, created_at) in &entries {
            let obj = serde_json::json!({
                "id": id,
                "content": content,
                "tags": tags,
                "stored_by": stored_by,
                "created_at": created_at,
            });
            println!(
                "{}",
                serde_json::to_string(&obj)
                    .map_err(|e| miette::miette!("JSON serialization failed: {e:#}"))?
            );
        }
        return Ok(());
    }

    if entries.is_empty() {
        println!("No memories match '{query}' for agent '{agent}'.");
        return Ok(());
    }

    println!(
        "{:<6} {:<61} {:<20} CREATED_AT",
        "ID", "CONTENT", "STORED_BY"
    );
    for (id, content, _tags, stored_by, created_at) in &entries {
        let truncated = truncate_content(content, 60);
        let stored_by = stored_by.as_deref().unwrap_or("(unknown)");
        println!(
            "{:<6} {:<61} {:<20} {}",
            id, truncated, stored_by, created_at
        );
    }

    // Pagination footer (text mode only)
    if entries.len() as i64 == limit {
        println!(
            "\n{} results shown  (--offset {} for next page)",
            limit,
            offset + limit
        );
    }

    Ok(())
}

async fn cmd_memory_delete(home: &Path, agent: &str, id: i64) -> miette::Result<()> {
    use right_db::OptionalExtension;
    use std::io::{self, Write};

    let db = resolve_agent_db(home, agent).await?;
    let conn = &db.connection;

    // Check soft-deleted rows too (hard-delete works on any existing row).
    let any_row: Option<(String, Option<String>)> = conn
        .query_row(
            "SELECT content, stored_by FROM memories WHERE id = ?1",
            [id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .await
        .optional()
        .map_err(|e| miette::miette!("DB query failed: {e:#}"))?;

    match any_row {
        None => {
            return Err(miette::miette!(
                "memory entry {id} not found for agent '{agent}'"
            ));
        }
        Some((content, stored_by)) => {
            println!("  id:        {id}");
            println!("  content:   {}", truncate_content(&content, 60));
            println!(
                "  stored_by: {}",
                stored_by.as_deref().unwrap_or("(unknown)")
            );
        }
    }

    print!("Hard-delete this entry? [y/N]: ");
    io::stdout()
        .flush()
        .map_err(|e| miette::miette!("stdout flush failed: {e}"))?;
    let mut input = String::new();
    io::stdin()
        .read_line(&mut input)
        .map_err(|e| miette::miette!("failed to read input: {e}"))?;

    if input.trim().to_lowercase() != "y" {
        println!("Aborted.");
        return Ok(());
    }

    let deleted = conn
        .execute("DELETE FROM memories WHERE id = ?1", [id])
        .await
        .map_err(|e| miette::miette!("failed to delete memory: {e:#}"))?;
    if deleted == 0 {
        return Err(miette::miette!(
            "memory entry {id} not found for agent '{agent}'"
        ));
    }

    println!("Deleted memory entry {id}.");
    Ok(())
}

fn cmd_pair(home: &Path, agent_name: Option<&str>) -> miette::Result<()> {
    let agent_name = agent_name.unwrap_or("right");

    let agents_dir = right_config::agents_dir(home);
    let all_agents = right_agent::agent::discover_agents(&agents_dir)?;

    let agent = all_agents
        .iter()
        .find(|a| a.name == agent_name)
        .ok_or_else(|| {
            let available: Vec<&str> = all_agents.iter().map(|a| a.name.as_str()).collect();
            miette::miette!(
                "agent '{}' not found. Available agents: {}",
                agent_name,
                available.join(", ")
            )
        })?;

    // Ensure schemas exist (function may run without prior cmd_up).
    let claude_dir = agent.path.join(".claude");
    std::fs::create_dir_all(&claude_dir)
        .map_err(|e| miette::miette!("failed to create .claude dir for '{}': {e:#}", agent_name))?;
    std::fs::write(
        claude_dir.join("reply-schema.json"),
        right_codegen::REPLY_SCHEMA_JSON,
    )
    .map_err(|e| {
        miette::miette!(
            "failed to write reply-schema.json for '{}': {e:#}",
            agent_name
        )
    })?;
    std::fs::write(
        claude_dir.join("cron-schema.json"),
        right_codegen::CRON_SCHEMA_JSON,
    )
    .map_err(|e| {
        miette::miette!(
            "failed to write cron-schema.json for '{}': {e:#}",
            agent_name
        )
    })?;

    // Assemble system prompt on host.
    let base_prompt =
        right_codegen::generate_system_prompt(&agent.name, &agent.path.to_string_lossy());
    let mut prompt = base_prompt;
    prompt.push_str("\n## Operating Instructions\n");
    prompt.push_str(right_codegen::OPERATING_INSTRUCTIONS);
    prompt.push('\n');
    for (file, header) in [
        ("IDENTITY.md", "## Your Identity"),
        ("SOUL.md", "## Your Personality and Values"),
        ("USER.md", "## Your User"),
        ("TOOLS.md", "## Environment and Tools"),
    ] {
        if let Ok(content) = std::fs::read_to_string(agent.path.join(file)) {
            prompt.push_str(&format!("\n{header}\n"));
            prompt.push_str(&content);
            prompt.push('\n');
        }
    }
    let prompt_path = claude_dir.join("composite-system-prompt.md");
    std::fs::write(&prompt_path, &prompt).map_err(|e| {
        miette::miette!("failed to write system prompt for '{}': {e:#}", agent_name)
    })?;

    let claude_bin = which::which("claude")
        .or_else(|_| which::which("claude-bun"))
        .map_err(|_| miette::miette!("claude CLI not found in PATH (tried: claude, claude-bun)"))?;

    use std::os::unix::process::CommandExt;
    let err = std::process::Command::new(claude_bin)
        .arg("--system-prompt-file")
        .arg(&prompt_path)
        .arg("--dangerously-skip-permissions")
        .current_dir(&agent.path)
        .exec();

    Err(miette::miette!("failed to launch claude: {err}"))
}

async fn cmd_mcp_status(home: &Path, agent_filter: Option<&str>) -> miette::Result<()> {
    let agents_dir = right_config::agents_dir(home);
    let mut agent_names = if let Some(name) = agent_filter {
        let dir = agents_dir.join(name);
        if !dir.is_dir() {
            return Err(miette::miette!("agent not found: {name}"));
        }
        vec![name.to_owned()]
    } else {
        let mut names = std::fs::read_dir(&agents_dir)
            .map_err(|e| miette::miette!("cannot read agents dir: {e:#}"))?
            .filter_map(Result::ok)
            .filter(|entry| entry.path().is_dir())
            .filter_map(|entry| entry.file_name().into_string().ok())
            .collect::<Vec<_>>();
        names.sort();
        names
    };
    agent_names.sort();

    let client = right_mcp::internal_client::InternalClient::new(home.join("run/internal.sock"));
    let mut any = false;
    for agent_name in agent_names {
        let response = client
            .mcp_list(&agent_name)
            .await
            .map_err(|e| miette::miette!("list MCP servers for {agent_name}: {e:#}"))?;
        for server in response.servers {
            println!(
                "{agent_name}  {} [{}]",
                server.name,
                server.url.as_deref().unwrap_or("-")
            );
            any = true;
        }
    }
    if !any {
        println!("No MCP servers configured.");
    }
    Ok(())
}
