# Modules

> **Status:** descriptive doc. Re-read and update when modifying this
> subsystem (see `CLAUDE.md` → "Architecture docs split"). Code is
> authoritative; this file may have drifted.

## Module Map

### right-core (stable platform foundation)

- `config/` - `GlobalConfig` (tunnel) and `RIGHT_HOME` resolution.
- `agent_types.rs` - shared agent configuration and discovery DTOs (`AgentConfig`, `AgentDef`, sandbox/memory/STT config types).
- `runtime_state.rs` - process-compose ports, runtime state JSON, and API-token generation.
- `ui/` - brand-conformant CLI atoms, blocks, recaps, prompts, and theme detection.
- `openshell.rs` and `openshell_proto` - OpenShell gRPC mTLS client, generated proto types, sandbox lifecycle wrappers, SSH helpers, and policy helpers.
- `platform_store.rs` - content-addressed platform store deployment to `/sandbox/.platform/`.
- `sandbox_exec.rs` - clonable gRPC sandbox execution handle.
- `stt.rs` - `WhisperModel`, whisper model cache paths, ffmpeg detection, and model download.
- `test_cleanup.rs` and `test_support.rs` - live-sandbox test cleanup and `TestSandbox`.
- Single-file modules: `error.rs`, `process_group.rs`, `time_constants.rs`.

### right-agent (core)

- `agent/` — agent discovery (presence detected by `agent.yaml`) and compatibility re-exports for shared agent types.
- `runtime/` — process-compose REST client, dependency checks, and compatibility re-exports for runtime state primitives.
- Single-file modules: `doctor.rs`, `init.rs`, `rebootstrap.rs`, `cron_spec.rs`, `tunnel/`, `usage/`.

### right-codegen

- `pipeline.rs` — per-agent and cross-agent codegen orchestration.
- `contract.rs` — sanctioned codegen writers and registries (see Upgrade & Migration Model).
- `agent_def.rs`, `settings.rs`, `claude_json.rs`, `mcp_config.rs`, `mcp_instructions.rs`, `policy.rs`, `process_compose.rs`, `cloudflared.rs`, `telegram.rs`, `plugin.rs`, `skills.rs` — generated artifacts and bundled skill/template installation.
- `templates/` and `skills/` — compiled codegen-owned prompt, process-compose, cloudflared, and skill assets.

### right-memory

- `hindsight.rs` — Hindsight Cloud API client and DTOs.
- `resilient.rs`, `circuit.rs`, `classify.rs`, `guard.rs`, `status.rs` — memory failure handling, policy labels, circuit state, classification, and status reporting.
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
- `telegram/` — bot adaptor, dispatcher, handler, per-session worker, session table, chat-ID filter, OAuth callback server, Telegram markdown rendering/splitting, and attachment delivery (with STT integration).
- `login.rs` — token-based Claude login flow (setup-token, env var injection).
- `sync.rs` — background platform-store sync to `/sandbox/.platform/`.
- `cron.rs`, `cron_delivery.rs` — cron engine and delivery loop (resumes main session so cron results land in agent context).
- `reflection.rs` — `reflect_on_failure` primitive (see Reflection Primitive).
- `stt/` — host-side voice/video_note transcription (ffmpeg + whisper-rs + Russian markers).
- `error.rs` — `BotError` types.
