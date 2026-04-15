# Cron Delivery Verbatim Relay

**Date:** 2026-04-15
**Status:** Approved

## Problem

Cron jobs produce a `notify.content` field with the final user-facing message (formatted text, links, descriptions). The delivery system pipes this as YAML into a Haiku CC session that resumes the main agent session. Without explicit instructions, Haiku summarizes the content instead of relaying it verbatim. Users receive "Report delivered! 11 events found" instead of the actual events with links.

## Why delivery goes through CC

The delivery CC session resumes the main user session (`--resume`). This ensures cron results land in the agent's conversation context, so the user can discuss results naturally ("which exhibition was at Musée Rodin?") without the agent having to re-fetch anything.

## Design

### Approach A: Instruction prefix in stdin

Add a plain-text instruction block before the YAML in the stdin payload. The system prompt remains unchanged (preserves prompt cache on resume).

Format sent to CC stdin:

```
You are delivering a cron job result to the user.
The `content` field below is the FINAL user-facing message — send it VERBATIM in your response.
Do NOT summarize, rephrase, or omit any part of the content.
You MAY prepend a short contextual intro (1 sentence max) if recent conversation was on a different topic, so the message feels natural.
Ignore the attachments field — attachments are sent separately.

Here is the YAML report of the cron job:

cron_result:
  job: "city-scout"
  ...
```

### Why not approach B (delivery-specific system prompt)

Delivery resumes the main session. Changing the system prompt would break prompt cache (5-min TTL). The cost of a cache miss on every delivery outweighs the reliability gain of system-prompt-level instructions.

If approach A proves unreliable (Haiku ignores stdin instructions), migrate to B: add a `delivery_mode: bool` parameter to `build_prompt_assembly_script()` that appends a delivery instruction block to the system prompt.

### Attachments

Attachments from cron are always sent to Telegram, unconditionally. Haiku does not decide whether to include them. The instruction tells Haiku to ignore the attachments field since they are handled separately by the delivery code.

## Changes

1. **`crates/bot/src/cron_delivery.rs`** — `format_cron_yaml()`: prepend instruction text before YAML. Add code comment about fallback to approach B.
2. **`ARCHITECTURE.md`** — Document why delivery goes through CC (session context), in the cron delivery section.
