# Changelog
## [0.3.3] - 2026-06-08


### Bug Fixes

- **worker**: Let all-mode group batches reach cc
- **filter**: Keep all-mode bot loop guard through addressed fallback
- **mention**: Topic-root service message is not a reply-to-bot
- **bot**: Make response-mode callbacks scope-safe
- **bot**: Publish locked allowlist updates in order
- **bot**: Reuse prepared connection for reply gate
- **providers**: Repair legacy generic provider types
- **bot**: Report provider composition degradation
- **cli**: Require opened group for mode clear
- **providers**: Re-enable providers_v2_enabled at startup
- **dashboard**: Confirm built-in provider composition before yaml write
- **dashboard**: Confirm config-update provider composition
- **providers**: Require generic endpoint content when confirming composition
- **dashboard**: Report generic composition by endpoint content
- **dashboard**: Distinguish unknown provider composition state
- **providers**: Keep composition waits recoverable
- **providers**: Detect composition via effective policy (GetSandboxConfig)
- **providers**: Harden legacy generic repair

### Documentation

- **prompt**: Teach agent provider placeholder/binary model + 401 trigger
- Document text vs truncated_text and reply context tiers
- **mcp**: Advertise provider_capabilities in server instructions

### Features

- **right-db**: Session-context gate queries for reply rendering
- **filter**: Respond-to-all mode gating per scope
- **bot**: /mode and /mode_group inline-keyboard handlers
- **bot**: Reply-render decision logic and constants
- **bot**: Gate reply context before rendering
- **supervisor**: Confirm provider composition after reload (bring-up + hot-reconcile)
- **usage**: Select dashboard usage range server-side
- **allowlist**: Effective response-mode lookup and setters
- **mcp**: Add provider_capabilities built-in tool
- **allowlist**: Response-mode schema v2 (addressed/all) with backward-compatible parse
- **cli**: Right agent mode allowlist mirror
- **stt**: Add `right stt preload` and prefetch model in CI
- **dashboard**: Ensure and surface provider composition state

### Performance

- **filter**: Skip response-mode lookup for private chats

### Refactor

- **bot**: Group reply gate parameters

### Testing

- **usage**: Cover dashboard usage range query
- **cli**: Cover response mode validation

### Style

- **clippy**: Clear workspace clippy gate under clippy 1.96

## [0.3.2] - 2026-06-05

- Provider credentials are now reliably injected in long-running sandboxes. Previously, the `right-github` provider's composed OpenShell policy was never confirmed loaded in the running sandbox — credentials fell through to the raw tunnel rule and were sent as the literal placeholder string, causing 401 errors from GitHub. Generic providers were also folded directly into `policy.yaml` as a separate source of truth. All providers now use OpenShell profile-backed gateway composition, and `right up` confirms the composed policy is loaded in the sandbox after every provider attach or reconcile. Existing generic providers are migrated automatically on the next `right up`; no changes to `agent.yaml` are required.

- The sandbox supervisor now reads the real OpenShell sandbox phase before deciding to degrade, not just gateway reachability. A sandbox in `Phase: Error` is surfaced as unavailable to users in Telegram rather than silently failing. Starting sandboxes are classified separately from running ones so they are not prematurely treated as ready, and error state is preserved through recovery retries instead of being discarded.

## [0.3.1] - 2026-06-05


### Bug Fixes

- **openshell**: Read sandbox phase from SandboxStatus (0.0.56 relocation)

### Features

- **dashboard**: Compute usage windows in viewer timezone
- **dashboard**: Accept usage timezone query

## [0.3.0] - 2026-06-04

### Learning & Skills

- The skill-learning pipeline is now fully automatic: after each foreground turn a Haiku prefilter classifies whether the interaction contains reusable logic; if yes, a probe-writer forks to write or patch a `rightx-*` skill package; a periodic curator consolidates and archives the library. The old manual background review stage is replaced by this lightweight prefilter → probe-writer path.
- Recurring cron jobs now feed the same skill-learning pipeline — a successful recurring run captures a probe anchor and passes it through the same prefilter/probe-writer path as foreground turns.
- Skill learning is capped by a configurable daily USD budget with a circuit breaker that pauses learning on repeated probe failures and resets on a successful review. Budget exhaustion and circuit state are observable in the dashboard.
- Skill lifecycle — use counts, last-patched date, curator transitions — is now stored in the database and visible in the dashboard Knowledge tab. Operators can pin skills from the dashboard to prevent the curator from evicting them.
- Per-skill learning cost (create, patch, maintain, usage) is recorded in `data.db.skill_spend`; per-skill spend aggregates and budget-skip counts are exposed as dashboard read models.

