# Hindsight Memory Staleness Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Surface the observation date Hindsight already returns on every recalled memory, so agents stop reciting stale point-in-time facts as current.

**Architecture:** Read-side only. Parse the date fields Hindsight already sends (`mentioned_at`, `occurred_start/end`) into `RecallResult`, drop the dead `score` field, add a pure renderer that prefixes each recalled memory with `[observed <date>]` for the host auto-recall (composite-memory) path, and add one prompt-tier sentence telling the agent to re-verify dated facts. No write-path change, no API change, no LLM calls, no staleness thresholds. The agent-facing `memory_recall` MCP tool already serializes raw results, so it gains the fields automatically.

**Tech Stack:** Rust (edition 2024), serde, the project `right-memory` / `bot` / `right` / `right-codegen` crates. Test runner: `devenv shell -- cargo test`.

---

## Background for the implementer (read once)

- The Hindsight recall HTTP response returns **13 fields per result**, but
  `RecallResult` (`crates/right-memory/src/hindsight.rs:18-26`) parses only
  `text`, `score`, `type`. Verified live: the real response has
  `id, text, type, entities, context, occurred_start, occurred_end,
  mentioned_at, document_id, metadata, chunk_id, tags, source_fact_ids`.
- **`score` is NOT in the real response** — it always deserializes to `None`.
  We remove it.
- `mentioned_at` (retain time) is present on every result;
  `occurred_start`/`occurred_end` (event time) are often present. Timestamp
  formats are **inconsistent** (`2026-05-03T20:37:15.80514` no-tz vs
  `2026-05-14T00:18:26+00:00`), so the renderer takes the leading `YYYY-MM-DD`
  via simple prefix slicing — no datetime parsing, no clock.
- `serde` ignores unknown JSON fields by default, so the existing mock fixtures
  that omit the new fields keep deserializing fine. The only test that breaks is
  the one asserting `score`.
- `join_recall_texts` (`hindsight.rs:99-105`) is used by exactly two host
  call sites: `worker.rs:2204` (prefetch) and `worker.rs:2846` (blocking
  recall). Both feed composite-memory. The agent-facing MCP tool
  (`aggregator.rs:271-293`) serializes `results` directly and does NOT use this
  function.
- Run all cargo commands through `devenv shell --`.

## File Structure

- `crates/right-memory/src/hindsight.rs` — `RecallResult` struct (add date
  fields, drop `score`); new pure `render_recall_with_dates` function; unit
  tests; fix the one `score` assertion.
- `crates/bot/src/telegram/worker.rs` — swap the two `join_recall_texts` calls
  for the new renderer.
- `crates/right-codegen/templates/right/prompt/OPERATING_INSTRUCTIONS.md` — one
  sentence in the `## Memory` section.
- `docs/architecture/memory.md` — document the real recall response shape and
  the `[observed <date>]` framing; fix existing drift.
- `PROMPT_SYSTEM.md` — note the `[observed <date>]` recall framing.

---

## Task 1: Add date fields to `RecallResult`, remove dead `score`

**Files:**
- Modify: `crates/right-memory/src/hindsight.rs:18-26`
- Modify (test): `crates/right-memory/src/hindsight.rs:541-569`

- [ ] **Step 1: Write the failing test**

Add this test inside the existing `#[cfg(test)] mod tests` block in
`crates/right-memory/src/hindsight.rs` (place it right after the existing
`recall_sends_correct_request` test, around line 569):

