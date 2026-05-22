# Skill Learning: Probe-Writer + Curator (Hermes-aligned)

> **Successor:** `docs/superpowers/specs/2026-05-22-prefilter-classifier-and-curator-state-design.md` refines the prefilter into a 3-mode classifier, migrates curator state to `data.db`, and adds a multi-signal curator trigger.

**Date:** 2026-05-22
**Status:** Design approved; pending implementation plan
**Supersedes (partially):** `2026-05-21-learning-fork-probe-design.md` — the report-only fork-probe is replaced by a write-capable probe-writer, and the `skill_nudge_signals` queue is removed.

## Problem

The 2026-05-21 fork-probe shipped as a classifier-only safety net: it detects when the foreground agent missed a learning moment and records a signal in `skill_nudge_signals` with `source = 'fork_probe'`. Nothing consumes these signals. The loop `agent works → skill materializes` is open at the consumer side; signals sit in a table no production code reads.

At the same time, the only path that **does** write skill files is the foreground `/right-learn-skill` skill — the agent calls it mid-conversation. That path pollutes user-facing context with skill-authorship reasoning, and empirically the agent forgets to invoke it on routine learnable moments.

Stage 2's deprecated `learning_episode` pipeline (episode selector + reviewer + drain) was report-only too; we kept it behind a flag in case background `skill_review_reports` become useful, but it never wrote skills either.

Hermes Agent (`github.com/NousResearch/hermes-agent`) ships a working closed loop with the same separation we want: a fork after each turn that decides write/update/skip, plus a periodic curator that consolidates and ages. We borrow the architecture.

## Goals

- Close the loop: skill files materialize automatically after foreground turns that produced a reusable pattern.
- Keep foreground writes for **explicit user intent** ("save this as a skill"). Distinguish from auto-writes via a provenance flag — curator only touches auto-writes.
- Periodic curator consolidates near-duplicates, ages stale skills, archives unused ones. Never deletes.
- Race-safe under concurrent user messages.
- Budget-gated. Operator-pauseable.

## Non-goals

