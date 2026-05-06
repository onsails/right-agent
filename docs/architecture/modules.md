# Modules

> **Status:** descriptive doc. Re-read and update when modifying this
> subsystem (see `CLAUDE.md` → "Architecture docs split"). Code is
> authoritative; this file may have drifted.

## Module Map

### right-core (stable platform foundation)

- `config/` - `GlobalConfig` (tunnel) and `RIGHT_HOME` resolution.
- `ui/` - brand-conformant CLI atoms, blocks, recaps, prompts, and theme detection.
- `openshell.rs` and `openshell_proto` - OpenShell gRPC mTLS client, generated proto types, sandbox lifecycle wrappers, SSH helpers, and policy helpers.
- `platform_store.rs` - content-addressed platform store deployment to `/sandbox/.platform/`.
- `sandbox_exec.rs` - clonable gRPC sandbox execution handle.
- `stt.rs` - `WhisperModel`, whisper model cache paths, ffmpeg detection, and model download.
- `test_cleanup.rs` and `test_support.rs` - live-sandbox test cleanup and `TestSandbox`.
- Single-file modules: `error.rs`, `process_group.rs`, `time_constants.rs`.

### right-agent (core)

- `agent/` — agent discovery (presence detected by `agent.yaml`) and types (`AgentDef`, `AgentConfig`, `RestartPolicy`).
- `codegen/` — per-agent and cross-agent code generation: settings, `.claude.json`, `.mcp.json`, policy, process-compose, TOOLS.md, MCP instructions, bundled skills, cloudflared. The helper API in `codegen/contract.rs` is the only sanctioned writer (see Upgrade & Migration Model).
- `memory/` — Hindsight Cloud client (`hindsight.rs`), composite memory in file or Hindsight mode (`composite.rs`), schema migrations, prompt-injection guard. `store.rs` is legacy SQLite memory retained for migration compat.
- `runtime/` — `RuntimeState` JSON persistence, process-compose REST client, dependency checks.
- `mcp/` — OAuth credentials, internal UDS client (bot→aggregator), OAuth flow, proxy backend, token refresh scheduler.
- Single-file modules: `doctor.rs`, `init.rs`, `rebootstrap.rs`, `cron_spec.rs`, `tunnel/`, `usage/`.

### right (CLI)

- `main.rs` — CLI dispatcher.
- `aggregator.rs` — MCP Aggregator (Aggregator + ToolDispatcher + BackendRegistry).
- `right_backend.rs` — built-in MCP tools (memory, cron, mcp_list, bootstrap).
- `internal_api.rs` — internal REST API on Unix socket.
- `memory_server.rs` — deprecated CLI-only MCP stdio server.

### right-bot

- `lib.rs` — entry: resolve agent dir, open `data.db`, sandbox lifecycle, start teloxide.
- `telegram/` — bot adaptor, dispatcher, handler, per-session worker, session table, chat-ID filter, OAuth callback server, prompt assembly, attachments (with STT integration), `invocation.rs` (`ClaudeInvocation` builder — see Claude Invocation Contract).
- `login.rs` — token-based Claude login flow (setup-token, env var injection).
- `sync.rs` — background platform-store sync to `/sandbox/.platform/`.
- `cron.rs`, `cron_delivery.rs` — cron engine and delivery loop (resumes main session so cron results land in agent context).
- `reflection.rs` — `reflect_on_failure` primitive (see Reflection Primitive).
- `stt/` — host-side voice/video_note transcription (ffmpeg + whisper-rs + Russian markers).
- `error.rs` — `BotError` types.