```rust
    #[tokio::test]
    async fn recall_parses_date_fields() {
        let (_handle, url) = mock_hindsight_server(
            r#"{"results": [{
                "id": "11111111-1111-1111-1111-111111111111",
                "text": "Notion via Composio is unauthorized",
                "type": "experience",
                "mentioned_at": "2026-05-27T08:57:00+00:00",
                "occurred_start": "2026-05-27T08:00:00.12345",
                "occurred_end": null,
                "document_id": "22222222-2222-2222-2222-222222222222"
            }]}"#,
            200,
        )
        .await;

        let client = test_client(&url);
        let results = client.recall("notion", None, None).await.unwrap();

        assert_eq!(results.len(), 1);
        let r = &results[0];
        assert_eq!(r.id.as_deref(), Some("11111111-1111-1111-1111-111111111111"));
        assert_eq!(r.mentioned_at.as_deref(), Some("2026-05-27T08:57:00+00:00"));
        assert_eq!(r.occurred_start.as_deref(), Some("2026-05-27T08:00:00.12345"));
        assert_eq!(r.occurred_end, None);
        assert_eq!(r.fact_type.as_deref(), Some("experience"));
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `devenv shell -- cargo test -p right-memory recall_parses_date_fields`
Expected: FAIL — compile error, no field `id` / `mentioned_at` / `occurred_start` / `occurred_end` on `RecallResult`.

- [ ] **Step 3: Update the struct**

Replace `crates/right-memory/src/hindsight.rs:18-26` with:

```rust
/// A single recall result from Hindsight.
///
/// Hindsight returns more fields than we consume; only the ones used by the
/// recall renderer and the agent-facing `memory_recall` tool are modeled.
/// `serde` ignores the rest. Note: the live API does NOT return a `score`
/// field, so it is intentionally absent.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RecallResult {
    pub text: String,
    #[serde(rename = "type", default)]
    pub fact_type: Option<String>,
    /// Stable memory id (UUID). Present on every result.
    #[serde(default)]
    pub id: Option<String>,
    /// When the fact was retained (ISO 8601, inconsistent format). Present on
    /// every result; the de-facto record-creation timestamp.
    #[serde(default)]
    pub mentioned_at: Option<String>,
    /// Event-time start for datable facts (ISO 8601). Often null.
    #[serde(default)]
    pub occurred_start: Option<String>,
    /// Event-time end for datable facts (ISO 8601). Often null.
    #[serde(default)]
    pub occurred_end: Option<String>,
}
```

- [ ] **Step 4: Fix the `score` assertion in the existing test**

In `crates/right-memory/src/hindsight.rs`, in test `recall_sends_correct_request`,
delete the now-invalid line (currently line 557):

```rust
        assert_eq!(results[0].score, Some(0.95));
```

(Leave the mock fixture JSON containing `"score": 0.95` as-is — serde ignores
the unknown field, which documents that real payloads may carry extra keys.)

- [ ] **Step 5: Run tests to verify they pass**

Run: `devenv shell -- cargo test -p right-memory hindsight`
Expected: PASS — `recall_parses_date_fields`, `recall_sends_correct_request`,
and the other `recall_*` tests all green.

- [ ] **Step 6: Commit**

```bash
git add crates/right-memory/src/hindsight.rs
git commit -m "feat(memory): parse Hindsight recall date fields, drop dead score"
```

---

## Task 2: Add the date-annotating recall renderer

**Files:**
- Modify: `crates/right-memory/src/hindsight.rs:99-105` (add new fn after `join_recall_texts`)
- Test: `crates/right-memory/src/hindsight.rs` (tests module)

- [ ] **Step 1: Write the failing tests**

Add to the `#[cfg(test)] mod tests` block in
`crates/right-memory/src/hindsight.rs`:

```rust
    fn rr(text: &str, occurred: Option<&str>, mentioned: Option<&str>) -> RecallResult {
        RecallResult {
            text: text.to_string(),
            fact_type: None,
            id: None,
            mentioned_at: mentioned.map(|s| s.to_string()),
            occurred_start: occurred.map(|s| s.to_string()),
            occurred_end: None,
        }
    }

    #[test]
    fn render_prefers_occurred_start() {
        let out = render_recall_with_dates(&[rr(
            "Notion is down",
            Some("2026-05-27T08:00:00.12345"),
            Some("2026-05-31T00:00:00+00:00"),
        )]);
        assert_eq!(out, "- [observed 2026-05-27] Notion is down");
    }

    #[test]
    fn render_falls_back_to_mentioned_at() {
        let out = render_recall_with_dates(&[rr(
            "User prefers Russian",
            None,
            Some("2026-05-21T10:00:00+00:00"),
        )]);
        assert_eq!(out, "- [observed 2026-05-21] User prefers Russian");
    }

    #[test]
    fn render_no_date_is_bare_bullet() {
        let out = render_recall_with_dates(&[rr("Timeless fact", None, None)]);
        assert_eq!(out, "- Timeless fact");
    }

    #[test]
    fn render_unparseable_date_is_bare_bullet() {
        let out = render_recall_with_dates(&[rr("Weird", None, Some("not-a-date"))]);
        assert_eq!(out, "- Weird");
    }

    #[test]
    fn render_joins_multiple_preserving_order() {
        let out = render_recall_with_dates(&[
            rr("First", Some("2026-01-02T00:00:00Z"), None),
            rr("Second", None, None),
        ]);
        assert_eq!(out, "- [observed 2026-01-02] First\n\n- Second");
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `devenv shell -- cargo test -p right-memory render_`
Expected: FAIL — `render_recall_with_dates` not found.

- [ ] **Step 3: Implement the renderer**

Insert immediately after `join_recall_texts` (after line 105) in
`crates/right-memory/src/hindsight.rs`:

```rust
/// Extract a `YYYY-MM-DD` date prefix from an ISO-8601-ish timestamp.
///
/// Hindsight timestamp formats are inconsistent (`2026-05-03T20:37:15.80514`
/// with no zone vs `2026-05-14T00:18:26+00:00`). We do not need a real
/// datetime — only the calendar date — so we validate and slice the leading
/// `YYYY-MM-DD`. Returns `None` if the head is not a plausible date.
fn date_prefix(ts: &str) -> Option<&str> {
    let head = ts.get(..10)?;
    let b = head.as_bytes();
    if b.len() == 10
        && b[4] == b'-'
        && b[7] == b'-'
        && b[..4].iter().all(u8::is_ascii_digit)
        && b[5..7].iter().all(u8::is_ascii_digit)
        && b[8..10].iter().all(u8::is_ascii_digit)
    {
        Some(head)
    } else {
        None
    }
}

