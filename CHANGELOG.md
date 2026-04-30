# Changelog
## [0.2.7] - 2026-04-30


### Bug Fixes

- **oauth**: Drop misleading "next session" notice from auth success
- **oauth**: Try origin-only well-known URLs for path-bearing MCP
- **oauth**: Skip speculative probes on any non-2xx, not just 404
- **oauth**: WWW-Authenticate parser rejects empty quoted value

### Documentation

- **oauth**: Refresh discover_as comment to match new tolerant contract

### Features

- **oauth**: Parse resource_metadata from WWW-Authenticate header
- **oauth**: Probe WWW-Authenticate for resource_metadata URL

### Testing

- **oauth**: Regression for Linear-pattern AS discovery
- **oauth**: Tighten as_metadata_urls assertions to positional indices
- **oauth**: Pin WWW-Authenticate path with wiremock .expect(1)
- **oauth**: Clarify Step 0 implications in discovery tests

## [0.2.6] - 2026-04-29


### Bug Fixes

- **bot**: Tighten bootstrap_photo visibility and avoid PNG clone
- **bot**: End webhook stream on signal so dispatcher exits cleanly
- **bot**: Drain task panicked when run_async returned Err early
- **bot**: Bootstrap welcome photo as caption + square coal frame
- **webhook**: Drop trailing slash so axum nest matches Telegram POSTs
- **brand**: Drop DarkGrey from inquire chrome — render as pastel blue on macOS Terminal
- **brand**: Orange '>' cursor in inquire prompts
- **policy**: Include /var/log in read_only to silence false drift WARN
- **doctor**: Drop trailing slash from expected webhook URL
- **config**: Propagate read_global_config error from McpServer; doctor doc
- **init**: Write config.yaml before per-agent codegen
- **brand**: Lowercase main.rs prompts + monochrome inquire RenderConfig
- **init**: New agents created sandbox 'rightclaw-{name}' but agent.yaml said 'right-{name}'
- **rebootstrap**: Correct misleading --yes doc (it's yes/no, not typed-name)
- **runtime**: Use X-PC-Token-Key for process-compose API auth
- **cron**: Single-source delivery timings; drop misleading trigger Confirm:

### Documentation

- **ui**: Doc comment on Line struct
- **ui**: Add doc comments on splash and section pub fns
- **init**: Update stale --force references to --force-recreate
- **rebootstrap**: Document migrate:false assumption in deactivate_active_sessions
- **mcp**: Document operation-error convention and per-tool codes

### Features

- **bot**: Add bootstrap_photo module with predicate and PNG asset
- **bot**: Send bootstrap welcome photo with first agent reply
- **bot**: Webhook router module with secret-token enforcement
- **bot**: Mount webhook router on bot.sock UDS server
- **bot**: Dispatch via webhook UpdateListener instead of long-poll
- **bot**: SetWebhook register loop with retry/backoff
- **sync**: Drop AGENTS.md from reverse-sync allowlist
- **codegen**: Cloudflared is unconditional in pipeline & process-compose
- **bot**: Rename UDS to bot.sock
- **codegen**: /tg/<agent>/.* ingress rule per agent
- **doctor**: Expect webhook to be set; healthz check; FAIL on missing tunnel
- **agent**: Best-effort deleteWebhook on destroy
- **mcp**: Add tool_error helper and From<ProxyError> for CallToolResult
- **ui**: Scaffold right-agent::ui module skeleton
- **ui**: Theme detection (color/mono/ascii)
- **ui**: Rail + semantic glyphs with three theme tiers
- **ui**: Status line + block builder with column alignment
- **ui**: Splash + section header
- **ui**: Recap builder with column-aligned status block
- **ui**: Writers + BlockAlreadyRendered sentinel docs
- **register**: Skeleton + no-PC path
- **register**: PC-alive happy path with optional restart
- **init**: Stop emitting AGENTS.md template on agent init
- **rebootstrap**: Add module skeleton with plan() and tests
- **rebootstrap**: Add backup_host_files and backup_sandbox_files
- **rebootstrap**: Add delete_identity_from_host
- **rebootstrap**: Add write_bootstrap_md
- **rebootstrap**: Add deactivate_active_sessions
- **rebootstrap**: Add delete_identity_from_sandbox
- **rebootstrap**: Implement execute() orchestrator
- **config**: Make Cloudflare Tunnel mandatory
- **wizard**: Drop Skip option from tunnel setup
- **aggregator**: Translate ProxyError at dispatch boundary
- **aggregator**: Memory_retain operation errors return is_error
- **aggregator**: Memory_recall/reflect operation errors return is_error
- **right_backend**: Allowlist and bootstrap_done emit structured tool_error
- **wizard**: Require Telegram bot token in `right agent init`
- **wizard**: Confirm on Ctrl+C, require chat ID in `right agent init`
- **doctor**: Render diagnostics as brand-conformant block
- **status**: Brand-conformant rail+glyph block
- **init**: Splash + dependency probe block
- **init**: Section headers + sandbox-creation status lines
- **init**: Recap block replaces footer
- **agent-init**: Section header + recap
- **cli**: --no-color global flag
- **cli**: Hot-add new agent to running process-compose
- **prompt**: Drop AGENTS.md section from composite system prompt
- **rebootstrap**: Wire CLI subcommand right agent rebootstrap
- **rebootstrap**: Surface sandbox-cleanup-skipped to operator

### Miscellaneous

- **bot**: Use bytes = "1.0" per project versioning rule
- **bot**: Simplify bootstrap_photo and CcReply

### Refactor

- **bot**: Expose is_first_call from invoke_cc via CcReply struct
- **bot**: Drop obsolete pre-startup deleteWebhook
- **ui**: Tighten theme detection visibility to pub(crate)
- **init**: Lowercase-first prompt copy per brand
- **register**: Single warn on reload failure
- **mcp**: Simplify pass — shorten tool_error paths, fix tempdir leak
- **wizard**: Lowercase tunnel/telegram/chat-id copy + rail status
- **wizard**: Drop duplicate theme rebinds in DeleteAndRecreate
- **agent-init**: Drop duplicate theme rebind; rename test
- **wizard**: Lowercase settings menu copy + rail saved lines
- **wizard**: Lowercase memory/stt/sandbox copy + rail status
- **wizard**: Consolidate theme rebinds + diagnostic unreachable msg
- **wizard**: Brand warn lines on validation re-prompt
- **cli**: Rename agent init --force to --force-recreate
- **agent-def**: Drop agents_path field
- **rebootstrap**: Simplify sandbox preamble + propagate host delete errors
- **rebootstrap**: Brand-conformant CLI output via ui:: helpers

### Testing

- **bot**: Webhook router integration tests
- **right-bot**: #[ignore] claude_upgrade_lifecycle as slow
- **codegen**: Write minimal config.yaml in tempdir-based tests
- Raise MAX_CONCURRENT_SANDBOX_TESTS to 30
- Add acquire_test_name_lock for cross-worktree resource locks
- TestSandbox holds per-name lock across worktrees
- Shared sandbox for upload/download/verify + wait_for_ssh
- **register**: Cover stale and malformed state.json
- **ui**: Recap rendering for init's three end states
- Drop AGENTS.md from doctor/platform_store/destroy fixtures
- **rebootstrap**: Add live-sandbox integration test
- **right**: Right up rejects missing/incomplete tunnel config
- **right**: Ignore init_warns_when_host_creds_missing post-mandatory-tunnel
- **right_backend**: Cover bootstrap_done structured error path
- **aggregator**: Cover Hindsight operation-error mappings
- Drop slow/duplicate tests, replace sandbox check with manifest unit test
- **right**: Cross-worktree lock for right up tunnel tests
- **doctor**: Rename + ascii-fallback assertions
- **agent-init**: Assert recap block on completion
- **voice**: Lowercase + no-exclamation regression for prompt labels
- **voice**: Cover Select options + lowercase 'use HINDSIGHT_API_KEY'
- **brand**: Ascii fallback + --no-color flag coverage
- **brand**: Conformance lint — rail + no-marketing + no-period
- **cli**: Update agent init tests for --force-recreate rename
- **cli**: Clarify --force comment in negative test
- **cli**: Drop AGENTS.md from cli_integration fixtures and assertions
- **rebootstrap**: Add CLI surface tests

## [0.2.5] - 2026-04-27


### Bug Fixes

- **bot/worker**: Collect_batch keeps debounce idle-timeout semantics

### Features

- **bot/filter**: Admit Telegram media-group siblings without per-message mention
- **bot/worker**: Carry media_group_id on DebounceMsg
- **bot/worker**: Drop unaddressed group batches before invoking CC

### Miscellaneous

- **bot**: Clippy fixups for media-group changes

### Refactor

- **bot/filter**: RoutingDecision.address becomes Option<AddressKind>
- **bot/worker**: Extract debounce loop into collect_batch helper

### Testing

- **bot/worker**: Adaptive debounce window for media-group batches
- **bot/filter**: Regression for lost media-group siblings

## [0.2.4] - 2026-04-27


### Miscellaneous

- Update Cargo.lock dependencies

## [0.2.3] - 2026-04-24


### Bug Fixes

- **bot**: Render agent-error stderr as HTML <pre> in Telegram
- **bot**: Check filesystem policy drift before hot-reload apply
- **doctor**: Remove AGENTS.md existence check
- **clippy**: Duplicated_attributes and never_loop
- **clippy**: Clone_on_copy on SandboxMode/NetworkPolicy
- **clippy**: Derivable_impls on SttConfig and AuthMethod
- **clippy**: Collapsible_if across cron_spec, init, proxy, attachments, handler
- **clippy**: Assorted mechanical lints
- Address review-loop findings (2 iterations)
- **aggregator**: Disable rmcp 1.4+ DNS-rebinding Host check
- **policy**: Drop deprecated tls: terminate from generated policies
- **clippy**: More mechanical fixes across rightclaw-cli
- **clippy**: Site-level allows for judgment-call lints

### Features

- **bot**: Warn on filesystem policy drift at startup
- **codegen**: Scaffold contract module with CodegenKind types
- **codegen/contract**: Add write_regenerated helper
- **codegen/contract**: Add write_agent_owned helper
- **codegen/contract**: Add write_merged_rmw helper
- **codegen/contract**: Add write_and_apply_sandbox_policy
- **codegen/contract**: Add per-agent and cross-agent registries
- **codegen/contract**: Add write_regenerated_bytes for binary skill content

### Refactor

- **bot**: Route policy apply through write_and_apply_sandbox_policy
- **codegen/pipeline**: Route static-content writes through write_regenerated
- **codegen/pipeline**: Route settings.local.json through write_agent_owned
- **codegen/pipeline**: Route agent secret injection through write_merged_rmw
- **codegen/pipeline**: Route policy.yaml seed through write_regenerated
- **codegen/pipeline**: Route cross-agent writes through write_regenerated
- **codegen/claude_json**: Route .claude.json through write_merged_rmw
- **codegen/mcp_config**: Route mcp.json writes through contract helpers
- **codegen/skills**: Route skill writes through write_regenerated
- **codegen/skills**: Use write_agent_owned for installed.json
- **codegen/contract**: Extract ensure_parent_dir, wire write_and_apply_sandbox_policy

### Testing

- **codegen/contract**: Assert Regenerated outputs are idempotent
- **codegen/contract**: Assert AgentOwned files not overwritten
- **codegen/contract**: Assert MergedRMW preserves unknown fields
- **codegen/contract**: Assert registry covers all per-agent writes
- **policy**: Integration test for live-sandbox policy apply
