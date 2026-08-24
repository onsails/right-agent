# External Integrations

> **Status:** descriptive doc. Re-read and update when modifying this
> subsystem (see `AGENTS.md` → "Architecture docs split"). Code is
> authoritative; this file may have drifted.

Load-bearing rules (Agent Sandbox Conventions, Sandbox Gotchas) stay in
`ARCHITECTURE.md`. This file is the reference inventory.

| System | Protocol | Notes |
|--------|----------|-------|
| process-compose | REST API (TCP :18927) | Health, process start/stop/restart, logs, shutdown |
| Claude Code CLI | Subprocess (`claude -p` via the sandbox SDK's exec stream) | Runs inside the microVM, structured JSON output |
| Claude Code CLI | Env var (`CLAUDE_CODE_OAUTH_TOKEN`) | Auth token from `setup-token`, injected into `claude -p`; token DB open/query failures abort command construction, while only an absent row runs without the env var |
| microsandbox | Rust SDK (`microsandbox` crate, pinned by `right_sandbox::PINNED_SDK_VERSION`) | microVM create/attach/phase, exec + streaming, guest filesystem, egress policy, provider secret bindings |
| Telegram | frankenstein (Bot API client), webhook over Cloudflare Tunnel | RightBot wrapper (governor throttle + cached get_me), per-agent allowlist; `channel_post` is validated against the live allowlist and delivered through the bot-local UDS endpoint |
| Cloudflare Tunnel | CLI (`cloudflared`) | Named tunnel, DNS CNAME, credentials file |
| MCP Aggregator | HTTP (:8100/mcp) + Unix socket (internal API) | Aggregates built-in + external MCP backends, per-agent Bearer auth |
| ffmpeg | system | Decode voice/video_note to PCM for whisper-rs. Optional — bot runs without it; voice transcription disabled. doctor warns. |
| ironclaw_safety | crate | Memory-content sanitization (write) and untrusted-content wrapping (read). See `docs/architecture/memory.md`. |
