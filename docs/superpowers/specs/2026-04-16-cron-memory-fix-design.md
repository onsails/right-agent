# Cron Memory Fix

## Problem

Three bugs in cron/delivery memory handling:

1. **Cron auto-recall injects stale context into system prompt.** Cron prompts are static instructions, not user queries. Recall results are irrelevant and corrupt Hindsight's user representations. ARCHITECTURE.md already says "cron jobs skip memory" but the code still calls `recall_and_deploy_composite_memory()` in both `cron.rs` and `cron_delivery.rs`.

2. **Cron auto-retain pollutes memory bank.** Cron summaries retained with no `document_id` create orphan documents. Hermes-agent skips retain entirely for cron (`skip_memory=True`). Crons that need to remember something can call `memory_retain` explicitly.

3. **Worker recall truncation counts bytes, not characters.** `truncate_to_char_boundary(&input, 800)` limits to 800 bytes. For multibyte UTF-8 (Cyrillic, YAML with Unicode), 800 bytes can exceed 500 tokens — the hard server-side limit on Hindsight's recall endpoint (HTTP 400). Hermes-agent truncates to 800 characters, which stays within the 500-token limit.

## Context

Error from production logs:
```
WARN rightclaw_bot::telegram::prompt: recall failed: hindsight API error (HTTP 400):
  {"detail":"Query too long: 968 tokens exceeds maximum of 500. Please shorten your query."}
```

The Hindsight recall API rejects queries exceeding 500 tokens. This limit is server-side and not configurable.

Hermes-agent approach (`NousResearch/hermes-agent`):
- `skip_memory=True` for all cron invocations — disables both recall and retain
- `recall_max_input_chars = 800` — truncates by character count, not byte count
- Crons retain nothing automatically; explicit `memory_retain` tool calls still work

## Changes

### 1. Remove auto-recall from cron and delivery

**`crates/bot/src/cron.rs`**

Remove the `recall_and_deploy_composite_memory()` call before cron execution (~lines 308–312). Remove the prefetch recall spawn after cron completion (~lines 573–589). Set `memory_mode = None` instead of `Some(MemoryMode::Hindsight { ... })` so prompt assembly skips the composite-memory section.

**`crates/bot/src/cron_delivery.rs`**

Remove the `recall_and_deploy_composite_memory()` call before delivery (~lines 417–420). Set `memory_mode = None`.

Both changes: the `hindsight` parameter to `execute_job()` and `deliver_cron_result()` is no longer needed for recall. Check whether it can be removed entirely (see change 2).

### 2. Remove auto-retain from cron

**`crates/bot/src/cron.rs`**

Remove the auto-retain `tokio::spawn` block after cron completion (~lines 558–571). Remove the prefetch cache invalidation inside it — without auto-retain, no cache invalidation is needed.

After this change, the `hindsight` parameter to `execute_job()` is unused. Remove it from the function signature and all call sites. Same for `prefetch_cache`.

Crons retain MCP tool access — `memory_retain` and `memory_recall` remain available through the MCP aggregator for explicit use within cron prompts.

### 3. Fix worker recall truncation: bytes → characters

**`crates/bot/src/telegram/worker.rs`**

Rename `RECALL_MAX_INPUT_CHARS` to clarify it means characters (value stays 800 — matches hermes).

Replace `truncate_to_char_boundary()` implementation: truncate by character count using `.char_indices()`, not by byte length. Current implementation uses `.len()` (bytes); for ASCII this is identical, but for multibyte UTF-8 it under-counts characters and can produce queries exceeding 500 tokens.

Before:
```rust
fn truncate_to_char_boundary(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}
```

After:
```rust
fn truncate_to_chars(s: &str, max_chars: usize) -> &str {
    match s.char_indices().nth(max_chars) {
        Some((byte_idx, _)) => &s[..byte_idx],
        None => s,
    }
}
```

Update all call sites and tests.

## Files changed

| File | Change |
|------|--------|
| `crates/bot/src/cron.rs` | Remove auto-recall (pre-exec + prefetch), remove auto-retain, remove `hindsight`/`prefetch_cache` params from `execute_job()` |
| `crates/bot/src/cron_delivery.rs` | Remove auto-recall, set `memory_mode = None`, remove `hindsight` param from `deliver_cron_result()` |
| `crates/bot/src/telegram/worker.rs` | Replace `truncate_to_char_boundary` with `truncate_to_chars` (count characters not bytes), update tests |
| `crates/bot/src/telegram/prompt.rs` | No changes — `recall_and_deploy_composite_memory` stays for worker use |
| `ARCHITECTURE.md` | Remove "Auto-retain after cron completion is still active" — align doc with new behavior |

## Testing

### Unit tests — `truncate_to_chars`

- ASCII string shorter than limit → unchanged
- ASCII string longer than limit → truncated to exact char count
- Multibyte UTF-8 (Cyrillic) → truncated by char count, not byte count
- Mixed ASCII + multibyte → correct boundary
- Empty string → empty
- Limit = 0 → empty

### Existing tests

Cron tests that reference `hindsight`/`prefetch_cache` params must be updated to remove those arguments.

### Manual verification

After deploy, confirm:
- No `recall failed` warnings in cron logs
- No `cron auto-retain` or `cron prefetch recall` log lines
- Worker recall still works (check for `prefetch cache miss, blocking recall` on first message)