- Embedding-based similarity / vector search. Pure LLM judgement, matching Hermes.
- Per-skill filesystem locks. Concurrent probe-writer race risk is real but rare; we accept it.
- Memory + skill combined review (Hermes' `_COMBINED_REVIEW_PROMPT`). Out of scope; our memory subsystem is decoupled.
- Migrating existing `.claude/skills/rightx-*` to the new provenance scheme automatically. New skills get tagged; existing get default `created_by="foreground"` (curator-immune) until manually opted in.

## Architecture overview

```
foreground user turn
  ↓ CC main session → agent reply → bot sends to Telegram → assistant_message archived
  ↓
ProbeAnchor captured synchronously (user_msg + assistant_reply + main_session_uuid)
  ↓
Async fire-and-forget chain:
  ↓
  prefilter classifier (Haiku, 1-2s, $0.001-0.005):
    input: anchor (user_msg + assistant_reply + tool_call_summary)
    output: { should_probe: bool, reason: string }
  ↓ should_probe == false → exit
  ↓ should_probe == true
  ↓
  per-agent skill-write mutex acquired (queue, no drop)
  ↓
  probe-writer fork: claude -p --resume <main_session_uuid> --fork-session
    --model <inherit main>
    --allowedTools "Write,Read,Bash,mcp__right__skill_learning_start,mcp__right__skill_learning_finish"
    --max-turns 8
    system prompt: inherited verbatim (cache hit)
    prompt: anchor + class-first instructions + skill_index
  ↓
  if probe-writer writes: created/updated rightx-*/SKILL.md with provenance created_by="probe_writer"
  if probe-writer skips: no file change, no DB row
  ↓
  mutex released
```

Independently, on a cron tick:

```
gateway ticker (every 60s, existing)
  ↓
curator should_run_now? (last_run_at < now - interval_hours AND idle >= min_idle_hours)
  ↓ no → return
  ↓ yes
  per-agent skill-write mutex acquired
  ↓
  snapshot_skills → tarball backup
  ↓
  apply_automatic_transitions (pure Rust, no LLM): stale / archive / pin bypass
  ↓
  curator fork: claude -p --no-resume (fresh session)
    --model <inherit main, or explicit override>
    --allowedTools "Read,Bash,mcp__right__skill_learning_start,mcp__right__skill_learning_finish"
    --max-turns 9999
    system prompt: CURATOR_SYSTEM_PROMPT (codegen const)
    prompt: skill_inventory + lifecycle_stats + class-first consolidation rules
  ↓
  iterative: list → view → patch/create/archive
  only touches created_by IN ('probe_writer', 'curator')
  pinned + bundled never touched
  ↓
  write curator_runs/<utc>.json log
  last_run_at = now
  ↓
  mutex released
```

The foreground path keeps `/right-learn-skill` for explicit user intent. When invoked, `skill_learning_start` runs with `is_background_review() == false`, marking the skill `created_by="foreground"`. Curator never touches `foreground`-marked skills.

## Components

### 1. ProbeAnchor and race-safety

Captured synchronously by `bot::telegram::worker` at the end of each foreground turn, before any async work spawns. Anchor includes verbatim user message text, verbatim assistant reply text, the main session uuid, capture timestamp, chat id, thread id. Passed by value into prefilter and probe-writer paths.

**Race-safety via existing per-session mutex.** Probe-writer's `claude -p --resume <main> --fork-session` is a `--resume` caller, so it naturally participates in the existing per-session mutex (ARCHITECTURE.md → "Per-session mutex on `--resume`"). Probe-writer acquires the mutex before spawning CC, spawns the fork, **waits for the fork's `system/init` event** (parsed from `stream-json` stdout — the same handshake pattern as background-continuation in `bot::background::request_background_continuation`), then releases the mutex. The fork is now on its own session_id and the main session is free for subsequent user turns.

Hold-window for the mutex: only the fork-init phase (~200-500ms typical), not the full probe-writer lifetime. If a new user message arrives during this window, its worker waits on the same mutex; ordering is FIFO.

**Anchor as belt-and-suspenders.** Between anchor capture and mutex acquisition there is a ~1-2s window (prefilter latency). If a new user message wins the mutex during that window, it completes its turn first and its turn appears in main's JSONL before probe-writer's fork init reads it. Probe-writer's fork would then inherit a transcript containing both turns. The anchor in probe-writer's prompt ("review ONLY the anchored exchange; ignore newer activity") makes the model focus on the right turn. The mutex prevents partial-read races; the anchor handles ordering ambiguity.

Stronger guarantee (hold mutex from anchor capture through fork init, blocking new user messages for ~1.5-2.5s on every learning-eligible turn) is rejected: UX cost (perceived bot lag) outweighs the marginal correctness gain over anchor-based mitigation.

### 2. Prefilter classifier (Haiku)

New module `crates/bot/src/learning_prefilter.rs`.

Invokes `claude -p --model claude-haiku-4-5-20251001 --tools "" --no-mcp-config --output-format json --json-schema PREFILTER_SCHEMA --max-turns 1`. Prompt contains anchor + brief tool-call summary; asks Haiku to decide whether the turn likely produced a reusable workflow worth a deeper writer pass. Schema is two fields: `should_probe: bool` and `reason: string`.

Latency target: 1-2s. Cost target: $0.001-0.005 per turn. Failure (non-zero exit, parse error, timeout) → assume `should_probe = false`, log warn.

Hermes uses a counter-based trigger (`>= 10 tool iterations`) instead of an LLM classifier. The choice for Haiku is principled: a counter fires on long turns that did nothing learnable (false positives) and misses short turns with strong user-correction signals (false negatives). Haiku at $0.001/turn is cheap enough that the precision improvement is worth it.

### 3. Probe-writer (inherit main model)

`crates/bot/src/learning_probe_writer.rs` (renamed/extended from current `learning_probe.rs`).

Spawned only when prefilter returns `should_probe = true`. Forks main session via `--resume <main_session_uuid> --fork-session --session-id <probe_uuid>`. Inherits the cached system prompt verbatim (Hermes pattern: `background_review.py:436` → preserves Anthropic prefix-cache hits).

Tools whitelist: `Write`, `Read`, `Bash`, `mcp__right__skill_learning_start`, `mcp__right__skill_learning_finish`. No other tools, no other MCP servers exposed.

Max turns: 16. Aligned with Hermes' production value (`background_review.py:398`). Budget: survey (2-4 reads) + decide + start + write SKILL.md + optional `references/`/`scripts/` files + finish = 5-9 turns typical, 16 leaves comfortable margin without runaway.

Prompt structure delivered as the first user message in the fork (Hermes pattern: prompt-as-user-msg keeps system cached):

```
<probe_writer_anchor>
USER (target): {user_msg_text}
ASSISTANT (target): {assistant_reply_text}
</probe_writer_anchor>

<probe_writer_instructions>
[Class-first guidance: survey existing rightx-* skills, prefer update over create.
 Naming, package shape, start/finish protocol, quality rules — currently in
 right-learn-skill SKILL.md, now moved into this constant.]
</probe_writer_instructions>

<skill_index>
{collected_rightx_skill_index — host or sandbox, bounded}
</skill_index>

Decide: update existing skill, create new rightx-* skill, or exit silently.
Ignore any forked-session content that arrived after the anchored exchange.
```

Provenance: `is_background_review()` is true during probe-writer execution. `skill_learning_start` writes `created_by = "probe_writer"` into the skill's `.usage.json` record. Curator-eligible.

Failure (non-zero exit, max-turns reached without finish, timeout) → log warn, no retry. Probe-writer is best-effort; the next learnable turn provides another opportunity.

### 4. Curator (inherit main model by default)

New module `crates/bot/src/learning_curator.rs`. Triggered by the existing gateway cron ticker (every 60s wall clock) but gated by `should_run_now`:

- `learning.curator_enabled = true`.
- `!learning.curator_paused` (operator pause flag, set via CLI subcommand).
- `now - last_run_at >= curator_interval_hours` (default 168 = 7 days).
- `now - latest_user_activity_at >= curator_min_idle_hours` (default 2).

Last-run timestamp lives in `agents/<name>/.claude/skills/.curator_state.json`.

When firing:

1. Acquire per-agent skill-write mutex (shared with probe-writer).
2. `snapshot_skills`: tar-gzip `.claude/skills/` to `agents/<name>/curator_backups/<utc>/skills.tar.gz`. Excludes `.archive/` and `.curator_backups/`. Best-effort: failure logs warn, doesn't block run.
3. `apply_automatic_transitions`: pure-Rust pass over `.usage.json`. Computes `latest_activity_at = max(last_used_at, last_patched_at)` per skill. Active → stale when `now - latest_activity_at > stale_after_days` (default 30). Stale → active on any new use. Any → archived when `now - latest_activity_at > archive_after_days` (default 90). Pinned bypasses all transitions. Archive moves directory to `.claude/skills/.archive/rightx-<slug>-<ts>/`.
4. Curator fork: `claude -p --session-id <curator-uuid> --no-resume`. Tools whitelist: `Read`, `Bash`, `mcp__right__skill_learning_start`, `mcp__right__skill_learning_finish`. Max turns 9999 (Hermes default; large because iterative tool use over hundreds of skills).
5. Curator system prompt is its own codegen constant `CURATOR_SYSTEM_PROMPT`, NOT inherited from main agent. Specialized for consolidation/lifecycle decisions.
6. Curator's initial user message contains the skill inventory (names + descriptions + `.usage.json` stats) plus class-first consolidation rules. Skill bodies are loaded on-demand via `Read` tool calls — the LLM decides which to deep-view.
7. After completion, write `curator_runs/<utc>.json` with action log: `[{action: "patch" | "create" | "archive", skill_name, reason, before_hash, after_hash}, ...]`.
8. Update `last_run_at`.
9. Release mutex.

Permission rules:
- Curator can only patch/archive/create skills with `created_by IN ('probe_writer', 'curator')`.
- `created_by = 'foreground'` (user-explicit-intent) is permanently excluded.
- Bundled `.claude/skills/*` (those shipped by codegen) excluded. Identified by `created_by = "bundled"` in `.usage.json`, set when codegen installs them on bot startup.
- Pinned skills (`pinned: true` in `.usage.json`) excluded from archive but patch allowed (Hermes pattern). Pinning is an **operator escape hatch** via CLI (`right agent skill pin <name>`), not a user-facing UI feature.
- Never delete; archive (rename) only.

#### 4.1 Consolidation mechanics

Curator can perform structural consolidation when its LLM pass identifies near-duplicates or overlapping coverage. Three tactics (Hermes-aligned per `agent/curator.py:330-447`):

**Tactic 1: Merge into existing umbrella.**
Two narrow skills `rightx-foo-a` and `rightx-foo-b` cover the same topic, and an umbrella `rightx-foo` already exists. Curator:
- Patches `rightx-foo`'s SKILL.md to incorporate techniques/gotchas from `rightx-foo-a` and `rightx-foo-b`.
- Moves the full content of the narrow skills into `rightx-foo/references/foo-a.md` and `references/foo-b.md` for retrieval.
- Archives `rightx-foo-a` and `rightx-foo-b` directories with annotation `absorbed_into = "rightx-foo"` written to their final `.usage.json` records.

**Tactic 2: Create new umbrella with demotion.**
Two narrow skills overlap but no umbrella exists. Curator creates `rightx-<umbrella-slug>`, writes a class-level SKILL.md, demotes both originals into the new umbrella's `references/` subdir, archives originals with `absorbed_into` annotation.

**Tactic 3: Demote single skill to references.**
One narrow skill is fully covered by an existing broader skill's scope. Curator moves the narrow skill's SKILL.md into the broader skill's `references/<slug>.md`. Archives original with `absorbed_into` annotation.

**`absorbed_into` annotation**: written to the archived skill's final `.usage.json` record. Format:
```json
{
  "rightx-foo-a": {
    "state": "archived",
    "archived_at": "2026-05-29T14:00:00Z",
    "absorbed_into": "rightx-foo",
    "created_by": "probe_writer",
    ...
  }
}
```

**Cron skill-ref migration**: agent's `cron_specs` may reference skill names. Bot startup scans active `cron_specs` for skill references; for each absorbed skill (presence in `.usage.json` with `absorbed_into != null`), rewrites the cron spec to point at the umbrella. One-time migration per absorption event, logged.

Never delete. Always archive. `absorbed_into` records the lineage for future reference and rollback (operator can restore from `.curator_backups/<ts>/skills.tar.gz`).

### 5. Foreground `/right-learn-skill` skill (preserved, simplified)

`crates/right-codegen/skills/right-learn-skill/SKILL.md` stays as the explicit-user-intent entry point. Rewritten content:

- "When to use" → narrows to "user explicitly says save / remember / fix this".
- "Deferred signal" section → **deleted**. The reply-schema `learning_signal` / `skill_issue_signal` fields go away (no consumer remains).
- "Create / Update" — protocol same as today: `skill_learning_start(action=create|update)` + write/patch + `skill_learning_finish`.
- Provenance: caller is foreground → `is_background_review() == false` → `created_by = "foreground"`.

Operating-instructions update: the existing line "When you discover a reusable procedure..., use the `/right-learn-skill` skill. It decides whether to create or update a `rightx-*` learned skill, or leave a nudge signal" is rewritten to "When the **user** explicitly asks you to save / remember / fix a `rightx-*` skill, use the `/right-learn-skill` skill. The platform handles routine learning automatically."

### 5a. `used_skill_receipts` (vocal-to-user signal + bump_use source)

Reply-schema field becomes **required** (was optional + nullable). Empty array allowed; null / absence forbidden.

`REPLY_SCHEMA_JSON` change:

```json
"used_skill_receipts": {
  "type": "array",
  "items": {
    "type": "object",
    "properties": {
      "package_name": { "type": "string", "pattern": "^rightx-" },
      "message": { "type": "string", "minLength": 1 }
    },
    "required": ["package_name", "message"]
  }
}
```

Plus addition to top-level `"required": ["content", "used_skill_receipts"]`.

CC's structured-output validator rejects replies missing the field — the agent cannot accidentally omit it. Empty array is the no-skill-used case.

`OPERATING_INSTRUCTIONS.md` line rewrite (currently line 44-46):

> "Always include `used_skill_receipts` in your reply. Empty array `[]` if no `rightx-*` skill applied. Non-empty array (one entry per skill) when one or more skills materially guided your answer. The `message` field describes the workflow you applied — e.g. \"Built and verified npm package\" — and is shown to the user. Avoid generic messages like \"Done\"."

**Worker rendering** (`crates/bot/src/cc/worker_reply.rs::append_used_skill_receipts`): rewrite from current `\n\n<message>` to:

```
<reply content>

💡 <message> (<code>rightx-foo</code>)
```

(One line per receipt; `package_name` in Telegram HTML `<code>` tag so it renders monospace and is visually distinct from the agent's prose.)

**`bump_use` hook**: worker, at receipt-parsing site, for each receipt whose `package_name` matches `^rightx-`, calls `lifecycle::usage::bump_use(agent_dir, package_name)` which updates `.usage.json::<name>::{use_count++, last_used_at=now}`. Non-`rightx-` receipts are dropped silently (schema pattern should catch them upstream; defense-in-depth).

Backward-compatibility window: agents on pre-spec codegen may transiently emit `null` or omit the field. Bot worker tolerates absence/null as empty during the transition (no schema-validation re-prompt loop). After codegen propagates and CC re-loads schemas, the strict path takes over.

### 6. Per-agent skill-write mutex

`Arc<RwLock<HashMap<AgentName, Arc<tokio::sync::Mutex<()>>>>>` on bot worker context. Acquired before spawning probe-writer OR curator. Released on exit. Sequential ordering by acquisition time. No drop of late-arriving probes: probe-writer waits its turn.

This protects against:
- Two probe-writers trying to create the same `rightx-<slug>` simultaneously (`skill_learning_start MustNotExist` would race).
- Curator running while a probe-writer is mid-write.
- Multiple probes patching the same `.usage.json` (Hermes uses flock at file level; we serialize at agent level for simplicity).

No per-skill granularity. Cheap; sufficient.

### 7. Usage tracking (`.usage.json`)

New file `agents/<name>/.claude/skills/.usage.json`. Format:

```json
{
  "rightx-foo": {
    "use_count": 12,
    "patch_count": 1,
    "last_used_at": "2026-05-21T14:23:00Z",
    "last_patched_at": "2026-05-20T11:15:00Z",
    "state": "active",
    "pinned": false,
    "created_by": "probe_writer",
    "created_at": "2026-05-15T10:00:00Z",
    "archived_at": null,
    "absorbed_into": null
  }
}
```

`view_count` / `last_viewed_at` from Hermes are intentionally omitted: their `bump_view` hook fires on an explicit `skill_view` tool call (agent-driven open). In our system CC auto-loads skills based on description matching, with no observable per-skill activation signal. Tracking views without that signal would be either overcount (all loaded skills bumped) or zero — neither useful. Staleness relies on `last_used_at` (from receipt parsing) and `last_patched_at` (from curator/probe-writer mutations) only.

Atomic write via tempfile + rename. flock on Linux/macOS for concurrent updaters. Updates triggered by:

- `bump_use` — worker on receipt parsing in `crates/bot/src/cc/worker_reply.rs` (see §5a). Increments `use_count`, sets `last_used_at = now`.
- `bump_patch` — on `skill_learning_finish` with `status = "updated"`. Increments `patch_count`, sets `last_patched_at = now`.
- `mark_created` — on `skill_learning_finish` with `status = "created"`. Records `created_by` from the active provenance source.
- `mark_archived` — on curator archive action. Sets `state = "archived"`, `archived_at = now`, optionally `absorbed_into = "<umbrella>"`.

Staleness criterion (used by curator's `apply_automatic_transitions`):

```
latest_activity_at = max(last_used_at, last_patched_at)
stale     = (now - latest_activity_at) > stale_after_days   (default 30)
archived  = (now - latest_activity_at) > archive_after_days (default 90)
```

`absorbed_into = null` for normal skills; set to the absorbing umbrella's name on archive-via-consolidation (see §4.1).

Records for newly discovered skills (not yet in `.usage.json`) are created lazily on first read. Default `created_by = "foreground"` (conservative — keeps existing pre-migration skills curator-immune).

`pinned` is set via operator CLI (`right agent skill pin <name>`), not user-facing. Bypasses archive transition; patch still allowed.

## Data model changes

### Database

Mark as legacy (don't drop):
- `skill_nudge_signals` — no longer written.
- `learning_episodes` — no longer written.
- `skill_review_reports` — no longer written by probe; curator may opt to repurpose later (out of scope).

Drop in code (use `#[allow(dead_code)]` or remove):
- `right_agent::learned_skills::NudgeSignalSource` enum.
- `record_nudge_signal`, `select_reply_signal`.
- `NudgeSignalRecord.source`.

Migration v28: no DDL change. Tables remain for one release cycle, then dropped in v29.

### Reply schema

Remove `learning_signal` and `skill_issue_signal` fields from `REPLY_SCHEMA_JSON` — no consumer remains. Foreground agents may still emit them transiently on old responses; bot worker drops them silently during the transition window.

Remove `FORK_PROBE_SCHEMA_JSON` and `FORK_PROBE_PROMPT` (added in `2026-05-21-learning-fork-probe-design.md`). Probe-writer's "output" is filesystem mutation via tool calls, not JSON. No structured output schema needed — `--max-turns` bound plus tool-use loop is the contract.

`skill_nudge_signals.source` column and the `NudgeSignalSource` enum (also from `2026-05-21-learning-fork-probe-design.md`) become dead code. Left in place for one release cycle; dropped in v29 alongside the legacy tables.

### Configuration

`LearningConfig`:

```yaml
learning:
  prefilter_enabled: true
  prefilter_model: "claude-haiku-4-5-20251001"
  probe_writer_enabled: true
  probe_writer_model: null               # null → inherit agent.model
  curator_enabled: true
  curator_model: null                    # null → inherit agent.model
  curator_interval_hours: 168
  curator_min_idle_hours: 2
  curator_stale_after_days: 30
  curator_archive_after_days: 90
  curator_paused: false
  max_daily_budget_usd: 1.00             # carries forward
```

Removed (deprecated, ignored if present in agent.yaml):
- `fork_probe_enabled`, `fork_probe_model`, `background_review_enabled`, `episode_selector_model`, `episode_selector_max_budget_usd`, `episode_settle_seconds`, `circuit_failure_threshold`, `circuit_cooldown_minutes`.

Wizard prompts for the new fields. Deprecated fields log a one-time warn on load.

## Cost model

Per agent per day, with 100 user turns:

- Prefilter (Haiku, 100 turns × $0.001-0.005) → $0.10-0.50/day.
- Probe-writer (Opus main with cache hit, ~10 fires × $0.30-0.80) → $3-8/day.
- Curator (Opus main, weekly, ~$1-3/run) → $0.15-0.45/day amortized.

Total: $3.25-8.95/day per active agent. The `$1/day` default budget is tight; will gate probe-writer often on chatty agents. We surface this in the wizard and recommend $5+ for active agents. Budget gate covers probe-writer + curator; prefilter is out of the gate (its cost is already minimal).

Failure-throttling stays the same as today: daily budget exceeded → all gated invocations skip until UTC midnight. Pre-existing `consecutive_review_failures` circuit-breaker columns are unused by the new pipeline; left as dead schema for v28, dropped in v29.

## Failure modes

| Failure | Behavior |
|---|---|
| Prefilter exits non-zero / timeout / parse fail | Assume `should_probe = false`. Log warn. Probe-writer not spawned. |
| Probe-writer exits non-zero / max-turns | Log warn. Skill may be half-written → next session run sees inconsistent state. Mitigation: `skill_learning_finish` validates `SKILL.md` exists before recording success; if absent, no `.usage.json` entry created. Half-written dir gets archived on next curator pass. |
| Probe-writer hits timeout (60s wall-clock) | Force-killed. Same as exit non-zero. |
| Mutex contended → probe-writer waits | Acceptable. Each probe is anchored, so delay doesn't change semantics. |
| Curator crashes mid-run | Backup tarball exists; `last_run_at` is updated only on successful completion → retry on next tick. Manual recovery: untar backup. |
| User sends new message mid-prefilter | Anchor isolates: prefilter sees only the captured exchange. New message starts a separate turn with its own anchor. |
| Two probe-writers concurrently for the same chat | Mutex serializes. Second one waits. |
| `created_by="foreground"` skill mistakenly marked auto | Permanent — once marked, curator can touch. We accept; ops mitigation is `pinned: true`. |

## Migration

Existing agents (already running with `2026-05-21-learning-fork-probe-design` schema):

1. **Bot restart picks up new codegen** — new `LearningConfig` fields default to safe values; old fields silently ignored.
2. **No DB migration** required at v28 — old tables left in place, drop deferred to v29.
3. **Existing `rightx-*` skills** without `.usage.json` entries get default `created_by = "foreground"` on first probe-writer / curator pass that observes them. Curator-immune until manual opt-in.
4. **`/right-learn-skill` SKILL.md** updated via codegen. Agents on next CC restart see the new shorter version (Hermes-style "explicit intent only").
5. **Operating instructions** rewritten. Agent no longer instructed to invoke `/right-learn-skill` for routine learning.
6. **`learning_signal` / `skill_issue_signal` reply-schema fields** removed. Foreground agents may still emit them in old responses transiently; bot worker ignores.

No sandbox recreation. No `right agent init`. Plain `right restart <agent>`.

## Testing

Per `AGENTS.md` cadence. TDD per module:

- `learning_prefilter`: schema validation, prompt builder, anchor injection, failure → false default.
- `learning_probe_writer`: tool whitelist, max-turns, anchor enforcement, provenance flag set via `is_background_review()`.
- `learning_curator`: `should_run_now` gates, `apply_automatic_transitions` table (stale/archive/pin bypass), backup creation, decision-log emit.
- `lifecycle::usage` (new module): atomic ops, flock, bump_* functions, race-safety.
- Mutex behavior: parallel probe-writers serialize; curator blocks while probe-writer holds.

Integration:
- End-to-end with mocked CC: foreground reply → prefilter says yes → probe-writer mock writes skill → `.usage.json` updated → curator (manually triggered) → action log written.
- One ci_claude_ live test for prefilter (real Haiku call).
- One ci_claude_ live test for probe-writer happy path.

Final: `cargo test --workspace` before merge.

## Open questions (deferred to plan)

1. **Half-written skill recovery.** A probe-writer killed between `skill_learning_start` and writing SKILL.md leaves a dir without SKILL.md. `skill_learning_finish` validation catches and rejects. Curator's first pass identifies and archives such orphans. Implementation detail in plan.
2. **Curator dry-run mode.** `learning.curator_dry_run: bool` flag (default false). When true, curator runs the LLM pass but blocks all `skill_manage` write actions, only logs decisions. Useful for operator validation. Decide if v1 ships with this or defers.
3. **`.usage.json` for bundled skills.** Bundled (codegen-installed) skills get records too with `created_by = "bundled"` so `bump_use` stats are visible in the dashboard. Curator's permission rule excludes `created_by = "bundled"` from any write action. Plan needs to wire this on bundle install/upgrade.
4. **Main session bloat.** If a chat fires many learnable turns rapidly, probe-writers queue under the mutex. Each fork inherits a growing main session transcript. Cache hit rates degrade as the main session JSONL grows. Likely acceptable for v1; revisit if observed in production.
5. **Schema migration window for `used_skill_receipts` required.** When agents restart on the new codegen, they immediately get the strict schema. Replies emitted during the restart-and-roll-out interval may fail validation. Worker tolerates absence/null transitively (degrades to empty array). Question: how long do we keep that tolerance? Suggest one release cycle, then remove.

## Implementation handoff

The plan must:
- Add v28 migration (no DDL, just code-level dead-code annotations on signal tables).
- New modules: `learning_prefilter.rs`, `learning_probe_writer.rs` (renamed), `learning_curator.rs`, `lifecycle/usage.rs`.
- New codegen constants: `PROBE_WRITER_INSTRUCTIONS`, `CURATOR_SYSTEM_PROMPT`, `PROBE_WRITER_ANCHOR_TEMPLATE`.
- `LearningConfig` field additions + deprecation warnings on removed fields.
- Wizard prompts updates.
- `/right-learn-skill` SKILL.md rewrite (explicit-intent only, no deferred-signal section).
- Operating instructions rewrite (auto-learning by platform; explicit `/right-learn-skill` only on user ask; **MUST** emit `used_skill_receipts`).
- `REPLY_SCHEMA_JSON` cleanup: remove `learning_signal`/`skill_issue_signal`, make `used_skill_receipts` required-not-nullable with `^rightx-` pattern on `package_name`.
- `append_used_skill_receipts` rewrite: render `💡 <message> (<code>rightx-foo</code>)` per receipt.
- `bump_use` hook in `worker_reply` receipt-parsing path.
- Probe-writer + curator integrate with existing per-session mutex and existing `system/init` handshake from `bot::background`. No new mutex primitive; reuse.
- Operator CLI: `right agent skill pin <name>` / `unpin` / `list-pins` subcommand wiring into `.usage.json::pinned`.
- Dashboard updates: drop `signals_by_source_24h`, add `skill_lifecycle_overview` reading `.usage.json` (active/stale/archived counts, recently-used top-N).
- TDD per `AGENTS.rust.md` cadence; final `cargo test --workspace`.
