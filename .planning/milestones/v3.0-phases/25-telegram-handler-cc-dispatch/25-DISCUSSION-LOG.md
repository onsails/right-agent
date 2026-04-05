# Phase 25: Discussion Log

**Session:** 2026-03-31
**Workflow:** discuss-phase

---

## Folded Todo
- **"Document CC gotcha — Telegram messages dropped while agent is streaming"** → folded in (score 0.6)

## Gray Areas Selected
User selected: Typing indicator, Session CRUD crate, Error reply format
User added: Debounce, Structured output protocol, Media handling

---

## Q&A Log

### Debounce window
**Q:** How long to wait for more messages before firing claude -p?
**A:** 500ms (recommended)

### Batched message format
**Q:** How to combine batched messages?
**A:** "We need to provide some structured format which claude understands (via some system prompt?)" →
Deferred to structured output discussion.

### Structured output protocol
**Q:** Input format + output schema for claude -p communication?
**A:** Tool-use schema (recommended) — AGENTS.md defines a `reply` tool, CC always calls it,
rightclaw parses `tool_use` block from `--output-format json`. Batched messages in XML wrapper.

### Reply tool schema
**Q:** What fields in the `reply` tool?
**A:** `content + reply_to_message_id` (recommended)
*User note:* "Also include message_id to reply to specific message. And media support needed."

### Media scope
**Q:** Media in reply tool — Phase 25 or stub for later?
**A:** Schema stub only — `media_paths: Vec<String> | null` defined, Phase 25 logs warning and skips.
*User note:* "Don't forget to implement in later phases."

### Typing indicator
**Q:** Send once or repeat every ~4s?
**A:** Repeat every 4s (recommended) — spawn task, re-send ChatAction::Typing every 4s, cancel on completion.

### Session CRUD crate
**Q:** telegram_sessions CRUD in bot crate or rightclaw crate?
**A:** bot crate (recommended) — `crates/bot/src/telegram/session.rs`

### Error reply format
**Q:** What does a claude -p failure look like in Telegram?
**A:** ⚠️ + code block (recommended) — `⚠️ Agent error (exit N):\n\`\`\`\nstderr (truncated 300 chars)\n\`\`\``

---

## Deferred Ideas
- Full media sending (sendPhoto/sendDocument)
- Receiving Telegram files/images
- Configurable debounce per agent.yaml
- Streaming responses / edit-in-place (v3.1)
- `/fast` bare mode (v3.1)