### Dashboard

- The Telegram Mini App dashboard (opened with `/mcp`) now provides a full agent management UI with tabs for Overview, Activity, Knowledge, Usage, Identity, and Health — replacing most operational log-watching with observable dashboards.
- MCP server management moved from Telegram slash commands into the dashboard: add servers with URL-first detection and an explicit auth-mode choice (OAuth, headers, or URL as-is), view live connection status, and start OAuth flows — with completion status visible inline as you wait.
- Dashboard views are now chart-first: spend timelines, learning-flow funnels, and cost/activity correlations replace the previous table layout.
- Provider credential management is now in the dashboard (opened with `/providers`): attach typed gateway-backed credentials to sandboxed agents with no secrets stored on disk.
- Cron jobs can be deleted directly from the dashboard without using the CLI.

### Providers & Sandbox

- A new gateway-backed credential provider system lets operators attach typed credentials (GitHub, Anthropic, custom) to sandboxed agents. The `right-github` managed profile ships built-in, giving agents full GitHub access (read, push, PR management) with credentials injected at the gateway — never stored in the sandbox or visible to the agent.
- When the OpenShell gateway is unreachable at startup or becomes unavailable mid-session, the bot stays up, tells users in Telegram that the sandbox is temporarily unavailable, and auto-recovers when the gateway comes back without requiring a restart.
- `/sandbox/.local/bin` is now the standard user-local binary directory in all sandboxes. npm is configured with `NPM_CONFIG_PREFIX=/sandbox/.local` so CLIs installed by agents are immediately on `PATH` without manual configuration.

### MCP

- A background MCP health reconciler probes each registered backend on an adaptive debounced schedule, automatically detecting recovery from outages and keeping dashboard connection status accurate without requiring an agent restart.
- Operators can now register Tailscale, LAN, and loopback MCP servers via the dashboard; the detection gate resolves hostnames before classifying them, so private hostnames are not incorrectly offered OAuth discovery.
- MCP servers that require multiple HTTP headers for authentication (e.g. a bearer token plus a workspace ID) can now be registered with all headers stored securely.

### Cron & Background

- Each cron spec accepts a `model` parameter (`haiku`, `sonnet`, `opus`) set at create/update time, overriding the global model for that specific job — mechanical scheduled tasks no longer have to run on the default model.
- `cron_trigger` gains a `notify=true` flag that forces delivery to Telegram regardless of the cron's own silent decision or idle gate — useful for triggering an on-demand verification report without wiring up a separate watcher cron.
- Cron runs that return a terminal result event no longer hang waiting for the subprocess to exit; the stream breaks immediately on the terminal event and delivery proceeds even if the process does not exit cleanly.
- Background runs that fail (timeout, budget exceeded, error) now produce a user-facing diagnostic message instead of disappearing silently.

### Telegram & Agents

- Agents can create, rename, close, and reopen Telegram forum topics via four new MCP tools (`mcp__right__forum_topic_create`, `_edit`, `_close`, `_reopen`) — letting them organize group conversations programmatically.
- A "Working..." anchor now appears immediately after Claude starts processing (not after the first stream event), giving users the Stop and Background controls right away during high time-to-first-token turns.
- Error messages now include a "Details" button for viewing the raw error JSON on demand. Rate-limit errors (429/529) show a human-readable message and skip the reflection turn to avoid hammering the throttled endpoint.
- Recalled Hindsight memories are tagged with an observed date (e.g. `[observed 2026-05-27]`), signaling to agents that facts may be stale and should be re-verified before asserting them as current.
- Three new MCP tools enable agents to search past messages: `mcp__right__thread_search` and `mcp__right__chat_search` use local full-text indexes over archived Telegram messages, and `mcp__right__get_messages_by_id` fetches specific messages — all scoped server-side to the current chat context.

### Platform

- Idle `claude-opus-…-1m` sessions that exceed 40% context usage are automatically compacted after 2 hours of inactivity. The `/compact` call runs under the per-session lock, collapsing conversation history while preserving recent context.
- The per-session chat context block has moved from the system prompt to a per-turn volatile stdin prefix. The system prompt is now stable across turns for the same agent, enabling better prompt caching and reducing per-turn overhead.
- `right setup-path` idempotently adds the `right` binary directory to the current user's shell rc files (`.bashrc`, `.zshrc`, etc.), fixing post-install "command not found" errors in new shells.
- `right-composio` is now a built-in bundled skill, available in agent sandboxes without manual installation. It guides agents on when to use Composio's action workbench versus embedding large tool responses directly in context.

