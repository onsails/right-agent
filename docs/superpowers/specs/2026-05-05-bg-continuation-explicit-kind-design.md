# Background Continuation: Explicit Job Kind & Mandatory Notify

**Date:** 2026-05-05
**Status:** Design (pending implementation plan)

## Problem

Two defects in the recently shipped background-continuation feature
(spec: `2026-05-05-background-continuation-design.md`):

### Defect 1 — silent bg run drops the user's answer

`bg-162518-2b1ad135` ran to `status='success'` and was auto-deleted
without delivering anything to Telegram (`delivery_status='silent'`,
`no_notify_reason=null`). The forked agent legally returned
`{notify: null, summary: "..."}` because it inherited the regular
cron schema (`CRON_SCHEMA_JSON`), where silent runs are valid.

For a regular cron, silence is correct ("nothing to report"). For a
bg-continuation, silence drops the user's answer — there was a real
question waiting for a real reply. The schema/contract doesn't
distinguish.

### Defect 2 — main session is unaware of in-flight bg jobs

While a bg job is running, the main (foreground) session has no
indication that work is queued. The user can ask "what's happening with
that thing you were doing?" and the agent can only guess. The bg job's
ID arrives in the main session's history only when delivery synthesises
a user-turn after completion.

### Defect 3 (root cause for D1, surfaced during design) — kind is encoded as string-magic

Today "this row is a bg-continuation" is detected by:

1. `ScheduleKind::Immediate` AND
2. `prompt` starts with `X-FORK-FROM: <uuid>\n`

The kind discriminator lives inside the user-message field. Adding a
second downstream effect (schema selection) on top of this string-magic
multiplies the risk of rule drift. Job-kind needs to be first-class.

## Goal

Bg-continuation is structurally distinct from a regular cron job at the
type level. The schema enforces non-silent output. The main session
receives a small, always-fresh marker listing in-flight bg jobs by ID so
it can be conversationally aware and (via existing `cron_show_run`)
fetch their results.

## Non-goals

- New MCP tools for bg jobs. `cron_list_runs` / `cron_show_run` already
  cover the agent-side path.
- Cancelling or showing live progress of running bg jobs (covered by
  unrelated "Future work" in the parent spec).
- Changing how regular crons (`Recurring`, `OneShotCron`, `RunAt`,
  `Immediate`) decide to notify. Their existing `CRON_SCHEMA_JSON`
  and `silent` semantics stay.
- Cross-chat awareness. The marker is filtered by `target_chat_id`.

## Decisions summary

| Decision | Choice |
|---|---|
| Job-kind representation | New `ScheduleKind::BackgroundContinuation { fork_from: Uuid }` variant alongside existing `Recurring`, `OneShotCron`, `RunAt`, `Immediate`. |
| DB encoding | `schedule = '@bg:<uuid>'` sentinel, no DDL change. |
| Where fork_from is parsed | New `ScheduleKind::from_db_row(schedule, run_at, recurring) -> Result<ScheduleKind, String>` extracted from the inline match in `load_specs_from_db` (cron_spec.rs:707-721). Single source of truth. |
| Drop X-FORK-FROM prompt header | Yes. `prompt` carries only the user-facing system notice. |
| Schema for bg | New `BG_CONTINUATION_SCHEMA_JSON`, `notify` required + non-null, `notify.content` `minLength: 1`. |
| Schema selection | `matches!(spec.schedule_kind, BackgroundContinuation { .. })`. |
| Continuation-prompt tweak | Add explicit "silence is not allowed" line. |
| Main-session awareness | `<background-jobs>` marker appended to `composite-memory.md` next to `<memory-status>`. |
| Awareness data source | `SELECT id, started_at, status FROM cron_runs WHERE target_chat_id = ?1 AND ((status='running') OR (status='success' AND delivered_at IS NULL))`. |
| Defensive delivery for silent bg | Dropped. Schema enforcement is sufficient; `cron_show_run` is the escape hatch. |
| Migration of in-flight `@immediate`+X-FORK-FROM rows | One-time startup migration; overwrites `schedule` and strips header from `prompt`. |
| `insert_immediate_cron` helper | Deleted. Sole production caller (`enqueue_background_job`) switches to new `insert_background_continuation`. Existing tests for the old helper are migrated to the new one or removed if redundant. |

