# Memory subsystem

> **Status:** descriptive doc. Re-read and update when modifying this
> subsystem (see `CLAUDE.md` → "Architecture docs split"). Code is
> authoritative; this file may have drifted.

## Memory

Two modes, configured per-agent via `memory.provider` in agent.yaml:

**Hindsight mode (primary):** Hindsight Cloud API (`api.hindsight.vectorize.io`),
one bank per agent. Three MCP tools exposed via aggregator:
`memory_retain`, `memory_recall`, `memory_reflect`. Prefetch cache is in-memory
(lost on restart → blocking recall on first interaction).

Auto-retain after each turn: content formatted as JSON role/content/timestamp
array, `document_id` = CC session UUID (same as `--resume`), `update_mode:
"append"` so only new content triggers LLM extraction (O(n) vs O(n²) for
full-session replace). Tags: `["chat:<chat_id>"]` for per-chat scoping.

Auto-recall before each `claude -p`: query truncated to 800 chars, tags
`["chat:<chat_id>"]` with `tags_match: "any"` (returns per-chat + global untagged
memories). Prefetch uses same parameters.

**Cron jobs skip memory:** Cron and delivery sessions perform no auto-recall
or auto-retain. Cron prompts are static instructions — recall results would be
irrelevant and corrupt user memory representations (same approach as hermes-agent
`skip_memory=True`). Crons can call `memory_recall` and `memory_retain` MCP tools
explicitly when needed.

**Backgrounded turns retain user message at fork time:** When a foreground turn
is sent to background (auto-timeout at 10 min, or user clicks the Background
button), the worker's `Backgrounded` arm retains the user message *only* (no
assistant text yet) keyed by the main `--resume` session UUID with
`update_mode: "append"`. Without this, the cron-delivery answer relayed back
through `--resume <main>` would arrive over a session whose user turn was
never recorded in Hindsight (cron-side sessions skip auto-retain). The
assistant turn extends the same document later via either an explicit
`memory_retain` MCP call from the cron prompt or the next foreground turn's
auto-retain.

**File mode (fallback):** Agent manages `MEMORY.md` via CC Edit/Write.
Bot injects file contents into system prompt (truncated to 200 lines).
No MCP memory tools.

The legacy `store_record` / `query_records` / `search_records` / `delete_record`
tools are removed from the surface; their backing tables (`memories`,
`memories_fts`, `memory_events`) are retained for migration compat.

## Memory Resilience Layer

`memory::resilient::ResilientHindsight` wraps `HindsightClient` with:
- per-process circuit breaker (closed→open after 5 fails in 30s; 30s initial
  open with doubling backoff to a 10 min cap; 1h hard open on Auth)
- classified retries (Transient/RateLimited yes; Auth/Client/Malformed no)
- SQLite-backed `pending_retains` queue (1000-row cap, 24h age cap)
- `watch::Sender<MemoryStatus>` signalling Healthy/Degraded/AuthFailed

The bot runs a single drain task (30s interval, batch 20, stop on first
non-Client failure). The aggregator shares the same SQLite queue via the
per-agent `data.db`; it enqueues on failure but never drains.

Telegram alerts (`memory_alerts` table, 24h dedup, 1h startup cleanup) fire
on:
- `AuthFailed` transition
- >20 `Client`-kind drops in a 1h rolling window (`client_flood`)

Doctor checks queue size (500/900 row thresholds), oldest-row age (1h/12h
thresholds), and long-standing (>24h) alerts.
