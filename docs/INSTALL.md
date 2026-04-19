# Installation

## Prerequisites

Install these before running the installer:

### Claude Code CLI

```sh
npm install -g @anthropic-ai/claude-code
claude  # authenticate
```

Requires an active Claude subscription. RightClaw calls `claude -p` directly — no API key needed.

### Telegram bot token

1. Open [@BotFather](https://t.me/BotFather) in Telegram.
2. Send `/newbot`, follow the prompts.
3. Save the bot token. The installer's wizard will ask for it.

### cloudflared

A Cloudflare account plus the `cloudflared` CLI, authenticated. The installer creates the named tunnel for you.

```sh
# macOS
brew install cloudflare/cloudflare/cloudflared

# Linux
# https://developers.cloudflare.com/cloudflare-one/connections/connect-networks/downloads/

cloudflared tunnel login   # authenticate against your Cloudflare account
```

If you don't have a Cloudflare account yet, sign up at https://dash.cloudflare.com/sign-up (free tier is sufficient).

## Quick Install

One command installs `rightclaw`, `process-compose`, and NVIDIA OpenShell:

```sh
curl -LsSf https://raw.githubusercontent.com/onsails/rightclaw/master/install.sh | sh
```

The installer then runs the interactive `rightclaw init` wizard (asks for the Telegram bot token, sandbox mode, network policy) and `rightclaw doctor` to verify the setup.

Supported platforms: linux x86_64, linux aarch64, darwin aarch64 (Apple Silicon).

## Build from Source

For platforms without a prebuilt binary, or when you want to run from a checkout:

```sh
git clone https://github.com/onsails/rightclaw.git
cd rightclaw
cargo install --path crates/rightclaw-cli
rightclaw init
rightclaw doctor
```

This path requires a Rust toolchain (edition 2024).

## After install

```sh
rightclaw up
```

`rightclaw up` launches your agents via process-compose. Message your Telegram bot from your account — the agent replies.

Re-run `rightclaw doctor` whenever something seems off. It checks dependencies, agent configuration, sandbox connectivity, MCP status, and tunnel health.