## Schema-selection discriminator

Today's two truths (ScheduleKind and X-FORK-FROM prompt prefix) collapse
to one: the variant itself.

```rust
let json_schema = match &spec.schedule_kind {
    ScheduleKind::BackgroundContinuation { .. } =>
        right_agent::codegen::BG_CONTINUATION_SCHEMA_JSON,
    _ => right_agent::codegen::CRON_SCHEMA_JSON,
};
let fork_from = match &spec.schedule_kind {
    ScheduleKind::BackgroundContinuation { fork_from } => Some(fork_from.to_string()),
    _ => None,
};
```

`fork_from.is_some()` ⇔ bg schema selected ⇔ `--fork-session` emitted.
A single match arm makes the three downstream effects co-vary by
construction. Prompt-header parsing in `execute_job` is removed.

## Components and code changes

### `right-agent/src/cron_spec.rs` — new variant

```rust
pub enum ScheduleKind {
    Recurring(String),
    OneShotCron(String),
    RunAt(DateTime<Utc>),
    Immediate,
    BackgroundContinuation { fork_from: Uuid },   // NEW
}
```

DB encoding: `schedule = '@bg:<uuid>'`. Existing `'@immediate'` sentinel
keeps mapping to `ScheduleKind::Immediate`.

Refactor (small, scoped to this PR): extract the inline match in
`load_specs_from_db` (cron_spec.rs:707-721) into
`ScheduleKind::from_db_row(schedule: &str, run_at: Option<&str>, recurring: i64) -> Result<ScheduleKind, String>`.
Single source of truth for sentinel parsing. `load_specs_from_db` calls
it and skips rows that fail to parse with a `tracing::error!` (current
behaviour). Branches:
- `run_at = Some(_)` → `RunAt(dt)`
- `schedule == "@immediate"` → `Immediate`
- `schedule.starts_with("@bg:")` → parse trailing UUID; on failure
  return `Err`. On success: `BackgroundContinuation { fork_from }`
- `recurring == 0` → `OneShotCron(schedule.to_string())`
- otherwise → `Recurring(schedule.to_string())`

`is_one_shot()` returns `true` for `BackgroundContinuation` (auto-deletes
after one fire, like other one-shots).

`cron_schedule()` returns `None` for `BackgroundContinuation`.

`Display for ScheduleKind` extends to write `format!("@bg:{fork_from}")`
for `BackgroundContinuation` — round-trips with `from_db_row`.

`insert_immediate_cron` (cron_spec.rs:358) is **deleted**. Replaced by:

```rust
pub fn insert_background_continuation(
    conn: &rusqlite::Connection,
    job_name: &str,
    prompt: &str,
    fork_from: Uuid,
    target_chat_id: i64,
    target_thread_id: Option<i64>,
    max_budget_usd: Option<f64>,
) -> Result<CronSpecResult, String>
```

Stores `schedule = format!("@bg:{fork_from}")`, `run_at = NULL`,
`recurring = 0`, `lock_ttl = IMMEDIATE_DEFAULT_LOCK_TTL` (semantics
identical to bg today). Calls into `create_spec_v2` with a new
`bg_fork_from: Option<Uuid>` parameter (keeps the validator/insert path
unified) — see resolve_schedule_fields update below.

`resolve_schedule_fields` extends the 4-way mutual exclusion to
`schedule | run_at | immediate | bg_fork_from`. The bg case stores
`@bg:<uuid>` into `db_schedule`, `run_at = None`, `recurring = 0`, no
warning. Reaching this branch via `create_spec_v2` is bot-internal —
`cron_create` MCP path never sets `bg_fork_from`, only
`insert_background_continuation` does.

`BackgroundContinuation` is not constructible from CLI/MCP. Tests for
`insert_immediate_cron` (`cron_spec_tests.rs:1117,1135,1137` and
integration test `crates/bot/tests/cron_immediate.rs`) are migrated to
`insert_background_continuation` with a fixed `fork_from = Uuid::nil()`
or random — they exercise the same DB path with the new sentinel.