/// Render recalled memories as bullets, prefixing each with its observed date
/// (`[observed YYYY-MM-DD]`) when one is available.
///
/// Date source preference: `occurred_start` (event time) else `mentioned_at`
/// (retain time). Memories with no parseable date render as a bare bullet,
/// preserving the prior `join_recall_texts` behavior. The date lets the agent
/// judge whether a point-in-time fact is stale; we deliberately do not
/// classify or filter — see
/// `docs/superpowers/specs/2026-05-31-hindsight-memory-staleness-design.md`.
pub fn render_recall_with_dates(results: &[RecallResult]) -> String {
    results
        .iter()
        .map(|r| {
            let date = r
                .occurred_start
                .as_deref()
                .and_then(date_prefix)
                .or_else(|| r.mentioned_at.as_deref().and_then(date_prefix));
            match date {
                Some(d) => format!("- [observed {d}] {}", r.text),
                None => format!("- {}", r.text),
            }
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `devenv shell -- cargo test -p right-memory render_`
Expected: PASS — all five `render_*` tests green.

- [ ] **Step 5: Commit**

```bash
git add crates/right-memory/src/hindsight.rs
git commit -m "feat(memory): add date-annotating recall renderer"
```

---

## Task 3: Use the renderer at the two host recall sites

**Files:**
- Modify: `crates/bot/src/telegram/worker.rs:2204`
- Modify: `crates/bot/src/telegram/worker.rs:2846`

- [ ] **Step 1: Swap the prefetch site**

In `crates/bot/src/telegram/worker.rs`, change line 2204 from:

```rust
                            let content = right_memory::hindsight::join_recall_texts(&results);
```

to:

```rust
                            let content = right_memory::hindsight::render_recall_with_dates(&results);
```

- [ ] **Step 2: Swap the blocking-recall site**

In `crates/bot/src/telegram/worker.rs`, change line 2846 from:

```rust
                    let content = right_memory::hindsight::join_recall_texts(&results);
```

to:

```rust
                    let content = right_memory::hindsight::render_recall_with_dates(&results);
```

- [ ] **Step 3: Remove now-dead `join_recall_texts` if unused**

Confirm no remaining callers:

Run: `grep -rn "join_recall_texts" crates`
Expected: only the definition in `hindsight.rs:99` (plus any tests of it).

If the only remaining reference is the definition itself and it has no test,
delete the `join_recall_texts` function (its old body, lines ~99-105) to avoid
dead code warnings. If it has its own unit test, delete that test too. If you
are unsure, leave it and add `#[allow(dead_code)]` is NOT acceptable — per
project norms remove genuinely dead code that your change orphaned. (This
function is orphaned only by this change, so removing it is in scope.)

- [ ] **Step 4: Build to verify it compiles**

Run: `devenv shell -- cargo build -p bot`
Expected: builds clean, no unused-function warning for `join_recall_texts`.

- [ ] **Step 5: Commit**

```bash
git add crates/bot/src/telegram/worker.rs crates/right-memory/src/hindsight.rs
git commit -m "feat(memory): render observed dates in host auto-recall injection"
```

---

## Task 4: Add the prompt-tier re-verify sentence

**Files:**
- Modify: `crates/right-codegen/templates/right/prompt/OPERATING_INSTRUCTIONS.md` (the `## Memory` section, currently starting line 21)

- [ ] **Step 1: Add the sentence**

In `crates/right-codegen/templates/right/prompt/OPERATING_INSTRUCTIONS.md`,
locate the `## Memory` section. After the existing paragraph that ends
"`mcp__right__memory_retain` stores that fallback context." insert this single
sentence as its own paragraph:

```markdown
Recalled memories are tagged `[observed <date>]` with when the fact was seen; a dated fact reflects that past moment — verify the current state with a live check before asserting it.
```

- [ ] **Step 2: Verify prompt-tier brevity**

Confirm the addition is one sentence and the section did not grow with examples.
Re-read the `## Memory` section; it should read cleanly. No command to run.

- [ ] **Step 3: Commit**

```bash
git add crates/right-codegen/templates/right/prompt/OPERATING_INSTRUCTIONS.md
git commit -m "docs(prompt): tell agents to re-verify dated recalled memories"
```

---

## Task 5: Update architecture + prompt-system docs

**Files:**
- Modify: `docs/architecture/memory.md`
- Modify: `PROMPT_SYSTEM.md`

- [ ] **Step 1: Document the recall response shape and framing in memory.md**

In `docs/architecture/memory.md`, in the Hindsight-mode section (near the
auto-recall paragraph around lines 22-24), add a short paragraph:

```markdown
**Recall response shape.** Hindsight returns ~13 fields per result; the client
models `text`, `type`, `id`, `mentioned_at`, `occurred_start`, `occurred_end`
(`right_memory::hindsight::RecallResult`) and ignores the rest. The API does
**not** return a `score`. Host auto-recall renders each memory as
`- [observed <date>] <text>` via `render_recall_with_dates` (date =
`occurred_start` else `mentioned_at`, sliced to `YYYY-MM-DD`); memories with no
parseable date render as a bare bullet. The agent-facing `memory_recall` MCP
tool serializes the structured results directly, so it exposes the date fields
as JSON. There is no staleness filtering or TTL — the date is surfaced and the
agent judges currency.
```

- [ ] **Step 2: Update PROMPT_SYSTEM.md**

In `PROMPT_SYSTEM.md`, find the memory/composite-memory description (the recall
injection wording). Add a sentence noting that recalled memories are rendered as
`- [observed <date>] <text>` and that the operating instructions direct the
agent to re-verify dated facts before asserting them as current. Keep it to one
or two sentences consistent with the surrounding style.

- [ ] **Step 3: Commit**

```bash
git add docs/architecture/memory.md PROMPT_SYSTEM.md
git commit -m "docs: document observed-date recall rendering and response shape"
```

---

## Task 6: Final workspace verification

- [ ] **Step 1: Run the full workspace test suite**

Run: `devenv shell -- cargo test --workspace`
Expected: PASS. Record any pre-existing unrelated failures; the memory tests
(`right-memory` `recall_*`, `render_*`) and `bot` build must be green.

- [ ] **Step 2: Run clippy on the touched crates**

Run: `devenv shell -- cargo clippy -p right-memory -p bot -p right`
Expected: no new warnings introduced by this change.

- [ ] **Step 3: Final commit (only if Step 1/2 required fixes)**

```bash
git add -A
git commit -m "test: fixes from full workspace verification"
```

---

## Self-Review notes

- **Spec coverage:** parse fields (T1) ✓; drop `score` (T1) ✓; date renderer
  (T2) ✓; wire host recall (T3) ✓; prompt line (T4) ✓; docs incl. drift fix
  (T5) ✓; final workspace test (T6) ✓. The spec's note that `memory_recall`
  gains fields "for free" is verified in T1/T5 (serialized results) — no
  separate task needed because the tool serializes the struct directly.
- **Non-goals honored:** no write-path/`memory_retain` change, no TTL/threshold,
  no filtering/hiding, no delete pass, no extra LLM calls.
- **Type consistency:** `render_recall_with_dates(&[RecallResult]) -> String`
  and `date_prefix(&str) -> Option<&str>` used consistently across T2/T3;
  `RecallResult` field names (`occurred_start`, `mentioned_at`, `fact_type`,
  `id`) identical in T1 struct, T2 tests, and T2 renderer.
