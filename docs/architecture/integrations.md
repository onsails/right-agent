# External Integrations

> **Status:** descriptive doc. Re-read and update when modifying this
> subsystem (see `AGENTS.md` → "Architecture docs split"). Code is
> authoritative; this file may have drifted.

Load-bearing rules (OpenShell Integration Conventions, OpenShell Policy
Gotchas) stay in `ARCHITECTURE.md`. This file is the reference inventory.

| System | Protocol | Notes |
|--------|----------|-------|
| process-compose | REST API (TCP :18927) | Health, process start/stop/restart, logs, shutdown |
| Claude Code CLI | Subprocess (`claude -p` via SSH) | Runs inside sandbox, structured JSON output |
| Claude Code CLI | Env var (CLAUDE_CODE_OAUTH_TOKEN) | Auth token from setup-token, injected into claude -p |
| OpenShell | gRPC + mTLS (active gateway endpoint) | Sandbox create/poll/reuse, policy hot-reload, exec, file verification |
| OpenShell | CLI (`openshell sandbox upload/download`) | File transfer (no gRPC equivalent yet) |
| Telegram | frankenstein (Bot API client), webhook over Cloudflare Tunnel | RightBot wrapper (governor throttle + cached get_me), per-agent allowlist; `channel_post` is validated against the live allowlist and delivered through the bot-local UDS endpoint |
| Cloudflare Tunnel | CLI (`cloudflared`) | Named tunnel, DNS CNAME, credentials file |
| MCP Aggregator | HTTP (:8100/mcp) + Unix socket (internal API) | Aggregates built-in + external MCP backends, per-agent Bearer auth |
| ffmpeg | system | Decode voice/video_note to PCM for whisper-rs. Optional — bot runs without it; voice transcription disabled. doctor warns. |
| ironclaw_safety | crate | Memory-content sanitization (write) and untrusted-content wrapping (read). See `docs/architecture/memory.md`. |