### `right-agent/src/codegen/agent_def.rs` — new schema

```rust
/// Structured-output schema for background-continuation cron runs.
/// `notify` is required and non-null; `notify.content` must be a
/// non-empty string. `summary` is required (kept for log/analytics
/// parity with `CRON_SCHEMA_JSON`). `no_notify_reason` is absent —
/// silence is not a valid outcome for this job kind.
pub const BG_CONTINUATION_SCHEMA_JSON: &str = r#"{
  "type":"object",
  "properties":{
    "notify":{
      "type":"object",
      "properties":{
        "content":{"type":"string","minLength":1},
        "attachments":{
          "type":["array","null"],
          "items":{
            "type":"object",
            "properties":{
              "type":{"enum":["photo","document","video","audio","voice","video_note","sticker","animation"]},
              "path":{"type":"string"},
              "filename":{"type":["string","null"]},
              "caption":{"type":["string","null"]},
              "media_group_id":{"type":["string","null"]}
            },
            "required":["type","path"]
          }
        }
      },
      "required":["content"]
    },
    "summary":{"type":"string"}
  },
  "required":["summary","notify"]
}"#;
```

Tests in `agent_def_tests.rs` mirror existing `CRON_SCHEMA_JSON`
coverage: shape assertion, key-presence, attachments still optional.

### `bot/src/cron.rs` — drop X-FORK-FROM parsing

