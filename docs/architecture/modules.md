# Modules

> **Status:** descriptive doc. Re-read and update when modifying this
> subsystem (see `AGENTS.md` → "Architecture docs split"). Code is
> authoritative; this file may have drifted.

## Module Map

### right-platform-knobs

- `IDLE_THRESHOLD_SECS` / `IDLE_THRESHOLD_MIN` - UX-politeness gate for cron delivery and matching agent-facing prose.

### right-prompt-safety

- `sanitize_memory_content` - write-side Hindsight memory sanitization.
- `wrap_memory_for_prompt`, `memory_wrap_prefix`, `memory_wrap_suffix`, `escape_memory_close_delimiter` - read-side untrusted-content wrapping for memory prompt assembly.

### right-runtime-state

- `PC_PORT` and `MCP_HTTP_PORT` - process-compose and MCP HTTP default ports.
- `RuntimeState` / `AgentState` - persisted `<home>/run/state.json` schema.
- `read_state`, `write_state`, `generate_pc_api_token` - runtime-state IO and process-compose API token generation.

### right-config

- `src/lib.rs` - `GlobalConfig`, `TunnelConfig`, `AggregatorConfig`, `RIGHT_HOME` resolution, global config YAML IO, and agents/backups path helpers.

### right-ui

- `atoms.rs`, `line.rs`, `header.rs`, `splash.rs`, `writer.rs` - brand-conformant CLI atoms and blocks.
- `recap.rs` - command recap rendering.
- `prompts.rs` - interactive prompt helpers.
- `theme.rs` - terminal theme detection.

### right-process

- `lib.rs` - cancel-safe process-group child handling via `ProcessGroupChild`.

### right-hostpath

- `lib.rs` - host-side shell `PATH` integration for the `right` CLI: idempotent, atomic managed-block edits to the user's shell rc file(s) (zsh/bash/fish). Pure logic (home/shell/exe passed as params); used only by the `right` binary, not re-exported.

### right-sandbox

- `handle.rs` - the unified `SandboxHandle`: create/attach, readiness, exec, filesystem, secrets, health, lifecycle. The only crate depending on the microsandbox SDK.
- `agent.rs` - Right's create-time conventions (`agent_sandbox_spec`, guest image/user/home, restrictive-egress allow list).
- `egress.rs`, `secrets.rs`, `resources.rs`, `spec.rs` - the typed create-time spec: network stance, source-ref provider bindings + TLS bypass list, VM resources.
- `names.rs` - `sandbox_name`/`fit_sandbox_name`/`resolve_sandbox_name`, total mapping into the SDK's name space.
- `runtime.rs` - `ensure_runtime_installed` + `diagnose_host` (hypervisor preflight).

### right-platform-store

- `lib.rs` - content-addressed platform-managed sandbox file deployment to `/sandbox/.platform/`.

### right-agent-config

- `src/lib.rs` - shared agent configuration and discovery DTOs (`AgentConfig`, `AgentDef`, sandbox/memory/STT config types, `WhisperModel`).

### right-stt

- `src/lib.rs` - host-side whisper model cache paths, ffmpeg detection, model download, and cache warming.

### right-agent (core)

- `agent/` — agent discovery (presence detected by `agent.yaml`) and compatibility re-exports for agent config types from `right-agent-config`.
- `runtime/` — process-compose REST client and dependency checks. Runtime-state primitives live in `right-runtime-state`.
- Single-file modules: `doctor.rs`, `init.rs`, `rebootstrap.rs`, `cron_spec.rs`, `tunnel/`, `usage/`.

### right-db

- `Connection`, `Transaction`, `DbError` — async project-owned wrappers over the local Turso driver for per-agent SQLite-compatible `data.db`; filesystem opens use Turso multiprocess WAL so bot and MCP aggregator processes can share the same DB and may create Turso sidecar files such as `data.db-tshm`, while in-memory opens stay single-process. Legacy FTS5 cleanup uses bundled `rusqlite` only inside locked `migrate: true` schema bootstrap, not runtime opens.
- `multiprocess_io.rs` — Turso IO adapter that disables legacy DB/WAL file locks for filesystem-backed multiprocess-WAL opens while preserving shared WAL coordination.
- `migrations.rs` — ordered idempotent migration runner.
- `conversation.rs` — transcript archive and FTS search storage helpers.
- `test_support.rs` — migrated temp `data.db` fixtures for crate tests.

### right-codegen

- `pipeline.rs` — per-agent and cross-agent codegen orchestration.
- `contract.rs` — sanctioned codegen writers and registries (see Upgrade & Migration Model).
- `agent_def.rs`, `settings.rs`, `claude_json.rs`, `mcp_config.rs`, `mcp_instructions.rs`, `policy.rs`, `process_compose.rs`, `cloudflared.rs`, `telegram.rs`, `plugin.rs`, `skills.rs` — generated artifacts and bundled skill/template installation.
- `templates/` and `skills/` — compiled codegen-owned prompt, process-compose, cloudflared, and skill assets.

