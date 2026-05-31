# Hindsight memory staleness — design

**Date:** 2026-05-31
**Status:** Approved design, pre-plan

## Problem

Agent `agent-b` was asked for its Notion agenda and replied that Notion had been
"unauthorized since 27 мая." This was false: `agent-b` produced a working,
live-linked agenda on 28 May — a day *after* the date it cited. The agent
recalled a stale fact from a real 27 May incident (Composio gateway flapping +
a re-auth share mismatch) and recited it as current truth without
re-verifying.

Root cause in the memory subsystem: **recall throws away every date.**
`HindsightProvider`'s `join_recall_texts` (`crates/right-memory/src/hindsight.rs:99`)
renders each recalled memory as a bare `- {text}` bullet. The agent cannot tell
a fact observed today from one observed last week — they look identically
current.

## What we verified (load-bearing facts)

A live recall against the `right` bank (2026-05-31) plus the v0.7.1 Hindsight
OpenAPI spec established the real surface — correcting several earlier wrong
assumptions:

1. **Recall already returns the dates we need.** Each result carries 13 fields;
   `RecallResult` (`hindsight.rs:18`) parses only 3 (`text`, `score`, `type`).
   The dropped fields include:
   - `mentioned_at` — when the fact was retained (**present on every result**).
   - `occurred_start` / `occurred_end` — event time (often present).
   - `id`, `document_id`, `tags`, `context`, `entities`, `metadata`, `chunk_id`,
     `source_fact_ids`.
2. **`score` does not exist** in the real response. It is a dead `Option` field
   (always `None`); a mock test fixture wrongly includes it.
3. **No in-content text marker is viable.** Default Hindsight extraction rewrites
   content into clean fact sentences; an embedded `[mem:...]` marker does not
   survive. (A `verbatim` bank-level mode exists but is undocumented.) This
   killed an earlier "classify at write-time via a text marker" approach.
4. **No code-side staleness rule is viable either.** Every stored fact's date is
   by definition in the past, so "has a past date → flag" fires on everything
   and discriminates nothing. A TTL/age threshold would be an arbitrary magic
   number. Staleness is poorly formalizable in code but trivial for the model.
5. Timestamp formats are **inconsistent across rows**
   (`2026-05-03T20:37:15.80514` no-tz vs `2026-05-14T00:18:26+00:00`) — parsing
   must be lenient.
6. Auto-retain is real and host-driven (`spawn_auto_retain`, `worker.rs:603`);
   most facts are Hindsight-extracted, not agent-authored. So a fix that only
   touches the agent-driven `memory_retain` tool would miss the majority. This
   is another reason the fix lives on the **read** side.

## Design

Minimal, read-side only. **No write-path change, no API change, no extra LLM
calls, no thresholds, no staleness classification.** We stop discarding the
dates Hindsight already sends, show them to the agent, and add one prompt line
telling the agent to re-verify dated facts before asserting them as current.
The model judges staleness per fact; code only surfaces the data.

### 1. Parse the dropped fields

`crates/right-memory/src/hindsight.rs`, `RecallResult`:

- Add `id`, `mentioned_at`, `occurred_start`, `occurred_end` (all
  `Option<String>`, `#[serde(default)]`).
- **Remove the dead `score` field** and fix the mock fixture/test that asserts
  it. (Per project FAIL-FAST/clean-code norms: don't keep a field the API never
  returns.) Keep `type` (renamed `fact_type`).

### 2. Render the date next to every fact

Replace `join_recall_texts` (or add a sibling used by the prompt path) with a
renderer that, per memory, prefixes a single observed-date tag derived from the
best available date (`occurred_start` else `mentioned_at`), normalized to a
`YYYY-MM-DD` prefix (lenient: take the date head of whatever format arrives;
if no date parses, render the bare text as today's behavior does):

```
- [observed 2026-05-27] Notion via Composio is unauthorized
- [observed 2026-05-21] User prefers Russian on request
- Some fact with no parseable date   (unchanged fallback)
```

No "stale" / "re-verify" / type-conditional logic in the renderer — every fact
gets the same neutral `[observed <date>]` treatment. Determinism: the renderer
is a pure function `render_recall(results) -> String`; no clock read inside (it
only formats dates already in the data), so it is unit-testable directly.

Both recall consumers go through this: the host auto-recall injection
(`worker.rs:2204`, `2846`) and the agent-facing `memory_recall` MCP tool
(`crates/right/src/aggregator.rs:277`). The file provider (`MEMORY.md`) is
unaffected — it has no Hindsight dates and renders plain.

### 3. One prompt line

`crates/right-codegen/templates/right/prompt/OPERATING_INSTRUCTIONS.md`, in the
existing `## Memory` section (prompt-tier brevity — one sentence):

> Recalled memories are tagged with the date they were observed; a dated fact
> reflects that past moment — verify current state with a live check before
> asserting it.

Optionally tighten `wrap_external_content` wording
(`crates/right-prompt-safety` / `ironclaw_safety`) to note memory may be
outdated — only if it doesn't fight the injection-defense text. Treated as
nice-to-have, not required.

## Files touched

- `crates/right-memory/src/hindsight.rs` — add date/id fields, drop `score`,
  new pure renderer; fix mock + test.
- `crates/bot/src/telegram/worker.rs` — call the new renderer at the two recall
  sites (mechanical).
- `crates/right/src/aggregator.rs` — `memory_recall` uses the new renderer.
- `crates/right-codegen/templates/right/prompt/OPERATING_INSTRUCTIONS.md` — one
  line.
- `docs/architecture/memory.md` — document that recall now surfaces observed
  dates; fix the existing drift (it under-describes the recall response shape)
  and note `score` is not returned.
- `PROMPT_SYSTEM.md` — reflect the new `[observed <date>]` recall framing
  (kept in sync per project convention).

## Testing

Targeted during dev; one full `cargo test --workspace` at the end (project
cadence).

- `render_recall` unit tests (pure): `occurred_start` present → uses it;
  only `mentioned_at` → uses it; both absent → bare text (legacy behavior);
  mixed/inconsistent timestamp formats both normalize to `YYYY-MM-DD`;
  unparseable date → bare text, no panic; multiple memories ordering preserved.
- `RecallResult` deserialization: real 13-field payload parses; missing
  optional fields default to `None`; absence of `score` is fine.
- Recall integration at both worker sites and the MCP tool produce the
  `[observed <date>]` form.

## Non-goals (explicitly out of scope)

- Write-time classification / in-content markers (extraction strips them).
- Any TTL, decay, expiry, or age-threshold logic (no magic numbers).
- Dropping, downranking, or hiding stale facts (flag-by-date only; never hide).
- A "forget"/delete pass over old facts. (Hindsight supports delete-by-
  `document_id`; a future curator could use it, but not here.)
- Changing the agent-facing `memory_retain` tool or the auto-retain payload.
- Using `metadata`/`tags` for server-side recall filtering (metadata is not
  filterable; not needed for this fix).