Remove the `let (fork_from_main_session, prompt_for_cc) = …` block
(`cron.rs:329-358`). Replace with the `match &spec.schedule_kind` shown
in [Schema-selection discriminator](#schema-selection-discriminator).

`prompt_for_cc = spec.prompt.clone()` — no header to strip.

`json_schema` field of `ClaudeInvocation` consumes the chosen schema.

The "agent that calls cron_create cannot hijack `--resume`" guard is
preserved by construction: only `BackgroundContinuation` carries
`fork_from`, and only `insert_background_continuation` produces that
variant. `cron_create` MCP path validates against `Cron`/`OneShot`
shapes and rejects unknown sentinels.

### `bot/src/telegram/worker.rs` — caller update

`build_continuation_prompt` (worker.rs:465) gets one extra line:

```text
You MUST produce a non-empty notify.content. Silence is not a valid
outcome for this turn — the user is waiting for an answer.
```

`enqueue_background_job` (worker.rs:486) drops the `X-FORK-FROM:`
prefix construction. It calls `insert_background_continuation` with
`fork_from = Uuid::parse_str(main_session_id)?`. If parse fails (this
should never happen — `main_session_id` is always a real UUID at the
worker layer) the function returns an error and the bg-flow falls back
to the existing reflection error path (the worker logs and edits the
thinking message to a failure banner).

### `bot/src/telegram/prompt.rs` — second marker slot

`deploy_composite_memory` signature changes:

```rust
pub(crate) async fn deploy_composite_memory(
    content: &str,
    label: &str,
    agent_dir: &Path,
    resolved_sandbox: Option<&str>,
    status_marker: Option<&str>,
    bg_marker: Option<&str>,        // NEW
) -> Result<(), DeployError>
```

The function appends `bg_marker` after `status_marker` with the same
`\n\n` separator pattern. The `(None, None)` guard in the caller
extends to `(None, None, None)` — only delete the file when all three
slots are empty.

Alternative considered: replace the two `Option<&str>` parameters with
`extra_markers: &[&str]`. Rejected — at two call sites with two distinct
semantic slots, named parameters carry intent better than an
order-dependent slice.

### `bot/src/telegram/worker.rs` — bg marker builder

```rust
fn build_bg_marker(conn: &Connection, target_chat_id: i64) -> Option<String> {
    let mut stmt = conn.prepare(
        "SELECT id, job_name, started_at, status \
         FROM cron_runs \
         WHERE target_chat_id = ?1 \
           AND ((status = 'running') OR (status = 'success' AND delivered_at IS NULL)) \
         ORDER BY started_at",
    ).ok()?;
    let rows: Vec<(String, String, String, String)> = stmt
        .query_map([target_chat_id], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
        })
        .ok()?
        .filter_map(Result::ok)
        .collect();
    if rows.is_empty() { return None; }
    let body = rows.iter()
        .map(|(id, name, ts, st)| format!("{name} (run {id}) — started {ts}, {st}"))
        .collect::<Vec<_>>()
        .join("\n");
    Some(format!("<background-jobs>\n{body}\n</background-jobs>"))
}
```

The query covers two states:
- `status='running'` — job in flight.
- `status='success' AND delivered_at IS NULL` — job finished, answer is
  queued for delivery (held by `IDLE_THRESHOLD_SECS` until the chat goes
  idle). The agent should be aware that an answer exists even before it
  lands.

The query is unscoped to `bg-*` job-name prefix on purpose: any
in-flight cron belonging to this chat is useful awareness. If we later
disambiguate (only show bg-continuation jobs), we filter by
`job_name LIKE 'bg-%'`. Acceptable today.

Both `id` (run UUID) and `job_name` (`bg-XXX`) are included so the agent
can pass either to `cron_show_run` (which accepts run IDs).

Caller (worker.rs:1686 area) opens the per-agent connection that's
already opened nearby for hindsight (or a short-lived new one), runs
`build_bg_marker`, and passes the result as `bg_marker` to
`deploy_composite_memory`.

### Startup migration (`bot/src/cron.rs`)

On bot startup, before the cron reconcile loop starts:

```rust
fn migrate_legacy_bg_continuation(conn: &Connection) -> Result<usize, rusqlite::Error> {
    let tx = conn.unchecked_transaction()?;
    let mut migrated = 0;
    {
        let mut stmt = tx.prepare(
            "SELECT name, prompt FROM cron_specs WHERE schedule = '@immediate'"
        )?;
        let rows: Vec<(String, String)> = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .filter_map(Result::ok)
            .collect();
        for (name, prompt) in rows {
            let Some(rest) = prompt.strip_prefix("X-FORK-FROM: ") else { continue };
            let Some((sess, body)) = rest.split_once('\n') else { continue };
            if Uuid::parse_str(sess).is_err() { continue; }
            tx.execute(
                "UPDATE cron_specs SET schedule = ?1, prompt = ?2 WHERE name = ?3",
                rusqlite::params![format!("@bg:{sess}"), body, name],
            )?;
            migrated += 1;
        }
    }
    tx.commit()?;
    Ok(migrated)
}
```

Idempotent: a row that already has `schedule LIKE '@bg:%'` is filtered
by the `WHERE schedule = '@immediate'` clause and untouched. A row whose
prompt no longer starts with `X-FORK-FROM:` (legitimately a plain
Immediate, hypothetical for now) is left alone. Logs the count at INFO
on every startup.

Lock and `cron_runs` rows for in-flight legacy jobs are not touched —
they will complete under their original `Immediate` semantics if the
process is mid-flight, and fresh runs of the same spec will use the
new variant.

### ARCHITECTURE.md updates

Replace the "Background continuation: X-FORK-FROM convention" section
with "Background continuation: `BackgroundContinuation` schedule kind".
Update `docs/architecture/sessions.md` if it carries the same wording.
Cite in commit message.

## Testing strategy

### Unit (pure)

| Test | File | Verifies |
|---|---|---|
| `bg_kind_db_roundtrip` | `cron_spec_tests.rs` | `BackgroundContinuation { fork_from } ↔ '@bg:<uuid>'` via `from_db_row` + `Display` |
| `bg_kind_invalid_uuid_errors` | `cron_spec_tests.rs` | `from_db_row("@bg:not-a-uuid", None, 0)` returns `Err` |
| `bg_kind_is_one_shot` | `cron_spec_tests.rs` | `is_one_shot()` returns true |
| `bg_kind_no_cron_schedule` | `cron_spec_tests.rs` | `cron_schedule()` returns None |
| `immediate_kind_still_parses` | `cron_spec_tests.rs` | `from_db_row("@immediate", None, 0)` → `Immediate` (regression guard) |
| `bg_continuation_schema_shape` | `agent_def_tests.rs` | `notify` required, `notify.content` minLength=1 |
| `bg_marker_emits_running_runs_for_chat` | `worker.rs::tests` | filter by `target_chat_id` and `status='running'` |
| `bg_marker_empty_returns_none` | `worker.rs::tests` | no rows → None |
| `migrate_legacy_bg_continuation_rewrites_row` | `cron.rs::tests` | `@immediate` + `X-FORK-FROM:` → `@bg:<uuid>` + body |
| `migrate_legacy_bg_continuation_idempotent` | `cron.rs::tests` | second call migrates 0 rows |
| `migrate_legacy_bg_continuation_skips_invalid_uuid` | `cron.rs::tests` | malformed UUID row left untouched |
| `enqueue_background_job_uses_bg_kind` | `worker.rs::tests` | row inserted with `schedule='@bg:<main>'`, no header in prompt |
| `build_continuation_prompt_forbids_silence` | `worker.rs::tests` | "Silence is not a valid outcome" present |

### Integration (TestSandbox)

| Test | Verifies |
|---|---|
| `bg_job_completes_with_required_notify` | Bg job completes → `cron_runs.delivery_status='pending'`, `notify_json` non-null and non-empty |
| `bg_job_invocation_uses_bg_schema` | Captured `claude -p` argv contains the bg schema JSON, not the cron one |
| `bg_marker_visible_in_running_foreground` | While a bg job is running, foreground turn's `composite-memory.md` contains `<background-jobs>` listing the bg id |
| `legacy_bg_row_migrated_on_startup` | Pre-insert legacy `@immediate`+`X-FORK-FROM` row → restart bot → row updated to `@bg:<uuid>`, fires correctly |

### TDD ordering

1. `ScheduleKind::BackgroundContinuation` parser + serializer (pure).
2. `BG_CONTINUATION_SCHEMA_JSON` constant + shape tests (pure).
3. Replace `enqueue_background_job` + `insert_background_continuation`
   (pure → DB).
4. `cron.rs::execute_job` schema/fork-from selection (integration with
   stub claude — assert argv).
5. `migrate_legacy_bg_continuation` (pure → DB).
6. `build_bg_marker` + `deploy_composite_memory` plumbing (pure → DB).
7. End-to-end bg run delivers non-silent notify (TestSandbox).

## Risks and trade-offs

### `notify.content` minLength = 1 may be too lax

An agent could return `notify.content = " "` to game schema while
saying nothing useful. Acceptable for v1. If observed in production we
add a stricter regex (e.g. `\\S{4,}`) — schema-only fix, no code change.

### Two markers in `composite-memory.md` may compound prompt-cache misses

Each marker change invalidates the cache prefix. `<memory-status>` is
already volatile (Hindsight degraded → healthy on retry). Adding
`<background-jobs>` adds a second axis of churn — every bg-job lifecycle
change invalidates the cache. Mitigation: order matters less than
existence; place markers AFTER the `<memory-context>` block so the
recall body itself stays cache-stable. Worst case: a chat with constant
bg-job churn loses prompt-cache hits for the system tail. Cost is
bounded (markers are short).

### Migration runs on every startup

`migrate_legacy_bg_continuation` is O(N) over `@immediate` rows. After
the first successful pass, future passes touch zero rows. Cost is
negligible for any realistic deployment.

### Removing the X-FORK-FROM-in-prompt convention

Any external tool that reads `cron_specs.prompt` directly and expected
the header (none today) breaks. Searched: no callers outside `cron.rs`.
Internal-only convention removed cleanly.

### Forked child sees its own bg marker

The cron path also assembles its system prompt via
`build_prompt_assembly_script` (`prompt.rs:54`), which `cat`s
`composite-memory.md`. So the forked CC session reading the
`<background-jobs>` block will see a row for itself
(`status='running'` for the very run it is). This is a harmless
artefact — the prompt makes the bg job's task explicit, and seeing
itself in the marker doesn't change the answer. Worth noting in case
log output looks confusing during debugging.

If we later want to suppress it, the worker writes
`composite-memory.md` per foreground turn; cron-path could deploy a
*separate* `composite-memory.md` without the marker (or skip the
deploy entirely — cron's prompt context is the forked jsonl, recall
is intentionally skipped per ARCHITECTURE.md "Cron jobs skip memory").

### Schema mismatch on schema-violating responses

If the agent somehow violates `BG_CONTINUATION_SCHEMA_JSON` (returns
malformed JSON or missing `notify`), `--json-schema` validation in CC
fails. The existing failure path
(`classify_cron_failure` → `reflect_on_failure`) catches it, marks the
run `'failed'`, and delivery uses `DELIVERY_INSTRUCTION_FAILURE`. The
user gets a failure summary instead of the actual answer. Worse than
success, better than silence. Acceptable.

## Migration & rollout

### Required steps

1. Code lands; bot processes restart via `right restart <agent>` or
   `process-compose on_failure`. New code is `Regenerated(BotRestart)` —
   picked up automatically.
2. ARCHITECTURE.md and `docs/architecture/sessions.md` updated in the
   same commit. PROMPT_SYSTEM.md if it references the schema.

### Not required

- DB schema migration (sentinel string change only).
- Sandbox recreation.
- Manual user/operator action for already-deployed agents.

### Compatibility with existing crons

- New `BackgroundContinuation` variant requires updates at:

  **Compiler-enforced (exhaustive `match`):**
  - `cron_spec.rs::cron_schedule()`, `is_one_shot()`, `Display`
  - `cron_spec.rs::resolve_schedule_fields` (extended to 4-way exclusion)
  - `cron_spec.rs::load_specs_from_db` → moved to `from_db_row`

  **NOT compiler-enforced (`matches!` macros — silent miss if forgotten):**
  - `cron.rs:1121` `matches!(.., RunAt(dt) if dt <= now)` — bg variant is fired in the Immediate branch instead, so this filter stays unchanged. Verify by integration test that bg jobs don't double-fire.
  - `cron.rs:1142-1146` Immediate-fire filter — **extend to**
    `Immediate | BackgroundContinuation { .. }`. Without this the bg
    job never fires. Critical-path miss.
  - `cron.rs:1182-1186` "skip from run_job_loop" filter — **extend to**
    `RunAt(_) | Immediate | BackgroundContinuation { .. }`. Without
    this the reconciler spawns a long-running cron-style handle for
    the bg row, which is wrong (one-shot, not recurring).
  - `cron.rs:331` X-FORK-FROM detection block — **deleted** in this PR.

  **Schema/fork-from in execute_job:** new `match &spec.schedule_kind`
  selecting bg schema and `fork_from`. Exhaustive — compiler-enforced.

- Test callsites referencing existing variants stay unchanged unless
  they specifically test bg behaviour. New tests for
  `BackgroundContinuation` are listed under Testing strategy.
- Legacy `@immediate` rows with X-FORK-FROM headers are migrated at
  startup and become indistinguishable from freshly-inserted bg rows.
- `BG_CONTINUATION_SCHEMA_JSON` is independent of `CRON_SCHEMA_JSON` —
  changes to one do not affect the other.

### Rollback

Revert the commit. The startup migration is one-way (old code expects
`@immediate` + `X-FORK-FROM:`; new rows have neither). Rollback path:
- Revert code.
- Run a one-time SQL fix for any `@bg:<uuid>` rows still in the DB:
  `UPDATE cron_specs SET schedule='@immediate', prompt='X-FORK-FROM: '||substr(schedule, 5)||CHAR(10)||prompt WHERE schedule LIKE '@bg:%'`.
  Or simpler: `DELETE FROM cron_specs WHERE schedule LIKE '@bg:%'` —
  pending bg deliveries are dropped, the user is on their own. For a
  short-lived feature this is acceptable.

## Future work (out of scope)

- Strict `notify.content` regex if " " or single-char bypass observed.
- Surface `<background-jobs>` marker in CLI `/cron list` output too.
- MCP tool for the agent to cancel a bg job (`cron_cancel(job_name)`).
- Generic bg-job kinds beyond "continuation" (e.g.,
  `BackgroundResearch { from_session, query }`) — would extend the
  variant tree, no schema-selection plumbing change needed beyond the
  match arm.
