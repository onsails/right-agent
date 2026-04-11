# Changelog
## [0.1.0] - 2026-04-11


### Bug Fixes

- **bot**: Parse KEY=VALUE env file format in resolve_token
- **bot**: Inline json-schema, parse structured_output, add --debug passthrough
- **bot**: Stop CC Telegram plugin from racing with native bot for updates
- **bot**: Sandbox compat — dangerously-skip-permissions, USE_BUILTIN_RIPGREP, /start command
- **bot**: Pass --debug through rightclaw up → process-compose → bot subprocess
- **bot**: Log CC stderr at INFO when --debug (was DEBUG, invisible at default log level)
- **worker**: Handle plain-string CC result in parse_reply_output
- **29-01**: Sandbox dependency detection for nix environments
- **30-01**: Collapse nested if in cron.rs to fix clippy collapsible_if warning
- **v3.2-gaps**: Cloudflared --config flag + bot startup MCP warn
- **v3.2-gaps**: /mcp list emoji icons + /doctor HTML code block
- Restore MCP OAuth implementation deleted in f37e9da
- Invoke_cc uses SSH instead of non-existent openshell exec
- Address code review findings — policy gen, child monitoring, doctor, ssh check
- Reap sandbox create child, make refresh scheduler sandbox-aware
- Sandbox create is long-lived monitor — drop handle instead of wait
- Shell-escape SSH remote args — JSON schema breaks remote shell
- Improve invoke_cc debuggability — log sandbox mode, exit code, stderr
- Log stdout+stderr at ERROR level on claude -p failure
- Copy .claude/ into staging before sandbox upload
- Address code review findings for login flow
- Wildcard anthropic domains in sandbox policy, tighten OAuth URL scraper
- Token refresh writes to agent_dir/.mcp.json, not staging/
- Run sync immediately on startup, add /sandbox trust to .claude.json
- Block bot startup until initial sync completes
- Extract_auth_url matches claude.com, cleaner Telegram messages
- Sync writes fixed .claude.json with correct filename (was .claude.json.fixed)
- Two-step URL extraction — wait for 'Browser didn't open' then extract URL
- Set PTY width to 500 cols so OAuth URL doesn't wrap across lines
- Send auth code with \r (CC TUI needs carriage return), add API key to success patterns
- Integration test sets CLAUDE_CONFIG_DIR to prevent host config pollution
- Login code prompt detection and code submission
- Add --mcp-config + --strict-mcp-config to invoke_cc
- Add .mcp.json to sync_cycle
- Add logging for every reply outcome in worker
- Allow external MCP servers through OpenShell network policy
- Address review issues (2 iterations)
- Cancel OAuth refresh timer on MCP server removal + rename memory tools to record
- **bot**: Resolve clippy warnings (collapsible-if, too-many-arguments)
- Address review issues (iteration 1)
- Address review issues (iteration 2)
- Guard reverse sync deletions against sandbox unreachability
- Address review issues (2 iterations)
- Add network_policy to bot test struct literal
- Address review issues (2 iterations)
- Attachments used agent_name instead of sandbox_name for upload/download
- Address review issues (2 iterations)
- Resolve host IP dynamically for sandbox MCP connectivity
- Remove inline keyboard on completion, tighten StopTokens visibility
- Extract agents_dir() helper, fix SSH discovery bug, add agent list
- Use directory paths for all upload_file callers
- Atomic activate_session, label search in /switch, touch on switch, tests
- Address review issues (2 iterations)
- Rename cronsync skill dir to rightcron, fixing sandbox upload
- Clippy collapsible_if + update schema version test to V5
- Address review issues (2 iterations)
- Address review issues (2 iterations)
- Reverse sync downloaded from wrong path (.claude/agents/ instead of /sandbox/)
- Reverse sync only CC-modified files, not codegen-generated ones
- Address review issues — error handling, HTML escaping, trigger tracking
- Address review issues (2 iterations)

### Documentation

- **38-01**: Complete TunnelConfig credentials-file refactor plan

### Features

