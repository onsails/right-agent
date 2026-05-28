# Changelog
## [0.2.16] - 2026-05-28


### Bug Fixes

- **memory**: Initialize rustls provider in resilient tests
- **prompt**: Document sandbox user-local bins
- **prompt**: Make sandbox tool guidance a section
- **prompt**: Delegate remember routing to right-memory
- **prompt**: Tighten identity routing contract
- **prompt**: Document inbound reply metadata
- **right-composio**: Rewrite trigger so the skill actually loads
- **prompt**: Point MCP-error learning at TOOLS.md not CC memory
- **mcp**: Close loopback url validation gaps
- **mcp**: Cancel in-flight oauth refresh requests
- **oauth**: Treat non-metadata probes as misses
- **db**: Migrate legacy bg runs by schedule
- **db**: Preserve async run thread snapshots
- **db**: Migrate legacy immediate background specs
- **db**: Preserve legacy immediate fork sources
- **db**: Make libsql sync wrapper runtime-safe
- **db**: Shut down libsql runtime on open errors
- **db**: Preserve migration runner semantics
- **db**: Cover libsql migration rollback
- **db**: Enforce readonly turso wrapper guard
- **db**: Reject readonly pragma call setters
- **db**: Preserve legacy conversation upserts
- **db**: Scrub legacy fts5 before turso opens
- **db**: Harden turso review follow-ups
- **db**: Avoid turso sync bridge LocalSet panic
- **db**: Drop legacy learning tables
- **db**: Retry transient turso file locks
- **db**: Serialize schema bootstrap
- **db**: Allow multiprocess turso opens
- **bot**: Classify cron-backed backgrounds as async backgrounds
- **bot**: Report failed background async runs
- **bot**: Reject silent background continuation output
- **bot**: Hold session lock through background handoff
- **bot**: Harden interrupted handoff recovery
- **bot**: Strengthen learned-skill review prompt
- **bot**: Log learned-skill review outcomes
- **bot**: Deploy sandbox user-local env
- **bot**: Harden sandbox env sync
- **bot**: Make sandbox env bashrc repair idempotent
- **bot**: Normalize sandbox env bashrc blocks
- **bot**: Source sandbox user-local env for claude
- **bot**: Source sandbox env for review runs
- **bot**: Align review env path fallback
- **bot**: Reject non-file sandbox bashrc
- **bot**: Update learning review gate callsites
- **bot**: Broaden execution event redaction
- **bot**: Harden execution event persistence
- **bot**: Redact header-style execution secrets
- **bot**: Capture learning episodes before review gate
- **bot**: Harden learning episode drain
- **bot**: Harden selected episode review
- **bot**: Clear stale Telegram command scopes
- **bot**: Harden dashboard route auth
- **bot**: Satisfy dashboard route clippy
- **dashboard**: Harden telegram launch surface
- **dashboard**: Address review hardening
- **dashboard**: Address review issues (2 iterations)
- **bot**: Split doctor output for Telegram
- **dashboard**: Address review issues across simplify + 2 iterations
- **bot**: Use configured model for async delivery
- Address review-loop iter2 findings
- **bot**: Probe parser handles CC structured_output envelope and insert-order
- **bot**: Use probe_signal_source helper to silence dead-code warning
- **cron**: Abort owned jobs after shutdown timeout
- **bot**: Delay worker shutdown until handoff drain
- **bot**: Close shutdown handoff races
- **bot**: Address review issues (2 iterations)
- **bot**: Harden background learning cleanup
- **dashboard**: Harden skill lifecycle pin API
- **db**: Rollback dropped turso transactions
- **dashboard**: Stop advertising legacy evidence snippets
- **mcp**: Tighten oauth status redaction
- **mcp**: Report oauth completion to dashboard
- **mcp**: Preserve oauth terminal status
- **mcp**: Ignore replayed provider oauth errors
- **mcp**: Atomically fail pending oauth status
- **mcp**: Harden oauth status polling
- **mcp**: Purge terminal oauth status and match any error code
- **mcp**: Retain recent terminal oauth statuses
- **bot**: Clarify mini app auth failures
- **bot**: Bump learning prefilter max_turns to 5
- **agent**: Enforce async run helper invariants
- **bot**: Harden async delivery progress
- **bot**: Recover interrupted background handoffs
- **cron**: Preserve immediate lock defaults
- Address review issues (2 iterations)
- **agent**: Enforce learning episode transitions
- **agent**: Constrain learning episode terminal states
- **config**: Validate learning episode settings
- **bot**: Harden learning episode review
- **bot**: Address review-loop findings for learning episode selector
- **bot**: Pin opus 1m in model menu
- Address review-loop findings (simplify + 2 iterations)
- **learning**: Address review-loop findings (simplify + 2 iterations)
- **db**: Preserve fallible params and read-only destroy backup
- **db**: Address libsql review findings
- **config**: Ignore deprecated learning keys in reload diff
- **cron**: Catch peak :00/:30 minutes in step expressions
- **memory**: Address review issues across 2 iterations
- **cron**: Harden async run history queries
- **test**: Satisfy workspace clippy checks
- **mcp**: Improve Telegram HTTP warning UX
- **memory**: Route remember requests by persistence target
- **memory**: Keep retain as residual fallback
- **cron**: Deliver only explicit notify decisions
- Address review-loop iter1 findings
- **mcp**: Gate oauth success on reconnect
- **platform**: Harden mcp oauth and learning diagnostics
- **db**: Address cc review follow-ups
- **db**: Open backups with writable vacuum source
- Address review issues (2 iterations)
- **db**: Retry transient legacy scrubber locks
- Address review issues across MCP dashboard control surface
- **mcp**: Harden dashboard control surface
- **mcp**: Apply review-loop followups
- **backup**: Add database sidecar cleanup helper
- **backup**: Exclude database sidecars from tar
- **restore**: Remove database sidecars
- **restore**: Preserve canonical database snapshot
- **restore**: Reject tar-only database state
- **backup**: Exclude database sidecars from destroy backups
- **providers**: Simplify-pass: panics, prefix collision, YAML injection, ordering
- **providers**: Iter-1 review — credential redaction, rollback logs, FAIL FAST
- **providers**: Iter-2 review — strip collateral, argv credential, CRLF, partial failure
- **providers**: Address review-loop iter1+iter2 findings

