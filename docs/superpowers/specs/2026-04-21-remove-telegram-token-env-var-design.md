# Remove RC_TELEGRAM_TOKEN env var — read token from agent.yaml only

**Date:** 2026-04-21
**Status:** Approved

## Problem

Telegram bot token is baked into `process-compose.yaml` as `RC_TELEGRAM_TOKEN` env var at `rightclaw up` time. When `config_watcher` detects an `agent.yaml` change and restarts the bot (exit code 2), process-compose re-runs the same command with the **same frozen environment**. The bot's `resolve_token()` checks the env var first, so the updated `agent.yaml` token is never reached.

Result: changing `telegram_token` in `agent.yaml` requires a full `rightclaw down && rightclaw up` cycle instead of the expected automatic restart.

## Root Cause

`RC_TELEGRAM_TOKEN` and `RC_TELEGRAM_TOKEN_FILE` env vars are a purely internal mechanism — process-compose template writes the token from `agent.yaml` into env, then `resolve_token()` reads it back. Nobody sets these externally. The entire env var round-trip is unnecessary because the bot already parses `agent.yaml` into `AgentConfig` at every startup.

## Solution

Remove the env var indirection entirely. Single source of truth: `agent.yaml → AgentConfig.telegram_token`.

### Changes

**`crates/bot/src/telegram/mod.rs`:**
- `resolve_token()`: remove env var priority chain (`RC_TELEGRAM_TOKEN`, `RC_TELEGRAM_TOKEN_FILE`). Read only from `config.telegram_token`.
- Remove `token_from_file_content()` helper and its tests.
- Update `resolve_token_returns_err_when_nothing_configured` test: remove env var guard.
- Keep `resolve_token_inline_field` test as-is.

**`templates/process-compose.yaml.j2`:**
- Remove the `{% if agent.token_inline %}` / `RC_TELEGRAM_TOKEN={{ agent.token_inline }}` / `{% endif %}` block.

**`crates/rightclaw/src/codegen/process_compose.rs`:**
- Remove `token_inline` field from `BotProcessAgent`.
- Remove `token_inline` mapping in `generate_process_compose()`.
- Agent skip logic (`telegram_token.is_none()` → skip) stays — it's the marker for "this is a telegram bot".

**`crates/rightclaw/src/codegen/process_compose_tests.rs`:**
- Remove or rewrite `inline_token_uses_rc_telegram_token` test — process-compose output no longer contains the token.

**`crates/bot/src/error.rs`:**
- Update `NoToken` error message: remove env var mention, keep only "configure telegram_token in agent.yaml".

### Not changed

- `AgentConfig.telegram_token` field — stays as-is.
- `generate_process_compose()` still skips agents without `telegram_token`.
- No new env vars introduced.

## Verification

- `cargo build --workspace`
- `cargo test --workspace`
- Manual: change `telegram_token` in agent.yaml → bot auto-restarts → uses new token without `rightclaw down/up`.
