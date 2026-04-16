# Hindsight Retain Alignment — Design Spec

## Goal

Align RightClaw's Hindsight auto-retain/recall with Hindsight best practices and hermes-agent conventions. Fix cost inefficiency (unbounded document growth), add chat-scoped tagging, structured content format, and recall query truncation.

## Context

Current implementation creates a separate Hindsight document per conversation turn with no `document_id`, no tags, plain text format, and no recall query length limit. This wastes Hindsight retain tokens (each turn = new LLM extraction with no dedup) and provides no chat-level scoping for multi-chat agents.

Hermes-agent sends the entire accumulated session on every retain (O(n²) cost). We improve on this by using Hindsight's `document_id` + `update_mode: "append"` with delta retain — only new content triggers LLM extraction (O(n) cost).

## Changes

### 1. `hindsight.rs` — Add `document_id`, `update_mode`, `tags` to retain; add `tags`/`tags_match` to recall

**RetainItem** gains three optional fields:

```rust
struct RetainItem {
    content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    context: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    document_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    update_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tags: Option<Vec<String>>,
}
```

**`retain()` signature** changes:

```rust
pub async fn retain(
    &self,
    content: &str,
    context: Option<&str>,
    document_id: Option<&str>,
    update_mode: Option<&str>,
    tags: Option<&[String]>,
) -> Result<RetainResponse, MemoryError>
```

**QueryRequest** (used by recall) gains optional tags:

```rust
struct QueryRequest {
    query: String,
    budget: String,
    max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    tags: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tags_match: Option<String>,
}
```

**`recall()` signature** changes:

```rust
pub async fn recall(
    &self,
    query: &str,
    tags: Option<&[String]>,
    tags_match: Option<&str>,
) -> Result<Vec<RecallResult>, MemoryError>
```

All new parameters are `Option` — existing callers (MCP tools in aggregator) pass `None` to preserve current behavior.

### 2. `worker.rs` — Auto-retain with document_id, tags, structured format, context label

**Auto-retain** (fire-and-forget `tokio::spawn` after each turn):

- `document_id` = `root_session_id` from `get_active_session()` (the CC session UUID). This is the same UUID used for `--resume`, so all turns in a session share one Hindsight document.
- `update_mode` = `"append"` — Hindsight appends new content to existing document. Delta retain skips unchanged chunks, so only the new turn triggers LLM extraction.
- `tags` = `vec![format!("chat:{chat_id}")]`
- `context` = `"conversation between RightClaw Agent and the User"`
- Content format — JSON array matching hermes convention:

```json
[
  {"role": "user", "content": "...", "timestamp": "2026-04-16T12:00:00Z"},
  {"role": "assistant", "content": "...", "timestamp": "2026-04-16T12:00:05Z"}
]
```

The `session_uuid` for `document_id` comes from the session lookup that already happens at line ~694 (`get_active_session` or `create_session`). The value must be captured and passed to the retain block.

**Auto-recall** (blocking recall before `claude -p` and prefetch after each turn):

- `tags` = `vec![format!("chat:{chat_id}")]`
- `tags_match` = `"any"` — returns per-chat memories + untagged global memories (explicit `memory_retain` tool calls have no tags, so they're visible to all chats)
- Query truncation: truncate `input` to 800 chars before recall/prefetch, with char boundary handling (`floor_char_boundary` or equivalent)

### 3. `aggregator.rs` — Update MCP tool callers to pass None for new params

The `HindsightBackend::tools_call` in aggregator calls `retain()` and `recall()` directly. These calls need `None` for the new optional parameters — MCP tool calls from agents don't pass `document_id`/`update_mode`/`tags` (agents use simple content + optional context).

### 4. `ARCHITECTURE.md` — Update memory section

Add documentation about:
- `document_id` = session UUID, `update_mode: "append"` for delta retain
- Tags: `["chat:<chat_id>"]` for chat scoping
- Recall tags with `tags_match: "any"`
- Content format: JSON array with role/content/timestamp
- Query truncation: 800 chars

## Files to modify

| File | Changes |
|------|---------|
| `crates/rightclaw/src/memory/hindsight.rs` | Add `document_id`, `update_mode`, `tags` to `RetainItem`; add `tags`/`tags_match` to `QueryRequest`; update `retain()` and `recall()` signatures; update tests |
| `crates/bot/src/telegram/worker.rs` | Structured JSON content, `document_id` from session, tags, context label, recall query truncation |
| `crates/rightclaw-cli/src/aggregator.rs` | Pass `None` for new params in `HindsightBackend::tools_call` |
| `ARCHITECTURE.md` | Document new retain/recall behavior |

## Key decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| `document_id` source | CC session UUID (`root_session_id`) | Same UUID used for `--resume`, natural session boundary |
| `update_mode` | `"append"` | Delta retain = O(n) cost vs hermes O(n²). Hindsight skips unchanged chunks |
| Content format | JSON array (hermes style) | Structured roles + timestamps enable entity attribution and temporal queries |
| Tags | `["chat:<chat_id>"]` | Scopes memories per Telegram chat; `tags_match: "any"` includes global untagged memories |
| Context label | `"conversation between RightClaw Agent and the User"` | Matches hermes convention, descriptive for Hindsight extraction |
| Recall truncation | 800 chars | Matches hermes `recall_max_input_chars`, prevents exceeding Hindsight query limits |
| `retain_every_n_turns` | Every turn (1), not configurable | With append + delta retain, per-turn cost is low |
| `async: true` | Keep | Matches hermes, fire-and-forget pattern |
