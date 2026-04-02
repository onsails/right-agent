---
id: SEED-012
status: dormant
planted: 2026-04-01
planted_during: v3.0 / phase 28.2
trigger_when: any milestone that adds multi-agent features, rightclaw up flow improvements, or agent validation hardening
scope: small
---

# SEED-012: Validate that Telegram bot tokens are unique across all agents

## Why This Matters

Two agents sharing the same Telegram bot token silently compete for the same update stream via
long-polling (or webhook). Telegram delivers each update to only one `getUpdates` caller — the
other agent drops it silently. The failure mode is non-obvious: messages disappear, one agent
responds to messages intended for the other, or both agents race and produce duplicate replies.
This is a misconfiguration that `rightclaw up` should catch before launching anything.

## When to Surface

**Trigger:** any milestone touching agent validation, `rightclaw up` startup flow, or multi-agent UX.

This seed should be presented during `/gsd:new-milestone` when the milestone scope matches any of:
- Agent configuration validation / startup checks
- Multi-agent launch improvements
- Doctor / preflight check expansion
- Token / credential management features

## Scope Estimate

**Small** — A few hours. `rightclaw up` already collects all `AgentDef`s before launch. Resolving
tokens (`resolve_telegram_token`) is already implemented. The check is: collect all resolved tokens,
detect duplicates, emit a clear `miette` error with both agent names and the token prefix, and abort.

## Breadcrumbs

- `crates/rightclaw/src/codegen/telegram.rs` — `resolve_telegram_token()` — resolves token from
  inline value or token file; run this for all agents and compare
- `crates/rightclaw/src/agent/types.rs:68-75` — `AgentConfig.telegram_token` /
  `telegram_token_file` fields
- `crates/rightclaw-cli/src/main.rs` — `rightclaw up` entry point — the validation should run here
  before any agent process is spawned
- `crates/rightclaw/src/doctor.rs` — existing doctor checks (bubblewrap, socat) as structural
  reference for adding a new preflight check

## Notes

- The token uniqueness check should run even when some agents have no Telegram config — skip
  `None` tokens, only compare agents that have one set.
- Error output should show the token prefix (first 8 chars) only — never the full secret in logs.
- Planted while investigating the CC + native bot racing on updates (commit `96d0cde`), which
  surfaced exactly this class of issue.
