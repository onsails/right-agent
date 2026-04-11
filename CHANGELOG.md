# Changelog
## [0.1.0] - 2026-04-11


### Bug Fixes

- Remove stale plugin install hint — settings.json handles it now
- Require telegram user ID when token is provided
- Add --use-uds flag so process-compose creates Unix socket
- Switch PcClient from Unix socket to TCP (process-compose --use-uds crashes TUI)
- Use process-compose CLI client for restart instead of REST API
- Use script(1) for real PTY, disable restart (PC bug)
- Use --detached instead of --detached-with-tui (TUI needs /dev/tty)
- Stable lock file for rightcron hook, cleared on each rightclaw up
- **260327-04d**: Use absolute binary path in .mcp.json command field
- **up**: Pre-create .claude/shell-snapshots/ to prevent CC Bash tool source error
- **19-01**: Telegram false-positive, RC_AGENT_NAME injection, mcp_config_path removal
- **up**: Symlink agent .claude/plugins to host plugins — telegram plugin not installed
- Auto-install Telegram plugin during rightclaw up
- Auto-install bun runtime for Telegram channel plugin
- **bot**: Inline json-schema, parse structured_output, add --debug passthrough
- **bot**: Pass --debug through rightclaw up → process-compose → bot subprocess
- **29-01**: Sandbox dependency detection for nix environments
- **37**: Address post-review issues
- **v3.2-gaps**: Cloudflared --config flag + bot startup MCP warn
- **v3.2-gaps**: Cloudflared --config must precede 'run' subcommand
- **cloudflared**: Use credentials-file for local ingress instead of --token
- **38**: Suppress dead_code on legacy token field, remove needless borrow
- **39-02**: Guard prompt_telegram_token with yes flag
- Restore MCP OAuth implementation deleted in f37e9da
- Cloudflared --overwrite-dns + doctor HTML escape fallback
- **43**: Gate brew_prefix() under #[cfg(target_os = "macos")]
- Restore v3.3 MCP tools + planning artifacts deleted by 9297d83
- Address code review findings — policy gen, child monitoring, doctor, ssh check
- `down` checks REST API instead of stale state.json
- Allow external MCP servers through OpenShell network policy
- Remove stale "restart required" notes from MCP tool messages
- Address review issues (2 iterations)
- Cancel OAuth refresh timer on MCP server removal + rename memory tools to record
- Fail early when tunnel credentials file is missing
- Update generate_policy call sites with NetworkPolicy param
- Address review issues (2 iterations)
- Address review issues (2 iterations)
- Address review issues (iteration 1)
- Address review issues (2 iterations)
- Resolve host IP dynamically for sandbox MCP connectivity
- Extract agents_dir() helper, fix SSH discovery bug, add agent list
- Clippy collapsible_if in MCP cron tools
- Address review issues (2 iterations)
- Address review issues — error handling, HTML escaping, trigger tracking
- Address review issues (2 iterations)

### Documentation

- **38-01**: Complete TunnelConfig credentials-file refactor plan

### Features

