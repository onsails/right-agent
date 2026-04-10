# RightClaw 🫱

Multi-agent runtime for Claude Code. Sandboxed. Subscription-compliant. Everything in chat.

## Prerequisites

> Full installation guide: [docs/INSTALL.md](docs/INSTALL.md)

**Required:**
- [Rust](https://rustup.rs/) toolchain
- [process-compose](https://github.com/F1bonacc1/process-compose) v1.100.0+
- [Claude Code CLI](https://docs.anthropic.com/en/docs/claude-code) (unsandboxed agents authenticate locally; sandboxed agents authenticate via Telegram login flow)
- Telegram bot token (via [@BotFather](https://t.me/BotFather))

**For sandboxed agents:**
- [NVIDIA OpenShell](https://github.com/NVIDIA/OpenShell) (only required when agents use `sandbox: mode: openshell`)

**Highly recommended:**
- [cloudflared](https://developers.cloudflare.com/cloudflare-one/connections/connect-networks/) (authenticated, with a named tunnel)

## Quick Start

```sh
cargo install --path crates/rightclaw-cli
rightclaw init --telegram-token <YOUR_BOT_TOKEN>
rightclaw up
```

This launches your first agent accessible via Telegram. The init wizard asks whether to run inside an OpenShell sandbox (default) or directly on the host.

Add more agents with `rightclaw agent init <name>` — each agent independently chooses its sandbox mode.

## What Is This?

RightClaw orchestrates multiple independent Claude Code sessions, each running inside its own [NVIDIA OpenShell](https://github.com/NVIDIA/OpenShell) sandbox. It calls `claude -p` directly — your existing Claude subscription works as-is, no token arbitrage, no API key sharing. Because it builds on Claude Code rather than replacing it, you get native features for free: memory, skills, MCP. On top of that, RightClaw adds its own persistent memory store (SQLite with FTS5/BM25 search), declarative cron scheduling, and agent personalities. OpenShell sandboxes are Docker containers — easy to back up, snapshot, and migrate. Everything is managed through Telegram, including Claude login and MCP OAuth authorization.

## Features

### Runtime

- **Multi-agent orchestration** — process-compose TUI to provision and monitor all agents from a single screen
- **Declarative cron engine** — YAML-defined scheduled tasks with run tracking and Telegram notifications
- **Restart policies** — `on_failure`, `always`, `never` with configurable backoff
- **Diagnostics** — `rightclaw doctor` validates dependencies, sandbox health, MCP status, and tunnel connectivity
- **Media attachments** — files flow both directions between Telegram and your agents

### Developer Experience

- **Claude skills ecosystem** — compatible with [skills.sh](https://skills.sh) skill format
- **MCP support** — automatic OAuth token refresh, add/remove servers via chat
- **Everything-in-chat** — Claude login, MCP OAuth, bot commands — no terminal needed after `rightclaw up`
- **Agent personalities** — onboarding flow where each agent discovers its own identity and tone
- **Persistent memory** — per-agent SQLite store with full-text search (FTS5/BM25)

### Security

NVIDIA OpenShell containers per agent, credential isolation, declarative network and filesystem policies, prompt injection detection. See [Security Model](docs/SECURITY.md) and [Policy Guide](docs/SECURITY.md#configuring-policies) for details.

### Compliance

- Calls `claude -p` directly — works with your existing Claude subscription
- No token arbitrage, no API key sharing, fully compliant with Anthropic's Terms of Service

## Roadmap

- [x] Multi-agent orchestration (process-compose)
- [x] Per-agent sandbox configuration (OpenShell or direct host access)
- [x] `rightclaw agent init` — add agents with independent sandbox modes
- [x] Telegram bot interface
- [x] Persistent memory (SQLite FTS5/BM25)
- [x] MCP support with OAuth token refresh
- [x] Claude login via chat
- [x] MCP OAuth via chat
- [x] Declarative cron engine
- [x] Agent personality / onboarding
- [x] Media attachments (both directions)
- [x] Restart policies with backoff
- [x] `rightclaw doctor` diagnostics
- [x] Claude skills ecosystem compatibility
- [ ] Agent backup & restore (`rightclaw backup` / `rightclaw restore`)
- [ ] Agent templates — pre-built configs with MCPs, skills, and identity presets
- [ ] Telegram group chats
- [ ] Telegram chat threads
- [ ] Budget control per-turn
- [ ] Agent-to-agent communication
- [ ] Binary distribution (homebrew, nix, releases)
- [ ] Google Chrome integration
- [ ] Karpathy's LLM Wiki integration
