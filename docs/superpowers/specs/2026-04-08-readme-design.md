# README Design Spec

## Goal

Write a concise, open-source-ready README for RightClaw. Target audience: OpenClaw/ZeroClaw users and Claude Code power users who already understand the ecosystem.

## Approach

"Show, don't tell" — lead with quick start, then explain, then features.

## Deliverables

Three files:
1. `README.md` — concise overview (~120-150 lines)
2. `docs/INSTALL.md` — detailed installation guide
3. `docs/SECURITY.md` — security model deep-dive

## Structure

### README.md

#### 1. Header
- `# RightClaw 🫱`
- Tagline: "Multi-agent runtime for Claude Code. Sandboxed. Subscription-compliant. Everything in chat."

#### 2. Prerequisites
Compact list — names + versions only, no install instructions. Link to `docs/INSTALL.md` for details.

**Required:**
- Rust toolchain
- process-compose v1.100.0+
- NVIDIA OpenShell
- Claude Code CLI (authenticated)
- Telegram bot token (via @BotFather)

**Highly recommended:**
- cloudflared (authenticated, with a named tunnel)

#### 3. Quick Start
```
cargo install --path crates/rightclaw-cli
rightclaw init
rightclaw up
```
One-liner: "This launches your first agent inside an NVIDIA OpenShell sandbox, accessible via Telegram."

#### 4. What is this?
Single paragraph covering:
- Orchestrates multiple independent Claude Code sessions
- Each in its own OpenShell sandbox (Docker containers — easy backup, portability)
- Calls `claude -p` directly — existing Claude subscription works, no token arbitrage, no API key sharing
- Leverages Claude Code native features: memory, skills, MCP
- Also has own persistent memory store (SQLite FTS5) for long-term recall
- Agents have personalities, scheduled tasks, MCP with auto token refresh
- Everything managed through Telegram — including Claude login and MCP OAuth authorization

#### 5. Features
Two categories (security extracted to docs/SECURITY.md), short bullets only:

**Runtime**
- Multi-agent orchestration via process-compose — single TUI to monitor all agents
- Declarative cron engine with run tracking and Telegram notifications
- Restart policies (on_failure, always, never) with backoff
- `rightclaw doctor` — validates deps, sandbox health, MCP status, tunnel connectivity
- Media attachments both directions (Telegram ↔ agent)

**Developer Experience**
- Claude skills ecosystem compatibility (skills.sh)
- MCP support with automatic OAuth token refresh
- Everything-in-chat: Claude login, MCP OAuth, bot commands — no terminal needed after `rightclaw up`
- Agent personality system — onboarding where agent discovers its own identity and tone (BOOTSTRAP.md)
- Persistent memory with FTS5/BM25 search

**Security** — one-liner summary + link:
- NVIDIA OpenShell containers, credential isolation, declarative policies. See [Security Model](docs/SECURITY.md).

**Compliance** — inline, 2 bullets:
- Calls `claude -p` directly — works with your existing Claude subscription
- No token arbitrage, no API key sharing, fully compliant with Anthropic's ToS

#### 6. Roadmap

### docs/INSTALL.md

Detailed installation guide:
1. Platform-specific prerequisites
   - Rust toolchain (edition 2024) + cargo
   - process-compose v1.100.0+ (link to releases)
   - NVIDIA OpenShell (link to GitHub + install docs)
   - Claude Code CLI — install + authenticate
   - Telegram bot token via @BotFather
   - cloudflared — install, authenticate, create named tunnel
2. Build from source: `cargo install --path crates/rightclaw-cli`
3. Initialize: `rightclaw init --telegram-token <TOKEN>`
4. Verify: `rightclaw doctor`
5. Launch: `rightclaw up`
6. Note: friendlier distribution (homebrew, nix, binary releases) coming soon

### docs/SECURITY.md

Security model deep-dive:
- **Sandbox architecture** — OpenShell containers per agent, what's isolated (filesystem, network, credentials)
- **Credential isolation** — host credentials never enter sandbox, each agent authenticates independently via OAuth
- **Network policy** — HTTPS proxy, TLS MITM for L7 inspection, wildcard domain allowlists
- **Declarative policies** — per-agent policy.yaml (filesystem + network rules), policy hot-reload
- **Prompt injection guard** — OWASP-derived pattern matching before memory store insert
- **Access control** — Chat ID allowlist per agent, empty = block all (secure default), protected MCP servers
- **Compliance** — calls `claude -p` directly, no token arbitrage, Anthropic ToS compliant

---

Back in README.md:

#### 7. Roadmap
Checkbox format. Two groups:

**Done:**
- [x] Multi-agent orchestration (process-compose)
- [x] NVIDIA OpenShell sandbox per agent
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

**Planned:**
- [ ] Telegram group chats
- [ ] Telegram chat threads
- [ ] Agent-to-agent communication
- [ ] Binary distribution (homebrew, nix, releases)
- [ ] Google Chrome integration
- [ ] Karpathy's LLM Wiki integration

## Constraints
- No license section (handled separately)
- README.md: ~120-150 lines. docs/INSTALL.md and docs/SECURITY.md: as long as needed but concise
- No explanation of what Claude Code or agents are — audience knows
- Tone: technical, matter-of-fact, not combative toward OpenClaw but clearly positioned as the secure alternative
