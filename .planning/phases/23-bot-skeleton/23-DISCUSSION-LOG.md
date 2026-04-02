# Phase 23: Bot Skeleton - Discussion Log

> **Audit trail only.** Do not use as input to planning, research, or execution agents.
> Decisions are captured in CONTEXT.md — this log preserves the alternatives considered.

**Date:** 2026-03-31
**Phase:** 23-bot-skeleton
**Areas discussed:** Crate placement, allowed_chat_ids semantics, SIGTERM shutdown scope, --agent resolution

---

## Crate Placement

| Option | Description | Selected |
|--------|-------------|----------|
| rightclaw-cli module | Add teloxide to CLI, create bot.rs. Simpler, consistent with memory_server.rs pattern. | |
| New rightclaw-bot crate | New workspace crate at crates/bot per single-responsibility principle. | ✓ |
| rightclaw core library | Add teloxide to lib crate — makes core heavy for all consumers. | |

**User's choice:** New `crates/bot` crate (package: `rightclaw-bot`)
**Notes:** "we need to start thinking about crate division in crates. bot must not live in main crate. we need a separate rightclaw-bot module. in crates/bot. we will also support more than telegram in future but let's keep everything in -bot for now"

---

## allowed_chat_ids Semantics

| Option | Description | Selected |
|--------|-------------|----------|
| Empty = allow all | Permissive default — no restriction unless explicitly set. | |
| Empty = block all | Secure default — must opt-in to each chat_id. | ✓ |

**User's choice:** Empty = block all
**Notes:** Secure default. Claude added: bot warns at startup when list is empty so operators notice the behaviour.

---

## SIGTERM Shutdown Scope

| Option | Description | Selected |
|--------|-------------|----------|
| Minimal — signal exits | Install SIGTERM handler, exit cleanly. Full machinery deferred to Phase 25. | |
| Full structure now | Wire Arc<Mutex<Vec<Child>>> + kill loop now; Phase 25 just adds to the list. | ✓ |

**User's choice:** Full structure now
**Notes:** Avoids touching shutdown logic twice across phases.

---

## --agent Resolution

| Option | Description | Selected |
|--------|-------------|----------|
| Resolves dir itself | Searches RIGHTCLAW_HOME/agents/<name>/. Works without env vars. RC_AGENT_DIR as override. | ✓ |
| RC_AGENT_DIR required | Env-var only. Simpler code, but requires manual export for local testing. | |

**User's choice:** Resolves dir itself
**Notes:** RC_AGENT_DIR still honoured as override — process-compose (Phase 26) will inject it.

---

## Claude's Discretion

- Exact module layout inside crates/bot/src/
- CancellationToken vs AtomicBool for shutdown signalling
- HashSet<i64> vs Vec<i64> for internal allowed_chat_ids storage

## Deferred Ideas

- Non-Telegram bot support — future phases in crates/bot
- Streaming responses / edit-in-place — v3.1
