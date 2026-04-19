# Memory Failure Handling

Date: 2026-04-20
Status: approved

## Overview

Harden every memory operation in RightClaw against upstream and local failures.
Today, any non-2xx from Hindsight, any `composite-memory.md` write/upload
error, and any unreadable `MEMORY.md` all fall through to the same pattern:
`WARN` log and silent degradation. That is acceptable for a one-off blip but
masks real problems (auth failures hidden for days, auto-retain data loss
during outages, prefetch-then-blocking-recall hammering a known-dead upstream
twice per message).

The design introduces a single resilient wrapper around `HindsightClient`,
shared error classification, a circuit breaker per process, a persistent retain
queue with a 24h age cap, an explicit `<memory-status>` signal injected into
the system prompt, and `doctor` checks that flag queue health.

## Goals

- No silent data loss on transient upstream outages: failed auto-retains are
  queued and re-tried for up to 24h.
- No upstream hammering during outages: once a circuit is open, callers skip
  without issuing HTTP requests.
- Agent-visible memory state: the system prompt exposes `Healthy`, `Degraded`,
  or `AuthFailed` so the agent can respond honestly instead of answering
  without context as if it had it.
- Loud auth failures: a wrong/rotated API key triggers one Telegram
  notification (not a WARN buried in logs) and blocks retry storms.
- Loud Client-error (4xx) floods: a burst of persistent 4xx retains (suggesting
  upstream API drift or a payload bug) surfaces through the agent and via a
  one-shot Telegram alert.
- `HindsightClient`'s HTTP surface is unchanged; the wrapper is a drop-in
  replacement for its consumers. `MemoryError` gains classified variants so
  `classify()` is structural, not regex over an opaque string.
- Scope: every memory moment except cron. That means Hindsight auto-recall,
  auto-retain, prefetch, MCP tools (`memory_retain`/`recall`/`reflect`), bank
  provisioning, `composite-memory.md` write/upload, and `MEMORY.md` file-mode
  read.

## Non-Goals

- Cross-process circuit state sharing. Bot and aggregator each maintain their
  own breaker. The bot is the high-volume caller; the aggregator fires only on
  explicit agent MCP tool calls. Shared state would cost an IPC hop on the hot
  path for no proven benefit.
- Cross-backend MCP error-convention alignment (Ok(CallToolResult{is_error})
  vs Err(anyhow)). Keep the existing convention: tool failures as Rust `Err`.
  If/when aligned, it is a separate spec covering all backends.
- Cron path changes. Crons intentionally skip memory (per current
  architecture); this design preserves that.
- User-facing `Degraded` notifications. Only `AuthFailed` and `ClientFlood`
  page the user. Short blips stay in the agent-marker channel.
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

### Invariant: same-config coupling of bot and aggregator

Both processes construct their wrapper from the same `agent.yaml` (and env
variables that flow from it). Neither process has private memory config.
`api_key` and `bank_id` MUST match across processes. Any config change flows
through `config_watcher`, which triggers a coordinated restart of both
processes. This keeps the drain-shared `pending_retains` table correct: a row
enqueued by the aggregator will be drained by the bot against the *same*
upstream bank. The `source` column on `pending_retains` remains debug-only
given this invariant.

## Error Classification

### `MemoryError` gains structured variants

Today `hindsight.rs` collapses all transport errors into
`MemoryError::HindsightRequest(String)`. `classify()` cannot distinguish
timeout from DNS from body-parse without regex. The wrapper needs structural
information.

```rust
enum MemoryError {
    Sqlite(rusqlite::Error),
    Migration(rusqlite_migration::Error),
    InjectionDetected,
    NotFound(i64),
    Hindsight { status: u16, body: String },  // unchanged
    HindsightTimeout,                          // reqwest::Error::is_timeout()
    HindsightConnect(String),                  // reqwest::Error::is_connect() | is_request()
    HindsightParse(String),                    // reqwest::Error::is_decode() | json parse
    HindsightOther(String),                    // fallback
}
```