### Documentation

- **prompt**: Clarify identity file ownership
- **prompt**: Teach agent to self-schedule retry crons
- **prompt**: Downgrade mechanical subagents to sonnet, vocalize dispatch
- **db**: Update worker db concurrency comment
- **memory**: Document scoped conversation search
- **mcp**: Document dashboard control surface

### Features

- **prompt**: Teach agent tool delegation
- **codegen**: Register right-composio built-in skill (stub)
- **codegen**: Right-composio playbook content
- **prompt**: Tighten subagent rule around intermediate-result relevance
- **prompt**: Advertise /right-composio under Core Skills
- **cron**: Require explicit delivery decision output
- **codegen**: Add FORK_PROBE_SCHEMA_JSON and FORK_PROBE_PROMPT
- **codegen**: Require used_skill_receipts in REPLY_SCHEMA, drop signal fields
- **codegen**: Probe-writer + curator constants; drop FORK_PROBE_*
- **codegen**: OPERATING_INSTRUCTIONS — MUST emit receipts, explicit-only /right-learn-skill
- **codegen**: /right-learn-skill — explicit-user-intent only, drop deferred-signal section
- **mcp**: Allow loopback mcp registration
- **mcp**: Add URL-first auth detection
- **mcp-client**: Provider_* methods on InternalClient
- **memory**: Add conversation transcript storage
- **db**: Add async runs table
- **db**: Add circuit-breaker fields to skill_nudge_state
- **db**: Add source column to skill_nudge_signals (v27 migration)
- **db**: V28 migration adds wall_elapsed_ms to usage_events
- **db**: V29 migration adds curator_state singleton table
- **lifecycle**: Add db-backed skill lifecycle crate
- **db**: Migrate legacy fts indexes to turso
- **mcp**: Persist multi-header credentials
- **memory**: Archive telegram transcript messages
- **memory**: Link transcript rows to agent turns
- **bot**: Gate background handoff
- **bot**: Start background continuations immediately
- **bot**: Persist typed execution events
- **bot**: Select learning episodes
- **bot**: Review selected learning episodes
- **dashboard**: Scaffold mini app crate
- **bot**: Expose dashboard routes
- **dashboard**: Add telegram launch surface
- **bot**: Expose learning dashboard routes
- **bot**: Parse claude result timing
- **bot**: Add thinking anchor helper
- **bot**: Show working anchor after invoke start
- **bot**: Log claude result timing
- **dashboard**: Add v2 api types
- **dashboard**: Add activity routes
- **dashboard**: Add usage api
- **dashboard**: Expose learning episodes
- **dashboard**: Add skills inventory
- **dashboard**: Add identity view api
- **dashboard**: Add health api
- **dashboard**: Wire v2 overview state
- **bot**: Move usage command to dashboard
- **bot**: Format telegram partial reply quotes
- **bot**: Capture telegram partial reply quotes
- **bot**: Wire learning-episode drain to daily-budget gate and usage events
- **bot**: Wire worker skill review to daily-budget gate and usage events
- **bot**: Extract alert dedup helpers and add learning circuit-open alert
- **bot**: Fire circuit-open alert on learning failure transitions
- **bot**: Tag reply-field-sourced nudge signals at worker ingestion site
- **bot**: Add learning_probe module with pure gate and parse logic
- **bot**: Spawn post-turn fork-probe and persist non-null signals
- **bot**: Gate DrainScheduler on background_review_enabled flag
- **bot**: Warn at startup when background learning is deprecated but legacy rows exist
- **dashboard**: Expose learning signals_by_source_24h read model
- **bot**: Lifecycle::usage module skeleton (UsageRecord, enums)
- **bot**: Lifecycle::usage atomic flock-protected R/W
- **bot**: Lifecycle::usage bump/mark/pin operations
- **bot**: Lifecycle::transitions apply_automatic_transitions
- **bot**: Lifecycle::snapshot tar+gz backup of skill packages
- **bot**: Receipt rendering with visual marker + bump_use hook
- **bot**: ProbeAnchor capture at end of foreground turn
- **bot**: Learning_prefilter module (Haiku classifier prompt + parser)
- **bot**: Learning_prefilter::run async Haiku call with usage tracking
- **bot**: Rename learning_probe → learning_probe_writer; replace contents; remove legacy fork-probe spawn
- **bot**: Wire post-turn prefilter → probe-writer pipeline in worker
- **bot**: Learning_curator module with should_run_now gate
- **bot**: Learning_curator::run_if_due orchestrates snapshot+transitions+LLM fork
- **bot**: Curator ticker spawned at agent startup (60s interval)
- **dashboard**: Drop signals_by_source, add skill_lifecycle_overview
- **bot**: ProbeAnchor carries turn stats + used skill receipts
- **bot**: Prefilter returns three-mode PrefilterDecision with schema validation
- **bot**: Prefilter prompt renders baselines + receipts + skill index summary
- **bot**: Prefilter::run computes baselines and renders skill summary
- **bot**: Probe-writer accepts directed PrefilterHint and branches prompt
- **curator**: CuratorState gains DB-backed load/save helpers
- **curator**: Multi-signal gate with cost spike + skill change + time fallback
- **cron**: Persist shutdown interruptions
- **bot**: Type foreground background requests
- **bot**: Flush ready deliveries on shutdown
- **bot**: Background foreground turns on shutdown
- **bot**: Show shutdown background banner
- **dashboard**: Add visual analytics
- **bot**: Register background learning invocations
- **dashboard**: Add skill lifecycle pin API
- **dashboard**: Add authenticated MCP routes
- **dashboard**: Start MCP OAuth from web UI
- **mcp**: Add oauth status store
- **mcp**: Expose dashboard oauth status
- **mcp**: Return dashboard oauth flow ids
- **agent**: Add async run helpers
- **db**: Add learning episode storage
- **agent**: Add LEARNING_SOURCES single source of truth
- **agent**: Record learning costs in usage_events
- **agent**: Daily-budget gate with circuit-open skip
- **agent**: Reset circuit state on review success
- **agent**: Record_review_failure with circuit breaker
- **usage**: Add learning_fork_probe source + insert helper
- **agent**: Add NudgeSignalSource enum and persist on record_nudge_signal
- **usage**: Replace learning_fork_probe with prefilter/probe_writer/curator sources
- **usage**: UsageBreakdown carries wall_elapsed_ms (foreground-only)
- **usage**: Turn_baseline module computes per-agent P50/P90/P99
- **curator**: Cost-spike + skill-change-count helpers for multi-signal gate
- **memory**: Bind mcp invocations to chat scope
- **memory**: Expose scoped conversation search tools
- **config**: Add learning episode settings
- **config**: Max_daily_budget_usd and circuit knobs in LearningConfig
- **config**: Add probe_model, fork_probe_enabled, background_review_enabled
- **cli**: Wizard prompts for probe_model, fork_probe_enabled, background_review_enabled
- **right**: Skill_learning_finish wires lifecycle::usage created/patch hooks
- **cli**: Right agent skill pin/unpin/list-pins operator commands
- **cli**: Wizard prompts for prefilter/probe_writer/curator settings
- **bot**: Worker measures wall_elapsed_ms + accumulates rightx receipts per turn
- **mcp**: Skill_learning_finish accepts hint_outcome; probe-writer instructs
- **wizard**: Prompts for curator trigger + prefilter baseline knobs
- **dashboard**: Project overview signals
- **learning**: Add background learning invocation kinds
- **db**: Make right database async
- **mcp**: Inject multiple HTTP auth headers
- **mcp**: Expose multi-header internal API
- **bot**: Open dashboard from mcp command
- **openshell**: Spawn_sandbox accepts --provider flags
- **internal-api**: ProviderApiError taxonomy + validators (name, env var, slug)
- **internal-api**: /provider-list + /provider-types routes with ProviderView/ProviderStatus DTOs
- **internal-api**: /provider-create routes (built-in + generic) with ordered rollback
- **internal-api**: /provider-rotate, /provider-config-update, /provider-remove + per-(agent,name) mutex
- **right-up**: Ensure_v2_enabled at startup with conditional fatal

