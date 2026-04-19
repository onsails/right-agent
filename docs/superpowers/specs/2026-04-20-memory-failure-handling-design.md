# Memory Failure Handling

## Overview

Harden every memory operation in RightClaw against upstream and local failures.
Today, any non-2xx from Hindsight, any `composite-memory.md` write/upload error,
and any unreadable `MEMORY.md` all fall through to the same pattern: `WARN` log
and silent degradation. That is acceptable for a one-off blip but masks real
problems (auth failures hidden for days, auto-retain data loss during outages,
prefetch-then-blocking-recall hammering a known-dead upstream twice per
message).

The design introduces a single resilient wrapper around `HindsightClient`,
shared error classification, a circuit breaker per process, a persistent retain
queue with a 24h age cap, and an explicit `<memory-status>` signal injected into
the system prompt so the agent knows when memory is degraded.

## Goals

- No silent data loss on transient upstream outages: failed auto-retains are
  queued and re-tried for up to 24h.
- No upstream hammering during outages: once a circuit is open, callers skip
  without issuing HTTP requests.
- Agent-visible memory state: the system prompt exposes `Healthy`, `Degraded`,
  or `AuthFailed` so the agent can respond honestly ("my memory is flaky right
  now") instead of answering without context as if it had it.
- Loud auth failures: a wrong/rotated API key triggers one Telegram
  notification (not a WARN buried in logs) and blocks retry storms.
- No change to `HindsightClient`'s surface; the wrapper is a drop-in
  replacement for its consumers.
- Scope: every memory moment except cron. That means Hindsight auto-recall,
  auto-retain, prefetch, MCP tools (`memory_retain`/`recall`/`reflect`), bank
  provisioning, `composite-memory.md` write/upload, and `MEMORY.md` file-mode
  read.

## Non-Goals

- Cross-process circuit state sharing. Bot and aggregator each maintain their
  own breaker. The bot is the high-volume caller; the aggregator fires only on
  explicit agent MCP tool calls. Shared state would cost an IPC hop on the hot
  path for no proven benefit.
- Cron path changes. Crons intentionally skip memory (per current
  architecture); this design preserves that.
- Changes to `HindsightClient`. It stays a dumb HTTP client.
- User-facing `Degraded` notifications. Only `AuthFailed` pages the user. Short
  blips stay in the agent-marker channel.
- File-mode `MEMORY.md` resilience machinery. The file is local disk; read
  failures indicate an install/codegen bug and are best diagnosed via `doctor`,
  not runtime retry.

## Module Layout

All new abstractions live in the shared `rightclaw` crate under
`crates/rightclaw/src/memory/`:

```
resilient.rs      — ResilientHindsight wrapper (same public API as HindsightClient)
circuit.rs        — CircuitState machine + failure counter
classify.rs       — ErrorKind enum + MemoryError::classify()
retain_queue.rs   — SQLite-backed pending_retains queue + drain_tick()
status.rs         — MemoryStatus enum + watch::Sender/Receiver plumbing
```

Consumers:

- `crates/bot/src/lib.rs` constructs one `Arc<ResilientHindsight>` per bot
  startup; passes it through `WorkerContext.hindsight` (replacing the current
  `Arc<HindsightClient>`). The bot also spawns a single drain task that calls
  `retain_queue::drain_tick()` every 30s.
- `crates/rightclaw-cli/src/aggregator.rs` constructs its own independent
  `Arc<ResilientHindsight>`; `HindsightBackend` receives it instead of a raw
  `HindsightClient`. The aggregator does **not** run a drain task.
- `pending_retains` is a shared SQLite table in the per-agent `data.db`. Both
  processes may enqueue (WAL mode + `unchecked_transaction` keeps it safe).
  Only the bot drains.

## Error Classification

`MemoryError::classify()` produces:

```rust
enum ErrorKind {
    Transient,   // 5xx, timeout, connect error, DNS fail, reqwest::Error without status
    RateLimited, // 429 (honour Retry-After when present)
    Auth,        // 401, 403
    Client,      // 400, 404, 422 — our bug, not upstream's
    Malformed,   // JSON parse error on response body
}
```

Behaviour per kind:

| Kind          | Retry                            | Breaker tick         | Retain queue | Agent marker | Telegram         |
|---------------|----------------------------------|----------------------|--------------|--------------|------------------|
| `Transient`   | yes (async paths)                | +1                   | enqueue      | `Degraded`   | —                |
| `RateLimited` | honour `Retry-After` else yes    | +1                   | enqueue      | `Degraded`   | —                |
| `Auth`        | **no**                           | immediate `Open(1h)` | **skip**     | `AuthFailed` | one-shot, 24h dedup |
| `Client`      | no                               | **no**               | skip + ERROR | —            | —                |
| `Malformed`   | no                               | +1                   | enqueue      | `Degraded`   | —                |

`Client` does not tick the breaker because ticking on a caller bug would
eventually block healthy traffic forever. `Auth` does not enqueue retains
because a new API key may bind to a different `bank_id`, making drained old
entries land in the wrong place.

## Retry Policy Per Operation

Retries run only while the breaker is `Closed`/`HalfOpen`. When `Open`, every
call returns `ResilientError::CircuitOpen { retry_after }` without touching the
network.

| Operation                               | Per-attempt timeout | Retries | Total budget | Rationale                                                                                   |
|-----------------------------------------|---------------------|---------|--------------|---------------------------------------------------------------------------------------------|
| Blocking recall (worker, pre-claude)    | 3s                  | **0**   | 3s           | User is waiting; replaces today's ad-hoc 5s `tokio::time::timeout`. Wrapper owns the timeout. |
| Auto-retain (worker, post-turn)         | 10s                 | 2       | ~25s         | Background; losing the data is expensive, we spend retries.                                 |
| Prefetch recall (worker, post-reply)    | 5s                  | 1       | ~8s          | Background but next turn will blocking-recall anyway; second retry has low value.           |
| MCP `memory_retain` (agent invokes)     | 10s                 | 1       | ~12s         | Agent waits; most calls hit a healthy upstream so one retry is enough.                      |
| MCP `memory_recall`                     | 5s                  | 0       | 5s           | Agent waits; on failure surface tool error so agent can decide.                             |
| MCP `memory_reflect`                    | 15s                 | 0       | 15s          | Already expensive; retries double p95.                                                      |
| `get_or_create_bank` (startup)          | 10s                 | 3       | ~35s         | Runs once; worth waiting.                                                                   |

Backoff is exponential with jitter: `500ms * 2^n ± 0-250ms`.

The wrapper owns the timeout for every call; call sites remove
`tokio::time::timeout(...)` wrappers (single source of truth).

## Circuit Breaker

Three states:

```rust
enum CircuitState {
    Closed,
    Open { until: Instant },
    HalfOpen,
}
```

Thresholds for `Transient` / `RateLimited` / `Malformed`:

- `5 failures in a 30s rolling window` → `Open { until: now + 30s }`.
- After `until` → `HalfOpen`.
- In `HalfOpen` the next real call is a probe. Success → `Closed`. Failure →
  `Open { until: now + 60s }` (double backoff).
- Absolute cap: open duration never exceeds 10 min. After 10 min, force a
  `HalfOpen` to probe whether upstream has recovered even if nothing else
  triggered it.

Special transitions:

- `Auth` error → immediate `Open { until: now + 1h }` and
  `MemoryStatus::AuthFailed`. Only resets via a successful startup
  `get_or_create_bank` probe (i.e. user must rotate the key and restart the
  agent); on reset the Telegram dedup entry is cleared so the next auth failure
  re-notifies.
- `Client` error → no state change; healthy upstream, broken caller.

Counters: `VecDeque<Instant>` of recent failure timestamps; entries older than
30s are evicted on each push. Success in `Closed` does not reset the deque —
stale failures expire by time.

Public API:

```rust
impl ResilientHindsight {
    async fn retain(...) -> Result<RetainResponse, ResilientError>;
    async fn recall(...) -> Result<Vec<RecallResult>, ResilientError>;
    async fn reflect(...) -> Result<ReflectResponse, ResilientError>;
    fn status(&self) -> MemoryStatus;
    fn subscribe_status(&self) -> watch::Receiver<MemoryStatus>;
}

enum ResilientError {
    Upstream(MemoryError),
    CircuitOpen { retry_after: Option<Duration> },
}
```

## Persistent Retain Queue

Migration V14:

```sql
CREATE TABLE pending_retains (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    content         TEXT NOT NULL,
    context         TEXT,
    document_id     TEXT,
    update_mode     TEXT,
    tags_json       TEXT,          -- JSON array of strings, NULL if no tags
    created_at      TEXT NOT NULL, -- ISO8601
    attempts        INTEGER NOT NULL DEFAULT 0,
    last_attempt_at TEXT,
    last_error      TEXT,
    source          TEXT NOT NULL  -- 'bot' | 'aggregator' (debug only)
);

CREATE INDEX idx_pending_retains_created ON pending_retains(created_at);
```

Enqueue rules (both processes):

- On `Transient` / `RateLimited` / `Malformed` during a retain call → insert
  with `attempts = 0`.
- On `Auth` / `Client` → do not enqueue.
- On `CircuitOpen` for a retain call → enqueue (we never tried; queue for
  later).
- Pre-enqueue cap: if row count exceeds 1000, delete the oldest before insert.
  Unbounded growth is the failure mode to avoid.

Drain loop (bot only, `tokio::spawn` in `lib.rs`):

```
loop {
    sleep(30s);
    if wrapper.status() != Healthy { continue; }   // Degraded OK — breaker may be HalfOpen

    let batch = SELECT * FROM pending_retains ORDER BY created_at ASC LIMIT 20;
    for entry in batch {
        if entry.created_at < now - 24h {
            DELETE entry; log WARN "retain dropped: >24h"; continue;
        }
        match wrapper.retain(entry.into_args()).await {
            Ok(_) => DELETE entry,
            Err(Upstream(e)) if e.classify() == Auth => break, // pointless to drain
            Err(Upstream(_)) | Err(CircuitOpen{..}) => {
                UPDATE entry SET attempts+=1, last_attempt_at=now, last_error=...;
                break; // don't storm on a struggling upstream
            }
        }
    }
}
```

Design choices:

- Batch size 20 — after a long outage, don't flood upstream with the entire
  backlog at once.
- Stop on first failure within a batch — if upstream is starting to fail again,
  back off.
- Order `ASC` by `created_at` — preserve chronology of conversation turns.
- Cleanup by age happens inline in the drain loop; no separate cleanup task.
- `attempts` is debug telemetry only; 24h age cap is the only drop criterion.

Concurrency nuance: auto-retain uses `session_uuid` as `document_id` with
`update_mode: "append"`. If retain N is queued while retain N+1 for the same
session goes straight through (brief breaker-close window), Hindsight resolves
ordering server-side via its own timestamps. Acceptable.

## Status Signalling

### Agent marker in system prompt

`MemoryStatus`:

```rust
enum MemoryStatus {
    Healthy,
    Degraded { since: Instant },
    AuthFailed { since: Instant },
}
```

Transitions:

- Breaker `Closed` and queue drained → `Healthy`.
- Breaker `Open`/`HalfOpen`, or any `pending_retains` row with `attempts > 0`,
  or any non-Auth inline failure in the last turn → `Degraded`.
- Any `Auth` error → `AuthFailed`. Exits only via startup bank probe success.

Injected into the composite memory section of the system prompt:

- `Healthy` → no marker (prompt-cache friendly).
- `Degraded` → append:
  ```
  <memory-status>degraded — recall may be incomplete or stale, retain may be queued</memory-status>
  ```
- `AuthFailed` → append:
  ```
  <memory-status>unavailable — memory provider authentication failed, memory ops will error until the user rotates the API key</memory-status>
  ```

The marker is written at the end of `composite-memory.md`, after the recall
content. That file is already a per-turn, cache-bust-tolerant region; the
stable prompt prefix (IDENTITY/SOUL/AGENTS/TOOLS/MCP instructions) is not
affected.

### Telegram one-shot notification

`crates/bot/src/telegram/memory_alerts.rs` (new):

- Subscribes to `wrapper.subscribe_status()`.
- On `AuthFailed` transition, consults `memory_alerts` SQLite table (new,
  migration V14 ships both tables):
  ```sql
  CREATE TABLE memory_alerts (
      alert_type    TEXT PRIMARY KEY,
      first_sent_at TEXT NOT NULL
  );
  ```
- If no row or `first_sent_at < now - 24h`, send to every chat in the agent's
  allowlist:
  ```
  ⚠️ Memory provider authentication failed.
  Rotate the Hindsight API key (HINDSIGHT_API_KEY) and restart the agent.
  Memory ops are disabled until then.
  ```
  Upsert the row with `first_sent_at = now`.
- On successful recovery to `Healthy` (startup bank probe passes), delete the
  row so the next failure re-notifies.

No Telegram notification for `Degraded` — that channel is reserved for states
requiring user action.

## Non-Hindsight Memory Moments

### `composite-memory.md` write/upload (`prompt.rs:118-137`)

Today: `WARN` + continue; agent sees empty memory and cannot tell.

Change:

- `deploy_composite_memory()` returns `Result<(), DeployError>`.
- Worker handles `Err` by inlining the recall content into the system-prompt
  assembly shell script via heredoc, bypassing the file-based path. Recall
  output is bounded by `max_tokens` (8192), safely under argv limits.
- If the inline fallback also fails (exotic shell-escape issue), log `WARN` and
  locally flip this turn's `MemoryStatus` to `Degraded`. This is a
  per-turn-only status (does not tick the Hindsight breaker).

### `MEMORY.md` read (file mode, `prompt.rs:94-96`)

Today: `head -200 MEMORY.md` silently produces empty on read error.

Change: keep the shell path but annotate explicit unreadability:

```sh
if [ -s {root_path}/MEMORY.md ]; then
  head -200 {root_path}/MEMORY.md 2>/dev/null \
    || echo "<memory-status>MEMORY.md unreadable</memory-status>"
fi
```

No full status machinery for file-mode: a `MEMORY.md` read error means an
install/codegen bug, not runtime degradation. Signal to the agent is enough.

### SQLite memory module

Today: `open_connection()` propagates `MemoryError::Sqlite`; bot fails fast on
startup. This is correct (FAIL FAST — no retain queue, no sessions, no crons
without SQLite). The design preserves this.

Add a `doctor.rs` check: `data.db` exists, WAL mode is enabled, migration
version matches. This aids diagnosis without changing runtime behaviour.

### Drain task self-failure

If SQLite hiccups inside a drain tick: `WARN` + sleep to next tick. The drain
task itself is not a catastrophic path; process-compose restarts the bot if
something worse happens.

## Testing

### Unit tests

- `classify.rs` — status-code-to-kind table.
- `circuit.rs` — state transitions under `tokio::time::pause()`: closed→open
  after N fails, open→half-open on timer, half-open success→closed, half-open
  fail→open with doubled backoff, auth→immediate long-open, client→no tick,
  10-minute absolute cap.
- `retain_queue.rs` — enqueue + drain + age cap + 1000-row eviction + FIFO
  order, against a real SQLite `tempdir()`.
- `resilient.rs` — wrapper behaviour with mock HTTP servers (following
  `hindsight.rs` test conventions): 5xx→retry→success, `N×5xx`→circuit opens,
  401→`AuthFailed` status + no retry, rate-limit honours `Retry-After`.
- `status.rs` — watch channel transitions for every `ErrorKind`.

### Integration tests

- Full outage scenario: mock Hindsight returns 500 for the duration; a worker
  message still produces a reply; retain is enqueued; breaker `Open`;
  `<memory-status>degraded</memory-status>` appears in `composite-memory.md`.
- Recovery: mock returns 500 then flips to 200 after ~30s; breaker transitions
  `HalfOpen`→`Closed`; drain flushes the queue; marker disappears.
- Auth failure: mock returns 401; `AuthFailed` status set; `memory_alerts` row
  inserted; a second failure in the same 24h window does not re-send the
  Telegram message.
- Queue eviction: populate `pending_retains` to 1000, enqueue one more, verify
  the oldest row is gone.
- Independence: two mock servers back the bot and the aggregator respectively;
  tripping one breaker does not affect the other.

### Out of scope for tests

- Live Hindsight API (tests must be hermetic).
- Cron memory path (crons skip memory by design).
- `#[ignore]` is not used on any test (per `CLAUDE.rust.md`). Where a live
  sandbox is needed, `TestSandbox::create()` is the entry point; in this
  design, all upstream interactions are mockable.

## Migration & Rollout

- Migration V14 adds `pending_retains` and `memory_alerts` tables. Idempotent
  (`CREATE TABLE IF NOT EXISTS`). Both the bot and the aggregator run
  migrations via `open_connection(..., migrate: true)`; existing agents pick up
  the new tables on next startup.
- Backward compatibility: new config fields default to the previous behaviour
  (no new user-facing flags in this design — retry/breaker thresholds are code
  constants for now; if tuning is needed they can be promoted to `agent.yaml`
  later).
- Upgrade-friendly: already-deployed agents adopt the change on bot restart
  without sandbox recreation. No files-on-sandbox changes.
- Observability: new `tracing` spans at every transition (breaker open/close,
  retain enqueue/drain/drop, status change). Existing log aggregation picks
  them up without change.

## Open Questions

None at spec approval time. Threshold numbers (5-in-30s breaker trip, 30s
initial open, 1000-row queue cap, 24h age cap, batch size 20) are informed
defaults; tune against real production traffic once the implementation ships.
