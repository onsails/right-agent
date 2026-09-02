# Agent-authored messages carry structured Rich Content, not Markdown

Status: superseded by ADR 0005 for attachment captions and progress

Agent-authored Telegram messages were plain Markdown strings. Telegram's Rich
Markdown parser interpreted financial prose as formatting: paired `$` amounts
in a RiskOff channel post rendered as italic math, and underscores in wallet or
ticker names opened emphasis. Escaping is not a fix, because the agent decides
what is literal and Right cannot recover that intent from the text afterwards.

## Decision

Agent-authored standalone messages carry a Right-owned `RichContent` value
instead of a Markdown string. `RichContent` is exactly one of `{"text": "..."}`
(literal text) or `{"blocks": [...]}` (paragraph, heading, list, quote, code,
table). Inline content is a flat run list of `{text, marks?, link?}`. Rust
validates the value and maps it to Telegram `InputRichMessage.blocks`; a
Telegram rejection falls back to normalized plain text.

The contract covers final replies, bootstrap replies, cron and background
notify content, `mcp__right__send_message`, and `mcp__right__channel_post`.
Attachment captions, `send_progress`, and platform-authored notices keep the
existing regular-message paths.

## Considered options

- **Keep Markdown, escape harder.** Rejected: escaping cannot distinguish
  literal `$3,200` from intended math without the author's intent.
- **Keep Markdown, convert locally to Telegram blocks.** Rejected: Right would
  still reconstruct intent from ambiguous text, and the agent contract would
  keep a parser in the loop.
- **Expose Telegram's own recursive rich DTOs.** Rejected: it couples every
  agent prompt to Bot API churn and inflates the output schema.

## Consequences

- Formatting is declared, never inferred; `$`, `_`, and `*` stay literal.
- Structured-output and MCP schemas grow; the shallow block/run shape keeps
  that cost bounded and model validation tractable.
- Archives keep normalized visible text only, so transcript search is
  unchanged and no database migration is required.
- Existing queued `delivery_json` strings deserialize as literal plain text.