## [0.2.15] - 2026-05-18

- Agents can now save reusable skill packages from real work using `/right-learn-skill` — captured workflows and API discoveries are persisted as `rightx-*` packages available in future sessions.
- The hourly keepalive now detects when Right MCP connectivity is broken inside a Claude session and automatically repairs the auth cache, recovering agents that previously went silent without a manual restart.
- `right agent restore` validates Hindsight memory bindings before writing any files, preventing partial restores on validation failure, and preserves the original agent's memory bank bindings when restoring to a different environment.
- `right agent backup` preserves symlink targets in sandbox archives and accepts a new `--include-rebuildable` flag to include cache and dependency directories normally excluded from backups.
- Cloudflared now restarts automatically when the ingress configuration changes (e.g. after destroying an agent), so the active tunnel immediately reflects the new agent list without a manual restart.
- When Hindsight returns HTTP 402 (quota limit), the memory system stops enqueueing new memories and skips circuit breaker ticks — quota exhaustion no longer drives agents into open-circuit failure mode.
- The Claude auto-upgrade check no longer logs errors when `claude upgrade` reports the installed version is already current.

## [0.2.14] - 2026-05-14

- Agents can now send mid-turn progress messages to Telegram via the new `mcp__right__send_progress` tool — for example "fetching your data..." while the main response is still running. Rate-limited to one message per 30 seconds per foreground turn; cron, delivery, and reflection invocations do not have access to this tool.
- OAuth token refresh now survives transient network failures: the scheduler retries with exponential backoff instead of permanently stopping after the first blip. When a token expires mid-tool-call, the MCP server now correctly reports needs-auth with a `/mcp auth` hint rather than falsely showing connected status. Slow in-flight refresh operations no longer block `/mcp auth` commands from being processed.
- Agents sending a batch of attachments that Telegram rejects as a media group (for example, WebP images) now fall back to sending each file individually instead of failing.
- The `/right-memory` and `/right-reflect` built-in skills were deployed to the host but not uploaded to agent sandboxes, so agents could not invoke them. Fixed: the deployer now uses the canonical skill name list as its source of truth.
- Built-in Right skills are renamed from concatenated names (`rightcron`, `rightmcp`, `rightmemory`, `rightreflect`, `rightskills`) to hyphenated names (`right-cron`, `right-mcp`, `right-memory`, `right-reflect`, `right-skills`). Existing agent sandboxes are migrated automatically on bot startup.

## [0.2.13] - 2026-05-13

- Cron jobs can now use the Agent tool to spawn sub-agents for parallel research and multi-step orchestration. Budget caps via `max_budget_usd` continue to apply per invocation.
- Send `/debug on`, `/debug off`, or `/debug status` in any agent's Telegram thread to toggle Claude debug logging without restarting the bot. When on, debug logs appear inside the sandbox at `/sandbox/.claude/logs/` and the setting persists in `agent.yaml`.
- A Show Thinking button now appears in agent replies, letting you toggle Claude's reasoning trace on or off directly from the Telegram chat.
- Every agent now ships a `/rightreflect` skill, letting the agent read its own past conversation-history JSONL files inside the sandbox to answer "why did you..." questions and debug wrong decisions.
- Cron agents now receive an explicit delivery contract in their system prompt: their structured output is the Telegram delivery channel, there is no live user to clarify with, and they must not promise delivery faster than the idle window. The idle window was lowered from 3 minutes to 2 minutes.

## [0.2.12] - 2026-05-09

- When a Hindsight Cloud account runs out of credits, agents now enter quota-exhausted mode: memory retains pause without tripping the circuit breaker, and the agent tells you to top up at hindsight.vectorize.io in its next reply. Quota clears automatically when credits are restored.
- When the bot starts with a quota-exhausted Hindsight account, it logs a clear error pointing to the top-up URL instead of falling into an indefinite retry loop.
- Typing indicator failures in Telegram (for example, in forum supergroup topics) now surface as WARN log entries instead of being silently dropped.

## [0.2.11] - 2026-05-08