### right-dashboard

- `auth.rs` — Telegram Mini App `initData` validation and allowlist authorization helpers.
- `api_types.rs` — dashboard DTOs for bootstrap, overview, activity, knowledge, usage, identity, health, feature/capability flags, and error response bodies.
- `read_model.rs` — read-only `right-db` projection facade for activity overview/run detail and the public activity compatibility entry points.
- `read_model/activity.rs` — activity projections over async runs, usage rows, cron specs, run notifications, and bounded run logs.
- `read_model/dashboard_overview.rs` — top-level Mini App overview aggregation over active work, recent failures, today's usage, learning candidates, and injected runtime health summaries.
- `read_model/learning.rs` — learned-skill overview projections over `skill_learning_events`, `skill_lifecycle`, `curator_state`, usage, and trusted conversation data. It must not query removed Stage 2 tables.
- `read_model/usage.rs` — usage/cost projections over `usage_events`, including selectable Usage-tab ranges, selected-window totals, source splits, cron-job summaries, daily series, and model summaries. Usage read models accept a viewer timezone and bucket Usage-tab ranges by that local calendar before converting bounds back to UTC for storage filtering.
- `skill_inventory.rs` — bounded host-side skill inventory/detail helpers grouped as core, learned, and other.
- `identity_files.rs` — bounded host-side identity-file summary/detail helpers for `IDENTITY.md`, `SOUL.md`, and `USER.md`.
- `assets.rs` — embedded static dashboard asset lookup and content types.
- `frontend/` — Vue/Vite source for the Mini App dashboard.
- `frontend/src/components/charts/` — Vue/ECharts components for overview signal timeline, cost/learning river, usage spend chart, and learning flow.
- `static/dashboard/` — checked-in generated dashboard output embedded into the Rust binary; Vite hashed chunks live under `generated/assets/` and are stored in Git LFS.

### right-memory

- `hindsight.rs` — Hindsight Cloud API client and DTOs.
- `resilient.rs`, `circuit.rs`, `classify.rs`, `status.rs` — memory failure handling, circuit state, classification, and status reporting.
- `prefetch.rs` — recall prefetch cache.
- `retain_queue.rs` — `right-db` backed pending-retain queue in per-agent `data.db`.
- `error.rs` — semantic-memory error type and `right-db` boundary.

### right-mcp

- `credentials.rs` — MCP server registry, OAuth state persistence, auth tokens, URL helpers.
- `internal_client.rs` — bot-to-aggregator Unix-socket client.
- `oauth.rs`, `refresh.rs`, `reconnect.rs` — OAuth discovery, token refresh, and reconnect handling.
- `proxy.rs` — upstream MCP proxy backend and auth injection.
- `tool_error.rs` — MCP tool-error helpers.

### right (CLI)

- `main.rs` — CLI dispatcher.
- `aggregator.rs` — MCP Aggregator (Aggregator + ToolDispatcher + BackendRegistry).
- `right_backend.rs` — built-in MCP tools (memory, cron, conversation, channels, and provider capabilities).
- `internal_api.rs` — internal REST API on Unix socket.
- `memory_server.rs` — deprecated CLI-only MCP stdio server.

### right-bot

- `lib.rs` — entry: resolve agent dir, open `data.db`, sandbox lifecycle, start the Telegram bot (frankenstein webhook).
- `cc/` — generic Claude Code subprocess plumbing: invocation builder, prompt assembly, stream parser, structured-reply parser, outbound DTOs, and shared markdown helpers.
- `telegram/` — frankenstein `RightBot` client wrapper (`tg_bot.rs`), update router (`router.rs`), command parser (`command.rs`), `Message` field helpers (`msg_ext.rs`), handler, per-session worker, session table, chat-ID filter, OAuth callback server, Telegram markdown rendering/splitting, dashboard routes, and attachment delivery (with STT integration).
- `telegram/dashboard.rs` — Axum route mounting for `/dashboard/<agent>/`, dashboard API handlers, Telegram menu/button setup, and injected bot-owned auth/runtime state.
- `telegram/dashboard/health.rs` — explicit read-only `/health/*` probes for doctor output and bounded sandbox disk/memory/process snapshots.
- `telegram/dashboard/identity.rs` — sandbox-first bounded identity file reads with host-mirror fallback.
- `telegram/dashboard/skills.rs` — sandbox-first bounded skill inventory/detail reads with host-mirror fallback.
- `login.rs` — token-based Claude login flow (setup-token, env var injection).
- `sync.rs` — background `right-platform-store` sync to `/sandbox/.platform/`.
- `cron.rs`, `async_delivery.rs` — cron engine and async delivery loop (resumes main session so cron/background results land in agent context).
- `reflection.rs` — `reflect_on_failure` primitive (see Reflection Primitive).
- `stt/` — host-side voice/video_note transcription (ffmpeg + whisper-rs + Russian markers).
- `error.rs` — `BotError` types.