- **01-01**: Scaffold Cargo workspace, devenv, and project conventions
- **01-02**: Wire CLI init/list commands and integration tests
- **02-01**: Add Phase 2 workspace deps, templates, and codegen module structure
- **02-03**: Wire all CLI subcommands into main.rs
- **03-03**: Extend init with telegram_token, BOOTSTRAP.md, and policy variant
- **03-04**: Wire Doctor subcommand and Init --telegram-token to CLI
- **03-04**: Shell wrapper --channels support and integration tests
- **04-02**: Add system prompt generation and update shell wrapper template
- **04-02**: Wire system prompt generation into cmd_up
- **03.2-01**: Add rightclaw pair subcommand for interactive agent setup
- Add --telegram-user-id for auto-pairing via access.json
- Add --debug flag to rightclaw up for Claude Code debug logging
- Rename cronsync to rightcron
- **08-01**: Wire generate_agent_claude_json and credential symlink into cmd_up and init
- **08-02**: Add allow_read to SandboxOverrides and switch to absolute denyRead paths
- **08-02**: Add HOME isolation integration tests and wire credential symlink into init
- **09-02**: Extend cmd_up per-agent loop with Phase 9 steps 6-9
- **10-01**: Add config strict-sandbox command to CLI
- **12-01**: Rename clawhub to skills — source dir, constant, install path, all test assertions
- **12-01**: Add stale .claude/skills/clawhub/ cleanup in cmd_up (SKILLS-05)
- **14-01**: Update Rust constant, include path, install path, and test assertions
- **15-01**: Remove stale .claude/skills/skills/ dir in cmd_up with unit tests (CLEANUP-02)
- **16-02**: Remove memory_path from AgentDef and all struct literal sites
- **16-03**: Wire open_db into cmd_up step 10
- **17-02**: MCP memory server + MemoryServer subcommand
- **17-02**: .mcp.json codegen in cmd_up + start_prompt memory reference
- **18-02**: Wire rightclaw memory subcommand group into CLI
- **23-03**: Wire rightclaw bot subcommand into CLI
- **24-02**: Wire generate_system_prompt into cmd_up and cmd_pair
- **25.5-01**: Wire agent_def into cmd_up + cmd_pair; delete system_prompt.rs
- **26-01**: Update cmd_up callsite; remove channels block; add bot-agent early-exit
- **27-02**: Rename MCP server to rightclaw + add cron_list_runs/cron_show_run tools
- **init**: Replace --telegram-user-id with --telegram-allowed-chat-ids
- **33-01**: Add McpCommands, cmd_mcp_status, and cmd_up auth warn
- **34-01**: GlobalConfig + read/write config.yaml + init --tunnel-token/--tunnel-url
- **34-02**: AS discovery RFC 9728->8414->OIDC, DCR, auth URL builder, token exchange
- **36-01**: Implement TunnelConfig::hostname() and remove --tunnel-hostname arg
- **37-01**: TunnelConfig hostname field + AgentDir/RightclawHome newtypes
- **37-03**: Write cloudflared-start.sh in cmd_up + eprintln MCP warning
- **v3.2-gaps**: Restore rightclaw mcp status CLI subcommand
- **38-02**: Update cmd_init — accept credentials-file, copy to tunnel dir, write TunnelConfig
- **38-02**: Update cmd_up — use TunnelConfig directly, add cloudflared module and wrapper script
- **39-01**: Auto-detect/create cloudflared named tunnel in rightclaw init
- **40-01**: Wire cloudflared pre-flight check and script passthrough in cmd_up
- **41-01**: Rewrite detect.rs -- check headers instead of credentials file
- **quick-260405-srr**: Rewire bot, delete oauth_callback.rs, clean workspace deps
- Show stdio MCP servers in /mcp list, protect rightmemory from removal
- **01-01**: Add agent_dir+rightclaw_home to MemoryServer, inject RC_RIGHTCLAW_HOME into .mcp.json
- **01-02**: Add mcp_add, mcp_remove, mcp_list, mcp_auth tools to MemoryServer
- **42-01**: Add ChromeConfig struct + RawChromeConfig + read/write support
- **42-02**: Extend generate_mcp_config() with chrome-devtools injection
- **42-03**: Wire chrome_cfg into cmd_up() per-agent loop
- **43-01**: Add ChromeConfig to config.rs + Chrome/MCP detection helpers
- **43-01**: --chrome-path arg + cmd_init single write path refactor
- **43-02**: Per-run Chrome path revalidation in cmd_up()
- HTTP MCP server, OpenShell sandbox module, refresh scheduler wiring
- Wire OpenShell sandbox lifecycle into cmd_up
- Cmd_down deletes OpenShell sandboxes
- Add --no-sandbox flag to rightclaw bot CLI
- Auth error detection + interactive login flow via Telegram
- Comprehensive login logging + per-agent file logging
- Per-agent secret in agent.yaml + HTTP mcp.json generation
- Add right-mcp-server process to process-compose
- Add memory-server-http CLI subcommand
- Guard mcp_auth with tunnel health check before OAuth discovery
- Add wizard module with interactive tunnel/telegram/config flows
- Add rightclaw config set and rightclaw agent config subcommands
- Add config watcher + notify deps for graceful restart
- Add --network-policy CLI flag to rightclaw init
- Add rightclaw agent init, sandbox mode in init wizard
- Add rightclaw reload command
- Agent init suggests rightclaw reload
- Bootstrap onboarding via composite system prompt
- Bot-owned codegen + config reload via wizard
- Implement rightclaw agent ssh command
- **cron**: Write cron-schema.json during codegen
- **cron**: MCP CRUD tools for cron specs (stdio transport)
- **cron**: MCP CRUD tools for cron specs (HTTP transport)

### Miscellaneous

- Fix clippy warnings from OpenShell integration
- Fix clippy warnings (collapsible_if, too_many_arguments, needless_borrow)

### Refactor

- **01-01**: Extract memory_server.rs tests to memory_server_tests.rs
- Update all callers to use flat .mcp.json credential functions
- Remove telegram_token_file — token via RC_TELEGRAM_TOKEN env var only
- Remove OpenShell sandbox lifecycle from cmd_up/cmd_down
- Remove CC native sandbox from settings.json — OpenShell is the security layer
- Remove plugin symlinks — Telegram CC plugin no longer used
- Rename .mcp.json to mcp.json (OpenShell dot-file workaround)
- Rename rightmemory → right across entire codebase
- Remove Chrome detection and injection from CLI
- Init delegates to wizard for tunnel and telegram setup
- Replace __skip_tunnel__ sentinel with TunnelOutcome enum, use enum-based menu
- Restructure config CLI + add combined settings wizard
- Per-agent sandbox mode in process-compose codegen
- Remove --no-sandbox CLI flag, bot reads sandbox mode from agent.yaml
- Extract codegen pipeline from cmd_up into shared function
- Extract filter_agents helper, avoid agent.yaml re-reads
- Extract AgentDef::sandbox_mode(), clean up error messages

### Testing

- **02-03**: Integration tests for new CLI subcommands
- **39-01**: Add failing tests for auto-tunnel helpers
- **01-02**: Add tests for all four new MCP tools in memory_server_tests.rs
- Remove stale CC-native sandbox tests from home_isolation
- Fix integration tests for inquire-based interactive prompts
- Integration tests for reload command and agent init hint
- Add multi-session lifecycle integration test + fix clippy

### Deps

- Add inquire 0.9 for interactive terminal prompts

### Merge

- **08-01**: Home isolation shell wrapper and claude_json codegen

### Ux

- Print next steps after init completes
- Smart OpenShell pre-flight check with interactive recovery
