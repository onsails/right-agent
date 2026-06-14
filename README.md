<p align="center">
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-Apache--2.0-E8632A.svg" alt="license"></a>
  <a href="https://github.com/onsails/right-agent/actions"><img src="https://github.com/onsails/right-agent/actions/workflows/build.yml/badge.svg" alt="build"></a>
  <a href="https://t.me/rightagent"><img src="https://img.shields.io/badge/Telegram-chat-E8632A?logo=telegram" alt="telegram"></a>
</p>

<p align="center">
  <img src="assets/lockup-horizontal.svg" height="36" alt="right agent">
</p>

right agent is an ai agent you run by messaging it. you give it real credentials
without handing them to the model: every agent runs in its own sandbox, every
credential lives outside it. the secret bytes never enter the box, so a
compromised agent can misuse a tool while it runs, but it cannot read the
credential or reach the open internet with it. for anyone tired of "grant all
permissions and hope," that is the change.

<p align="center">
  <img src="images/screenshot.png" alt="right agent in Telegram" width="720">
</p>

> today every agent runs on Claude Code (`claude -p`), so you need a Claude
> subscription. multi-provider support is in the works.

## quick start

```sh
curl -LsSf https://raw.githubusercontent.com/onsails/right-agent/master/install.sh | sh
```

open a new shell so `right` is on your `PATH`, then:

```sh
right up
```

message your bot on Telegram. the first chat walks you through login. from there
you manage everything from Telegram: `/mcp` and `/providers` open the dashboard.

## docs

full story, install guide, security model, concepts, and commands:
**https://onsails.github.io/right-agent/**

- [Install](https://onsails.github.io/right-agent/docs/install/)
- [Concepts](https://onsails.github.io/right-agent/docs/concepts/)
- [Security model](https://onsails.github.io/right-agent/docs/security/)
- [Telegram commands](https://onsails.github.io/right-agent/docs/commands/)

Contributor docs stay in the repo: [ARCHITECTURE.md](ARCHITECTURE.md),
[PROMPT_SYSTEM.md](PROMPT_SYSTEM.md).

## credits

built on [Claude Code](https://docs.anthropic.com/en/docs/claude-code),
[NVIDIA OpenShell](https://github.com/NVIDIA/OpenShell), and
[process-compose](https://github.com/F1bonacc1/process-compose). licensed under
Apache-2.0.