`HindsightClient` conversion logic (one place, `hindsight.rs`) inspects
`reqwest::Error` flags and picks the right variant. Public HTTP surface of
`HindsightClient` is unchanged.

### `ErrorKind`

`MemoryError::classify()` produces:

```rust
enum ErrorKind {
    Transient,   // 5xx, HindsightTimeout, HindsightConnect
    RateLimited, // 429 (honour Retry-After when present)
    Auth,        // 401, 403
    Client,      // 400, 404, 422 — our bug or upstream API drift
    Malformed,   // HindsightParse — bad body from upstream
}
```

### Behaviour per kind

| Kind          | Retry                         | Breaker tick        | Retain enqueue | Drain action                       | Agent marker     | Telegram             |
|---------------|-------------------------------|---------------------|----------------|------------------------------------|------------------|----------------------|
| `Transient`   | yes (async paths)             | +1                  | enqueue        | UPDATE attempts + break batch      | `Degraded` (via breaker) | —            |
| `RateLimited` | honour `Retry-After` else yes | +1                  | enqueue        | UPDATE attempts + break, respect RA | `Degraded`       | —                    |
| `Auth`        | **no**                        | immediate `Open(1h)` | **skip**       | `break` (stop draining)            | `AuthFailed`     | one-shot, 24h dedup  |
| `Client`      | no                            | **no**              | skip           | `DELETE entry` + ERROR log + continue; also bump drops-counter | `retain-errors` marker + `Degraded` if flood threshold crossed | one-shot Telegram alert if > 20 in 1h |
| `Malformed`   | no                            | +1                  | enqueue        | UPDATE attempts + break            | `Degraded`       | —                    |

Rationale:

- `Client` does not tick the breaker because ticking on a caller bug would
  eventually block healthy traffic forever. But persistent `Client` errors
  *do* signal an upstream breaking change or a payload bug. We surface them
  with loud ERROR logs, agent marker, and (if flooding) a Telegram alert.
- `Auth` does not enqueue retains because a new API key may bind to a
  different `bank_id`, making drained old entries land in the wrong place.
- `Malformed` ticks the breaker like `Transient` because it usually signals a
  partially-failing upstream serving HTML error pages instead of JSON.

## Retry Policy Per Operation

Retries run only while the breaker is `Closed`/`HalfOpen`. When `Open`, every
call returns `ResilientError::CircuitOpen { retry_after }` without touching the
network.

| Operation                               | Per-attempt timeout | Retries | Total budget | Rationale                                                                 |
|-----------------------------------------|---------------------|---------|--------------|---------------------------------------------------------------------------|
| Blocking recall (worker, pre-claude)    | 3s                  | **0**   | 3s           | User is waiting; replaces today's 5s `tokio::time::timeout`.              |
| Auto-retain (worker, post-turn)         | 10s                 | 2       | ~23s         | Background; losing the data is expensive, we spend retries.               |
| Prefetch recall (worker, post-reply)    | 5s                  | 1       | ~7s          | Background; next turn will blocking-recall anyway.                        |
| MCP `memory_retain` (agent invokes)     | 10s                 | 1       | ~12s         | Agent waits; one retry for transient blips.                               |
| MCP `memory_recall`                     | 5s                  | 0       | 5s           | Agent waits; on failure surface tool error.                               |
| MCP `memory_reflect`                    | 15s                 | 0       | 15s          | Already expensive; retries double p95.                                    |
| `get_or_create_bank` (startup)          | 10s                 | 3       | ~34s         | Runs once; visible as startup delay (see § Startup).                     |

Backoff is exponential with jitter: `500ms * 2^n + [0, 250ms)`. Total budgets
include backoff windows rounded.

The wrapper owns the timeout for every call; call sites remove
`tokio::time::timeout(...)` wrappers (single source of truth).