- Send `/model` in your agent's Telegram thread to switch between Default, Sonnet, Sonnet 1M, and Haiku without restarting the bot. The new model takes effect on the next message; scheduled cron jobs pick it up at run time.
- Memory content is now sanitized before being sent to Hindsight Cloud, and recalled memories are wrapped as untrusted external data in the system prompt — defending both Hindsight-mode and file-mode agents against prompt-injection via stored memories.
- When an MCP server returns an auth error with its own fix instructions (such as Composio's per-app OAuth flow), the agent now follows those instructions instead of overriding them with a generic `/mcp auth` suggestion.
- Markdown lists in agent replies no longer run the last bullet directly into the following paragraph text in Telegram.
- In Telegram groups, the `/model` inline keyboard now correctly blocks unauthorized users even when the callback arrives without an associated message.

## [0.2.10] - 2026-05-06


### Miscellaneous

- Update Cargo.toml dependencies

### Refactor

- Move skills/ and templates/ into right-agent crate

## [0.2.9] - 2026-05-05


### Bug Fixes

- **bot**: Fall back to API-key auth when MCP DCR fails
- **bot**: Block harness self-loop tools (ScheduleWakeup et al.)
- **cron**: Read delivery target from cron_runs, drop JOIN to cron_specs
- **openshell**: Rename test to reflect what it actually exercises
- **openshell**: Clarify tear_down_control_master logging
- **openshell**: Collapse nested if-let to satisfy clippy::collapsible_if
- **openshell**: Restore ssh_exec cancel-safety via RAII pid guard
- Address review-loop findings on background-continuation
- **cron**: Backfill cron_runs target from live specs in v18
- **cron**: Propagate cron_update target changes to undelivered runs
- **bot**: Opt out of ssh ControlMaster for long-lived claude -p

### Features

- **bot**: Clean stale ControlMaster socket at startup
- **bot**: Tear down ControlMaster on graceful shutdown
- **cron**: Fire ScheduleKind::Immediate jobs on next reconcile tick
- **invocation**: Add fork_session flag emitting --fork-session
- **worker**: Per-main-session mutex on --resume to close TOCTOU race
- **cron-delivery**: Acquire per-session mutex before --resume into main
- **bot**: Background button + handle_bg_callback dispatch
- **worker**: BgReason, Backgrounded outcome, enqueue helper, continuation prompt
- **cron**: Honour X-FORK-FROM header for background continuation jobs
- **worker**: Replace SafetyTimeout-reflection with Backgrounded path
- **bot**: Wire SessionLocks + BgRequests through dispatch and delivery
- **cron**: Snapshot target_chat_id/target_thread_id onto cron_runs
- **cron**: Add select_schema_and_fork helper for kind-aware invocation
- **cron**: Extend reconcile filters to fire BackgroundContinuation jobs
- **worker**: Instruct bg fork that silence is not a valid outcome
- **cron**: Startup migration for legacy @immediate+X-FORK-FROM rows
- **prompt**: Add bg_marker slot to deploy_composite_memory; stub builder
- **worker**: Build_bg_marker_for_chat surfaces in-flight bg runs to main session
- **openshell**: Add control_master_socket_path helper
- **openshell**: Append ControlMaster directives to generated SSH config
- **openshell**: Add check_control_master helper
- **openshell**: Add clean_stale_control_master and tear_down_control_master
- **cron**: Raise default budget to $5
- **cron**: Add ScheduleKind::Immediate variant with @immediate sentinel
- **cron**: Migrate cron_runs to carry target_chat_id/target_thread_id (v18)
- **cron**: Carry target_chat_id/target_thread_id on CronSpec
- **cron-spec**: Add ScheduleKind::BackgroundContinuation variant
- **codegen**: Add BG_CONTINUATION_SCHEMA_JSON for forked bg turns
- **worker**: Produce BackgroundContinuation rows; drop X-FORK-FROM prefix
- **migrate**: Tear down old ControlMaster during sandbox migration
- **cron**: Immediate kind in create_spec_v2 + insert_immediate_cron helper

### Miscellaneous

- **cron**: Remove dead X-FORK-FROM test mirror after kind-driven dispatch
- **up**: Log per-phase elapsed_ms before process-compose start

### Refactor

- **bot**: Address review feedback for MCP DCR fallback
- **cron**: Replace X-FORK-FROM prompt parsing with kind-driven dispatch
- **cron**: Extract reconcile predicate fns so regression tests bind to production
- **openshell**: Unify ssh -O control-op plumbing
- **cron**: Extract cron_spec tests to sibling file
- **cron-spec**: Extract ScheduleKind::from_db_row from inline match
- **bg-continuation**: Apply review-loop fixes

### Testing

- **openshell**: Verify ControlMaster engages multiplexing on first ssh call
- **cron**: Bump expected schema version to v18

## [0.2.8] - 2026-05-01


### Bug Fixes

- **bot**: Admit forwards through group routing filter
- **bot**: Extract attachments from reply_to_message
- **bot**: Preserve voice transcript from reply_to + add gate tests

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
