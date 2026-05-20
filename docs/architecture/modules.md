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

### right-openshell

- `openshell.rs` and `openshell_proto` - OpenShell gRPC mTLS client, generated proto types, sandbox lifecycle wrappers, SSH helpers, and policy helpers.
- `sandbox_exec.rs` - clonable gRPC sandbox execution handle.
- `test_cleanup.rs` and `test_support.rs` - live-sandbox test cleanup and `TestSandbox`.

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

### right-codegen

- `pipeline.rs` — per-agent and cross-agent codegen orchestration.
- `contract.rs` — sanctioned codegen writers and registries (see Upgrade & Migration Model).
- `agent_def.rs`, `settings.rs`, `claude_json.rs`, `mcp_config.rs`, `mcp_instructions.rs`, `policy.rs`, `process_compose.rs`, `cloudflared.rs`, `telegram.rs`, `plugin.rs`, `skills.rs` — generated artifacts and bundled skill/template installation.
- `templates/` and `skills/` — compiled codegen-owned prompt, process-compose, cloudflared, and skill assets.

### right-dashboard

- `auth.rs` — Telegram Mini App `initData` validation and allowlist authorization helpers.
- `api_types.rs` — dashboard API DTOs and error response bodies.
- `read_model.rs` — read-only SQLite projections for dashboard views.
- `assets.rs` — embedded static dashboard asset lookup and content types.
- `frontend/` — Vue/Vite source for the Mini App dashboard.
- `static/dashboard/` — checked-in built dashboard assets served by the bot.

### right-memory

- `hindsight.rs` — Hindsight Cloud API client and DTOs.
- `resilient.rs`, `circuit.rs`, `classify.rs`, `status.rs` — memory failure handling, circuit state, classification, and status reporting.
- `prefetch.rs` — recall prefetch cache.
- `retain_queue.rs` — SQLite-backed pending-retain queue using `right-db` migrations.
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
- `right_backend.rs` — built-in MCP tools (memory, cron, mcp_list, bootstrap).
- `internal_api.rs` — internal REST API on Unix socket.
- `memory_server.rs` — deprecated CLI-only MCP stdio server.

### right-bot

- `lib.rs` — entry: resolve agent dir, open `data.db`, sandbox lifecycle, start teloxide.
- `cc/` — generic Claude Code subprocess plumbing: invocation builder, prompt assembly, stream parser, structured-reply parser, outbound DTOs, and shared markdown helpers.
- `telegram/` — bot adaptor, dispatcher, handler, per-session worker, session table, chat-ID filter, OAuth callback server, Telegram markdown rendering/splitting, dashboard routes, and attachment delivery (with STT integration).
- `telegram/dashboard.rs` — Axum route mounting for `/dashboard/<agent>/`, dashboard API handlers, Telegram menu/button setup, and injected bot-owned auth/runtime state.
- `login.rs` — token-based Claude login flow (setup-token, env var injection).
- `sync.rs` — background `right-platform-store` sync to `/sandbox/.platform/`.
- `cron.rs`, `async_delivery.rs` — cron engine and async delivery loop (resumes main session so cron/background results land in agent context).
- `reflection.rs` — `reflect_on_failure` primitive (see Reflection Primitive).
- `stt/` — host-side voice/video_note transcription (ffmpeg + whisper-rs + Russian markers).
- `error.rs` — `BotError` types.