- **23-02**: Scaffold rightclaw-bot crate with full teloxide skeleton
- **23-03**: Wire rightclaw bot subcommand into CLI
- **24-02**: Wire generate_system_prompt into cmd_up and cmd_pair
- **25-01**: Implement session.rs — effective_thread_id + DB CRUD with TDD
- **25-02**: Implement pure helpers in worker.rs (TDD GREEN)
- **25-02**: Implement spawn_worker, invoke_cc async functions
- **25-03**: Create handler.rs with handle_message and handle_reset
- **25-03**: Rewrite dispatch.rs with DashMap worker map + BotCommand schema
- **25.5-02**: Migrate worker to --agent/--json-schema; replace parse_reply_tool
- **26-02**: Add deleteWebhook before run_telegram in bot/src/lib.rs
- **27-01**: Cron scheduling engine in crates/bot/src/cron.rs
- **init**: Replace --telegram-user-id with --telegram-allowed-chat-ids
- **bot**: Add CC invocation timeout + message routing debug logs
- **bot/diag**: Add diagnostic logging for update routing and filter drops
- **cron**: Parse CC structured output and send reply to Telegram
- **34-01**: Add workspace deps + OAuth types module with PKCE/state utilities
- **34-04**: Add oauth_callback.rs module with unit tests
- **34-04**: Wire oauth_callback server into lib.rs + dispatch.rs
- **34-04**: Implement /mcp and /doctor bot command handlers
- **35-01**: Backfill OAuth callback to write client_id and client_secret into credential
- **35-03**: Spawn run_refresh_scheduler in bot lib.rs at startup
- **36-01**: Implement TunnelConfig::hostname() and remove --tunnel-hostname arg
- **37-01**: TunnelConfig hostname field + AgentDir/RightclawHome newtypes
- **37-02**: Add tracing::info! at entry of all mcp bot handlers
- **41-01**: Rewrite detect.rs -- check headers instead of credentials file
- **41-02**: Remove credentials_path from run_refresh_scheduler
- **41-02**: Complete .credentials.json migration — remove all OAuth credential file references
- **quick-260405-srr**: Rewire bot, delete oauth_callback.rs, clean workspace deps
- Restore OAuth flow for headless agents, write tokens to .claude.json
- Show stdio MCP servers in /mcp list, protect rightmemory from removal
- HTTP MCP server, OpenShell sandbox module, refresh scheduler wiring
- Add --no-sandbox flag to rightclaw bot CLI
- Conditional SSH vs direct invoke_cc based on sandbox mode
- Auth error detection + interactive login flow via Telegram
- Persistent sandbox lifecycle + background sync + MCP upload
- PTY-driven login flow replaces PC login process
- Comprehensive login logging + per-agent file logging
- Per-agent secret in agent.yaml + HTTP mcp.json generation
- **bot**: Add attachment types, mime_to_extension, and constants
- **bot**: Add YAML input formatting and update DebounceMsg for attachments
- **config**: Add attachments.retention_days to AgentConfig
- **bot**: Add extract_attachments for all Telegram media types
- **bot**: Extract text and attachments from all Telegram media types in handler
- **bot**: Switch to stdin piping, YAML input, typed attachment output in worker
- **bot**: Implement attachment download/upload and outbound send pipelines
- **bot**: Create inbox/outbox directories on startup and add periodic cleanup
- Add config watcher + notify deps for graceful restart
- Wire graceful restart — CancellationToken through all subsystems
- Add reverse_sync_md — sync .md files from sandbox to host
- Wire reverse_sync_md into worker after each claude -p call
- Add NetworkPolicy enum to AgentConfig
- Bootstrap onboarding via composite system prompt
- Stream logging, live thinking, markdown rendering, response rules
- Bot-owned codegen + config reload via wizard
- Define StopTokens type and inject into dispatch/handler
- Add callback query handler for Stop button
- Stop button keyboard, token lifecycle, and post-completion edits
- Pass --model from agent.yaml to Telegram worker
- Cron budget control + model passthrough
- Warn when cron schedule uses :00 or :30 minutes
- Disable CC built-in cron, task, plan, and remote tools
- Rewrite session.rs for multi-session CRUD
- Update worker to use multi-session CRUD with label support
- Replace /reset with /new, /list, /switch commands
- **cron**: CronReplyOutput struct and parse_cron_output parser
- **cron**: Persist cron output to DB instead of direct Telegram delivery
- **cron**: Persist results to DB instead of direct Telegram delivery
- **cron**: Shared idle timestamp across handler/worker
- **cron**: Cron_delivery module — DB query/dedup/mark functions with tests
- **cron**: Delivery poll loop — idle detection, CC session delivery, cleanup
- **cron**: Wire delivery loop into bot startup
- **cron**: Write cron-schema.json during codegen
- **cron**: Replace load_specs filesystem scan with load_specs_from_db
- **cron**: Add /cron telegram command (list + detail)

### Miscellaneous

- **25-01**: Add uuid, dashmap, tokio-util, serde_json, which to bot crate
- Fix clippy warnings (collapsible_if, too_many_arguments, needless_borrow)

### Refactor

- **bot**: Address review findings — doc, noise, blank line
- **cron**: Address review issues — clippy, tests, comments
- Rewrite credential functions for flat .mcp.json structure
- Update all callers to use flat .mcp.json credential functions
- Invoke_cc uses openshell sandbox exec instead of direct claude -p
- Remove telegram_token_file — token via RC_TELEGRAM_TOKEN env var only
- Delete old codegen/sandbox.rs, replaced by openshell.rs
- Rename .mcp.json to mcp.json (OpenShell dot-file workaround)
- Rename rightmemory → right across entire codebase
- **bot**: Simplify attachment code (hoist dir creation, use writeln!, single mkdir)
- Restructure config CLI + add combined settings wizard
- Remove --no-sandbox CLI flag, bot reads sandbox mode from agent.yaml
- Extract send_html_reply + format_session_line helpers, fold touch into activate

### Testing

- **25.5-02**: Add failing tests for parse_reply_output structured output
- **bot**: Add wait_with_timeout helper + tests for CC invocation timeout
- **v3.2**: Complete MCP OAuth UAT — 8/9 passed, 1 blocked (tunnel DNS)
- Integration test for claude login PTY flow
- Add multi-session lifecycle integration test + fix clippy

### Ux

- Smart OpenShell pre-flight check with interactive recovery