### Miscellaneous

- **db**: Log transient lock diagnostics
- Clean up clippy issues introduced on this branch
- **agent**: Stub init.rs default YAML emitter for deprecated learning fields
- Drop NudgeSignalSource enum and source field from NudgeSignalRecord
- **model**: Point curated Opus choice at 4.8
- **bot**: Stub deprecated learning fields to unblock build through Task 18
- **right**: Remove unused lifecycle deps
- **lifecycle**: Remove usage json runtime references

### Refactor

- **prompt**: Drop four duplications already covered by base prompt
- **prompt**: Compress six sections and tighten grammar
- **db**: Add local libsql connection wrapper
- **db**: Run migrations through right-db
- **db**: Move conversation queries to libsql wrapper
- **db**: Migrate shared storage crates to right-db
- **db**: Port core wrappers to turso
- **db**: Remove libsql compatibility gate
- **cron**: Persist runs as async runs
- **bot**: Generalize async delivery
- **bot**: Consolidate sandbox user-local env contract
- **bot**: Inline one-line telegram quote helper
- **bot**: Parse cron delivery decisions
- **bot**: Record skill receipt usage in lifecycle db
- **bot**: Read curator lifecycle from db
- **bot**: Remove legacy stage two learning runtime
- **bot**: Remove legacy learning alert cleanup
- **dashboard**: Remove legacy learning history APIs
- **mcp**: Remove oauth telegram notification leftovers
- **bot**: Remove cron-backed background runtime
- Review addressing
- **async-runs**: Rename delivery result storage
- Simplify circuit-breaker review gate plumbing
- **agent**: Remove legacy learning review domain
- **learning**: Remove stale legacy references
- **cron**: Read run history from async runs
- **async-runs**: Update delivery decision consumers
- **right**: Write skill learning lifecycle to db
- **cli**: Remove skill pin commands
- **lifecycle**: Address review issues across 2 iterations
- **db**: Migrate callers to right-db connection
- **db**: Switch fresh schema to turso fts
- **internal_api_providers**: Pass gRPC client to provider fns

### Testing

- **memory**: Require identity ownership routing
- **codegen**: Cover right-composio in all_source_skill_files_are_installed
- **codegen**: Update Subagents-section guard needles to match rewrite
- **codegen**: Re-pin operating-instructions needles to compressed prompt
- **db**: Assert libsql connection path
- **db**: Prove local libsql sqlite features
- **db**: Assert transaction body runs before rollback
- **db**: Add turso fts compatibility gate
- **db**: Cover legacy routed upsert
- **db**: Capture bootstrap lock invariants
- **db**: Make concurrent bootstrap lock regression red
- **db**: Detect legacy scrubber overlap
- **bot**: Cover learned-skill review notice success
- **bot**: Define sandbox user-local env contract
- **bot**: Assert sandbox env precedes prompt assembly
- **cron**: Cover missing result stream parsing
- **dashboard**: Cover visual analytics responses
- **internal-api**: Sandbox_mode_none rejection for /provider-list

### Build

- **db**: Remove rusqlite dependencies

### Merge

- Background async runs

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
