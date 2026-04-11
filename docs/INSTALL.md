# Installation

## Prerequisites

### Rust Toolchain

```sh
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

RightClaw uses Rust edition 2024. Any recent stable toolchain will work.

### process-compose

Process orchestrator that powers multi-agent management. Version **1.100.0+** required.

```sh
# macOS
brew install f1bonacc1/tap/process-compose

# Linux — download from GitHub releases
# https://github.com/F1bonacc1/process-compose/releases
```

### NVIDIA OpenShell

Sandbox runtime for agent isolation. Currently in alpha.

```sh
# Follow install instructions at:
# https://github.com/NVIDIA/OpenShell
# https://docs.nvidia.com/openshell/latest/index.html
```

Requires Docker. OpenShell runs k3s containers inside Docker.

### Claude Code CLI

```sh
npm install -g @anthropic-ai/claude-code
claude  # follow prompts to authenticate
```

You must have an active Claude subscription. RightClaw calls `claude -p` directly — no API keys needed.

### Telegram Bot Token

1. Open [@BotFather](https://t.me/BotFather) in Telegram
2. Send `/newbot`, follow prompts
3. Save the bot token — you'll pass it to `rightclaw init`

### cloudflared (Highly Recommended)

Required for Telegram webhook tunneling. Without it, your bot needs a publicly reachable IP.

```sh
# macOS
brew install cloudflare/cloudflare/cloudflared

# Linux
# https://developers.cloudflare.com/cloudflare-one/connections/connect-networks/downloads/
```

After installing, authenticate and create a named tunnel:

```sh
cloudflared tunnel login
cloudflared tunnel create rightclaw
```

## Quick Install

Installs RightClaw, process-compose, and OpenShell:

```sh
curl -LsSf https://raw.githubusercontent.com/onsails/rightclaw/master/install.sh | sh
```

## Build from Source

```sh
git clone https://github.com/onsails/rightclaw.git
cd rightclaw
cargo install --path crates/rightclaw-cli
```

## Setup

```sh
# Initialize with your Telegram bot token
rightclaw init --telegram-token <YOUR_BOT_TOKEN>

# Verify everything is configured correctly
rightclaw doctor

# Launch your agents
rightclaw up
```

`rightclaw doctor` checks all dependencies, validates agent configuration, verifies sandbox connectivity, MCP status, and tunnel health. Run it whenever something seems off.