### Startup behaviour

`get_or_create_bank` runs at bot/aggregator startup. If it fails:

- **`Auth`** → log ERROR; set `MemoryStatus::AuthFailed`; send one-shot
  Telegram alert; start the bot anyway in degraded mode. All subsequent
  memory calls fail cleanly through the breaker (`CircuitOpen`). The user
  sees the alert and rotates the key. On next restart (via config_watcher
  or manual), startup probe retries.
- **`Transient` / `RateLimited` / `Malformed`** → log WARN; set
  `MemoryStatus::Degraded`; start the bot; background task re-probes every
  60s until success, then flips to `Healthy`.
- **`Client`** → log ERROR; start the bot in degraded mode; trigger the
  ClientFlood alert path (same as runtime Client-error handling).

Startup may block for up to ~34s waiting on the initial probe (the 3-retry
budget). This is visible in bot logs and acceptable for a once-per-process
event.

### Recovery from `AuthFailed`

Recovery requires a restart, not hot-reload:

- If the key lives in `agent.yaml` (`memory.api_key`), editing the file
  triggers `config_watcher` → bot exits with code 2 → process-compose
  restarts → startup probe runs again.
- If the key lives in `HINDSIGHT_API_KEY` env var, the user restarts the bot
  manually via the process-compose TUI (Ctrl+R on the agent's entry) or
  `rightclaw agent restart <name>`.

On a successful probe, `MemoryStatus` flips to `Healthy` and the
`memory_alerts` dedup row is cleared so the next auth failure re-notifies.

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
- Absolute cap: open duration never exceeds 10 min. After 10 min, force
  `HalfOpen` to probe whether upstream has recovered even if nothing else
  triggered it.

Special transitions:

- `Auth` error → immediate `Open { until: now + 1h }` and
  `MemoryStatus::AuthFailed`. Resets only via a successful startup
  `get_or_create_bank` probe after user-triggered restart.
- `Client` error → no state change at breaker level. Drops-counter + flood
  alerting handle this out-of-band.

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

    /// Batch-retain — single POST with multiple items (reduces drain HTTP count).
    async fn retain_many(items: &[RetainItem]) -> Result<RetainResponse, ResilientError>;

    /// Count of Client-error drops in the last 24h (rolling in-memory window).
    fn client_drops_24h(&self) -> usize;
}

enum ResilientError {
    Upstream(MemoryError),
    CircuitOpen { retry_after: Option<Duration> },
}
```

`HindsightClient::retain` is extended to accept `&[RetainItem]` in one POST
(Hindsight already accepts `items: Vec<_>` but the current client always
wraps a single item). This is the *only* change to `HindsightClient`'s
surface.

## Persistent Retain Queue

Migration V14 (idempotent):

```sql
CREATE TABLE IF NOT EXISTS pending_retains (
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
CREATE INDEX IF NOT EXISTS idx_pending_retains_created ON pending_retains(created_at);

CREATE TABLE IF NOT EXISTS memory_alerts (
    alert_type    TEXT PRIMARY KEY, -- 'auth_failed' | 'client_flood'
    first_sent_at TEXT NOT NULL
);
```

### Enqueue rules (both processes)

- On `Transient` / `RateLimited` / `Malformed` during a retain call → insert
  with `attempts = 0`. Single INSERT (no TX needed).
- On `CircuitOpen` for a retain call → insert (we never tried; queue for
  later).
- On `Auth` / `Client` → do not enqueue.
- Pre-enqueue cap: if row count exceeds 1000, delete the oldest first. When
  eviction fires, wrap the `DELETE oldest + INSERT new` pair in
  `conn.unchecked_transaction()` per SQLite rule.

### Drain loop (bot only)

```text
loop {
    sleep(30s);

    // Status-based gate: only drain when upstream is usable.
    if wrapper.status() != Healthy { continue; }

    // Read batch (no mutation — read outside TX).
    let batch = SELECT * FROM pending_retains ORDER BY created_at ASC LIMIT 20;
    if batch.is_empty() { continue; }

    // Group consecutive entries that share no semantic ordering concern
    // (different document_id) into a single POST via retain_many.
    // For same document_id entries keep serial order.

    let tx = conn.unchecked_transaction()?;

    for entry in batch {
        if entry.created_at < now - 24h {
            DELETE entry;                       // age cap
            log WARN "retain dropped: >24h";
            continue;
        }

        match wrapper.retain_many(&[entry.into()]).await {   // one-item for simplicity; groups TODO
            Ok(_) => DELETE entry,

            Err(Upstream(e)) => match e.classify() {
                Client => {
                    DELETE entry;
                    log ERROR "retain dropped on 4xx: id={id} body={body}";
                    wrapper.bump_client_drop();  // updates in-memory counter + marker trigger
                    continue;
                }
                Auth => {
                    // shouldn't happen (Auth never enqueued), but defensively:
                    break;
                }
                Transient | RateLimited | Malformed => {
                    UPDATE entry SET attempts=attempts+1, last_attempt_at=now, last_error=...;
                    break; // don't storm a struggling upstream
                }
            },

            Err(CircuitOpen{..}) => {
                UPDATE entry SET attempts=attempts+1, last_attempt_at=now, last_error='circuit_open';
                break;
            }
        }
    }

    tx.commit()?;
}
```

Design choices:

- Batch size 20 — after a long outage, don't flood upstream.
- Stop batch on `Transient` / `RateLimited` / `Malformed` / `CircuitOpen` —
  upstream is degraded, preserve back-off.
- Continue batch on `Client` — poison pill must not block legitimate work.
- Order `ASC` by `created_at` — preserve chronology of conversation turns.
- Cleanup by age happens inline (24h); no separate cleanup task.
- `attempts` is debug telemetry only; 24h age cap is the only drop criterion.
  Status does NOT key off `attempts` — see § Status Signalling.

### Idempotency on SIGKILL

If the bot dies between HTTP 200 and `DELETE entry`, a restart re-runs the
retain. Hindsight's fact extractor is expected to deduplicate at the fact
level (same content → same extracted facts → single slot in bank). We accept
this rare-window behavior. If production observation shows fact-level
duplicates, revisit with a `status='sending'` pre-mark column (trades
duplication risk for data-loss risk on interrupted sends).

### Append ordering across queue

Retain content is already a JSON array with explicit turn timestamps:

```json
[{"role":"user","content":"…","timestamp":"…"},
 {"role":"assistant","content":"…","timestamp":"…"}]
```

If retain N is queued while retain N+1 passes real-time, Hindsight will
receive them out of arrival order. Because event timestamps live inside the
content, Hindsight's fact extractor can date extracted facts by the content
timestamps, not the arrival order. `update_mode: append` reordering is
acceptable under this invariant. If extractor behaviour changes, the fallback
is per-session serialization (new retains for session X queue behind any
pending retains for X).

### Size footprint

Row count cap is 1000. Typical content is 1–5KB; pathological turns may hit
100KB. Worst case ≈ 100MB in SQLite, which does affect `VACUUM INTO` speed at
backup time but stays within single-digit-seconds on modern disks. We do not
add a byte-size cap; if real telemetry shows memory-db bloat, introduce one
later.

### Backup interaction

`rightclaw agent backup` captures `pending_retains` as-is. On restore:

- If the restored agent uses the same `bank_id` (normal case), drain sends
  the pending retains to the same bank. The 24h age cap filters stale
  entries.
- If restored under a different `bank_id` (unusual), records go to the new
  bank — this is a user choice, not a bug.

No restore-time truncation. Queue is transient state; losing it is worse
than draining into the correct bank.

## Status Signalling

### `MemoryStatus`

```rust
enum MemoryStatus {
    Healthy,
    Degraded { since: Instant },
    AuthFailed { since: Instant },
}
```

Rules:

- **Breaker `Closed` and no recent (< 30s) non-Auth failure** → `Healthy`.
- **Breaker `Open` or `HalfOpen`, or any non-Auth failure in the last 30s**
  → `Degraded`.
- **Any `Auth` error** → `AuthFailed`. Exits only via startup bank probe
  success (after user-triggered restart).

Queue size is **not** an input to status. The drain catches up invisibly once
the breaker is `Closed`. The agent marker "retain may be queued" remains
accurate for the `Degraded` window (turns during the breaker-open period).

### Per-turn local status (fallback from composite-memory deploy)

Worker-side fallback path (§ Non-Hindsight Moments) may locally compute a
per-turn status if `deploy_composite_memory` and the heredoc fallback both
fail. Worker combines sources at prompt assembly:

```rust
let effective = wrapper.status().max(local_turn_status);
```

`MemoryStatus` implements `Ord` by severity: `Healthy < Degraded <
AuthFailed`. The local status lives one turn and is reset on each new turn.
The wrapper status is the baseline across turns.

### Agent marker in system prompt

Injected into the composite memory section (`composite-memory.md`), which
MUST remain the last section of the system prompt assembly (see §
Invariants):

- `Healthy` + `client_drops_24h() == 0` → no marker (prompt-cache friendly).
- `Healthy` + `client_drops_24h() > 0` →
  ```
  <memory-status>retain-errors: N records dropped in last 24h due to bad payload — check logs</memory-status>
  ```
- `Degraded` →
  ```
  <memory-status>degraded — recall may be incomplete or stale, retain may be queued</memory-status>
  ```
- `AuthFailed` →
  ```
  <memory-status>unavailable — memory provider authentication failed, memory ops will error until the user rotates the API key</memory-status>
  ```

Bot-side wrapper only. Aggregator-side breaker state is NOT merged into the
marker; MCP tool errors surface directly to the agent in the tool-call
response text when the aggregator's circuit is open. This is deliberate: the
marker reflects pre-turn memory state (the recall context injection the bot
does), while MCP tool errors are mid-turn agent actions with their own
signalling channel.

### Telegram one-shot notifications

`crates/bot/src/telegram/memory_alerts.rs` (new) subscribes to
`wrapper.subscribe_status()` and watches `client_drops_24h()`:

**`AuthFailed` transition:**
- Check `memory_alerts` for `alert_type='auth_failed'`. If no row or
  `first_sent_at < now - 24h`, send to allowlist chats:
  ```
  ⚠️ Memory provider authentication failed.
  Rotate the Hindsight API key — set `memory.api_key` in agent.yaml or the
  HINDSIGHT_API_KEY env var — and restart the agent.
  Memory ops are disabled until then.
  ```
  Upsert the row with `first_sent_at = now`.
- On `Healthy` recovery (startup bank probe passes), delete the row.
- **Bot-startup cleanup:** on startup, `DELETE FROM memory_alerts WHERE
  alert_type='auth_failed' AND first_sent_at < now - 1h`. Reason: crash-loop
  without user action shouldn't silence alerts for 24h; giving the user a
  fresh alert every ≥1h of uptime during an ongoing outage is reasonable.

**`ClientFlood` threshold:**
- Track drops in `ResilientHindsight` as a `VecDeque<Instant>` of drop
  timestamps, evicting entries older than 1h on each push.
- If > 20 entries in the deque, fire the alert:
  ```
  ⚠️ Memory retains persistently rejected (HTTP 4xx) — possible API drift
  or bug in RightClaw. Check ~/.rightclaw/logs/<agent>.log for details.
  ```
- Dedup via `memory_alerts` with `alert_type='client_flood'`, same 24h window
  + 1h startup cleanup as `auth_failed`.

No Telegram notification for `Degraded` — that channel is reserved for
states requiring user action.

## Non-Hindsight Memory Moments

### `composite-memory.md` write/upload (`prompt.rs:118-137`)

Today: `WARN` + continue; agent sees empty memory and cannot tell.

Change:

1. `deploy_composite_memory()` returns `Result<(), DeployError>`.
2. Worker handles `Err` by inlining the recall content into the
   system-prompt assembly shell script via heredoc, bypassing the file-based
   path. Recall output is bounded by `max_tokens` (8192), safely under argv
   limits.
3. If the inline fallback also fails (exotic shell-escape issue), log `WARN`
   and locally flip this turn's status to `Degraded`. The wrapper status is
   unchanged; `effective_status = max(wrapper, local)` picks up the local
   bump.

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
install/codegen bug, not runtime degradation.

### SQLite memory module

Today: `open_connection()` propagates `MemoryError::Sqlite`; bot fails fast
on startup. This is correct (FAIL FAST) and preserved.

Doctor adds a `check_memory()` step (see § Doctor Integration).

### Drain task self-failure

If SQLite hiccups inside a drain tick: `WARN` + sleep to next tick. The
drain task itself is not a catastrophic path; process-compose restarts the
bot if something worse happens.

## Doctor Integration

`crates/rightclaw/src/doctor.rs` gains a `check_memory()` step, run from the
existing `rightclaw doctor` command per agent.

```text
1. data.db exists and opens                   → fail if missing
2. journal_mode = WAL                         → fail if not
3. user_version matches current migration      → fail if mismatch

4. pending_retains row count:
    < 500            → ok
    500 – 900        → WARN  "retain backlog growing: N entries"
    > 900            → ERROR "retain backlog near cap (N/1000)"

5. oldest pending_retains.created_at age:
    < 1h             → ok
    1h – 12h         → WARN  "drain behind by Nh — upstream may be degraded"
    > 12h            → ERROR "drain severely stuck — investigate logs"

6. memory_alerts rows:
    alert_type='auth_failed' AND first_sent_at < now - 24h
                     → ERROR "auth failed for >24h — key rotation overdue"
    alert_type='client_flood' AND first_sent_at < now - 24h
                     → ERROR "client-flood alert standing for >24h"
```

Threshold rationale:

- 500/900 out of the 1000-row cap (50%/90%).
- 1h/12h: healthy drain processes 40 records/min, so a 500-entry backlog
  drains in ~12 min. 1h backlog is already suspicious; 12h means drain is
  genuinely stuck.

Exit code: 0 if clean (optional WARN lines to stderr), 1 if any ERROR. Runs
per agent; aggregate agent results in the existing doctor output format.

## Invariants

These invariants must hold for the design to work. A test enforces each.

1. **Composite memory is the last system-prompt section.** `composite-memory.md`
   (and therefore the `<memory-status>` marker) MUST be the last section of
   the system prompt assembly to preserve prompt cache for all preceding
   blocks. A test in `prompt_tests.rs` asserts this: the section order in
   the assembled script places memory last.

2. **Bot and aggregator share `agent.yaml` as single config source.** Neither
   process has private `api_key` / `bank_id`. Config changes flow through
   `config_watcher` → coordinated restart.

3. **`MemoryStatus::Ord` is by severity.** `Healthy < Degraded < AuthFailed`.
   Merging two status sources takes `max()`.

## Testing

### Unit tests

- `classify.rs` — every `MemoryError` variant → expected `ErrorKind`. Covers
  5xx, 4xx, 401/403, 429, timeout, connect, parse.
- `circuit.rs` — state transitions under `tokio::time::pause()`: closed→open
  after N fails, open→half-open on timer, half-open success→closed,
  half-open fail→open with doubled backoff, auth→immediate long-open,
  client→no tick, 10-minute absolute cap.
- `retain_queue.rs` — enqueue + drain + age cap + 1000-row eviction + FIFO
  order, against a real SQLite `tempdir()`. Separate test for the
  classified-drain behaviour: Client → DELETE + continue; Transient → break;
  RateLimited with Retry-After; CircuitOpen.
- `resilient.rs` — wrapper behaviour with mock HTTP servers (following
  `hindsight.rs` test conventions): 5xx→retry→success, N×5xx→circuit opens,
  401→AuthFailed + no retry, rate-limit honours Retry-After,
  retain_many-batch POST shape.
- `status.rs` — watch channel transitions for every `ErrorKind`; status
  ordering (`Healthy < Degraded < AuthFailed`) and `max()` merge semantics.
- `doctor.rs` — queue threshold checks (under/at/over each tier) via
  fixtures.

### Integration tests

- **Full outage scenario:** mock Hindsight returns 500; a worker message
  still produces a reply; retain enqueues; breaker `Open`;
  `<memory-status>degraded</memory-status>` in `composite-memory.md`.
- **Recovery:** mock returns 500 then flips to 200 after ~30s; breaker
  transitions `HalfOpen`→`Closed`; drain flushes the queue; marker
  disappears.
- **Auth failure at runtime:** mock returns 401; `AuthFailed` set;
  `memory_alerts.auth_failed` inserted; second runtime 401 in the same 24h
  window does not re-send; bot restart after 1h+ uptime *does* re-send (1h
  cleanup rule).
- **Auth failure at startup:** mock returns 401 on `get_or_create_bank`; bot
  starts anyway in degraded mode; Telegram alert sent; agent sees
  `AuthFailed` marker.
- **Client flood:** mock returns 400 for every retain; drops counter
  rises; after 20 in 1h, `memory_alerts.client_flood` fires; agent marker
  shows retain-errors count.
- **Poison pill drain:** enqueue mix of valid and invalid payloads (mock
  returns 200 for valid, 400 for invalid); drain DELETEs invalid and
  continues to valid in same batch.
- **Queue eviction:** populate `pending_retains` to 1000, enqueue one more,
  verify the oldest row is gone.
- **Prompt-cache-stability test:** assert that `composite-memory.md` (and
  the `<memory-status>` marker) is the last block in the assembled
  system-prompt script (invariant 1).
- **Independence:** two mock servers back the bot and the aggregator
  respectively; tripping one breaker does not affect the other.

### Out of scope for tests

- Live Hindsight API (tests must be hermetic).
- Cron memory path (crons skip memory by design).
- `#[ignore]` is not used on any test (per `CLAUDE.rust.md`). Where a live
  sandbox is needed, `TestSandbox::create()` is the entry point; in this
  design, all upstream interactions are mockable.

## Migration & Rollout

- Migration V14 adds `pending_retains` and `memory_alerts` tables.
  Idempotent (`CREATE TABLE IF NOT EXISTS`). Both the bot and the aggregator
  run migrations via `open_connection(..., migrate: true)`.
- Backward compatibility: new `MemoryError` variants are added, not renamed;
  existing matches on `Hindsight { status, body }` continue to work. The
  wrapper is a net new type; existing `Arc<HindsightClient>` call sites
  migrate mechanically.
- Upgrade-friendly: already-deployed agents adopt the change on bot restart
  without sandbox recreation. No files-on-sandbox changes.
- Observability: new `tracing` spans at every transition (breaker
  open/close, retain enqueue/drain/drop, status change, client-flood trip).

## Open Questions

Thresholds (breaker 5-in-30s / first-open 30s / queue cap 1000 / drain batch
20 / drain interval 30s / 24h age cap / client-flood 20-in-1h / doctor
500/900 rows & 1h/12h age) are informed defaults. Tune against real traffic
once the implementation ships. No config keys are exposed yet; if tuning is
needed they can be promoted to `agent.yaml` later.
